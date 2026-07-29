use crate::llm::retry::{self, RetryError};
use crate::llm::{LLMAdapter, LLMEvent, LLMResponse, Usage};
use crate::types::{ContentBlock, Message, ModelConfig, Role, ToolCall, ToolSchema};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Google Gemini `generateContent` API adapter.
///
/// Endpoint: `https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}`
/// Authentication is via the `key` query parameter (no bearer token).
/// Roles are `user` / `model`; system messages go into `systemInstruction`.
/// Tool calling uses `functionDeclarations` (request) and `functionCall` / `functionResponse` (contents).
pub struct GeminiAdapter {
    config: ModelConfig,
    client: Client,
}

impl GeminiAdapter {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    fn build_url(&self, model: &str, stream: bool) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        let method = if stream { "streamGenerateContent" } else { "generateContent" };
        format!("{base}/models/{model}:{method}")
    }

    /// Walk the message list and build (systemInstruction, contents) for the Gemini request.
    ///
    /// Gemini requires `functionResponse.name` to match the preceding `functionCall.name`,
    /// but our internal `ContentBlock::ToolResult` only carries an `id`. We pre-scan all
    /// `ToolUse` blocks to build an `id -> name` map, then resolve names when emitting
    /// `functionResponse` parts.
    fn build_contents(
        messages: &[Message],
    ) -> (Option<GeminiSystemInstruction>, Vec<GeminiContent>) {
        // Map tool_use id -> name for resolving ToolResult.functionResponse.name
        let mut id_to_name: HashMap<String, String> = HashMap::new();
        for msg in messages {
            for block in &msg.content {
                if let ContentBlock::ToolUse { id, name, .. } = block {
                    id_to_name.insert(id.clone(), name.clone());
                }
            }
        }

        let mut system_instruction: Option<GeminiSystemInstruction> = None;
        let mut contents: Vec<GeminiContent> = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    let text: String = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        system_instruction = Some(GeminiSystemInstruction {
                            parts: vec![GeminiPart::text(text)],
                        });
                    }
                }
                Role::User | Role::Tool => {
                    let parts: Vec<GeminiPart> = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(GeminiPart::text(text.clone())),
                            ContentBlock::ToolResult { id, output } => {
                                let name = id_to_name.get(id).cloned().unwrap_or_else(|| id.clone());
                                let response = match output {
                                    crate::types::ToolOutput::Sync { result } => {
                                        // Gemini expects an object; wrap scalars.
                                        if result.is_object() {
                                            result.clone()
                                        } else {
                                            serde_json::json!({ "result": result })
                                        }
                                    }
                                    crate::types::ToolOutput::AsyncTask { handle, status_msg, .. } => {
                                        serde_json::json!({ "handle": handle, "status": status_msg })
                                    }
                                    crate::types::ToolOutput::Error { message } => {
                                        serde_json::json!({ "error": message })
                                    }
                                };
                                Some(GeminiPart {
                                    text: None,
                                    function_call: None,
                                    function_response: Some(GeminiFunctionResponse { name, response }),
                                })
                            }
                            _ => None,
                        })
                        .collect();
                    if !parts.is_empty() {
                        contents.push(GeminiContent {
                            role: "user".into(),
                            parts,
                        });
                    }
                }
                Role::Assistant => {
                    let parts: Vec<GeminiPart> = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(GeminiPart::text(text.clone())),
                            ContentBlock::ToolUse { name, args, .. } => Some(GeminiPart {
                                text: None,
                                function_call: Some(GeminiFunctionCall {
                                    name: name.clone(),
                                    args: Some(args.clone()),
                                }),
                                function_response: None,
                            }),
                            _ => None,
                        })
                        .collect();
                    if !parts.is_empty() {
                        contents.push(GeminiContent {
                            role: "model".into(),
                            parts,
                        });
                    }
                }
            }
        }

        (system_instruction, contents)
    }

    fn build_tools(tools: &[ToolSchema]) -> Vec<GeminiTools> {
        if tools.is_empty() {
            return Vec::new();
        }
        let decls: Vec<GeminiFunctionDeclaration> = tools
            .iter()
            .map(|t| GeminiFunctionDeclaration {
                name: t.name.clone(),
                description: Some(t.description.clone()),
                parameters: Some(t.parameters.clone()),
            })
            .collect();
        vec![GeminiTools { function_declarations: decls }]
    }
}

