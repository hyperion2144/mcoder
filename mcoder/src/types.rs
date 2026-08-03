// 设计文档 §3.6: CancellationToken::child 和 Message::assistant 为 forward-looking scaffolding
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 设计文档 §3.9: CancellationToken
/// 一次触发、多次感知的取消信号
/// 基于 tokio::sync::watch，所有工具可共享同一 token
/// 用法：
///   let token = CancellationToken::new();
///   token.cancel();  // 触发取消
///   token.is_cancelled()  // 同步检查
///   token.cancelled().await  // 异步等待取消
#[derive(Clone)]
pub struct CancellationToken {
    tx: Arc<watch::Sender<bool>>,
    rx: watch::Receiver<bool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            tx: Arc::new(tx),
            rx,
        }
    }

    /// 触发取消（不可逆）
    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }

    /// 同步检查是否已取消
    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }

    /// 异步等待取消信号
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let mut rx = self.rx.clone();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    /// 创建 child token：parent 取消时 child 也取消，但 child 取消不影响 parent
    pub fn child(&self) -> Self {
        let (tx, rx) = watch::channel(false);
        let parent_rx = self.rx.clone();
        let child_tx = Arc::new(tx);
        let child_tx_clone = child_tx.clone();
        tokio::spawn(async move {
            let mut parent_rx = parent_rx;
            while !*parent_rx.borrow() {
                if parent_rx.changed().await.is_err() {
                    return;
                }
            }
            let _ = child_tx_clone.send(true);
        });
        Self {
            tx: child_tx,
            rx,
        }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CancellationToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancellationToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        args: serde_json::Value,
    },
    ToolResult {
        id: String,
        output: ToolOutput,
    },
    /// 图片内容块：用户发送或 agent 通过 send_image 工具展示
    /// path 为图片文件路径（服务端保证可访问），media_type 为 MIME 类型
    Image {
        path: String,
        media_type: String,
    },
}

impl ContentBlock {
    /// 返回该块的近似大小（字符数），用于 compaction 前后对比
    pub fn text_len_or_size(&self) -> usize {
        match self {
            ContentBlock::Text { text } => text.len(),
            ContentBlock::ToolUse { name, args, .. } => name.len() + args.to_string().len(),
            ContentBlock::ToolResult { output, .. } => {
                serde_json::to_string(output).map(|s| s.len()).unwrap_or(0)
            }
            ContentBlock::Image { .. } => 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// 消息唯一 id（uuid），用于消息树分叉/切换
    pub id: String,
    /// 父消息 id：None=根消息（会话首条）；非 None 指向上游消息，构成消息树
    pub parent_id: Option<String>,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    /// 该轮 LLM 调用的 token 用量（仅 assistant 消息携带，用于在消息底下展示每轮消耗）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::llm::Usage>,
    /// m12: 仅供 UI 展示、不送入 LLM 上下文的标记（如 send_image 工具插入的 assistant 消息）
    /// #[serde(default)] 让旧 JSONL 文件加载时默认为 false
    #[serde(default)]
    pub display_only: bool,
}

impl Message {
    /// 构造新消息：自动生成 uuid，parent_id=None（由 SessionManager 在 append 前设置）
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            role,
            content,
            usage: None,
            display_only: false,
        }
    }

    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self::new(role, vec![ContentBlock::Text { text: text.into() }])
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::text(Role::User, text)
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::text(Role::Assistant, text)
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::text(Role::System, text)
    }

    /// 设置 usage（链式调用，供 process_response 携带该轮用量）
    pub fn with_usage(mut self, usage: Option<crate::llm::Usage>) -> Self {
        self.usage = usage;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutput {
    Sync {
        result: serde_json::Value,
    },
    AsyncTask {
        task_id: String,
        handle: String,
        status_msg: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 统一思考深度级别（5 档）
/// 映射到各协议的原生思考参数：
/// - OpenAI Chat: reasoning_effort (low/medium/high)
/// - OpenAI Responses: reasoning.effort + reasoning.summary
/// - Anthropic: thinking.budget_tokens (5k/16k/32k/64k)
/// - Gemini: thinkingConfig.thinkingBudget (0/2k/8k/24k/32k)
/// - Ollama: 不支持（忽略）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingDepth {
    #[default]
    None,
    Low,
    Medium,
    High,
    Max,
}

impl ThinkingDepth {
    /// 转为简短标签（UI 显示用）
    /// L1 修复: 与 UI 端 currentThinking.charAt(0).toUpperCase() 一致（用全名而非 "Med"）
    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Max => "Max",
        }
    }
}

