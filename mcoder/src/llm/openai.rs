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

pub struct OpenAIAdapter {
    config: ModelConfig,
    client: Client,
}

impl OpenAIAdapter {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    fn build_url(&self) -> String {
        format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'))
    }

    fn build_messages(messages: &[Message]) -> Vec<OpenAIMessage> {
        messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    crate::types::Role::User => "user",
                    crate::types::Role::Assistant => "assistant",
                    crate::types::Role::System => "system",
                    crate::types::Role::Tool => "tool",
                };

                let mut msg = OpenAIMessage {
                    role: role.to_string(),
                    content: None,
                    tool_calls: None,
                    tool_call_id: None,
                };

                for block in &m.content {
                    match block {
                        ContentBlock::Text { text } => {
                            msg.content = Some(text.clone());
                        }
                        ContentBlock::ToolUse { id, name, args } => {
                            let oai_tc = OpenAIToolCall {
                                id: id.clone(),
                                r#type: "function".to_string(),
                                function: OAIFunction {
                                    name: name.clone(),
                                    arguments: args.to_string(),
                                },
                            };
                            msg.tool_calls.get_or_insert_with(Vec::new).push(oai_tc);
                        }
                        ContentBlock::ToolResult { id, output } => {
                            msg.tool_call_id = Some(id.clone());
                            msg.content = Some(match output {
                                crate::types::ToolOutput::Sync { result } => result.to_string(),
                                crate::types::ToolOutput::AsyncTask { handle, status_msg, .. } => {
                                    format!("async: {} - {}", handle, status_msg)
                                }
                                crate::types::ToolOutput::Error { message } => {
                                    format!("Error: {}", message)
                                }
                            });
                        }
                    }
                }
                msg
            })
            .collect()
    }

    fn build_tools(tools: &[ToolSchema]) -> Vec<OpenAITool> {
        tools
            .iter()
            .map(|t| OpenAITool {
                r#type: "function".to_string(),
                function: OAIFunctionSchema {
                    name: t.name.clone(),
                    description: Some(t.description.clone()),
                    parameters: t.parameters.clone(),
                },
            })
            .collect()
    }
}

#[async_trait]
impl LLMAdapter for OpenAIAdapter {
    fn name(&self) -> &str {
        "openai"
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &ModelConfig,
    ) -> Result<LLMResponse> {
        let body = OAIRequest {
            model: config.name.clone(),
            messages: Self::build_messages(messages),
            tools: if tools.is_empty() { None } else { Some(Self::build_tools(tools)) },
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            stream: false,
        };
        let body_value = serde_json::to_value(&body).context("serializing OpenAI request")?;
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

                let data: OAIResponse = resp
                    .json()
                    .await
                    .map_err(RetryError::from)?;

                let choice = data
                    .choices
                    .into_iter()
                    .next()
                    .ok_or_else(|| RetryError::Fatal("no choices in OpenAI response".into()))?;
                let content = choice.message.content;
                let tool_calls = choice
                    .message
                    .tool_calls
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tc| ToolCall {
                        id: tc.id,
                        name: tc.function.name,
                        args: serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| {
                            serde_json::json!({"raw": tc.function.arguments})
                        }),
                    })
                    .collect();

                Ok(LLMResponse {
                    content,
                    tool_calls,
                    usage: data.usage.map(|u| Usage {
                        prompt_tokens: u.prompt_tokens,
                        completion_tokens: u.completion_tokens,
                        total_tokens: u.total_tokens,
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
        let body = OAIRequest {
            model: config.name.clone(),
            messages: Self::build_messages(messages),
            tools: if tools.is_empty() { None } else { Some(Self::build_tools(tools)) },
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            stream: true,
        };

        let resp = self
            .client
            .post(self.build_url())
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .send()
            .await
            .context("sending OpenAI stream request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API error {}: {}", status, text);
        }

        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut full_content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tool_args: Vec<String> = Vec::new();

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

                if let Ok(event) = serde_json::from_str::<OAIStreamEvent>(data) {
                    if let Some(choice) = event.choices.first() {
                        if let Some(content) = &choice.delta.content {
                            full_content.push_str(content);
                            let _ = tx.send(LLMEvent::ContentDelta(content.clone())).await;
                        }

                        if let Some(tcs) = &choice.delta.tool_calls {
                            for tc_delta in tcs {
                                while tool_calls.len() <= tc_delta.index {
                                    tool_calls.push(ToolCall {
                                        id: String::new(),
                                        name: String::new(),
                                        args: serde_json::Value::Null,
                                    });
                                    tool_args.push(String::new());
                                }
                                let tc = &mut tool_calls[tc_delta.index];
                                let args_buf = &mut tool_args[tc_delta.index];
                                if let Some(id) = &tc_delta.id {
                                    tc.id = id.clone();
                                }
                                if let Some(name) = &tc_delta.function.name {
                                    tc.name = name.clone();
                                }
                                if let Some(args) = &tc_delta.function.arguments {
                                    args_buf.push_str(args);
                                }
                            }
                        }
                    }
                }
            }
        }

        for (i, tc) in tool_calls.iter_mut().enumerate() {
            if i < tool_args.len() {
                tc.args = serde_json::from_str(&tool_args[i]).unwrap_or_else(|_| {
                    serde_json::json!({"raw": tool_args[i].clone()})
                });
            }
            let _ = tx.send(LLMEvent::ToolCallDone(tc.clone())).await;
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
struct OAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
}

#[derive(Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    r#type: String,
    function: OAIFunction,
}

#[derive(Serialize, Deserialize)]
struct OAIFunction {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct OpenAITool {
    r#type: String,
    function: OAIFunctionSchema,
}

#[derive(Serialize)]
struct OAIFunctionSchema {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    parameters: Value,
}

#[derive(Deserialize)]
struct OAIResponse {
    choices: Vec<OAIChoice>,
    #[serde(default)]
    usage: Option<OAIUsage>,
}

#[derive(Deserialize)]
struct OAIChoice {
    message: OAIMessageContent,
}

#[derive(Deserialize)]
struct OAIMessageContent {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[derive(Deserialize)]
struct OAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Deserialize)]
struct OAIStreamEvent {
    #[serde(default)]
    choices: Vec<OAIStreamChoice>,
}

#[derive(Deserialize)]
struct OAIStreamChoice {
    delta: OAIStreamDelta,
}

#[derive(Deserialize, Default)]
struct OAIStreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OAIStreamToolCall>>,
}

#[derive(Deserialize)]
struct OAIStreamToolCall {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: OAIStreamFunction,
}

#[derive(Deserialize, Default)]
struct OAIStreamFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}
