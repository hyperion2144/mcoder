// 设计文档 §8.2/§8.3: plugin 系统 + Hook 系统（M1）
// - HookHandler 支持 async（shell 命令可能阻塞）
// - ShellHookHandler 从 config 加载，支持 $FILE $SESSION_ID $TOOL $ARGS 变量替换
// - PluginManager 在 agent loop 的各 Hook 点被调用
#![allow(dead_code)]

pub mod hooks;
pub mod mcp;
pub mod skills;

use crate::types::HookConfig;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Agent 生命周期 Hook 点
/// 设计文档 §8.3.3: 与 config.toml 中的 event 字段对应
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPoint {
    /// Agent 启动时（server start）
    OnStart,
    /// 每轮 LLM 调用前
    BeforeLlmCall,
    /// LLM 返回后
    AfterLlmCall,
    /// 工具执行前
    BeforeToolCall,
    /// 工具执行后
    AfterToolCall,
    /// 会话创建时
    OnSessionCreate,
    /// 会话结束时
    OnSessionEnd,
    /// 文件修改前
    BeforeFileChange,
    /// 文件修改后
    AfterFileChange,
    /// Agent 停止时（server shutdown）
    OnStop,
    /// 设计文档 §3.5/§8.3.3: 上下文压缩前
    PreCompact,
}

impl HookPoint {
    /// 从配置中的 event 字符串解析为 HookPoint
    pub fn from_event_str(s: &str) -> Result<Self> {
        match s {
            "session_start" | "on_session_create" => Ok(HookPoint::OnSessionCreate),
            "session_end" | "on_session_end" => Ok(HookPoint::OnSessionEnd),
            "pre_tool_use" | "before_tool_call" => Ok(HookPoint::BeforeToolCall),
            "post_tool_use" | "after_tool_call" => Ok(HookPoint::AfterToolCall),
            "pre_llm_call" | "before_llm_call" => Ok(HookPoint::BeforeLlmCall),
            "post_llm_call" | "after_llm_call" => Ok(HookPoint::AfterLlmCall),
            "on_start" => Ok(HookPoint::OnStart),
            "on_stop" => Ok(HookPoint::OnStop),
            "before_file_change" => Ok(HookPoint::BeforeFileChange),
            "after_file_change" => Ok(HookPoint::AfterFileChange),
            "pre_compact" => Ok(HookPoint::PreCompact),
            other => anyhow::bail!("unknown hook event: {} (valid: session_start/session_end/pre_tool_use/post_tool_use/pre_llm_call/post_llm_call/on_start/on_stop/before_file_change/after_file_change/pre_compact)", other),
        }
    }
}

/// Hook 上下文，传递给 hook 处理器
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContext {
    pub hook: HookPoint,
    pub session_id: String,
    pub data: serde_json::Value,
}

impl HookContext {
    pub fn new(hook: HookPoint, session_id: impl Into<String>) -> Self {
        Self {
            hook,
            session_id: session_id.into(),
            data: serde_json::Value::Null,
        }
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = data;
        self
    }

    /// 从 data 中取字符串字段（用于变量替换）
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.data.get(key).and_then(|v| v.as_str())
    }
}

/// Hook 处理器 trait（async：shell 命令可能阻塞）
#[async_trait]
pub trait HookHandler: Send + Sync {
    fn name(&self) -> &str;
    fn hooks(&self) -> &[HookPoint];
    async fn execute(&self, ctx: &HookContext) -> Result<HookResult>;
}

/// Hook 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    /// 是否允许继续执行（false = 中止后续 hook 和原操作）
    pub allow: bool,
    /// 修改后的数据（覆盖 ctx.data，传给下一个 hook）
    pub modified_data: Option<serde_json::Value>,
    /// 消息（显示给用户）
    pub message: Option<String>,
}

impl Default for HookResult {
    fn default() -> Self {
        Self {
            allow: true,
            modified_data: None,
            message: None,
        }
    }
}

/// 插件 trait（M2+: 完整插件系统；M1 仅用 hooks）
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn init(&self) -> Result<()>;
    fn hooks(&self) -> Vec<Box<dyn HookHandler>> {
        Vec::new()
    }
    fn tools(&self) -> Vec<Box<dyn crate::tools::Tool>> {
        Vec::new()
    }
}

/// 插件管理器
pub struct PluginManager {
    hooks: RwLock<HashMap<HookPoint, Vec<Arc<dyn HookHandler>>>>,
    plugins: RwLock<Vec<Arc<dyn Plugin>>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            hooks: RwLock::new(HashMap::new()),
            plugins: RwLock::new(Vec::new()),
        }
    }

    pub async fn register_plugin(&self, plugin: Arc<dyn Plugin>) -> Result<()> {
        plugin.init()?;
        let handlers = plugin.hooks();
        let mut hooks = self.hooks.write().await;
        for handler in handlers {
            let handler: Arc<dyn HookHandler> = Arc::from(handler);
            for &hp in handler.hooks() {
                hooks.entry(hp).or_default().push(handler.clone());
            }
        }
        self.plugins.write().await.push(plugin);
        Ok(())
    }

    /// 注册单个 HookHandler
    pub async fn register_hook(&self, handler: Arc<dyn HookHandler>) {
        let mut hooks = self.hooks.write().await;
        for &hp in handler.hooks() {
            hooks.entry(hp).or_default().push(handler.clone());
        }
    }

    /// 从配置加载所有 hooks 并注册
    /// 设计文档 §8.3.3: [[hooks]] event + command + block
    pub async fn load_hooks_from_config(&self, configs: &[HookConfig]) -> Result<()> {
        for cfg in configs {
            match hooks::ShellHookHandler::from_config(cfg) {
                Ok(h) => {
                    let hp = h.hook_point;
                    self.register_hook(Arc::new(h)).await;
                    tracing::info!("loaded hook '{}' on {:?}", cfg.event, hp);
                }
                Err(e) => {
                    tracing::warn!("skip hook '{}': {}", cfg.event, e);
                }
            }
        }
        Ok(())
    }

    /// 执行某个 Hook 点的所有处理器
    pub async fn run_hooks(&self, hook: HookPoint, mut ctx: HookContext) -> Result<HookResult> {
        let hooks = self.hooks.read().await;
        let handlers = match hooks.get(&hook) {
            Some(h) => h,
            None => return Ok(HookResult::default()),
        };
        // 释放读锁后逐个执行（避免长时间持锁）
        let handlers: Vec<Arc<dyn HookHandler>> = handlers.clone();
        drop(hooks);

        let mut result = HookResult::default();
        for handler in handlers {
            let r = handler.execute(&ctx).await?;
            if !r.allow {
                return Ok(r);
            }
            if let Some(data) = &r.modified_data {
                ctx.data = data.clone();
            }
            result = r;
        }
        Ok(result)
    }

    pub async fn list_plugins(&self) -> Vec<(String, String)> {
        self.plugins
            .read()
            .await
            .iter()
            .map(|p| (p.name().to_string(), p.version().to_string()))
            .collect()
    }
}