/// M1 修复: 统一思考深度 -> 原生 JSON 参数的映射函数（替代散落在 4 个 adapter 里的硬编码）
/// 返回要注入到请求体里的字段列表（key, value 对）
pub fn thinking_to_native(protocol: &str, depth: ThinkingDepth) -> Vec<(&'static str, serde_json::Value)> {
    use serde_json::json;
    let p = protocol.to_ascii_lowercase();
    match depth {
        ThinkingDepth::None => vec![],
        ThinkingDepth::Low => match p.as_str() {
            "anthropic" => vec![("thinking", json!({"type": "enabled", "budget_tokens": 5000}))],
            "gemini" => vec![("thinking_config", json!({"thinkingBudget": 2048}))],
            "openai_responses" => vec![("reasoning", json!({"effort": "low"}))],
            // M4 修复: OpenAI Max 映射到 high（OpenAI 只支持 low/medium/high 三档）
            "ollama" => vec![], // Ollama 不支持思考参数
            _ => vec![("reasoning_effort", json!("low"))],
        },
        ThinkingDepth::Medium => match p.as_str() {
            "anthropic" => vec![("thinking", json!({"type": "enabled", "budget_tokens": 16000}))],
            "gemini" => vec![("thinking_config", json!({"thinkingBudget": 8192}))],
            "openai_responses" => vec![("reasoning", json!({"effort": "medium"}))],
            "ollama" => vec![],
            _ => vec![("reasoning_effort", json!("medium"))],
        },
        ThinkingDepth::High => match p.as_str() {
            "anthropic" => vec![("thinking", json!({"type": "enabled", "budget_tokens": 32000}))],
            "gemini" => vec![("thinking_config", json!({"thinkingBudget": 24576}))],
            "openai_responses" => vec![("reasoning", json!({"effort": "high"}))],
            "ollama" => vec![],
            _ => vec![("reasoning_effort", json!("high"))],
        },
        ThinkingDepth::Max => match p.as_str() {
            "anthropic" => vec![("thinking", json!({"type": "enabled", "budget_tokens": 64000}))],
            "gemini" => vec![("thinking_config", json!({"thinkingBudget": 32768, "includeThoughts": true}))],
            "openai_responses" => vec![("reasoning", json!({"effort": "high", "summary": "detailed"}))],
            "ollama" => vec![],
            _ => vec![("reasoning_effort", json!("high"))],
        },
    }
}

/// L3 修复: 校验 Anthropic thinking.budget_tokens 与 max_tokens 兼容性
/// budget_tokens 必须 < max_tokens，否则 API 拒绝
pub fn validate_thinking_budget(protocol: &str, max_tokens: Option<u32>, budget_tokens: Option<u32>) -> Result<(), String> {
    if protocol != "anthropic" {
        return Ok(());
    }
    let Some(budget) = budget_tokens else { return Ok(()) };
    let Some(max) = max_tokens else { return Ok(()) };
    if budget >= max {
        return Err(format!(
            "thinking.budget_tokens ({budget}) must be < max_tokens ({max})"
        ));
    }
    Ok(())
}