#[async_trait]
impl LLMAdapter for GeminiAdapter {
    fn name(&self) -> &str {
        "gemini"
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &ModelConfig,
    ) -> Result<LLMResponse> {
        let (system_instruction, contents) = Self::build_contents(messages);
        let tool_decls = Self::build_tools(tools);

        let body = GeminiRequest {
            contents,
            system_instruction,
            tools: if tool_decls.is_empty() { None } else { Some(tool_decls) },
            generation_config: Some(GeminiGenerationConfig {
                temperature: config.temperature,
                max_output_tokens: config.max_tokens,
            }),
        };

        let url = self.build_url(&config.name, false);
        let api_key = self.config.api_key.clone();
        let client = self.client.clone();

        retry::with_retry(move || {
            let client = client.clone();
            let url = url.clone();
            let api_key = api_key.clone();
            let body = body.clone();
            async move {
                let resp = client
                    .post(&url)
                    .query(&[("key", api_key.as_str())])
                    .json(&body)
                    .send()
                    .await
                    .map_err(RetryError::from)?;

                if !resp.status().is_success() {
                    let status = resp.status().as_u16();
                    let retry_after_secs = retry::parse_retry_after(resp.headers());
                    let text = resp.text().await.unwrap_or_default();
                    return Err(RetryError::HttpStatus {
                        status,
                        body: text,
                        retry_after_secs,
                    });
                }

                let data: GeminiResponse = resp
                    .json()
                    .await
                    .map_err(RetryError::from)?;

                let mut content = String::new();
                let mut tool_calls = Vec::new();

                if let Some(candidate) = data.candidates.into_iter().next() {
                    for part in candidate.content.parts {
                        if let Some(text) = part.text {
                            content.push_str(&text);
                        }
                        if let Some(fc) = part.function_call {
                            tool_calls.push(ToolCall {
                                id: uuid::Uuid::new_v4().to_string(),
                                name: fc.name,
                                args: fc.args.unwrap_or(Value::Null),
                            });
                        }
                    }
                }

                let usage = data.usage_metadata.map(|u| Usage {
                    prompt_tokens: u.prompt_token_count,
                    completion_tokens: u.candidates_token_count,
                    total_tokens: u.total_token_count,
                });

                Ok(LLMResponse {
                    content: if content.is_empty() { None } else { Some(content) },
                    tool_calls,
                    usage,
                })
            }
        })
        .await
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &ModelConfig,
        tx: tokio::sync::mpsc::Sender<LLMEvent>,
    ) -> Result<()> {
        let (system_instruction, contents) = Self::build_contents(messages);
        let tool_decls = Self::build_tools(tools);

        let body = GeminiRequest {
            contents,
            system_instruction,
            tools: if tool_decls.is_empty() { None } else { Some(tool_decls) },
            generation_config: Some(GeminiGenerationConfig {
                temperature: config.temperature,
                max_output_tokens: config.max_tokens,
            }),
        };

        let url = self.build_url(&config.name, true);
        let api_key = self.config.api_key.clone();

        let resp = self
            .client
            .post(&url)
            .query(&[("key", api_key.as_str()), ("alt", "sse")])
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("sending Gemini stream request: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Gemini API error {}: {}", status, text);
        }

        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut full_content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer.drain(..=pos);

                if line.is_empty() || !line.starts_with("data: ") {
                    continue;
                }

                let data = line.trim_start_matches("data: ").trim();
                if let Ok(event) = serde_json::from_str::<GeminiResponse>(data) {
                    if let Some(candidate) = event.candidates.into_iter().next() {
                        for part in candidate.content.parts {
                            if let Some(text) = part.text {
                                full_content.push_str(&text);
                                let _ = tx.send(LLMEvent::ContentDelta(text)).await;
                            }
                            if let Some(fc) = part.function_call {
                                let tc = ToolCall {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    name: fc.name,
                                    args: fc.args.unwrap_or(Value::Null),
                                };
                                let _ = tx.send(LLMEvent::ToolCallDone(tc.clone())).await;
                                tool_calls.push(tc);
                            }
                        }
                    }
                }
            }
        }

        let _ = tx
            .send(LLMEvent::Done(LLMResponse {
                content: if full_content.is_empty() { None } else { Some(full_content) },
                tool_calls,
                usage: None,
            }))
            .await;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "systemInstruction")]
    system_instruction: Option<GeminiSystemInstruction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTools>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "generationConfig")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Serialize, Clone)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Clone)]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionResponse")]
    function_response: Option<GeminiFunctionResponse>,
}

impl GeminiPart {
    fn text(t: String) -> Self {
        Self {
            text: Some(t),
            function_call: None,
            function_response: None,
        }
    }
}

#[derive(Serialize, Clone)]
struct GeminiFunctionCall {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<Value>,
}

#[derive(Serialize, Clone)]
struct GeminiFunctionResponse {
    name: String,
    response: Value,
}

#[derive(Serialize, Clone)]
struct GeminiSystemInstruction {
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Clone)]
struct GeminiTools {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Serialize, Clone)]
struct GeminiFunctionDeclaration {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<Value>,
}

#[derive(Serialize, Clone)]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "maxOutputTokens")]
    max_output_tokens: Option<u32>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiCandidateContent,
}

#[derive(Deserialize)]
struct GeminiCandidateContent {
    #[serde(default)]
    parts: Vec<GeminiPartResp>,
}

#[derive(Deserialize)]
struct GeminiPartResp {
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "functionCall")]
    function_call: Option<GeminiFunctionCallResp>,
}

#[derive(Deserialize)]
struct GeminiFunctionCallResp {
    name: String,
    #[serde(default)]
    args: Option<Value>,
}

#[derive(Deserialize)]
struct GeminiUsageMetadata {
    #[serde(rename = "promptTokenCount", default)]
    prompt_token_count: u32,
    #[serde(rename = "candidatesTokenCount", default)]
    candidates_token_count: u32,
    #[serde(rename = "totalTokenCount", default)]
    total_token_count: u32,
}
