// 设计文档 §7.2: 流式响应 struct 为 forward-looking scaffolding
#![allow(dead_code)]

use crate::llm::retry::{self, RetryError};
use crate::llm::{LLMAdapter, LLMEvent, LLMResponse, Usage};
use crate::types::{ContentBlock, Message, ModelConfig, ToolCall, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Anthropic Messages API adapter (/v1/messages)
/// Uses x-api-key header + anthropic-version header
/// Tool calls use tool_use / tool_result content blocks (matches our internal model well)
pub struct AnthropicAdapter {
    config: ModelConfig,
    client: Client,
}

impl AnthropicAdapter {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    fn build_url(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        // 自动补全 /v1 前缀：若 base_url 未以 /v1 结尾则追加
        let base = if base.ends_with("/v1") {
            base.to_string()
        } else {
            format!("{}/v1", base)
        };
        format!("{}/messages", base)
    }

    fn build_messages(messages: &[Message]) -> (Option<String>, Vec<AnthropicMessage>) {
        let mut system_prompt: Option<String> = None;
        let mut result = Vec::new();

        for msg in messages {
            match msg.role {
                crate::types::Role::System => {
                    // Anthropic uses a top-level system param, not a message
                    let text: String = msg.content.iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        system_prompt = Some(text);
                    }
                }
                crate::types::Role::User | crate::types::Role::Tool => {
                    let blocks: Vec<AnthropicContent> = msg.content.iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(AnthropicContent {
                                kind: "text".into(),
                                text: Some(text.clone()),
                                id: None,
                                name: None,
                                input: None,
                                tool_use_id: None,
                            }),
                            ContentBlock::ToolResult { id, output } => {
                                let text = match output {
                                    crate::types::ToolOutput::Sync { result } => result.to_string(),
                                    crate::types::ToolOutput::AsyncTask { status_msg, .. } => status_msg.clone(),
                                    crate::types::ToolOutput::Error { message } => format!("Error: {}", message),
                                };
                                Some(AnthropicContent {
                                    kind: "tool_result".into(),
                                    text: Some(text),
                                    id: None,
                                    name: None,
                                    input: None,
                                    tool_use_id: Some(id.clone()),
                                })
                            }
                            _ => None,
                        })
                        .collect();
                    if !blocks.is_empty() {
                        result.push(AnthropicMessage {
                            role: "user".into(),
                            content: blocks,
                        });
                    }
                }
                crate::types::Role::Assistant => {
                    let blocks: Vec<AnthropicContent> = msg.content.iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(AnthropicContent {
                                kind: "text".into(),
                                text: Some(text.clone()),
                                id: None,
                                name: None,
                                input: None,
                                tool_use_id: None,
                            }),
                            ContentBlock::ToolUse { id, name, args } => Some(AnthropicContent {
                                kind: "tool_use".into(),
                                text: None,
                                id: Some(id.clone()),
                                name: Some(name.clone()),
                                input: Some(args.clone()),
                                tool_use_id: None,
                            }),
                            _ => None,
                        })
                        .collect();
                    if !blocks.is_empty() {
                        result.push(AnthropicMessage {
                            role: "assistant".into(),
                            content: blocks,
                        });
                    }
                }
            }
        }

        (system_prompt, result)
    }

    fn build_tools(tools: &[ToolSchema]) -> Vec<AnthropicTool> {
        tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.name.clone(),
                description: Some(t.description.clone()),
                input_schema: t.parameters.clone(),
            })
            .collect()
    }
}