/// 每个模型的参数覆盖（存储在 ProviderConfig.model_params 中）
/// 用于在 provider 层级下为每个模型单独配置生成参数
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelParams {
    /// 思考深度（None = 不启用，默认 None）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_depth: Option<ThinkingDepth>,

    /// 温度（None = 使用协议默认值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// 最大输出 token
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// top_p 采样
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// top_k 采样（Gemini / Ollama 支持）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,

    /// 频率惩罚（OpenAI 支持）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,

    /// 存在惩罚（OpenAI 支持）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,

    /// 停止序列
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stop: Vec<String>,

    /// 上下文窗口大小（覆盖合成的默认 128000）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,

    /// 输入模态（覆盖合成的默认 ["text"]）
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub input: Vec<String>,

    /// 自定义参数透传（协议特有的未知字段，flatten 到请求体顶层）
    /// 例如 Ollama 的 num_ctx / num_gpu / num_thread
    /// 或 OpenAI 的 seed / response_format / logit_bias 等
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub name: String,
    pub protocol: ModelProtocol,
    pub api_key: String,
    pub base_url: String,
    pub context_window: u32,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// 模型支持的输入模态：text / image / audio / video
    /// 默认 ["text"]；视觉模型应配置为 ["text", "image"]
    #[serde(default = "default_input_modalities")]
    pub input: Vec<String>,

    // ===== 扩展生成参数（全部 serde default，向后兼容旧 config.toml）=====

    /// top_p 采样
    #[serde(default)]
    pub top_p: Option<f32>,
    /// top_k 采样（Gemini / Ollama 支持）
    #[serde(default)]
    pub top_k: Option<u32>,
    /// 频率惩罚（OpenAI 支持）
    #[serde(default)]
    pub frequency_penalty: Option<f32>,
    /// 存在惩罚（OpenAI 支持）
    #[serde(default)]
    pub presence_penalty: Option<f32>,
    /// 停止序列
    #[serde(default)]
    pub stop: Vec<String>,
    /// 思考深度（None = 不启用）
    #[serde(default)]
    pub thinking_depth: Option<ThinkingDepth>,
    /// 自定义参数透传（flatten 到请求体顶层）
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn default_input_modalities() -> Vec<String> {
    vec!["text".to_string()]
}

impl ModelConfig {
    /// 是否支持某种输入模态
    pub fn supports(&self, modality: &str) -> bool {
        self.input.iter().any(|m| m == modality)
    }

    /// 是否支持图片输入（便捷方法）
    pub fn supports_image(&self) -> bool {
        self.supports("image")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProtocol {
    /// OpenAI Chat Completions API (/v1/chat/completions)
    OpenaiChat,
    /// OpenAI-compatible Chat Completions (third-party providers like DeepSeek, Moonshot, etc.)
    OpenaiCompatible,
    /// OpenAI Responses API (/v1/responses)
    OpenaiResponses,
    /// Anthropic Messages API (/v1/messages)
    Anthropic,
    /// Google Gemini generateContent API
    Gemini,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleConfig {
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub max_iters: Option<u32>,
    pub timeout: Option<u32>,
    pub loop_condition: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactConfig {
    pub strategy: String,
    pub threshold: f32,
    pub keep_recent: u32,
    pub keep_first: u32,
    pub tool_results: String,
    /// LLM 摘要使用的模型名（None/空 = 用主模型；推荐 fast/cheap 模型如 haiku）
    /// 仅在 strategy="llm_summarize" 时生效
    pub summary_model: Option<String>,
    /// 分级 ToolResult 阈值覆盖（key = tool name, value = 字符阈值）
    /// 留空用默认 800
    pub tool_thresholds: HashMap<String, usize>,
    /// 分层摘要开关（超长 session 启用）
    pub layered_summary: bool,
    /// 每多少条消息生成一层摘要（layered_summary 开启时生效）
    pub layer_chunk_size: usize,
    /// 最多保留多少层历史摘要
    pub max_layers: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    pub compact: bool,
    pub theme: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub default_model: String,
    pub default_provider: Option<String>,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,
    #[serde(default)]
    pub roles: HashMap<String, RoleConfig>,
    #[serde(default)]
    pub loop_max_iters: u32,
    #[serde(default)]
    pub compact: CompactConfig,
    #[serde(default)]
    pub tui: TuiConfig,
    #[serde(default)]
    pub server: ServerConfig,
    /// 设计文档 §8.3: Hook 配置（shell 命令钩子）
    #[serde(default)]
    pub hooks: Vec<HookConfig>,
    /// 设计文档 §8.3: MCP server 配置
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
    /// 设计文档 §8.3: 记忆自动召回/捕获开关
    #[serde(default)]
    pub memory: MemoryConfig,
    /// 设计文档 §8.7: 工具安全配置
    #[serde(default)]
    pub tools: ToolsConfig,
    /// m9: read 工具用视觉模型生成 description 时的超时秒数（默认 8）
    #[serde(default = "default_image_description_timeout_secs")]
    pub image_description_timeout_secs: u64,
    /// Web 搜索配置
    #[serde(default)]
    pub web_search: WebSearchConfig,
    /// launch 工具配置
    #[serde(default)]
    pub launch: LaunchConfig,
    /// 设计文档 §8.8: 权限级别（yolo / standard / strict）
    #[serde(default)]
    pub permission: PermissionConfig,
    /// 界面语言: "en" (默认) | "zh"
    #[serde(default = "default_language")]
    pub language: String,
}

fn default_language() -> String {
    "en".into()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            default_model: "gpt-4o".into(),
            default_provider: None,
            providers: HashMap::new(),
            models: HashMap::new(),
            roles: HashMap::new(),
            loop_max_iters: 50,
            compact: CompactConfig::default(),
            tui: TuiConfig::default(),
            server: ServerConfig::default(),
            hooks: Vec::new(),
            mcp_servers: HashMap::new(),
            memory: MemoryConfig::default(),
            tools: ToolsConfig::default(),
            image_description_timeout_secs: default_image_description_timeout_secs(),
            web_search: WebSearchConfig::default(),
            launch: LaunchConfig::default(),
            permission: PermissionConfig::default(),
            language: default_language(),
        }
    }
}

/// Provider 配置：供应商（含协议、base_url、api_key、自动发现的模型列表）
/// 设计文档 §provider: 一个供应商包含多个 model，统一管理 base_url/api_key
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// 显示名（如 "OpenAI Official"）
    pub name: String,
    /// 协议：openai | anthropic | ollama | gemini | openai_responses | custom
    pub protocol: String,
    pub base_url: String,
    /// API key；支持 ${ENV_VAR} 语法
    pub api_key: String,
    /// 该供应商下用户配置的模型列表（key = model name）
    pub models: Vec<String>,
    /// 是否启用（默认 true）
    pub enabled: bool,
    /// per-model 参数覆盖
    /// key = model name，value = 该模型的参数配置
    /// 合成 ModelConfig 时，这些参数会覆盖默认值
    #[serde(default)]
    pub model_params: HashMap<String, ModelParams>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            protocol: "openai".into(),
            base_url: String::new(),
            api_key: String::new(),
            models: Vec::new(),
            enabled: true,
            model_params: HashMap::new(),
        }
    }
}

