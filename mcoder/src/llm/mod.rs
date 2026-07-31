// 设计文档 §7.2: LLM 流式接口和 Usage 统计为 forward-looking scaffolding
// 当前 chat() 同步接口已接入 agent loop；chat_stream()/LLMEvent 待流式 UI 接入
#![allow(dead_code)]

pub mod anthropic;
pub mod gemini;
pub mod openai;
pub mod openai_responses;
pub mod retry;

use crate::types::{Message, ModelConfig, ToolCall, ToolSchema};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

pub enum LLMEvent {
    ContentDelta(String),
    ToolCallStart(ToolCall),
    ToolCallDelta(String),
    ToolCallDone(ToolCall),
    Done(LLMResponse),
}

pub struct LLMResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    /// 非缓存的输入 token 数（Anthropic input_tokens 语义）
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    /// Anthropic prompt caching：从缓存读取的输入 token 数（其它 adapter 为 0）
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    /// Anthropic prompt caching：写入缓存的输入 token 数（其它 adapter 为 0）
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
}

impl Usage {
    /// 总输入 token = 非缓存 prompt + cache_read + cache_creation
    /// 用于上下文占用估算与展示
    pub fn total_input(&self) -> u64 {
        self.prompt_tokens as u64
            + self.cache_read_input_tokens as u64
            + self.cache_creation_input_tokens as u64
    }
}

#[async_trait]
pub trait LLMAdapter: Send + Sync {
    /// Short, stable identifier for the adapter family
    /// (e.g. `"openai"`, `"anthropic"`, `"gemini"`).
    fn name(&self) -> &str;

    /// Whether the provider supports caching tool definitions
    /// (Anthropic prompt caching via `cache_control`, etc.).
    /// Default is `false`; supporting adapters override to `true`.
    fn supports_tool_cache(&self) -> bool {
        false
    }

    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &ModelConfig,
    ) -> Result<LLMResponse>;

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &ModelConfig,
        tx: tokio::sync::mpsc::Sender<LLMEvent>,
    ) -> Result<()>;
}

pub type SharedLLM = Arc<dyn LLMAdapter>;

pub fn create_adapter(config: &ModelConfig) -> Result<SharedLLM> {
    use crate::types::ModelProtocol::*;
    // S2 修复: 在 adapter 创建时展开 ${ENV_VAR}，内存配置保留原始形式
    let mut expanded = config.clone();
    expanded.api_key = crate::config::expand_env_var(&expanded.api_key);
    match expanded.protocol {
        OpenaiChat | OpenaiCompatible => {
            Ok(Arc::new(openai::OpenAIAdapter::new(expanded)))
        }
        OpenaiResponses => {
            Ok(Arc::new(openai_responses::OpenAIResponsesAdapter::new(expanded)))
        }
        Anthropic => {
            Ok(Arc::new(anthropic::AnthropicAdapter::new(expanded)))
        }
        Gemini => {
            Ok(Arc::new(gemini::GeminiAdapter::new(expanded)))
        }
    }
}
