// 设计文档 §7.2: 流式响应 struct 为 forward-looking scaffolding
#![allow(dead_code)]

use crate::llm::retry::{self, RetryError};
use crate::llm::{LLMAdapter, LLMEvent, LLMResponse, Usage};
use crate::types::{ContentBlock, Message, ModelConfig, ThinkingDepth, ToolCall, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// OpenAI Responses API adapter (/v1/responses)
/// New API with structured input/output items, supports built-in tools
/// and function calling via the same tool schema.
pub struct OpenAIResponsesAdapter {
    config: ModelConfig,
    client: Client,
}

impl OpenAIResponsesAdapter {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    fn build_url(&self) -> String {
        format!("{}/responses", self.config.base_url.trim_end_matches('/'))
    }

    fn build_input(messages: &[Message]) -> Vec<ResponseInputItem> {
        messages
            .iter()
            .flat_map(|msg| {
                let role = match msg.role {
                    crate::types::Role::User => "user",
                    crate::types::Role::Assistant => "assistant",
                    crate::types::Role::System => "developer",
                    crate::types::Role::Tool => "user",
                };

                msg.content.iter().filter_map(move |block| {
                    match block {
                        ContentBlock::Text { text } => Some(ResponseInputItem {
                            role: role.to_string(),
                            content: vec![ResponseContent {
                                kind: "input_text".to_string(),
                                text: Some(text.clone()),
                                image_url: None,
                            }],
                        }),
                        ContentBlock::Image { path, media_type } => {
                            match std::fs::read(path) {
                                Ok(bytes) => {
                                    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                    let url = format!("data:{};base64,{}", media_type, data);
                                    Some(ResponseInputItem {
                                        role: role.to_string(),
                                        content: vec![ResponseContent {
                                            kind: "input_image".to_string(),
                                            text: None,
                                            image_url: Some(url),
                                        }],
                                    })
                                }
                                Err(e) => {
                                    tracing::warn!("failed to read image {}: {}", path, e);
                                    Some(ResponseInputItem {
                                        role: role.to_string(),
                                        content: vec![ResponseContent {
                                            kind: "input_text".to_string(),
                                            text: Some(format!("[image read error: {}]", path)),
                                            image_url: None,
                                        }],
                                    })
                                }
                            }
                        }
                        ContentBlock::ToolUse { id: _, name, args } => {
                            Some(ResponseInputItem {
                                role: "assistant".to_string(),
                                content: vec![ResponseContent {
                                    kind: "output_text".to_string(),
                                    text: Some(format!("{}({})", name, args)),
                                    image_url: None,
                                }],
                            })
                        }
                        ContentBlock::ToolResult { id: _, output } => {
                            let text = match output {
                                crate::types::ToolOutput::Sync { result } => result.to_string(),
                                crate::types::ToolOutput::AsyncTask { status_msg, .. } => status_msg.clone(),
                                crate::types::ToolOutput::Error { message } => format!("Error: {}", message),
                            };
                            Some(ResponseInputItem {
                                role: "user".to_string(),
                                content: vec![ResponseContent {
                                    kind: "input_text".to_string(),
                                    text: Some(text),
                                    image_url: None,
                                }],
                            })
                        }
                    }
                })
            })
            .collect()
    }

    fn build_tools(tools: &[ToolSchema]) -> Vec<ResponsesTool> {
        tools
            .iter()
            .map(|t| ResponsesTool {
                kind: "function".to_string(),
                name: t.name.clone(),
                description: Some(t.description.clone()),
                parameters: Some(t.parameters.clone()),
            })
            .collect()
    }
}

/// M1 修复: 抽 build_request helper + 统一 thinking_to_native
/// M2 修复: extra 字段与已有字段冲突时忽略
fn build_responses_body(
    config: &ModelConfig,
    input: Vec<ResponseInputItem>,
    tools: Option<Vec<ResponsesTool>>,
    stream: bool,
) -> serde_json::Value {
    use serde_json::json;
    let mut body = json!({
        "model": config.name,
        "input": input,
        "tools": tools,
        "temperature": config.temperature,
        "max_output_tokens": config.max_tokens,
        "stream": stream,
        "top_p": config.top_p,
    });
    let body_obj = body.as_object_mut().unwrap();
    if let Some(depth) = config.thinking_depth {
        for (k, v) in crate::types::thinking_to_native("openai_responses", depth) {
            body_obj.insert(k.to_string(), v);
        }
    }
    let body_keys: std::collections::HashSet<String> = body_obj.keys().cloned().collect();
    for (k, v) in &config.extra {
        if !body_keys.contains(k) {
            body_obj.insert(k.clone(), v.clone());
        }
    }
    body
}

#[async_trait]
impl LLMAdapter for OpenAIResponsesAdapter {
    fn name(&self) -> &str {
        "openai"
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &ModelConfig,
    ) -> Result<LLMResponse> {
        let body_value = build_responses_body(
            config,
            Self::build_input(messages),
            if tools.is_empty() { None } else { Some(Self::build_tools(tools)) },
            false,
        );
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
                    .bearer_auth(&api_key)
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

                let data: ResponsesApiResponse = resp
                    .json()
                    .await
                    .map_err(RetryError::from)?;

                let mut content = String::new();
                let mut tool_calls = Vec::new();

                for item in data.output.unwrap_or_default() {
                    match item.kind.as_str() {
                        "message" => {
                            if let Some(content_items) = &item.content {
                                for c in content_items {
                                    if c.kind == "output_text" {
                                        if let Some(t) = &c.text {
                                            content.push_str(t);
                                        }
                                    }
                                }
                            }
                        }
                        "function_call" => {
                            let args: Value = serde_json::from_str(&item.arguments.clone().unwrap_or_default())
                                .unwrap_or_else(|_| serde_json::json!({}));
                            tool_calls.push(ToolCall {
                                id: item.call_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                                name: item.name.clone().unwrap_or_default(),
                                args,
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
                        total_tokens: u.total_tokens,
                        cache_read_input_tokens: 0,
                        cache_creation_input_tokens: 0,
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
        // Responses API streaming uses SSE with event types
        let body_value = build_responses_body(
            config,
            Self::build_input(messages),
            if tools.is_empty() { None } else { Some(Self::build_tools(tools)) },
            true,
        );

        let resp = self
            .client
            .post(self.build_url())
            .bearer_auth(&self.config.api_key)
            .json(&body_value)
            .send()
            .await
            .context("sending OpenAI Responses stream request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI Responses API error {}: {}", status, text);
        }

        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut full_content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut stream_usage: Option<Usage> = None;

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
                if data == "[DONE]" {
                    break;
                }

                if let Ok(event) = serde_json::from_str::<ResponsesStreamEvent>(data) {
                    match event.kind.as_str() {
                        "response.output_text.delta" => {
                            if let Some(delta) = event.delta {
                                full_content.push_str(&delta);
                                let _ = tx.send(LLMEvent::ContentDelta(delta)).await;
                            }
                        }
                        "response.function_call_arguments.delta" => {
                            // accumulate tool call args
                        }
                        "response.output_item.done" => {
                            if let Some(item) = event.item {
                                if item.kind == "function_call" {
                                    let args: Value = serde_json::from_str(&item.arguments.clone().unwrap_or_default())
                                        .unwrap_or_else(|_| serde_json::json!({}));
                                    let tc = ToolCall {
                                        id: item.call_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                                        name: item.name.clone().unwrap_or_default(),
                                        args,
                                    };
                                    tool_calls.push(tc.clone());
                                    let _ = tx.send(LLMEvent::ToolCallDone(tc)).await;
                                }
                            }
                        }
                        "response.completed" => {
                            // 末尾事件携带 usage
                            if let Some(u) = event.usage {
                                stream_usage = Some(Usage {
                                    prompt_tokens: u.input_tokens,
                                    completion_tokens: u.output_tokens,
                                    total_tokens: u.total_tokens,
                                    cache_read_input_tokens: 0,
                                    cache_creation_input_tokens: 0,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let _ = tx
            .send(LLMEvent::Done(LLMResponse {
                content: if full_content.is_empty() { None } else { Some(full_content) },
                tool_calls,
                usage: stream_usage,
            }))
            .await;

        Ok(())
    }
}

// 注意: ResponsesRequest 已被 build_responses_body (serde_json::Value) 取代，
// 保留为设计文档 §7.2 forward-looking scaffolding，待流式结构稳定后启用。
// 此处不再重复定义，避免与 build_responses_body 产生双源真相。

#[derive(Serialize, Deserialize)]
struct ResponseInputItem {
    role: String,
    content: Vec<ResponseContent>,
}

#[derive(Serialize, Deserialize)]
struct ResponseContent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// 图片 URL（kind="input_image" 时使用，格式 data:<media>;base64,<data>）
    #[serde(skip_serializing_if = "Option::is_none")]
    image_url: Option<String>,
}

#[derive(Serialize)]
struct ResponsesTool {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<Value>,
}

#[derive(Deserialize)]
struct ResponsesApiResponse {
    output: Option<Vec<ResponsesOutputItem>>,
    usage: Option<ResponsesUsage>,
}

#[derive(Deserialize)]
struct ResponsesOutputItem {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content: Option<Vec<ResponseContent>>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct ResponsesUsage {
    input_tokens: u32,
    output_tokens: u32,
    total_tokens: u32,
}

#[derive(Deserialize)]
struct ResponsesStreamEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    delta: Option<String>,
    #[serde(default)]
    item: Option<ResponsesOutputItem>,
    /// response.completed 事件携带的累积 usage
    #[serde(default)]
    usage: Option<ResponsesUsage>,
}