impl ProviderConfig {
    /// 协议归一化（大小写不敏感）
    /// M14 修复: 统一转小写后匹配
    pub fn normalized_protocol(&self) -> &str {
        match self.protocol.to_ascii_lowercase().as_str() {
            "openai_responses" => "openai_responses",
            p if p.starts_with("openai") => "openai",
            "anthropic" => "anthropic",
            "ollama" => "ollama",
            "gemini" => "gemini",
            "custom" => "custom",
            _ => "openai",
        }
    }

    /// S2 修复: 检查 model name 是否属于此 provider
    /// 支持纯名（"gpt-4o"）和 "provider/model" 形式
    pub fn has_model(&self, provider_name: &str, model_name: &str) -> bool {
        if self.models.iter().any(|m| m == model_name) {
            return true;
        }
        // "provider/model" 形式
        let prefixed = format!("{provider_name}/{model_name}");
        if let Some(_rest) = prefixed.strip_prefix(&format!("{provider_name}/")) {
            // model_name 本身就是 "provider/model" 形式
            if model_name.starts_with(&format!("{provider_name}/")) {
                let bare = &model_name[provider_name.len() + 1..];
                return self.models.iter().any(|m| m == bare);
            }
        }
        // 也检查 models 列表中是否有 "provider/model" 形式的条目
        self.models.iter().any(|m| m == &prefixed || m == model_name)
    }

