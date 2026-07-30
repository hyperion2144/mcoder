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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct AppConfig {
    pub default_model: String,
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
}

fn default_image_description_timeout_secs() -> u64 {
    8
}

/// 设计文档 §8.7: 工具安全配置
/// 危险工具（browser_*, screen_*, app_*）默认需用户确认
/// 在 config.toml 中配置 auto_approve 列表可跳过确认
/// [tools]
/// auto_approve = ["browser_open", "browser_navigate", "browser_snapshot"]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsConfig {
    /// 自动批准的危险工具列表（无需用户确认）
    #[serde(default)]
    pub auto_approve: Vec<String>,
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
            tool_results: "summarize".into(),
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