#[async_trait]
impl LLMAdapter for AnthropicAdapter {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn supports_tool_cache(&self) -> bool {
        true
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &ModelConfig,
    ) -> Result<LLMResponse> {
        let (system, msgs) = Self::build_messages(messages);

        let body = AnthropicRequest {
            model: config.name.clone(),
            messages: msgs,
            system,
            tools: if tools.is_empty() { None } else { Some(Self::build_tools(tools)) },
            max_tokens: config.max_tokens.unwrap_or(4096),
            temperature: config.temperature,
            stream: false,
        };
        let body_value = serde_json::to_value(&body).context("serializing Anthropic request")?;
        let url = self.build_url();
        let api_key = self.config.api_key.clone();
        let client = self.client.clone();

        retry::with_retry(move || {
            let client = client.clone();
            let url = url.clone();
            let api_key = api_key.clone();
            let body_value = body_value.clone();
            async move {
                let resp = client
                    .post(&url)
                    .header("x-api-key", &api_key)
                    .header("anthropic-version", "2023-06-01")
                    .json(&body_value)
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

                let data: AnthropicResponse = resp
                    .json()
                    .await
                    .map_err(RetryError::from)?;

                let mut content = String::new();
                let mut tool_calls = Vec::new();

                for block in data.content {
                    match block.kind.as_str() {
                        "text" => {
                            if let Some(text) = block.text {
                                content.push_str(&text);
                            }
                        }
                        "tool_use" => {
                            tool_calls.push(ToolCall {
                                id: block.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                                name: block.name.unwrap_or_default(),
                                args: block.input.unwrap_or(Value::Null),
                            });
                        }
                        _ => {}
                    }
                }

                Ok(LLMResponse {
                    content: if content.is_empty() { None } else { Some(content) },
                    tool_calls,
                    usage: data.usage.map(|u| Usage {
                        prompt_tokens: u.input_tokens,
                        completion_tokens: u.output_tokens,
                        total_tokens: u.input_tokens + u.output_tokens,
                    }),
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
        let (system, msgs) = Self::build_messages(messages);

        let body = AnthropicRequest {
            model: config.name.clone(),
            messages: msgs,
            system,
            tools: if tools.is_empty() { None } else { Some(Self::build_tools(tools)) },
            max_tokens: config.max_tokens.unwrap_or(4096),
            temperature: config.temperature,
            stream: true,
        };

        let resp = self
            .client
            .post(self.build_url())
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .context("sending Anthropic stream request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error {}: {}", status, text);
        }

        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut full_content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut current_tc: Option<ToolCall> = None;
        let mut tc_args = String::new();

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
                if let Ok(event) = serde_json::from_str::<AnthropicStreamEvent>(data) {
                    match event.kind.as_str() {
                        "content_block_start" => {
                            if let Some(block) = &event.content_block {
                                if block.kind == "tool_use" {
                                    current_tc = Some(ToolCall {
                                        id: block.id.clone().unwrap_or_default(),
                                        name: block.name.clone().unwrap_or_default(),
                                        args: Value::Null,
                                    });
                                    tc_args.clear();
                                }
                            }
                        }
                        "content_block_delta" => {
                            if let Some(delta) = &event.delta {
                                if delta.kind == "text_delta" {
                                    if let Some(text) = &delta.text {
                                        full_content.push_str(text);
                                        let _ = tx.send(LLMEvent::ContentDelta(text.clone())).await;
                                    }
                                } else if delta.kind == "input_json_delta" {
                                    if let Some(partial) = &delta.partial_json {
                                        tc_args.push_str(partial);
                                    }
                                }
                            }
                        }
                        "content_block_stop" => {
                            if let Some(mut tc) = current_tc.take() {
                                if !tc_args.is_empty() {
                                    tc.args = serde_json::from_str(&tc_args)
                                        .unwrap_or_else(|_| serde_json::json!({"raw": tc_args.clone()}));
                                }
                                let _ = tx.send(LLMEvent::ToolCallDone(tc.clone())).await;
                                tool_calls.push(tc);
                            }
                        }
                        "message_stop" => break,
                        _ => {}
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

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
}

#[derive(Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContent>,
}

#[derive(Serialize, Deserialize)]
struct AnthropicContent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_use_id: Option<String>,
}

#[derive(Serialize)]
struct AnthropicTool {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    input_schema: Value,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Deserialize)]
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content_block: Option<AnthropicContent>,
    #[serde(default)]
    delta: Option<AnthropicDelta>,
}

#[derive(Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
}