    /// M11 修复: 从 ProviderConfig + model name 合成 ModelConfig（共享方法）
    /// M13 修复: "custom" 映射到 OpenaiCompatible 而非 OpenaiChat
    /// P2 修复: 应用 model_params 中的参数覆盖
    pub fn synthesize_model_config(&self, model_name: &str) -> ModelConfig {
        let protocol = match self.normalized_protocol() {
            "openai_responses" => ModelProtocol::OpenaiResponses,
            "anthropic" => ModelProtocol::Anthropic,
            "gemini" => ModelProtocol::Gemini,
            "custom" => ModelProtocol::OpenaiCompatible,
            _ => ModelProtocol::OpenaiChat,
        };
        let base_url = if self.normalized_protocol() == "ollama" {
            let trimmed = self.base_url.trim_end_matches('/');
            if trimmed.ends_with("/v1") {
                trimmed.to_string()
            } else {
                format!("{trimmed}/v1")
            }
        } else {
            self.base_url.clone()
        };
        // P2: 应用 per-model 参数覆盖
        let params = self.model_params.get(model_name).cloned().unwrap_or_default();
        ModelConfig {
            name: model_name.to_string(),
            protocol,
            api_key: self.api_key.clone(),
            base_url,
            context_window: params.context_window.unwrap_or(128_000),
            temperature: params.temperature,
            max_tokens: params.max_tokens,
            input: if params.input.is_empty() { vec!["text".to_string()] } else { params.input.clone() },
            // 扩展参数
            top_p: params.top_p,
            top_k: params.top_k,
            frequency_penalty: params.frequency_penalty,
            presence_penalty: params.presence_penalty,
            stop: params.stop,
            thinking_depth: params.thinking_depth,
            extra: params.extra,
        }
    }
}

/// 协议参数 schema（供 UI 渲染控件用）
/// M5 修复: 统一返回 `{fields: [{name, type, label?, min?, max?, options?, default?, description?}]}` 格式
/// UI 端不再需要兼容多种 schema 格式，直接遍历 fields 渲染控件即可
pub fn protocol_schema(protocol: &str) -> serde_json::Value {
    use serde_json::json;
    let p = protocol.to_ascii_lowercase();
    let fields = |entries: Vec<serde_json::Value>| entries;

    let result = match p.as_str() {
        "openai" | "openai_compatible" | "custom" => fields(vec![
            json!({"name": "thinking_depth", "type": "enum", "options": ["none", "low", "medium", "high"], "default": null, "label": "Thinking Depth"}),
            json!({"name": "temperature", "type": "float", "min": 0.0, "max": 2.0, "default": null, "label": "Temperature"}),
            json!({"name": "max_tokens", "type": "int", "min": 1, "default": null, "label": "Max Tokens"}),
            json!({"name": "top_p", "type": "float", "min": 0.0, "max": 1.0, "default": null, "label": "Top P"}),
            json!({"name": "frequency_penalty", "type": "float", "min": -2.0, "max": 2.0, "default": null, "label": "Frequency Penalty"}),
            json!({"name": "presence_penalty", "type": "float", "min": -2.0, "max": 2.0, "default": null, "label": "Presence Penalty"}),
            json!({"name": "stop", "type": "string_list", "default": [], "label": "Stop Sequences"}),
            json!({"name": "context_window", "type": "int", "min": 1, "default": 128000, "label": "Context Window"}),
            json!({"name": "extra", "type": "object", "default": {}, "label": "Extra", "description": "自定义参数透传（如 seed、response_format、logit_bias）"}),
        ]),
        "openai_responses" => fields(vec![
            json!({"name": "thinking_depth", "type": "enum", "options": ["none", "low", "medium", "high"], "default": null, "label": "Thinking Depth"}),
            json!({"name": "temperature", "type": "float", "min": 0.0, "max": 2.0, "default": null, "label": "Temperature"}),
            json!({"name": "max_tokens", "type": "int", "min": 1, "default": null, "label": "Max Tokens"}),
            json!({"name": "top_p", "type": "float", "min": 0.0, "max": 1.0, "default": null, "label": "Top P"}),
            json!({"name": "context_window", "type": "int", "min": 1, "default": 128000, "label": "Context Window"}),
            json!({"name": "extra", "type": "object", "default": {}, "label": "Extra"}),
        ]),
        "anthropic" => fields(vec![
            json!({"name": "thinking_depth", "type": "enum", "options": ["none", "low", "medium", "high", "max"], "default": null, "label": "Thinking Depth"}),
            json!({"name": "temperature", "type": "float", "min": 0.0, "max": 1.0, "default": null, "label": "Temperature"}),
            json!({"name": "max_tokens", "type": "int", "min": 1, "default": 4096, "label": "Max Tokens", "required": true}),
            json!({"name": "top_p", "type": "float", "min": 0.0, "max": 1.0, "default": null, "label": "Top P"}),
            json!({"name": "top_k", "type": "int", "min": 0, "default": null, "label": "Top K"}),
            json!({"name": "stop", "type": "string_list", "default": [], "label": "Stop Sequences"}),
            json!({"name": "context_window", "type": "int", "min": 1, "default": 128000, "label": "Context Window"}),
            json!({"name": "extra", "type": "object", "default": {}, "label": "Extra"}),
        ]),
        "ollama" => fields(vec![
            json!({"name": "thinking_depth", "type": "enum", "options": ["none"], "default": null, "label": "Thinking Depth"}),
            json!({"name": "temperature", "type": "float", "min": 0.0, "max": 2.0, "default": null, "label": "Temperature"}),
            json!({"name": "max_tokens", "type": "int", "min": 1, "default": null, "label": "Max Tokens"}),
            json!({"name": "top_p", "type": "float", "min": 0.0, "max": 1.0, "default": null, "label": "Top P"}),
            json!({"name": "top_k", "type": "int", "min": 0, "default": null, "label": "Top K"}),
            json!({"name": "context_window", "type": "int", "min": 1, "default": 128000, "label": "Context Window"}),
            json!({"name": "extra", "type": "object", "default": {}, "label": "Extra", "description": "Ollama 特有参数（num_ctx、num_gpu、num_thread、repeat_penalty）"}),
        ]),
        "gemini" => fields(vec![
            json!({"name": "thinking_depth", "type": "enum", "options": ["none", "low", "medium", "high", "max"], "default": null, "label": "Thinking Depth"}),
            json!({"name": "temperature", "type": "float", "min": 0.0, "max": 2.0, "default": null, "label": "Temperature"}),
            json!({"name": "max_tokens", "type": "int", "min": 1, "default": null, "label": "Max Tokens"}),
            json!({"name": "top_p", "type": "float", "min": 0.0, "max": 1.0, "default": null, "label": "Top P"}),
            json!({"name": "top_k", "type": "int", "min": 0, "default": null, "label": "Top K"}),
            json!({"name": "stop", "type": "string_list", "default": [], "label": "Stop Sequences"}),
            json!({"name": "context_window", "type": "int", "min": 1, "default": 128000, "label": "Context Window"}),
            json!({"name": "extra", "type": "object", "default": {}, "label": "Extra"}),
        ]),
        _ => vec![],
    };
    serde_json::json!({ "fields": result })
}

fn default_image_description_timeout_secs() -> u64 {
    8
}

/// 设计文档 §8.8: 权限级别
/// - Yolo: 全部自动执行（最高权限，agent 全权）
/// - Standard: 默认级别；只读工具自动，写/执行类需用户审批
/// - Strict: 所有非只读工具都需审批（最保守）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PermissionLevel {
    Yolo,
    #[default]
    Standard,
    Strict,
}

/// 设计文档 §8.8: 权限配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionConfig {
    /// 当前权限级别（默认 standard）
    pub level: PermissionLevel,
    /// yolo mode 时仍拒绝的工具（兜底白名单；如 mcp_* 未审计工具）
    /// 注意：仅 yolo 生效；standard/strict 不需要
    #[serde(default)]
    pub yolo_deny: Vec<String>,
    /// strict mode 时额外审批的工具列表（默认所有非只读都审批）
    /// 设置后只审批这些，其他写工具自动通过
    #[serde(default)]
    pub strict_require_approval: Vec<String>,
    /// strict mode 时额外自动通过的工具
    #[serde(default)]
    pub strict_auto: Vec<String>,
}

impl Default for PermissionConfig {
    fn default() -> Self {
        Self {
            level: PermissionLevel::Standard,
            yolo_deny: Vec::new(),
            strict_require_approval: Vec::new(),
            strict_auto: Vec::new(),
        }
    }
}

impl PermissionConfig {
    /// 判断工具调用是否需要用户审批
    /// 返回 Some(reason) 表示需要审批；None 表示自动通过
    pub fn requires_approval(&self, tool_name: &str) -> Option<String> {
        match self.level {
            PermissionLevel::Yolo => {
                // yolo mode：除 yolo_deny 兜底外全部自动
                if self.yolo_deny.iter().any(|t| t == tool_name) {
                    Some(format!("tool '{}' is in yolo deny list", tool_name))
                } else {
                    None
                }
            }
            PermissionLevel::Standard => {
                // standard：只读工具自动；写工具审批
                if is_readonly_tool(tool_name) {
                    None
                } else {
                    Some(format!(
                        "tool '{}' modifies state; confirm to execute",
                        tool_name
                    ))
                }
            }
            PermissionLevel::Strict => {
                // strict：默认所有非只读审批
                if self.strict_auto.iter().any(|t| t == tool_name) {
                    return None;
                }
                if !self.strict_require_approval.is_empty() {
                    // 配置了严格列表：只审批列表中的
                    if self.strict_require_approval.iter().any(|t| t == tool_name) {
                        Some(format!("tool '{}' is in strict approval list", tool_name))
                    } else {
                        None
                    }
                } else if is_readonly_tool(tool_name) {
                    None
                } else {
                    Some(format!(
                        "tool '{}' requires approval in strict mode",
                        tool_name
                    ))
                }
            }
        }
    }
}

/// 设计文档 §8.8: 工具分类 - 只读工具（不需要审批）
/// 注意：mcp_* / browser_* / screen_* / app_* 默认需要审批（按 dangerous 规则）
pub fn is_readonly_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read"
            | "grep"
            | "glob"
            | "lsp_diagnostics"
            | "lsp_hover"
            | "lsp_definition"
            | "lsp_references"
            | "lsp_symbols"
            | "lsp_completion"
            | "todo_read"
            | "memory_recall"
            | "memory_search"
            | "code_graph_query"
            | "code_graph_visualize"
            | "workflow_read"
            | "workflow_state"
            | "view_image"
            | "session_list"
            | "session_snapshot"
            | "model_list"
            | "role_list"
            | "ask_user"
            | "plan_read"
            | "ast_query"
    )
}

/// 设计文档 §8.7: 工具安全配置
/// 危险工具（browser_*, screen_*, app_*）默认需用户确认
/// 在 config.toml 中配置 auto_approve 列表可跳过确认
/// [tools]
/// auto_approve = ["browser_open", "browser_navigate", "browser_snapshot"]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    /// 自动批准的危险工具列表（无需用户确认）
    #[serde(default)]
    pub auto_approve: Vec<String>,
    /// LSP 写后异步诊断
    #[serde(default)]
    pub lsp_diagnostics: LspDiagnosticsConfig,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            auto_approve: Vec::new(),
            lsp_diagnostics: LspDiagnosticsConfig::default(),
        }
    }
}

impl ToolsConfig {
    /// 判断工具是否在自动批准列表中
    pub fn is_auto_approved(&self, tool_name: &str) -> bool {
        self.auto_approve.iter().any(|t| t == tool_name)
    }

    /// 设计文档 §8.7: 判断是否为危险工具（需确认）
    /// browser_* / screen_* / app_* 开头的工具都需要确认
    pub fn is_dangerous(tool_name: &str) -> bool {
        tool_name.starts_with("browser_")
            || tool_name.starts_with("screen_")
            || tool_name.starts_with("app_")
    }

    /// LSP 写后异步诊断配置
    pub fn lsp_diagnostics(&self) -> &LspDiagnosticsConfig {
        &self.lsp_diagnostics
    }
}

/// LSP 写后诊断：write/edit 后等 LSP 处理 N ms，异步推送诊断到 LLM context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnosticsConfig {
    /// 是否启用（默认 true）
    #[serde(default = "default_lsp_post_write")]
    pub post_write: bool,
    /// 等 LSP 处理时间（毫秒），rust-analyzer 推荐 1500，tsserver 800
    #[serde(default = "default_lsp_wait_ms")]
    pub wait_ms: u64,
    /// 最小严重度（warning | error | information | hint），低于此不返回
    #[serde(default = "default_lsp_min_severity")]
    pub min_severity: String,
    /// 单文件最多返回 N 条诊断
    #[serde(default = "default_lsp_max_results")]
    pub max_results: usize,
}

impl Default for LspDiagnosticsConfig {
    fn default() -> Self {
        Self {
            post_write: default_lsp_post_write(),
            wait_ms: default_lsp_wait_ms(),
            min_severity: default_lsp_min_severity(),
            max_results: default_lsp_max_results(),
        }
    }
}

fn default_lsp_post_write() -> bool { true }
fn default_lsp_wait_ms() -> u64 { 1500 }
fn default_lsp_min_severity() -> String { "warning".to_string() }
fn default_lsp_max_results() -> usize { 50 }

/// launch 工具配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchConfig {
    /// 每个 session 最多启动 N 个进程
    #[serde(default = "default_launch_max_processes")]
    pub max_processes_per_session: usize,
    /// 每个进程日志缓冲最大行数
    #[serde(default = "default_launch_max_log_lines")]
    pub max_log_lines_per_process: usize,
    /// stop 时优雅关闭超时（ms），超时后 SIGKILL
    #[serde(default = "default_launch_stop_timeout")]
    pub default_stop_timeout_ms: u64,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            max_processes_per_session: default_launch_max_processes(),
            max_log_lines_per_process: default_launch_max_log_lines(),
            default_stop_timeout_ms: default_launch_stop_timeout(),
        }
    }
}

fn default_launch_max_processes() -> usize { 20 }
fn default_launch_max_log_lines() -> usize { 5000 }
fn default_launch_stop_timeout() -> u64 { 3000 }

/// Web 搜索配置
/// 在 ~/.mcoder/config.toml 中配置：
/// [web_search]
/// provider = "tavily"        # tavily | serper | duckduckgo (默认)
/// api_key = "tvly-xxx"       # Tavily 或 Serper 的 API key
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebSearchConfig {
    /// 搜索引擎：tavily | serper | duckduckgo
    /// 默认 duckduckgo（无需 API key，但国内不稳定）
    #[serde(default)]
    pub provider: String,
    /// API key（tavily / serper 需要）
    #[serde(default)]
    pub api_key: String,
}

impl WebSearchConfig {
    pub fn provider(&self) -> &str {
        match self.provider.as_str() {
            "tavily" => "tavily",
            "serper" => "serper",
            _ => "duckduckgo",
        }
    }
    pub fn has_api_key(&self) -> bool {
        !self.api_key.is_empty()
    }
}

/// 设计文档 §8.3.3: Hook 配置（shell 命令）
/// 在 ~/.mcoder/config.toml 或 <project>/.mcoder/config.toml 中配置
/// [[hooks]]
/// event = "post_tool_use"
/// command = "rustfmt $FILE"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// 事件名：session_start / pre_tool_use / post_tool_use / pre_llm_call / post_llm_call / session_end
    pub event: String,
    /// 要执行的 shell 命令（支持 $FILE $SESSION_ID $TOOL $ARGS 变量替换）
    pub command: String,
    /// 是否阻止后续执行（默认 false）
    #[serde(default)]
    pub block: bool,
}

/// 设计文档 §8.3.2: MCP server 配置
/// [mcp_servers.filesystem]
/// command = "npx"
/// args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
///
/// [mcp_servers.remote]
/// url = "http://example.com:8080/sse"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// stdio 模式：启动命令（如 "npx", "node", "python"）
    /// 与 url 互斥：二选一
    #[serde(default)]
    pub command: String,
    /// 命令参数
    #[serde(default)]
    pub args: Vec<String>,
    /// 环境变量
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// SSE 模式：server URL（如 "http://localhost:8080/sse"）
    /// 与 command 互斥
    #[serde(default)]
    pub url: String,
}

/// 设计文档 §8.3.1: 记忆系统配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// 是否在会话开始时自动召回相关记忆
    pub auto_recall: bool,
    /// 自动召回的最大条目数
    pub recall_limit: u32,
    /// 是否在每轮结束后自动捕获关键决策
    pub auto_capture: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            auto_recall: true,
            recall_limit: 5,
            auto_capture: false, // M1 先关闭自动捕获，避免增加延迟
        }
    }
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            strategy: "auto".into(),
            threshold: 0.8,
            keep_recent: 5,
            keep_first: 2,
            tool_results: "tool_aware".into(),
            summary_model: None,
            tool_thresholds: HashMap::new(),
            layered_summary: false,
            layer_chunk_size: 30,
            max_layers: 5,
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 7654,
        }
    }
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            compact: false,
            theme: "default".into(),
        }
    }
}
