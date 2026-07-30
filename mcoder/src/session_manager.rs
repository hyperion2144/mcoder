// 设计文档 §3.4/§3.6: SessionList/PlanCreated 事件和 get_cancellation 为 forward-looking API
#![allow(dead_code)]

use crate::agent::async_tasks::{TaskManager, TaskStatus};
use crate::ask_user::{AskRegistry, AskRequest, AskSubmission};
use crate::agent::role::RoleRegistry;
use crate::agent::AgentSession;
use crate::llm::create_adapter;
use crate::memory::MemoryStore;
use crate::persistence::jsonl::{JsonlSession, SessionMeta};
use crate::persistence::session_state::{SessionStateStore, TodoRecord};
use crate::plugin::PluginManager;
use crate::tools::ToolRegistry;
use crate::types::{AppConfig, CancellationToken, ContentBlock, Message, ModelConfig, Role, ToolOutput};
use crate::workflow::extract_spawn_subagent;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};

// ==================== Phase 2: 统一 SessionSnapshot 类型 ====================
//
// 设计目标：
// - attach 不调用模型；仅聚合 内存状态 + sqlite
// - 字段名与共享 TS 类型 src/rpc/sessionSnapshot.ts 完全一致（snake_case + 单复数约定）
// - offset > 0 时 messages 仅返回增量；其它字段仍是 session 的当前全量最新值

/// Session 内嵌元数据（对应 TS SessionSnapshot.session）
#[derive(Debug, Clone, Serialize)]
pub struct SessionSnapshotSession {
    pub session_id: String,
    pub title: String,
    pub project_path: String,
    pub role: String,
    pub model: String,
    /// "idle" | "running" | "stopped"
    pub loop_state: String,
    /// 上次结束的原因（completed / cancelled / failed / max_iters_reached ...）
    /// None 表示尚未结束过
    pub stop_reason: Option<String>,
}

/// Context 用量与费用估算（agent loop 每轮刷新）
#[derive(Debug, Clone, Serialize)]
pub struct SessionSnapshotContext {
    /// 估算的 token 数（每 4 字符 ≈ 1 token，与 AgentSession::estimate_total_tokens 同口径）
    pub tokens: usize,
    /// 累计费用（USD），Phase 2 暂为 0.0；Phase 4 接入 model pricing
    pub cost: f64,
    /// 模型 context window 大小（来自 model_config.context_window）
    pub context_window: usize,
    /// 累计 LLM usage（真实 token 消耗，跨轮次累加）
    pub usage: crate::llm::Usage,
}

/// 单个 pending Ask 在 snapshot 中的镜像（仅 attach 时返回；不要长驻 store）
#[derive(Debug, Clone, Serialize)]
pub struct SessionSnapshotPendingAsk {
    pub ask_id: String,
    pub tool_call_id: String,
    pub session_id: String,
    pub request: AskRequest,
    pub created_at_ms: i64,
}

/// TaskManager 当前快照（Phase 2 best-effort；Phase 5: per-session DB 完整元数据）
#[derive(Debug, Clone, Serialize)]
pub struct SessionSnapshotTask {
    pub task_id: String,
    pub tool_name: String,
    /// Running | Pending | Completed | Failed | Cancelled | Interrupted
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_json: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_json: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
}

/// 完整 snapshot（对应 TS SessionSnapshot）
#[derive(Debug, Clone, Serialize)]
pub struct SessionSnapshot {
    pub session: SessionSnapshotSession,
    /// offset=None → 全量；offset=Some(n) → 仅返回第 n 条之后的消息
    pub messages: Vec<Message>,
    pub todos: Vec<TodoRecord>,
    pub plan: Option<serde_json::Value>,
    pub pending_ask: Option<SessionSnapshotPendingAsk>,
    pub tasks: Vec<SessionSnapshotTask>,
    pub context: SessionSnapshotContext,
    /// 当前 session 是否允许 send_message（loop_state != running）
    pub can_resume: bool,
}

/// 消息树节点（用于 session.tree 返回）
#[derive(Debug, Clone, Serialize)]
pub struct MessageTreeNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub role: String,
    pub preview: String,
    pub is_head: bool,
}

/// 消息树（用于 session.tree 返回）
#[derive(Debug, Clone, Serialize)]
pub struct MessageTree {
    pub nodes: Vec<MessageTreeNode>,
    pub head_id: Option<String>,
}

/// 生成消息预览（前 80 字符），用于消息树节点展示
fn message_preview(m: &Message) -> String {
    for b in &m.content {
        match b {
            crate::types::ContentBlock::Text { text } => {
                let chars: Vec<char> = text.chars().take(80).collect();
                return chars.into_iter().collect();
            }
            crate::types::ContentBlock::ToolUse { name, .. } => {
                return format!("[tool_use: {}]", name);
            }
            crate::types::ContentBlock::ToolResult { .. } => {
                return "[tool_result]".to_string();
            }
            crate::types::ContentBlock::Image { path, .. } => {
                return format!("[image: {}]", path);
            }
        }
    }
    String::new()
}

/// 设计文档 §8.3: 运行时配置覆盖（config.set 设置，不持久化）
/// key 用点状路径，如 "compact.threshold" / "memory.auto_recall"
static CONFIG_OVERRIDES: tokio::sync::OnceCell<RwLock<HashMap<String, serde_json::Value>>> =
    tokio::sync::OnceCell::const_new();

async fn config_overrides() -> &'static RwLock<HashMap<String, serde_json::Value>> {
    CONFIG_OVERRIDES
        .get_or_init(|| async { RwLock::new(HashMap::new()) })
        .await
}

/// 设计文档 §3.9: 每个 session 持有独立的 CancellationToken
/// session.cancel() 触发 token，agent loop 检查 token 决定是否退出
pub struct SessionEntry {
    pub session: Mutex<AgentSession>,
    pub cancellation: CancellationToken,
    /// 设计文档 §5.5: 记录订阅此 session 的 client 列表（用于多端同步）
    /// 每个 client attach 时增加，close 时减少
    pub client_count: std::sync::atomic::AtomicU32,
    /// 设计文档 §5.5: 防止同一 session 并发运行多个 agent loop
    /// send_message 时 CAS 0→1，loop 结束时置 0；已在运行则返回 409
    pub loop_running: std::sync::atomic::AtomicBool,
    /// 终审修复 #2：spawn_run_loop wrapper 与新 loop 竞态防护。
    /// 单调递增的 generation token；每次 spawn 新 loop 时 +1。
    /// 旧 loop 在执行 rdb sync / broadcast 之前先 token 与 entry.generation 比较，
    /// 若发现已不等（说明自己是上一代被替换），立即短路，绝不写 loop_state 或广播。
    /// 这是比单纯 loop_running CAS 更强的 fencing（CAS 只防"同时 spawn"，
    ///   不能防"旧 loop 还在写，新 loop 已接管"）。
    pub generation: std::sync::atomic::AtomicU64,
    /// todo gate: 上一次"准备结束"时观察到的未完成 todos 指纹
    /// 用于判定"无变化则结束"；指纹 = status|priority|content 拼接
    pub last_unfinished_todo_fingerprint: Mutex<Option<String>>,
    /// todo gate: 当前 strike 计数（指纹变了时 reset）。
    /// 终审修复 #3：每 loop 最多 3 strikes 后结构化提醒 + 结束，不再"1 strike 后即结束"。
    pub todo_gate_strikes: std::sync::atomic::AtomicU32,
    /// Phase 5: per-session TaskManager（绑 AsyncTaskStore，session-scoped 隔离）
    pub task_manager: Arc<TaskManager>,
    /// 启动 / attach 阶段 hydrate 的未注入 task 集合（避免重复推送）
    pub pending_injections: tokio::sync::Mutex<std::collections::HashSet<String>>,
}

/// 设计文档: per-project 资源集合
/// 每个 project_path 对应一组独立的 memory/journal/code_graph/lsp/debug/workflow
/// 通过 project_resources 缓存，实现多项目支持
struct ProjectResources {
    memory_store: Arc<crate::memory::MemoryStore>,
    journal: Arc<crate::tools::journal::FileJournal>,
    code_graph: Arc<crate::code_graph::CodeGraph>,
    lsp_manager: Arc<crate::lsp::LspManager>,
    debug_manager: Arc<crate::debug::DebugManager>,
    workflow: Arc<crate::workflow::WorkflowStore>,
}

impl ProjectResources {
    fn for_project(project_path: &Path) -> Result<Arc<Self>> {
        let project_dir = project_path.join(".mcoder");
        std::fs::create_dir_all(&project_dir)?;
        Ok(Arc::new(Self {
            memory_store: Arc::new(crate::memory::MemoryStore::open(
                &project_dir.join("memory.db"),
            )?),
            journal: Arc::new(crate::tools::journal::FileJournal::new(&project_dir)?),
            code_graph: crate::code_graph::CodeGraph::new(
                &project_dir.join("graph.db"),
                project_path,
            )?,
            lsp_manager: crate::lsp::LspManager::new(project_path.to_path_buf()),
            debug_manager: crate::debug::DebugManager::new(),
            workflow: Arc::new(crate::workflow::WorkflowStore::open(
                &project_dir.join("workflow.db"),
            )?),
        }))
    }
}

pub struct SessionManager {
    sessions: RwLock<HashMap<String, Arc<SessionEntry>>>,
    tools: Arc<ToolRegistry>,
    config: Arc<AppConfig>,
    plugins: Arc<PluginManager>,
    /// Phase 5: per-session TaskManager 映射（session_id → TaskManager）
    task_managers: RwLock<HashMap<String, Arc<TaskManager>>>,
    role_registry: Arc<RoleRegistry>,
    /// 设计文档 §2.1: 全局经验库（跨项目共享，不是 per-project 的）
    experience_store: Arc<MemoryStore>,
    /// 设计文档 §8.3.2: MCP 管理器引用，用于优雅关闭
    mcp_manager: Arc<crate::plugin::mcp::McpManager>,
    /// per-project 资源缓存：project_path → ProjectResources
    project_resources: RwLock<HashMap<PathBuf, Arc<ProjectResources>>>,
    /// Slash command 分发器（元命令 + 自定义命令 + skill 调用）
    command_dispatcher: Arc<crate::commands::CommandDispatcher>,
    event_tx: broadcast::Sender<ServerEvent>,
    /// ask_user 工具的 pending 池（per-session）
    ask_registry: Arc<AskRegistry>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Message {
        session_id: String,
        message: Message,
    },
    ToolCallStart {
        session_id: String,
        name: String,
    },
    ToolCallDone {
        session_id: String,
        name: String,
        success: bool,
    },
    SessionCreated {
        session_id: String,
        title: String,
    },
    SessionList {
        sessions: Vec<SessionMeta>,
    },
    /// 设计文档 §3.6: plan 创建后广播，等待用户 approve
    PlanCreated {
        session_id: String,
        plan: serde_json::Value,
    },
    /// 设计文档 §3.4: role 切换后广播
    RoleChanged {
        session_id: String,
        role: String,
    },
    /// 设计文档 §3.5: agent loop 结束通知（无工具调用、达到 max_iters、被取消等）
    /// unfinished_todos: 该 session 当前未完成（pending/in_progress）的 todos 完整结构
    ///   - 用于 client 显示"未完成清单"提示
    ///   - 服务端在 todo gate 判定"无变化则结束"时会同步附加一条 system 提醒消息
    SessionDone {
        session_id: String,
        reason: String,
        #[serde(default)]
        unfinished_todos: Vec<crate::persistence::session_state::TodoRecord>,
    },
    /// Todo 工具变更（add/update/remove/replace/clear_completed）后广播
    /// 客户端按 attached session 过滤，更新摘要条 + 完整 Todo 视图
    TodoUpdated {
        session_id: String,
        todos: Vec<crate::persistence::session_state::TodoRecord>,
        summary: crate::persistence::session_state::TodoSummary,
    },
    /// ask_user 工具：等待用户回答（per-session 阻塞，client 渲染 Ask 卡片）
    AskPending {
        session_id: String,
        ask_id: String,
        tool_call_id: String,
        request: AskRequest,
    },
    /// ask_user 工具：用户已提交答案（无论 cancelled / 完整回答）
    AskAnswered {
        session_id: String,
        ask_id: String,
        tool_call_id: String,
        submission: AskSubmission,
        /// 与 tool result 同步：LLM 拿到的结构化答案
        result: serde_json::Value,
    },
    /// ask_user 工具：被取消（仅外部 cancel / session cancel 触发，与 AskAnswered.cancelled 配套）
    AskCancelled {
        session_id: String,
        ask_id: String,
        tool_call_id: String,
    },
    /// LLM usage 报告：每轮 LLM 调用后广播 delta + cumulative + context_window
    /// 客户端用于更新上下文使用率（圆环/百分比）与 cost 展示
    UsageUpdated {
        session_id: String,
        delta: crate::llm::Usage,
        cumulative: crate::llm::Usage,
        context_window: usize,
    },
    Error {
        message: String,
    },
}

/// RAII guard：构造时 armed=true；drop 时若仍 armed 则把 flag 重置为 false。
/// 用于 send_message / send_message_with_images 中 CAS 成功后保证早退路径（ask/错误）
/// 也会重置 loop_running，避免会话永久 409 死锁。进入 spawn_run_loop 前设 armed=false
/// 把所有权移交给 loop task（task 结束时自行重置）。
struct LoopGuard<'a> {
    flag: &'a std::sync::atomic::AtomicBool,
    armed: bool,
}
impl Drop for LoopGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

fn loop_running_guard(flag: &std::sync::atomic::AtomicBool) -> LoopGuard<'_> {
    LoopGuard { flag, armed: true }
}

/// 清理 24h 以上的旧图片临时文件（best-effort，失败仅记录日志）
async fn cleanup_old_images(dir: &std::path::Path) {
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(86400);
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        if let Ok(meta) = entry.metadata().await {
            if let Ok(mtime) = meta.modified() {
                if mtime < cutoff {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
        }
    }
}

impl SessionManager {
    /// 读取当前 session 的未完成 todos（用于 SessionDone 附加字段）
    /// 数据库读取失败时返回空 Vec（不影响主流程）
    async fn unfinished_todos(&self, session_id: &str) -> Vec<crate::persistence::session_state::TodoRecord> {
        let store = match crate::persistence::session_state::SessionStateStore::for_session(session_id).await {
            Some(p) => p,
            None => return Vec::new(),
        };
        store.list_unfinished_todos(session_id).await.unwrap_or_default()
    }

    /// 发送 SessionDone，自动附加 unfinished_todos
    async fn emit_session_done(&self, session_id: &str, reason: &str) {
        let unfinished = self.unfinished_todos(session_id).await;
        // Phase 2: 同步持久化 loop_state=stopped + stop_reason
        self.persist_loop_state(session_id, "stopped", Some(reason)).await;
        let _ = self.event_tx.send(ServerEvent::SessionDone {
            session_id: session_id.to_string(),
            reason: reason.to_string(),
            unfinished_todos: unfinished,
        });
    }

    /// Phase 2: 持久化 loop_state / stop_reason 到 session_state SQLite
    /// 创建/运行/结束/cancel/fail 各生命周期点调用。
    /// store 打开失败时仅记 log，不影响主流程。
    async fn persist_loop_state(&self, session_id: &str, loop_state: &str, stop_reason: Option<&str>) {
        if let Some(store) = SessionStateStore::for_session(session_id).await {
            if let Err(e) = store.set_session_state(session_id, loop_state, stop_reason).await {
                tracing::warn!("session_state upsert failed for {}: {}", session_id, e);
            }
        } else {
            tracing::debug!("session_state store unavailable for {}; skip persist", session_id);
        }
    }
    pub fn new(
        tools: Arc<ToolRegistry>,
        config: Arc<AppConfig>,
        plugins: Arc<PluginManager>,
        role_registry: Arc<RoleRegistry>,
        experience_store: Arc<MemoryStore>,
        mcp_manager: Arc<crate::plugin::mcp::McpManager>,
        command_dispatcher: Arc<crate::commands::CommandDispatcher>,
        ask_registry: Arc<AskRegistry>,
    ) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(1024);
        Arc::new(Self {
            sessions: RwLock::new(HashMap::new()),
            tools,
            config,
            plugins,
            task_managers: RwLock::new(HashMap::new()),
            role_registry,
            experience_store,
            mcp_manager,
            project_resources: RwLock::new(HashMap::new()),
            command_dispatcher,
            event_tx,
            ask_registry,
        })
    }

    /// 分发 slash command（/xxx 输入，不含前导 /）
    /// 返回 DispatchResult，由 ws_server 层执行对应的 RPC 操作
    pub async fn dispatch_command(&self, input: &str) -> Result<crate::commands::DispatchResult> {
        self.command_dispatcher.dispatch(input).await
    }

    /// 列出所有可用命令（元命令 + 自定义命令 + user-invocable skills）
    pub async fn list_commands(&self) -> Vec<serde_json::Value> {
        self.command_dispatcher.list_all().await
    }

    /// 设计文档 §8.3.2: 优雅关闭所有 MCP server / LSP server / DAP adapter
    /// 在 server shutdown 时调用
    pub async fn shutdown(&self) {
        tracing::info!("shutting down session manager...");
        // 触发 OnStop hook
        let _ = self.plugins.run_hooks(
            crate::plugin::HookPoint::OnStop,
            crate::plugin::HookContext::new(crate::plugin::HookPoint::OnStop, ""),
        ).await;
        // 关闭所有 MCP server
        self.mcp_manager.shutdown_all().await;
        // 设计文档 §8.4.2/§8.4.3: 遍历所有 project_resources 关闭 LSP server 和 DAP adapter
        let map = self.project_resources.read().await;
        for (_, res) in map.iter() {
            res.lsp_manager.shutdown_all().await;
            res.debug_manager.shutdown_all().await;
        }
        tracing::info!("session manager shutdown complete");
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.event_tx.subscribe()
    }

    /// 暴露 event_tx 给 ask_user 工具做 late binding
    pub fn event_tx(&self) -> broadcast::Sender<ServerEvent> {
        self.event_tx.clone()
    }

    /// 解析模型配置：先按名称查 config.models，找不到则用 default_model
    fn resolve_model(&self, model_name: Option<&str>) -> Result<ModelConfig> {
        let name = model_name.unwrap_or(&self.config.default_model);
        if let Some(m) = self.config.models.get(name) {
            return Ok(m.clone());
        }
        // 没找到则用 default_model 的配置，若也没有则返回错误
        if let Some(m) = self.config.models.get(&self.config.default_model) {
            return Ok(m.clone());
        }
        anyhow::bail!(
            "model '{}' not found in config, and default_model '{}' also missing",
            name,
            self.config.default_model
        );
    }

    /// 获取或创建指定项目的 per-project 资源（双检锁避免重复创建）
    async fn get_or_create_resources(&self, project_path: &Path) -> Result<Arc<ProjectResources>> {
        {
            let map = self.project_resources.read().await;
            if let Some(res) = map.get(project_path) {
                return Ok(res.clone());
            }
        }
        let mut map = self.project_resources.write().await;
        if let Some(res) = map.get(project_path) {
            return Ok(res.clone());
        }
        let res = ProjectResources::for_project(project_path)?;
        map.insert(project_path.to_path_buf(), res.clone());
        Ok(res)
    }

    /// 构造 ToolContext：从 entry 获取 project_path，组装 per-project + 全局资源
    async fn build_tool_context(
        &self,
        session_id: &str,
        entry: &Arc<SessionEntry>,
    ) -> Result<crate::tools::ToolContext> {
        let (project_path, current_model) = {
            let agent = entry.session.lock().await;
            (
                agent.session.project_path().to_path_buf(),
                agent.model_config.clone(),
            )
        };
        let resources = self.get_or_create_resources(&project_path).await?;
        let project_dir = project_path.join(".mcoder");
        let project_hash = crate::persistence::jsonl::escape_project_path(&project_path);

        // per-session session_state store：若打开失败（极罕见：磁盘满/权限拒绝），返回 anyhow 错误
        let session_state = crate::persistence::session_state::SessionStateStore::for_session(session_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("cannot open session_state store for session {}", session_id))?;

        Ok(crate::tools::ToolContext {
            session_id: session_id.to_string(),
            tool_call_id: None,
            project_path,
            project_dir,
            project_hash,
            journal: resources.journal.clone(),
            memory_store: resources.memory_store.clone(),
            experience_store: self.experience_store.clone(),
            code_graph: resources.code_graph.clone(),
            lsp_manager: resources.lsp_manager.clone(),
            debug_manager: resources.debug_manager.clone(),
            // Phase 5: per-session TaskManager（每个 session 独立 DB 存储）
            task_manager: entry.task_manager.clone(),
            workflow: resources.workflow.clone(),
            session_state: Arc::new(session_state),
            event_tx: self.event_tx.clone(),
            cancellation: entry.cancellation.clone(),
            app_config: self.config.clone(),
            mcp_manager: Some(self.mcp_manager.clone()),
            current_model,  // already Arc<ModelConfig> from agent.model_config.clone()
        })
    }

    pub async fn create_session(
        self: &Arc<Self>,
        project: &Path,
        title: &str,
        model_name: Option<&str>,
    ) -> Result<String> {
        let model_config = Arc::new(self.resolve_model(model_name)?);
        let jsonl = JsonlSession::create(project, title, &model_config.name)?;
        let session_id = jsonl.id().to_string();

        // 获取或创建该项目的 per-project 资源
        let _resources = self.get_or_create_resources(project).await?;

        let llm = create_adapter(&model_config)?;
        let max_iters = self.config.loop_max_iters;
        let agent = AgentSession::new(jsonl, model_config, llm, self.tools.clone(), max_iters, self.role_registry.clone());

        // Phase 5: 创建 per-session TaskManager（绑 AsyncTaskStore；同 session 共享 DB）
        let task_manager = self.get_or_create_task_manager(&session_id).await?;

        let entry = Arc::new(SessionEntry {
            session: Mutex::new(agent),
            cancellation: CancellationToken::new(),
            client_count: std::sync::atomic::AtomicU32::new(0),
            loop_running: std::sync::atomic::AtomicBool::new(false),
            generation: std::sync::atomic::AtomicU64::new(0),
            last_unfinished_todo_fingerprint: Mutex::new(None),
            todo_gate_strikes: std::sync::atomic::AtomicU32::new(0),
            task_manager: task_manager.clone(),
            pending_injections: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        });

        self.sessions
            .write()
            .await
            .insert(session_id.clone(), entry);

        // Phase 2: 持久化 loop_state=idle（新建 session 默认 idle）
        self.persist_loop_state(&session_id, "idle", None).await;

        // 设计文档 §8.3.3: 触发 OnSessionCreate hook
        let hook_ctx = crate::plugin::HookContext::new(
            crate::plugin::HookPoint::OnSessionCreate,
            &session_id,
        ).with_data(serde_json::json!({"title": title}));
        let _ = self.plugins.run_hooks(
            crate::plugin::HookPoint::OnSessionCreate,
            hook_ctx,
        ).await;

        let _ = self.event_tx.send(ServerEvent::SessionCreated {
            session_id: session_id.clone(),
            title: title.to_string(),
        });

        Ok(session_id)
    }

    /// 启动时枚举 JsonlSession::list(None) 的唯一项目路径，
    /// 逐个打开对应的 session_state.db 并 mark_orphans_interrupted。
    /// attach 时 `get_or_create_task_manager` 也会做一次扫描，保留兜底。
    pub async fn mark_startup_orphans(self: &Arc<Self>) -> Result<()> {
        use std::collections::HashSet;
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for meta in JsonlSession::list(None)? {
            if seen.insert(meta.project_path.clone()) {
                if let Some(store) = crate::persistence::session_state::SessionStateStore::open_at(
                    crate::persistence::session_state::session_state_db_path_for_project(&meta.project_path),
                )
                .await
                {
                    let async_store = crate::persistence::async_task_store::AsyncTaskStore::new(
                        store.pool().clone(),
                    );
                    match async_store
                        .mark_orphans_interrupted(chrono::Utc::now().timestamp_millis())
                        .await
                    {
                        Ok(count) => tracing::info!(
                            "startup orphan sweep: project={} marked={} interrupted",
                            meta.project_path.display(),
                            count
                        ),
                        Err(e) => tracing::warn!(
                            "startup orphan sweep failed for {}: {}",
                            meta.project_path.display(),
                            e
                        ),
                    }
                }
            }
        }
        Ok(())
    }

    /// Phase 5c: 获取或创建 per-session TaskManager
    ///
    /// 关键变更：与 SessionStateStore 共享同一 SqlitePool 缓存（同一
    /// session_state.db）。这样写 todos / write tasks 走同一连接池，
    /// 避免不同连接池的锁顺序竞争。
    pub async fn get_or_create_task_manager(&self, session_id: &str) -> Result<Arc<TaskManager>> {
        // 快速路径：已存在
        {
            let map = self.task_managers.read().await;
            if let Some(m) = map.get(session_id) {
                return Ok(m.clone());
            }
        }
        // 慢速路径：双检锁
        let mut map = self.task_managers.write().await;
        if let Some(m) = map.get(session_id) {
            return Ok(m.clone());
        }
        // Phase 5c: 通过 SessionStateStore 打开共享 pool；AsyncTaskStore
        // 复用同一 pool（共享池缓存保证指向同一文件）
        let state_store = match crate::persistence::session_state::SessionStateStore::for_session(session_id).await {
            Some(s) => s,
            None => anyhow::bail!("cannot open session_state for task_manager: {}", session_id),
        };
        let pool = state_store.pool().clone();
        let store = Arc::new(crate::persistence::async_task_store::AsyncTaskStore::new(pool));
        // 原子标记 orphans（queued/running → interrupted）
        if let Err(e) = store
            .mark_orphans_interrupted(chrono::Utc::now().timestamp_millis())
            .await
        {
            tracing::warn!("mark_orphans_interrupted failed for {}: {}", session_id, e);
        }
        let mgr = TaskManager::new_for_session(session_id, store);
        // P1-8: 重新 hydrate DB 中已终态但未注入的 task（重启后 LLM 仍能看到结果）
        mgr.list_undelivered().await;
        map.insert(session_id.to_string(), mgr.clone());
        Ok(mgr)
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        JsonlSession::list(None)
    }

    pub async fn list_tools(&self) -> Vec<crate::types::ToolSchema> {
        self.tools.list_schemas()
    }

    pub async fn call_tool(&self, session_id: &str, name: &str, args: serde_json::Value) -> Result<crate::types::ToolOutput> {
        let entry = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned().context("session not found")?
        };
        let ctx = self.build_tool_context(session_id, &entry).await?;
        let call = crate::types::ToolCall {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            args,
        };
        self.tools.execute(&call, &ctx).await
    }

    pub async fn get_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            let agent = entry.session.lock().await;
            Ok(agent.messages.clone())
        } else {
            anyhow::bail!("session not found: {}", session_id)
        }
    }

    /// 消息树：返回所有消息节点 + 当前 head_id（用于客户端树视图）
    pub async fn get_message_tree(&self, session_id: &str) -> Result<MessageTree> {
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            let agent = entry.session.lock().await;
            let head_id = agent.current_head_id.clone();
            let nodes: Vec<MessageTreeNode> = agent.messages.iter().map(|m| {
                let preview = message_preview(m);
                MessageTreeNode {
                    id: m.id.clone(),
                    parent_id: m.parent_id.clone(),
                    role: format!("{:?}", m.role).to_lowercase(),
                    preview,
                    is_head: head_id.as_deref() == Some(&m.id),
                }
            }).collect();
            Ok(MessageTree { nodes, head_id })
        } else {
            anyhow::bail!("session not found: {}", session_id)
        }
    }

    /// 切换消息分支：将 current_head_id 切到指定消息。
    /// 不剪枝内存消息（保留全树供 get_message_tree / 再次 checkout 兄弟分支）；
    /// run_once 在调用 LLM 前会仅取 root->head 路径上的消息。
    /// 返回新 SessionSnapshot（messages 为路径上的消息）。
    pub async fn checkout(self: &Arc<Self>, session_id: &str, message_id: &str) -> Result<SessionSnapshot> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).context("session not found")?;

        // 竞态保护：loop 运行中禁止 checkout，避免 run_once 迭代间消息列表/head 被改
        if entry.loop_running.load(std::sync::atomic::Ordering::SeqCst) {
            anyhow::bail!("cannot checkout while agent loop is running for session {}", session_id);
        }
        let mut agent = entry.session.lock().await;

        // 在独立块内构建 id→message 索引并反向追溯根→target 路径，
        // 块结束时 by_id/path 的不可变借用一并释放，之后才能可变借用 agent
        let path_msgs: Vec<Message> = {
            let by_id: std::collections::HashMap<&str, &Message> =
                agent.messages.iter().map(|m| (m.id.as_str(), m)).collect();

            let target = by_id.get(message_id).context("message not found in tree")?;

            let mut path: Vec<&Message> = Vec::new();
            let mut cur = Some(*target);
            while let Some(m) = cur {
                path.push(m);
                cur = m.parent_id.as_deref().and_then(|pid| by_id.get(pid).copied());
            }
            path.reverse();
            path.into_iter().cloned().collect()
        };

        // 更新 head（内存 + 持久化），不替换 agent.messages（保留全树供 tree 视图/兄弟分支 checkout）
        agent.current_head_id = Some(message_id.to_string());
        agent.session.update_head_id(message_id)?;

        drop(agent);
        drop(sessions);

        // 构建 snapshot（复用路径消息）
        self.build_snapshot(session_id, path_msgs).await
    }

    /// 设计文档 §3.4: 切换 session 的 role（/mode 命令）
    /// 终审修复 #15：role 切换持久化到 session_state SQLite（key='role'），
    /// 服务重启后 snapshot / attach 复原角色，避免 model 与 tools 不一致。
    pub async fn set_role(&self, session_id: &str, role_name: &str) -> Result<()> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).context("session not found")?;
        let mut agent = entry.session.lock().await;

        // vision role 校验：当前模型必须支持图片输入
        if role_name == "vision" && !agent.model_config.supports_image() {
            anyhow::bail!(
                "vision role requires a model with image input modality. Current model '{}' has input={:?}. Please switch to a vision-capable model first (e.g. via /model set <name>).",
                agent.model_config.name,
                agent.model_config.input
            );
        }

        agent.switch_role(role_name)?;
        drop(agent); // 显式释放
        // 持久化 role 到 session_state（"key_value" 表）
        if let Some(store) = SessionStateStore::for_session(session_id).await {
            if let Err(e) = store.set_kv(session_id, "role", role_name).await {
                tracing::warn!("session_state set role persist failed: {}", e);
            }
        }
        // 广播 RoleChanged 事件（确保所有订阅 client 同步看到）
        let _ = self.event_tx.send(ServerEvent::RoleChanged {
            session_id: session_id.to_string(),
            role: role_name.to_string(),
        });
        tracing::info!("session {} switched to role: {}", session_id, role_name);
        Ok(())
    }

    /// 获取当前 role
    pub async fn current_role(&self, session_id: &str) -> Result<String> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).context("session not found")?;
        let agent = entry.session.lock().await;
        Ok(agent.current_role.clone())
    }

    /// 列出所有可用 role
    pub fn list_roles(&self) -> Vec<String> {
        self.role_registry.list().iter().map(|r| r.name.clone()).collect()
    }

    pub async fn send_message(
        self: &Arc<Self>,
        session_id: &str,
        content: &str,
    ) -> Result<()> {
        let entry = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .cloned()
                .context("session not found")?
        };

        // 设计文档 §5.5: 防止并发 agent loop
        // CAS 0→1，若已在运行则返回 409 Conflict
        if entry.loop_running.compare_exchange(
            false, true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        ).is_err() {
            anyhow::bail!("agent loop already running for session {} (409 Conflict)", session_id);
        }
        // RAII guard：早退（ask/错误）时自动重置 loop_running=false；
        // 进入 spawn_run_loop 前 disown（所有权移交给 loop task）。
        // 复用定义见下方 send_message_with_images 中同名 struct（local type）。
        let mut guard = loop_running_guard(&entry.loop_running);

        // 设计文档 §3.7: loop 已结束后完成的任务结果仍需追加，下次用户消息时模型可见
        // 在添加用户消息前，先 drain 上一轮 loop 结束后完成的异步任务
        self.inject_completed_tasks(session_id, &entry).await?;

        // ask_user 特殊处理：若该 session 当前有 pending Ask，普通文本输入
        // 应被视为对 Ask 的"其他"自由文本，不创建新 loop、不发新 user message
        if self.try_handle_text_for_pending_ask(session_id, content).await {
            return Ok(()); // guard drop 时重置 loop_running
        }

        // 设计文档 §8.3.1: 记忆自动召回
        // 会话首条用户消息时，搜索相关记忆并注入为 system 消息
        if self.config.memory.auto_recall {
            self.inject_recalled_memory(session_id, &entry, content).await?;
        }

        let user_msg = Message::user(content);
        {
            let mut agent = entry.session.lock().await;
            agent.add_message(user_msg.clone())?;
        }

        let _ = self.event_tx.send(ServerEvent::Message {
            session_id: session_id.to_string(),
            message: user_msg,
        });

        // Inject workflow context if configured (session_start hook)
        let _ = self.inject_workflow_context(session_id, &entry).await;

        let mgr = self.clone();
        let sid = session_id.to_string();
        let entry_clone = entry.clone();
        // Phase 2: 进入 loop 前持久化 loop_state=running
        mgr.persist_loop_state(&sid, "running", None).await;
        // 把 loop_running 的所有权移交给 loop task
        guard.armed = false;
        mgr.spawn_run_loop(sid, entry_clone);

        Ok(())
    }

    /// 发送含图片的用户消息：将 base64 图片数据保存为临时文件，构造 ContentBlock 列表后发送。
    /// images: Vec<(base64_data, media_type)>
    pub async fn send_message_with_images(
        self: &Arc<Self>,
        session_id: &str,
        content: &str,
        images: Vec<(String, String)>,
    ) -> Result<()> {
        use base64::Engine;

        let entry = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned().context("session not found")?
        };

        // CAS 先行：成功后再写图片文件，避免 session 不存在/CAS 失败时产生孤儿文件（C3）
        if entry.loop_running.compare_exchange(
            false, true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        ).is_err() {
            anyhow::bail!("agent loop already running for session {} (409 Conflict)", session_id);
        }
        let mut guard = loop_running_guard(&entry.loop_running);

        // 保存图片到 ~/.mcoder/tmp/images/（CAS 成功后才写盘）
        let home = dirs::home_dir().context("cannot determine home directory")?;
        let img_dir = home.join(".mcoder").join("tmp").join("images");
        tokio::fs::create_dir_all(&img_dir).await
            .context("failed to create image temp dir")?;

        // 清理 24h 以上的旧图片临时文件（best-effort，失败不影响发送）
        cleanup_old_images(&img_dir).await;

        // 构造内容块：仅当文本非空才追加 Text（m6）
        let mut blocks: Vec<ContentBlock> = Vec::new();
        if !content.is_empty() {
            blocks.push(ContentBlock::Text { text: content.to_string() });
        }
        for (data, media_type) in images {
            let ext = match media_type.as_str() {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "image/gif" => "gif",
                "image/webp" => "webp",
                "image/bmp" => "bmp",
                _ => "png",
            };
            let filename = format!("{}.{}", uuid::Uuid::new_v4(), ext);
            let path = img_dir.join(&filename);
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .context("invalid base64 image data")?;
            tokio::fs::write(&path, &bytes).await
                .context("failed to write image file")?;
            blocks.push(ContentBlock::Image {
                path: path.to_string_lossy().to_string(),
                media_type,
            });
        }

        self.inject_completed_tasks(session_id, &entry).await?;

        if self.try_handle_text_for_pending_ask(session_id, content).await {
            return Ok(()); // guard drop 重置 loop_running
        }

        if self.config.memory.auto_recall {
            self.inject_recalled_memory(session_id, &entry, content).await?;
        }

        let user_msg = Message::new(crate::types::Role::User, blocks);
        {
            let mut agent = entry.session.lock().await;
            agent.add_message(user_msg.clone())?;
        }

        let _ = self.event_tx.send(ServerEvent::Message {
            session_id: session_id.to_string(),
            message: user_msg,
        });

        // Inject workflow context if configured (session_start hook)
        let _ = self.inject_workflow_context(session_id, &entry).await;

        let mgr = self.clone();
        let sid = session_id.to_string();
        let entry_clone = entry.clone();
        mgr.persist_loop_state(&sid, "running", None).await;
        guard.armed = false;
        mgr.spawn_run_loop(sid, entry_clone);

        Ok(())
    }

    /// 设计文档 Phase 3: session.resume
    ///
    /// 行为矩阵（与 `decide_resume` 一致）：
    /// 1. `loop_running=true` 或 `loop_state ∈ {running, waiting_for_user}` → 409 Conflict
    /// 2. `stop_reason ∈ {blocked, cancelled, failed, unfinished_todos}` 或
    ///    `unfinished_todos > 0` → 持久化 running，向 JSONL/内存追加唯一系统消息
    ///    `[session resumed]`（含 stop_reason + unfinished 列表 + 避免重复已完成工作
    ///    的提示），复用 `spawn_run_loop` 启动入口；**绝不伪造 user message，CAS
    ///    已被 `loop_running` CAS 占用，绝不重复 CAS**
    /// 3. `loop_state ∈ {completed, idle, stopped}` 且无未完成工作 → 不启动模型，
    ///    返回 `{started:false, requires_user_input:true}`
    /// 4. `loop_state == waiting_for_user` → 返回 `{started:false, waiting_for_user:true}`
    ///    （保留 ask 流程）
    ///
    /// 返回值是一个 `serde_json::Value`，由 RPC 层直接透传给客户端；客户端据此
    /// 更新 UI（`loop_state` / `can_resume` 等）和消息流。
    pub async fn resume_session(
        self: &Arc<Self>,
        session_id: &str,
    ) -> Result<serde_json::Value> {
        // 1. 确保 session 在内存（必要时从 jsonl 重放）
        let in_memory = self.sessions.read().await.contains_key(session_id);
        if !in_memory {
            self.load_session_from_jsonl(session_id).await?;
        }
        let entry = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .cloned()
                .context("session not found")?
        };

        // 2. 读取持久化的 loop_state / stop_reason（注意：必须先读 DB 再做判断，
        //    否则并发 resume 可能看到陈旧的内存视图）
        let (db_loop_state, db_stop_reason) =
            match SessionStateStore::for_session(session_id).await {
                Some(store) => store.get_session_state(session_id).await,
                None => ("idle".to_string(), None),
            };
        let loop_running_inmem = entry.loop_running.load(std::sync::atomic::Ordering::SeqCst);
        let unfinished = self.unfinished_todos(session_id).await;

        // Phase 5: 检测 interrupted tasks（服务重启时被打断）
        // **Phase 5b**: 现在 `TaskStatus::Interrupted` 不再被映射为 Failed，
        // 直接读 `t.status` 字符串（= "Interrupted"）。
        let has_interrupted_tasks = self
            .list_tasks_for_session(session_id)
            .await
            .iter()
            .any(|t| t.get("status").and_then(|s| s.as_str()) == Some("Interrupted"));

        // 3. 决策
        let decision = {
            let has_pending_ask = if let Some(store) = SessionStateStore::for_session(session_id).await {
                store
                    .get_pending_ask(session_id)
                    .await
                    .is_some_and(|rec| {
                        use crate::persistence::session_state::PendingAskState;
                        rec.state == PendingAskState::Pending
                    })
            } else {
                false
            };
            let has_pending_plan = if let Some(store) = SessionStateStore::for_session(session_id).await {
                store
                    .get_pending_plan(session_id)
                    .await
                    .is_some_and(|rec| {
                        use crate::persistence::session_state::PendingPlanState;
                        rec.state == PendingPlanState::Pending
                    })
            } else {
                false
            };
            crate::resume_policy::decide_resume(
                loop_running_inmem,
                &db_loop_state,
                db_stop_reason.as_deref(),
                unfinished.len(),
                has_interrupted_tasks,
                has_pending_ask || has_pending_plan,
            )
        };
        match decision {
            crate::resume_policy::ResumeDecisionKind::Conflict => {
                anyhow::bail!(
                    "agent loop already running for session {} (loop_state={}, loop_running={})",
                    session_id, db_loop_state, loop_running_inmem
                );
            }
            crate::resume_policy::ResumeDecisionKind::WaitingForUser => {
                return Ok(serde_json::json!({
                    "started": false,
                    "waiting_for_user": true,
                    "loop_state": db_loop_state,
                }));
            }
            crate::resume_policy::ResumeDecisionKind::HealStopped => {
                // P1-6: stale waiting_for_user 自愈 → 写回 stopped，再走 NoWork 路径
                if let Some(store) = SessionStateStore::for_session(session_id).await {
                    let _ = store
                        .set_session_state(session_id, "stopped", Some("waiting_healed"))
                        .await;
                }
                tracing::warn!(
                    "resume: stale waiting_for_user without pending ask/plan for {}; healing to stopped",
                    session_id
                );
                return Ok(serde_json::json!({
                    "started": false,
                    "requires_user_input": true,
                    "loop_state": "stopped",
                    "stop_reason": "waiting_healed",
                    "reason": "stale waiting_for_user; ask/plan already resolved",
                }));
            }
            crate::resume_policy::ResumeDecisionKind::NoWork => {
                // 完成 / idle / stopped 且无未完成：不启动模型，等用户输入
                // Phase 5c: 显式 fallback 文案，避免空 stop_reason 给 client
                // 一个不明确的 reason
                let reason_str = db_stop_reason.as_deref().unwrap_or("idle");
                let fallback = match reason_str {
                    "completed" => "session completed; no pending work",
                    "max_iters_reached" => "agent loop reached max iterations",
                    "loop_condition_met" => "loop condition met",
                    "empty_response" => "agent returned empty response",
                    "cancelled" => "session was cancelled",
                    "ask_answered" | "ask_answered_restart" => "ask was answered",
                    "ask_cancelled" | "ask_cancelled_restart" => "ask was cancelled",
                    "plan_approved" => "plan was approved",
                    "plan_rejected" => "plan was rejected",
                    "plan_edited" => "plan was edited",
                    "idle" | "" => "session is idle; no pending work to resume",
                    other => other,
                };
                return Ok(serde_json::json!({
                    "started": false,
                    "requires_user_input": true,
                    "loop_state": db_loop_state,
                    "stop_reason": db_stop_reason,
                    "reason": fallback,
                }));
            }
            crate::resume_policy::ResumeDecisionKind::Start => {
                // 继续走启动路径
            }
        }

        // 4. CAS（与 send_message 同一原子操作；loop_running 已 CAS 0→1）
        if entry
            .loop_running
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_err()
        {
            anyhow::bail!(
                "agent loop already running for session {} (409 Conflict)",
                session_id
            );
        }

        // 5. 注入唯一 [session resumed] 系统消息（含 stop_reason + unfinished todo + 防
        //    重复已完成工作提示）。先写消息，再持久化 loop_state=running；消息必须
        //    先于 broadcast，让订阅者看到的"resumed system message"在消息流里出现
        //    在新回复之前。
        let resumed_lines: Vec<String> = unfinished
            .iter()
            .map(|t| {
                format!(
                    "- [{}] {} ({})",
                    t.status, t.content, t.priority
                )
            })
            .collect();

        // Phase 5: 收集本 session 的 interrupted tasks（来自 DB），注入系统消息
        // 让 agent inspect，决定是否重跑；**绝不自动调用原工具**
        let interrupted_tasks: Vec<serde_json::Value> = self
            .list_tasks_for_session(session_id)
            .await
            .into_iter()
            .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some("Interrupted"))
            .collect();

        let interrupted_lines: Vec<String> = interrupted_tasks
            .iter()
            .map(|t| {
                let id = t.get("task_id").and_then(|s| s.as_str()).unwrap_or("?");
                let tool = t.get("tool_name").and_then(|s| s.as_str()).unwrap_or("?");
                let args = t.get("args_json").cloned().unwrap_or(serde_json::Value::Null);
                let args_short = serde_json::to_string(&args)
                    .unwrap_or_default()
                    .chars()
                    .take(120)
                    .collect::<String>();
                let output = t.get("output_json").cloned().unwrap_or(serde_json::Value::Null);
                let output_short = if output.is_null() {
                    "(no output)".to_string()
                } else {
                    serde_json::to_string(&output)
                        .unwrap_or_default()
                        .chars()
                        .take(120)
                        .collect::<String>()
                };
                format!(
                    "- task_id={} tool={} args={} output={}\n    inspect and decide whether to rerun the original tool.",
                    id, tool, args_short, output_short
                )
            })
            .collect();

        let stop_reason_str = db_stop_reason.clone().unwrap_or_default();
        let mut resumed_text = if unfinished.is_empty() {
            format!(
                "[session resumed] previous loop stopped with reason=\"{}\"; \
                 resuming the agent loop with no remaining unfinished todos.",
                stop_reason_str
            )
        } else {
            format!(
                "[session resumed] previous loop stopped with reason=\"{}\"; \
                 resuming the agent loop to continue the following unfinished todos:\n{}\n\n\
                 Note: do NOT redo any work that has already been completed; \
                 only pick up the items listed below.",
                stop_reason_str,
                resumed_lines.join("\n")
            )
        };

        if !interrupted_lines.is_empty() {
            // 追加 interrupted task 段：明确告知 agent 这些是服务重启时被打断的，
            // 需 inspect 后决定，不自动重跑
            resumed_text.push_str(
                "\n\n[interrupted async tasks from previous run] \
                 The following background tasks were interrupted by a service restart \
                 and have NOT been automatically rerun. Inspect each and decide \
                 whether to rerun manually (do NOT auto-invoke the original tool):\n",
            );
            resumed_text.push_str(&interrupted_lines.join("\n"));
        }

        let resumed_msg = Message::system(resumed_text);
        {
            let mut agent = entry.session.lock().await;
            // 写入 JSONL + 内存（add_message 已经做了）
            agent.add_message(resumed_msg.clone())?;
        }
        let _ = self.event_tx.send(ServerEvent::Message {
            session_id: session_id.to_string(),
            message: resumed_msg,
        });

        // Inject workflow context if configured (session_start hook on resume)
        let _ = self.inject_workflow_context(session_id, &entry).await;

        // 6. 持久化 loop_state=running（覆盖之前的 stopped/stop_reason 字段）
        self.persist_loop_state(session_id, "running", None).await;

        // 7. 复用 spawn_run_loop 启动入口（与 send_message 共用 spawn 逻辑）
        let sid = session_id.to_string();
        self.spawn_run_loop(sid.clone(), entry.clone());

        Ok(serde_json::json!({
            "started": true,
            "loop_state": "running",
            "stop_reason": db_stop_reason,
            "resumed_todo_count": unfinished.len(),
            "interrupted_task_count": interrupted_tasks.len(),
        }))
    }

    /// Phase 3: 把 send_message / resume_session 共享的 spawn 逻辑抽出来
    ///
    /// 调用方必须在调用本函数前完成：
    /// 1. `loop_running.compare_exchange(false, true)` 已成功（保证只 spawn 一次）
    /// 2. 持久化 `loop_state=running`
    /// 3. 任何额外的系统消息已注入（resumed / recalled memory / 完成的任务等）
    ///
    /// spawn 完成后 `loop_running` 由本函数 spawn 的任务在结束时重置；
    /// 若 spawn 的 task panic，兜底写入 `stopped, failed`。
    fn spawn_run_loop(self: &Arc<Self>, sid: String, entry: Arc<SessionEntry>) {
        let mgr = self.clone();
        let entry_clone = entry.clone();
        // 终审修复 #2：每次 spawn 时把 generation 单调递增。
        // 旧 loop（task）持 my_gen 闭包变量；结束后它做的清理动作前先
        // compare entry.generation 与 my_gen：不等即已被新 loop 接管，自己短路。
        let my_gen = entry
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        tokio::spawn(async move {
            let result = mgr.run_agent_loop(&sid).await;
            // 终审修复 #2: 旧 loop 的清理动作仅在自己仍是当前 generation 时执行
            let now_gen = entry_clone.generation.load(std::sync::atomic::Ordering::SeqCst);
            if now_gen != my_gen {
                tracing::warn!(
                    "spawn_run_loop: my_gen={} superseded by current_gen={}; \
                     skipping cleanup to avoid clobbering new loop state",
                    my_gen, now_gen
                );
                return;
            }
            // 无论成功失败都要重置 loop_running
            entry_clone.loop_running.store(false, std::sync::atomic::Ordering::SeqCst);
            // run_agent_loop 内部已经在 exit 时调用了 persist_loop_state（按 reason）
            // 这里仅在 spawn 任务 panic / 错误退出时兜底写入 "failed"
            let (current_state, _current_reason) = match SessionStateStore::for_session(&sid).await {
                Some(store) => store.get_session_state(&sid).await,
                None => ("running".to_string(), None),
            };
            if current_state == "running" {
                mgr.persist_loop_state(&sid, "stopped", Some("failed")).await;
            }
            if let Err(e) = result {
                let _ = mgr.event_tx.send(ServerEvent::Error {
                    message: format!("agent loop error: {}", e),
                });
            }
        });
    }

    /// 设计文档 §8.3.1: 记忆自动召回
    /// 用首条用户消息作为 query，搜索项目记忆和全局经验
    /// 召回结果作为 system 消息注入（在用户消息之前）
    async fn inject_recalled_memory(
        &self,
        session_id: &str,
        entry: &Arc<SessionEntry>,
        query: &str,
    ) -> Result<()> {
        // P0-1 已用 loop_running CAS 保证同一 session 不会有并发 send_message
        // 这里仍用一次锁完成「检查首条 + 添加召回消息」以避免任何竞态
        let (is_first_user_msg, project_path) = {
            let agent = entry.session.lock().await;
            (
                !agent.messages.iter().any(|m| m.role == Role::User),
                agent.session.project_path().to_path_buf(),
            )
        };
        if !is_first_user_msg {
            return Ok(());
        }

        let resources = self.get_or_create_resources(&project_path).await?;
        let project_hash = crate::persistence::jsonl::escape_project_path(&project_path);

        let limit = self.config.memory.recall_limit.max(1);
        let mut recalled_text = String::new();

        // 搜索项目记忆
        match resources.memory_store.search(
            query,
            Some(crate::memory::MemoryScope::Project),
            Some(&project_hash),
            limit,
        ) {
            Ok(entries) if !entries.is_empty() => {
                recalled_text.push_str("[recalled project memories]\n");
                for e in entries {
                    recalled_text.push_str(&format!("- {}: {}\n", e.key, e.content));
                }
            }
            _ => {}
        }

        // 搜索全局经验
        match self.experience_store.search(
            query,
            Some(crate::memory::MemoryScope::Experience),
            None,
            limit,
        ) {
            Ok(entries) if !entries.is_empty() => {
                recalled_text.push_str("\n[recalled cross-project experiences]\n");
                for e in entries {
                    recalled_text.push_str(&format!("- {}: {}\n", e.key, e.content));
                }
            }
            _ => {}
        }

        if recalled_text.is_empty() {
            return Ok(());
        }

        let msg = Message::system(recalled_text);
        // 持锁完成 add_message，避免与 add_message(user_msg) 竞态
        {
            let mut agent = entry.session.lock().await;
            agent.add_message(msg.clone())?;
        }
        let _ = self.event_tx.send(ServerEvent::Message {
            session_id: session_id.to_string(),
            message: msg,
        });
        Ok(())
    }

    /// Inject workflow context if configured (session_start hook)
    async fn inject_workflow_context(&self, session_id: &str, entry: &Arc<SessionEntry>) -> Result<()> {
        let project_path = {
            let agent = entry.session.lock().await;
            agent.session.project_path().to_path_buf()
        };
        let workflow_config = project_path.join(".mcoder").join("workflow").join("config.yaml");
        if !workflow_config.exists() {
            return Ok(());
        }
        if let Some(compact) = crate::workflow::context::build_compact_context(&project_path) {
            let wf_msg = crate::types::Message::system(compact);
            let mut agent = entry.session.lock().await;
            agent.add_message(wf_msg.clone())?;
            drop(agent);
            let _ = self.event_tx.send(ServerEvent::Message {
                session_id: session_id.to_string(),
                message: wf_msg,
            });
        }
        Ok(())
    }

    /// async task 结果注入（幂等）：把 session 内已 completed/failed 但未注入的 task
    /// 找出后按 task_id 生成唯一 system message（"async task completed" / "async task failed"），
    /// append 成功后再 mark injected；重启后同一 task 不会被再次注入。
    async fn inject_completed_tasks(
        &self,
        session_id: &str,
        entry: &Arc<SessionEntry>,
    ) -> Result<()> {
        let task_records = entry.task_manager.list().await;
        if task_records.is_empty() {
            return Ok(());
        }
        let mut agent = entry.session.lock().await;
        for t in task_records {
            if !matches!(
                t.status,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
            ) {
                continue;
            }
            // 终态任务先持久化 status，再注入 system message
            if let Err(e) = entry
                .task_manager
                .store()
                .complete_terminal_state(
                    &t.id,
                    TaskStatus::to_db_state(&t.status),
                    t.result.as_deref().map(|value| serde_json::Value::String(value.to_string())),
                    t.error.clone(),
                )
                .await
            {
                tracing::warn!("inject_completed_tasks: persist terminal state failed: {}", e);
                continue;
            }
            let text = format!(
                "[async task completed] id={} name={} status={:?}{}{}",
                t.id,
                t.name,
                t.status,
                t.result
                    .as_deref()
                    .map(|r| format!("\nresult: {}", r))
                    .unwrap_or_default(),
                t.error
                    .as_deref()
                    .map(|e| format!("\nerror: {}", e))
                    .unwrap_or_default(),
            );
            let msg = Message::system(text.clone());
            // 写 JSONL + 内存（add_message 内部会 append + 更新 head）
            if let Err(e) = agent.add_message(msg.clone()) {
                tracing::warn!("inject_completed_tasks: add_message failed: {}", e);
            }
            // mark injected（幂等：重复调用不会重复注入）
            if let Err(e) = entry
                .task_manager
                .store()
                .mark_task_injected(&t.id, chrono::Utc::now().timestamp_millis())
                .await
            {
                tracing::warn!("inject_completed_tasks: mark_injected failed: {}", e);
            }
            let _ = self.event_tx.send(ServerEvent::Message {
                session_id: session_id.to_string(),
                message: msg,
            });
        }
        Ok(())
    }

    async fn run_agent_loop(self: &Arc<Self>, session_id: &str) -> Result<()> {
        let entry = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .cloned()
                .context("session not found")?
        };

        // 设计文档 §3.9: 获取 session 级 CancellationToken
        let cancel_token = entry.cancellation.clone();

        // 构造 ToolContext（per-session + per-project 资源）
        let ctx = self.build_tool_context(session_id, &entry).await?;

        // 确保 system prompt 存在
        {
            let mut agent = entry.session.lock().await;
            agent.ensure_system_prompt();
        }

        // 设计文档 §3.5: 计算 max_iters（role 优先，None=无限用 config 兜底）
        let max_iters = {
            let agent = entry.session.lock().await;
            agent.max_iters_for_current_role()
        };

        // 记录 break 退出的原因，用于区分循环自然结束（max_iters_reached）和 break 退出
        let mut break_reason: Option<String> = None;

        for iter in 0..max_iters {
            // 设计文档 §3.9: 每轮开始检查取消信号
            if cancel_token.is_cancelled() {
                tracing::info!("agent loop cancelled by user at iter {}", iter);
                let mut agent = entry.session.lock().await;
                let _ = agent.add_message(Message::system("[agent loop cancelled]"));
                self.emit_session_done(session_id, "cancelled").await;
                return Ok(());
            }

            // 设计文档 §4.9 策略2: 每轮 agent loop 开始时做文件快照
            // 对比上一轮结束时的文件状态，补录漏检的变更（外部进程或绕过 journal 的修改）
            let batch_id = ctx.journal.begin_batch(&ctx.project_path, &format!("loop_iter_{}", iter))
                .map_err(|e| tracing::warn!("begin_batch failed: {}", e))
                .ok();

            // 设计文档 §3.5: 每轮 LLM 调用前注入已完成的后台任务结果
            self.inject_completed_tasks(session_id, &entry).await?;

            // 设计文档 §3.5: 注入 role 特定上下文（plan/todo 状态）
            {
                let mut agent = entry.session.lock().await;
                agent.inject_role_context(&ctx.session_state).await?;
            }

            // BeforeLlmCall hook
            let hook_ctx = crate::plugin::HookContext {
                hook: crate::plugin::HookPoint::BeforeLlmCall,
                session_id: session_id.to_string(),
                data: serde_json::json!({"iter": iter}),
            };
            let hook_result = self.plugins.run_hooks(
                crate::plugin::HookPoint::BeforeLlmCall,
                hook_ctx,
            ).await?;
            if !hook_result.allow {
                tracing::info!("agent loop stopped by BeforeLlmCall hook at iter {}", iter);
                self.emit_session_done(session_id, "hook_blocked").await;
                break_reason = Some("hook_blocked".to_string());
                break;
            }

            // 设计文档 §3.9: LLM 调用可被取消（select between LLM 和 cancellation）
            let (assistant_msg, last_usage) = {
                let mut agent = entry.session.lock().await;
                tokio::select! {
                    r = agent.run_once() => r?,
                    _ = cancel_token.cancelled() => {
                        tracing::info!("LLM call cancelled at iter {}", iter);
                        let _ = agent.add_message(Message::system("[LLM call cancelled by user]"));
                        self.emit_session_done(session_id, "cancelled").await;
                        return Ok(());
                    }
                }
            };

            match assistant_msg {
                Some(msg) => {
                    // AfterLlmCall hook
                    let _ = self.plugins.run_hooks(
                        crate::plugin::HookPoint::AfterLlmCall,
                        crate::plugin::HookContext {
                            hook: crate::plugin::HookPoint::AfterLlmCall,
                            session_id: session_id.to_string(),
                            data: serde_json::json!({ "content_blocks": msg.content.len() }),
                        },
                    ).await;

                    let _ = self.event_tx.send(ServerEvent::Message {
                        session_id: session_id.to_string(),
                        message: msg.clone(),
                    });

                    // 广播 usage（若本轮拿到）
                    if let Some(u) = &last_usage {
                        let (cumulative, context_window) = {
                            let agent = entry.session.lock().await;
                            (agent.cumulative_usage.clone(), agent.model_config().context_window as usize)
                        };
                        let _ = self.event_tx.send(ServerEvent::UsageUpdated {
                            session_id: session_id.to_string(),
                            delta: u.clone(),
                            cumulative,
                            context_window,
                        });
                    }

                    let tool_calls: Vec<_> = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolUse { id, name, args } => {
                                Some(crate::types::ToolCall {
                                    id: id.clone(),
                                    name: name.clone(),
                                    args: args.clone(),
                                })
                            }
                            _ => None,
                        })
                        .collect();

                    if tool_calls.is_empty() {
                        // 设计文档 §3.5: 没有工具调用且无 running 后台任务 → 结束
                        // 若仍有后台任务在跑，继续轮询等待其完成
                        // Phase 5: per-session TaskManager
                        if entry.task_manager.has_running().await {
                            tracing::info!("no tool calls but background tasks still running, waiting...");
                            // 等待时也响应取消信号
                            tokio::select! {
                                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
                                _ = cancel_token.cancelled() => {
                                    tracing::info!("cancelled while waiting for background tasks");
                                    self.emit_session_done(session_id, "cancelled").await;
                                    return Ok(());
                                }
                            }
                            continue;
                        }
                        // ===== todo gate =====
                        // 没有工具调用、没有 running 后台任务 → 准备结束
                        // 但若有未完成 todos，强制 LLM 继续：
                        // 终审修复 #3：用 todo_gate 纯函数判定，最多 3 strikes 后结构化结束；
                        //   不自动 cancel todos；strike 计数 fingerprint 变化时 reset。
                        let unfinished = {
                            let store = ctx.session_state.clone();
                            store.list_unfinished_todos(&ctx.session_id).await.unwrap_or_default()
                        };
                        let (last_fp, last_strike) = {
                            let g = entry.last_unfinished_todo_fingerprint.lock().await;
                            (
                                g.clone(),
                                entry.todo_gate_strikes.load(std::sync::atomic::Ordering::SeqCst),
                            )
                        };
                        let decision = crate::todo_gate::decide_todo_gate(
                            &unfinished,
                            last_fp.as_deref(),
                            Some(last_strike),
                        );
                        match decision {
                            crate::todo_gate::TodoGateDecision::Finish => {
                                // 无未完成 todo，正常结束
                                self.emit_session_done(session_id, "completed").await;
                                break_reason = Some("completed".to_string());
                                break;
                            }
                            crate::todo_gate::TodoGateDecision::Continue { strike, message } => {
                                tracing::info!(
                                    "todo gate strike {}: {} unfinished todos, injecting reminder and continuing",
                                    strike,
                                    unfinished.len()
                                );
                                {
                                    let mut agent = entry.session.lock().await;
                                    let _ = agent.add_message(Message::system(message));
                                }
                                let fp = crate::todo_gate::fingerprint(&unfinished);
                                {
                                    let mut g = entry.last_unfinished_todo_fingerprint.lock().await;
                                    *g = Some(fp);
                                }
                                entry
                                    .todo_gate_strikes
                                    .store(strike, std::sync::atomic::Ordering::SeqCst);
                                continue;
                            }
                            crate::todo_gate::TodoGateDecision::FinishWithReminder {
                                strike,
                                message,
                            } => {
                                tracing::info!(
                                    "todo gate: {} strikes reached, finishing with structured reminder",
                                    strike
                                );
                                {
                                    let mut agent = entry.session.lock().await;
                                    let _ = agent.add_message(Message::system(message));
                                }
                                // reset 状态，下一轮重新计数（同一 fingerprint 下）
                                {
                                    let mut g = entry.last_unfinished_todo_fingerprint.lock().await;
                                    *g = None;
                                }
                                entry
                                    .todo_gate_strikes
                                    .store(0, std::sync::atomic::Ordering::SeqCst);
                                self.emit_session_done(session_id, "completed").await;
                                break_reason = Some("completed".to_string());
                                break;
                            }
                        }
                    }

                    // 设计文档 §3.9 / P1-10: 只读工具并发执行
                    // 把工具调用分两组：只读组 + 写组
                    // 只读组用 futures::join_all 并发；写组按顺序串行
                    let (readonly, writeonly) = split_tool_calls(&tool_calls);

                    // 并发执行只读工具
                    if !readonly.is_empty() {
                        let mgr = self.clone();
                        let sid = session_id.to_string();
                        let entry_clone = entry.clone();
                        let ct = cancel_token.clone();
                        let ctx_clone = ctx.clone();
                        let results = execute_readonly_concurrent(
                            &mgr, &sid, &entry_clone, &readonly, &ct, &ctx_clone
                        ).await;
                        for (tc, result_msg) in results {
                            let _ = self.event_tx.send(ServerEvent::Message {
                                session_id: session_id.to_string(),
                                message: result_msg,
                            });
                            let _ = self.event_tx.send(ServerEvent::ToolCallDone {
                                session_id: session_id.to_string(),
                                name: tc.name.clone(),
                                success: true,
                            });
                        }
                    }

                    // 串行执行写工具
                    for tc in &writeonly {
                        // 设计文档 §3.9: 工具执行前检查取消
                        if cancel_token.is_cancelled() {
                            tracing::info!("cancelled before tool {} at iter {}", tc.name, iter);
                            self.emit_session_done(session_id, "cancelled").await;
                            return Ok(());
                        }

                        let _ = self.event_tx.send(ServerEvent::ToolCallStart {
                            session_id: session_id.to_string(),
                            name: tc.name.clone(),
                        });

                        // 设计文档 §3.4: 检查 role 是否允许此工具
                        let role_allowed = {
                            let agent = entry.session.lock().await;
                            agent.is_tool_allowed(&tc.name)
                        };
                        if !role_allowed {
                            tracing::info!("tool {} blocked by role whitelist", tc.name);
                            let block_msg = Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                                    id: tc.id.clone(),
                                    output: crate::types::ToolOutput::Error {
                                        message: format!("tool '{}' is not allowed in current role", tc.name),
                                    },
                                }]);
                            let _ = self.event_tx.send(ServerEvent::Message {
                                session_id: session_id.to_string(),
                                message: block_msg.clone(),
                            });
                            {
                                let mut agent = entry.session.lock().await;
                                let _ = agent.add_message(block_msg);
                            }
                            let _ = self.event_tx.send(ServerEvent::ToolCallDone {
                                session_id: session_id.to_string(),
                                name: tc.name.clone(),
                                success: false,
                            });
                            continue;
                        }

                        // 设计文档 §8.7: 危险工具安全确认
                        // browser_* / screen_* / app_* 默认需用户确认，除非在 auto_approve 白名单中
                        // 检查方式：若危险且未自动批准，触发 BeforeToolCall hook（用户可配置 hook 拦截）
                        // 同时返回需要确认的提示给 agent，让 agent 询问用户
                        if crate::types::ToolsConfig::is_dangerous(&tc.name)
                            && !self.config.tools.is_auto_approved(&tc.name) {
                            // 触发 BeforeToolCall hook（用户可配置 hook 来自动批准或阻止）
                            let danger_ctx = crate::plugin::HookContext {
                                hook: crate::plugin::HookPoint::BeforeToolCall,
                                session_id: session_id.to_string(),
                                data: serde_json::json!({
                                    "tool": tc.name,
                                    "args": tc.args,
                                    "dangerous": true,
                                    "auto_approved": false
                                }),
                            };
                            let danger_result = self.plugins.run_hooks(
                                crate::plugin::HookPoint::BeforeToolCall,
                                danger_ctx,
                            ).await?;
                            if !danger_result.allow {
                                tracing::info!("dangerous tool {} blocked by hook (not auto-approved)", tc.name);
                                let block_msg = Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                                        id: tc.id.clone(),
                                        output: crate::types::ToolOutput::Error {
                                            message: format!(
                                                "tool '{}' is a dangerous operation (browser/screen/app control) and requires user confirmation. Add it to [tools] auto_approve in config to allow automatically.",
                                                tc.name
                                            ),
                                        },
                                    }]);
                                let _ = self.event_tx.send(ServerEvent::Message {
                                    session_id: session_id.to_string(),
                                    message: block_msg.clone(),
                                });
                                {
                                    let mut agent = entry.session.lock().await;
                                    let _ = agent.add_message(block_msg);
                                }
                                let _ = self.event_tx.send(ServerEvent::ToolCallDone {
                                    session_id: session_id.to_string(),
                                    name: tc.name.clone(),
                                    success: false,
                                });
                                continue;
                            }
                            // hook 未阻止，说明用户已通过 hook 配置确认，继续执行
                        }

                        // BeforeToolCall hook
                        let before_ctx = crate::plugin::HookContext {
                            hook: crate::plugin::HookPoint::BeforeToolCall,
                            session_id: session_id.to_string(),
                            data: serde_json::json!({ "tool": tc.name, "args": tc.args }),
                        };
                        let before_result = self.plugins.run_hooks(
                            crate::plugin::HookPoint::BeforeToolCall,
                            before_ctx,
                        ).await?;
                        if !before_result.allow {
                            let _ = self.event_tx.send(ServerEvent::ToolCallDone {
                                session_id: session_id.to_string(),
                                name: tc.name.clone(),
                                success: false,
                            });
                            continue;
                        }

                        // 设计文档 §8.3.3: 文件类工具额外触发 BeforeFileChange hook
                        let is_file_tool = matches!(tc.name.as_str(), "write" | "edit" | "ast_edit");
                        if is_file_tool {
                            let file_path = tc.args.get("file")
                                .and_then(|v| v.as_str())
                                .or_else(|| tc.args.get("path").and_then(|v| v.as_str()))
                                .unwrap_or("");
                            let file_ctx = crate::plugin::HookContext {
                                hook: crate::plugin::HookPoint::BeforeFileChange,
                                session_id: session_id.to_string(),
                                data: serde_json::json!({ "tool": tc.name, "file": file_path, "args": tc.args }),
                            };
                            let file_result = self.plugins.run_hooks(
                                crate::plugin::HookPoint::BeforeFileChange,
                                file_ctx,
                            ).await?;
                            if !file_result.allow {
                                tracing::info!("tool {} blocked by BeforeFileChange hook", tc.name);
                                let _ = self.event_tx.send(ServerEvent::ToolCallDone {
                                    session_id: session_id.to_string(),
                                    name: tc.name.clone(),
                                    success: false,
                                });
                                continue;
                            }
                        }

                        let result = {
                            let mut agent = entry.session.lock().await;
                            // C3: 取消时不合成消息 -- execute_tool 内部已检查取消并返回
                            // 相应的 error tool_result。若 cancel 在 select! 竞争中胜出
                            // 而 execute_tool 还没返回，说明工具执行尚未完成，
                            // 返回空 vec 让调用方跳过广播（agent 内部无半提交状态）。
                            tokio::select! {
                                r = agent.execute_tool(tc, &ctx) => r,
                                _ = cancel_token.cancelled() => {
                                    tracing::info!("tool {} cancelled during execution (select race)", tc.name);
                                    Ok(vec![])
                                }
                            }
                        };

                        let success = result.is_ok();
                        if let Ok(result_msgs) = &result {
                            // 遍历所有返回的消息依次广播（image 工具可能返回 2 条：tool_result + image user message）
                            for result_msg in result_msgs {
                                // 检查是否是 AsyncTask 返回
                                let is_async = result_msg.content.iter().any(|b| {
                                    matches!(b, ContentBlock::ToolResult { output, .. } if matches!(output, crate::types::ToolOutput::AsyncTask { .. }))
                                });

                                let _ = self.event_tx.send(ServerEvent::Message {
                                    session_id: session_id.to_string(),
                                    message: result_msg.clone(),
                                });

                                if is_async {
                                    tracing::info!("tool {} returned AsyncTask, agent can continue while it runs in background", tc.name);
                                }
                            }

                            // image 工具（action=send）：检测 image_sent 标记，追加含 Image 块的 assistant 消息并广播
                            // 只在最后一条消息（工具结果消息）上检测
                            if tc.name == "image" {
                                if let Some(last_msg) = result_msgs.last() {
                                    let image_msg = {
                                        let mut agent = entry.session.lock().await;
                                        agent.maybe_create_image_message(tc, last_msg)
                                    };
                                    if let Some(img_msg) = image_msg {
                                        let _ = self.event_tx.send(ServerEvent::Message {
                                            session_id: session_id.to_string(),
                                            message: img_msg,
                                        });
                                    }
                                }
                            }

                            // 自动调度子代理：workflow_update phase_next 返回 spawn_subagent 时
                            // 自动构造 subagent 工具调用并执行，无需 LLM 主动调用
                            if tc.name == "workflow_update" {
                                if let Some(result_msg) = result_msgs.first() {
                                    if let Some(spawn) = extract_spawn_subagent(result_msg) {
                                        tracing::info!(
                                            "auto-spawning subagent (role={}) for change {} phase {}",
                                            spawn.role, spawn.change_id, spawn.phase
                                        );
                                        let subagent_call = crate::types::ToolCall {
                                            id: uuid::Uuid::new_v4().to_string(),
                                            name: "subagent".into(),
                                            args: serde_json::json!({
                                                "op": "spawn",
                                                "role": spawn.role,
                                                "task": spawn.prompt,
                                            }),
                                        };
                                        let _ = self.event_tx.send(ServerEvent::ToolCallStart {
                                            session_id: session_id.to_string(),
                                            name: "subagent".into(),
                                        });
                                        let sub_result = {
                                            let mut agent = entry.session.lock().await;
                                            agent.execute_tool(&subagent_call, &ctx).await
                                        };
                                        let _ = self.event_tx.send(ServerEvent::ToolCallDone {
                                            session_id: session_id.to_string(),
                                            name: "subagent".into(),
                                            success: sub_result.is_ok(),
                                        });
                                        if let Ok(sub_msgs) = sub_result {
                                            for sub_msg in sub_msgs {
                                                let _ = self.event_tx.send(ServerEvent::Message {
                                                    session_id: session_id.to_string(),
                                                    message: sub_msg,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // AfterToolCall hook
                        let _ = self.plugins.run_hooks(
                            crate::plugin::HookPoint::AfterToolCall,
                            crate::plugin::HookContext {
                                hook: crate::plugin::HookPoint::AfterToolCall,
                                session_id: session_id.to_string(),
                                data: serde_json::json!({ "tool": tc.name, "success": success }),
                            },
                        ).await;

                        // 设计文档 §8.3.3: 文件类工具成功后触发 AfterFileChange hook
                        if is_file_tool && success {
                            let file_path = tc.args.get("file")
                                .and_then(|v| v.as_str())
                                .or_else(|| tc.args.get("path").and_then(|v| v.as_str()))
                                .unwrap_or("");
                            let _ = self.plugins.run_hooks(
                                crate::plugin::HookPoint::AfterFileChange,
                                crate::plugin::HookContext {
                                    hook: crate::plugin::HookPoint::AfterFileChange,
                                    session_id: session_id.to_string(),
                                    data: serde_json::json!({ "tool": tc.name, "file": file_path }),
                                },
                            ).await;
                        }

                        let _ = self.event_tx.send(ServerEvent::ToolCallDone {
                            session_id: session_id.to_string(),
                            name: tc.name.clone(),
                            success,
                        });
                    }

                    // 每轮工具调用后尝试 compact（上下文压缩）
                    {
                        let mut agent = entry.session.lock().await;
                        let est_tokens = agent.estimate_total_tokens();
                        let token_threshold = (agent.model_config().context_window as f32
                            * self.config.compact.threshold) as usize;
                        // 设计文档 §3.5/§8.3.3: 压缩前触发 PreCompact hook
                        if est_tokens >= token_threshold {
                            let _ = self.plugins.run_hooks(
                                crate::plugin::HookPoint::PreCompact,
                                crate::plugin::HookContext::new(
                                    crate::plugin::HookPoint::PreCompact,
                                    session_id,
                                ).with_data(serde_json::json!({
                                    "est_tokens": est_tokens,
                                    "threshold": token_threshold,
                                })),
                            ).await;
                        }
                        agent.maybe_compact(&self.config.compact);
                        // 设计文档 §3.5: 检查 loop mode 退出条件
                        if agent.check_loop_condition().await {
                            tracing::info!("loop condition met for role {}, exiting loop",
                                agent.current_role);
                            self.emit_session_done(session_id, "loop_condition_met").await;
                            return Ok(());
                        }
                    }

                    // 设计文档 §4.9 策略2: 每轮结束做 end_batch，对比快照补录漏检变更
                    if let Some(ref bid) = batch_id {
                        match ctx.journal.end_batch(bid, &format!("loop_iter_{}", iter)) {
                            Ok(changes) if !changes.is_empty() => {
                                tracing::info!("iter {}: detected {} untracked file changes", iter, changes.len());
                            }
                            Ok(_) => {}
                            Err(e) => tracing::warn!("end_batch failed: {}", e),
                        }
                    }
                }
                None => {
                    self.emit_session_done(session_id, "empty_response").await;
                    break_reason = Some("empty_response".to_string());
                    break;
                }
            }
        }

        // 设计文档 §3.5: 循环自然结束（达到 max_iters）才发送 done
        // 若是 break 退出（completed/hook_blocked/empty_response）则已在循环内发送过，不重复发送
        if break_reason.is_none() {
            self.emit_session_done(session_id, "max_iters_reached").await;
        }

        // 设计文档 §8.3.1: 记忆自动捕获
        // loop 结束后，若开启 auto_capture，抽取本轮关键决策存入项目记忆
        if self.config.memory.auto_capture {
            if let Err(e) = self.auto_capture_memory(session_id, &entry).await {
                tracing::warn!("auto capture memory failed: {}", e);
            }
        }

        Ok(())
    }

    /// 设计文档 §8.3.1: 记忆自动捕获
    /// 抽取本轮 agent loop 的关键信息（工具调用、文件改动、错误）存入 memory_store
    /// 采用轻量启发式：提取所有 write/edit/bash 工具调用 + error 消息
    async fn auto_capture_memory(
        &self,
        session_id: &str,
        entry: &Arc<SessionEntry>,
    ) -> Result<()> {
        let (messages, project_path) = {
            let agent = entry.session.lock().await;
            (agent.messages.clone(), agent.session.project_path().to_path_buf())
        };

        let mut captured: Vec<String> = Vec::new();
        let mut files_touched: Vec<String> = Vec::new();

        for msg in &messages {
            for block in &msg.content {
                match block {
                    ContentBlock::ToolUse { name, args, .. } => {
                        match name.as_str() {
                            "write" | "edit" | "ast_edit" => {
                                if let Some(file) = args.get("file").and_then(|v| v.as_str()) {
                                    files_touched.push(file.to_string());
                                }
                                captured.push(format!("[{}] {}", name, args));
                            }
                            "bash" => {
                                if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                                    captured.push(format!("[bash] {}", cmd));
                                }
                            }
                            _ => {}
                        }
                    }
                    ContentBlock::ToolResult { output, .. } => {
                        let s = serde_json::to_string(output).unwrap_or_default();
                        if s.contains("error") || s.contains("Error") {
                            captured.push(format!("[error] {}", &s[..s.len().min(200)]));
                        }
                    }
                    _ => {}
                }
            }
        }

        if captured.is_empty() && files_touched.is_empty() {
            return Ok(());
        }

        let resources = self.get_or_create_resources(&project_path).await?;
        let project_hash = crate::persistence::jsonl::escape_project_path(&project_path);

        let key = format!("session-{}-{}", session_id, chrono::Utc::now().timestamp());
        let content = format!(
            "files: {}\nactions:\n{}",
            files_touched.join(", "),
            captured.join("\n")
        );

        let mem_entry = crate::memory::MemoryEntry {
            id: None,
            scope: crate::memory::MemoryScope::Project,
            key,
            content,
            tags: vec![],
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            project_hash: Some(project_hash),
        };
        resources.memory_store.store(&mem_entry)?;
        tracing::debug!("auto captured memory: {}", mem_entry.key);
        Ok(())
    }

    // ==================== ask_user RPC ====================

    /// ask.pending - 客户端查询 session 当前是否有 pending Ask
    /// 设计：attach 时也会自动调一次，保证新连接的 client 能看到进行中的 Ask
    /// Phase 4: 优先内存；空时 fallback DB（service restart 路径）。
    pub async fn peek_ask(&self, session_id: &str) -> Option<serde_json::Value> {
        if let Some(p) = self.ask_registry.peek(session_id).await {
            return Some(serde_json::json!({
                "ask_id": p.ask_id,
                "tool_call_id": p.tool_call_id,
                "session_id": p.session_id,
                "request": p.request,
                "created_at_ms": p.created_at_ms,
            }));
        }
        // Fallback to DB (service restart)
        if let Ok(Some(store)) = self.ask_registry.store_for_session(session_id).await {
            if let Some(rec) = store.get_pending_ask(session_id).await {
                use crate::persistence::session_state::PendingAskState;
                if rec.state == PendingAskState::Pending {
                    return Some(serde_json::json!({
                        "ask_id": rec.ask_id,
                        "tool_call_id": rec.tool_call_id,
                        "session_id": rec.session_id,
                        "request": rec.request,
                        "created_at_ms": rec.created_at_ms,
                    }));
                }
            }
        }
        None
    }

    /// ask.answer - 提交答案（cancel=true 时忽略 answers）
    /// 校验：option 必须是原始选项之一（label 匹配）；整段 custom_response 也支持
    ///
    /// 使用 AskRegistry::submit_validated（原子校验 + 写入 + notify），避免
    /// "validate 通过但 pending 已被 take" 的竞态（issue 4）
    ///
    /// Phase 4: 内存无 pending 但 DB 有时走 restart 路径：
    ///   1) 验证 submission
    ///   2) 向 JsonlSession 追加匹配原 tool_call_id 的真实 ToolResult Message
    ///   3) DB answered
    ///   4) loop_state=stopped
    ///   5) 返回 can_resume=true（让 client 触发 resume_session）
    pub async fn answer_ask(
        &self,
        session_id: &str,
        ask_id: &str,
        submission: AskSubmission,
    ) -> Result<serde_json::Value> {
        // 1. 内存路径：找到 pending 时走标准 submit
        let in_mem_pending = self.ask_registry.peek(session_id).await;
        let req_for_validate = match in_mem_pending.as_ref() {
            Some(p) if p.ask_id == ask_id => Some(p.request.clone()),
            Some(_) => None,
            None => None,
        };
        if let Some(req) = req_for_validate {
            if let Err(errs) = self
                .ask_registry
                .submit_validated(session_id, ask_id, &req, submission)
                .await
            {
                anyhow::bail!(
                    "ask_user submission rejected: {}",
                    errs.iter().map(|e| e.as_str()).collect::<Vec<_>>().join("; ")
                );
            }
            return Ok(serde_json::json!({
                "accepted": true,
                "ask_id": ask_id,
                "via": "memory",
            }));
        }
        // 2. 内存无 pending → 查 DB（service restart 路径）
        let store = match self.ask_registry.store_for_session(session_id).await? {
            Some(s) => s,
            None => anyhow::bail!("no pending ask {} for session {}", ask_id, session_id),
        };
        use crate::persistence::session_state::PendingAskState;
        let rec = match store.get_pending_ask(session_id).await {
            Some(r) if r.ask_id == ask_id && r.state == PendingAskState::Pending => r,
            _ => anyhow::bail!("no pending ask {} for session {}", ask_id, session_id),
        };
        // 2.1 校验 submission
        let req: AskRequest = serde_json::from_value(rec.request.clone())
            .map_err(|e| anyhow::anyhow!("persisted request parse failed: {}", e))?;
        if let Err(errs) = crate::ask_user::validate_submission(&req, &submission) {
            anyhow::bail!(
                "ask_user submission rejected: {}",
                errs.iter().map(|e| e.as_str()).collect::<Vec<_>>().join("; ")
            );
        }
        // 2.2 向 JsonlSession 追加匹配原 tool_call_id 的真实 ToolResult Message
        let result_json = crate::ask_user::build_tool_result(&req, &submission);
        // 终审修复 #1: 校验 JSONL 存在匹配 ToolUse，绝不伪造/追加无主 ToolResult
        let jsonl_msgs = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .and_then(|entry| entry.session.try_lock().ok().map(|a| a.messages.clone()))
                .unwrap_or_default()
        };
        match crate::ask_user::verify_or_cancel_restart_pending_ask(
            &jsonl_msgs,
            &rec.tool_call_id,
            crate::ask_user::VerifyMode::ToolUse,
        ) {
            crate::ask_user::RestartAskDecision::Proceed { .. } => {
                self.append_tool_result_message(session_id, &rec.tool_call_id, &result_json)
                    .await
                    .context("append tool_result message after restart")?;
                // 2.3 DB answered + loop_state=stopped
                let now = chrono::Utc::now().timestamp_millis();
                let answered_db = store
                    .answer_pending_ask(
                        session_id,
                        serde_json::to_value(&submission).unwrap_or(serde_json::Value::Null),
                        result_json.clone(),
                        now,
                    )
                    .await
                    .context("db.answer_pending_ask")?;
                if !answered_db {
                    tracing::warn!(
                        "restart ask answer: db row already terminal for session {}; \
                         treated as idempotent answer",
                        session_id
                    );
                }
                store
                    .set_session_state(session_id, "stopped", Some("ask_answered_restart"))
                    .await
                    .context("db.set_session_state")?;
                // 2.4 广播 AskAnswered（client 看到 ask 卡片消失）
                let _ = self.event_tx.send(ServerEvent::AskAnswered {
                    session_id: session_id.to_string(),
                    ask_id: rec.ask_id.clone(),
                    tool_call_id: rec.tool_call_id.clone(),
                    submission: submission.clone(),
                    result: result_json,
                });
                Ok(serde_json::json!({
                    "accepted": true,
                    "ask_id": rec.ask_id,
                    "via": "db_restart",
                    "tool_call_id": rec.tool_call_id,
                    "can_resume": true,
                }))
            }
            crate::ask_user::RestartAskDecision::Cancel { reason } => {
                tracing::warn!(
                    "restart ask answer rejected for session={} ask_id={}: {}",
                    session_id, rec.ask_id, reason
                );
                let now = chrono::Utc::now().timestamp_millis();
                let cancelled_db = store
                    .cancel_pending_ask(session_id, now)
                    .await
                    .context("db.cancel_pending_ask")?;
                if !cancelled_db {
                    tracing::warn!(
                        "restart ask cancel: db row already terminal for session {}; \
                         treated as idempotent cancel",
                        session_id
                    );
                }
                store
                    .set_session_state(session_id, "stopped", Some("ask_cancelled_restart_orphan"))
                    .await
                    .context("db.set_session_state(cancelled)")?;
                let _ = self.event_tx.send(ServerEvent::AskCancelled {
                    session_id: session_id.to_string(),
                    ask_id: rec.ask_id.clone(),
                    tool_call_id: rec.tool_call_id.clone(),
                });
                Ok(serde_json::json!({
                    "accepted": false,
                    "ask_id": rec.ask_id,
                    "via": "db_restart_orphan_cancelled",
                    "reason": reason,
                    "can_resume": false,
                }))
            }
        }
    }

    /// Phase 4 helper: 向 JsonlSession 追加 ToolResult 消息，不带新 ToolUse。
    /// 这保证重启后 answer 仍能让原 tool_call 配对（不伪造新 ToolUse）。
    async fn append_tool_result_message(
        &self,
        session_id: &str,
        tool_call_id: &str,
        result_json: &serde_json::Value,
    ) -> Result<()> {
        let msg = Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                id: tool_call_id.to_string(),
                output: ToolOutput::Sync { result: result_json.clone() },
            }]);
        // 写 jsonl + 注入内存（add_message 内部会 append + 更新 head）
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            let mut agent = entry.session.lock().await;
            let _ = agent.add_message(msg);
        } else {
            // session 未 attach：直接写 jsonl + 更新 head（无内存注入）
            let mut jsonl = crate::persistence::jsonl::JsonlSession::load(session_id)
                .context("loading jsonl for tool_result append")?;
            let msg_id = msg.id.clone();
            // parent_id 设为当前 head（若有）
            let mut msg = msg;
            if msg.parent_id.is_none() {
                msg.parent_id = jsonl.current_head_id().map(|s| s.to_string());
            }
            jsonl.append(&msg)?;
            jsonl.update_head_id(&msg_id)?;
        }
        Ok(())
    }

    /// ask.cancel - 取消当前 session 的 pending Ask
    /// Phase 4: 内存无 pending 时也接受 DB cancel（service restart 路径）。
    pub async fn cancel_ask(&self, session_id: &str) -> Result<Option<serde_json::Value>> {
        let pending = self.ask_registry.cancel(session_id).await;
        if let Some(p) = pending {
            let _ = self.event_tx.send(ServerEvent::AskCancelled {
                session_id: session_id.to_string(),
                ask_id: p.ask_id.clone(),
                tool_call_id: p.tool_call_id.clone(),
            });
            return Ok(Some(serde_json::json!({
                "cancelled": true,
                "ask_id": p.ask_id,
                "tool_call_id": p.tool_call_id,
            })));
        }
        // Fallback: cancel from DB
        if let Ok(Some(store)) = self.ask_registry.store_for_session(session_id).await {
            use crate::persistence::session_state::PendingAskState;
            if let Some(rec) = store.get_pending_ask(session_id).await {
                if rec.state == PendingAskState::Pending {
                    let now = chrono::Utc::now().timestamp_millis();
                    let _ = store.cancel_pending_ask(session_id, now).await;
                    let _ = store
                        .set_session_state(session_id, "stopped", Some("ask_cancelled_restart"))
                        .await;
                    let _ = self.event_tx.send(ServerEvent::AskCancelled {
                        session_id: session_id.to_string(),
                        ask_id: rec.ask_id.clone(),
                        tool_call_id: rec.tool_call_id.clone(),
                    });
                    return Ok(Some(serde_json::json!({
                        "cancelled": true,
                        "ask_id": rec.ask_id,
                        "tool_call_id": rec.tool_call_id,
                        "via": "db_restart",
                    })));
                }
            }
        }
        Ok(Some(serde_json::json!({"cancelled": false})))
    }

    /// sessions.send 期间：若 session 存在 pending Ask，则把 input 视为整段自由文本答复（issue 3）
    /// 返回 true 表示已被消费（提交成功，不发新 user message）；返回 false 表示未消费或提交失败，
    /// 调用方应继续走普通 send_message 路径或上报明确错误（issue 5：失败不能吞输入）。
    ///
    /// **Phase 5c**：仅发送顶层 `custom_response`，**不再**逐题重复写入
    /// `Custom(note)`。这样：
    /// - 服务端 `validate_submission` 走"顶层 custom_response 兜底"路径（合法）
    /// - LLM 收到 tool result 时只看到一个 `custom_response`，而不是 N 个
    ///   per-question Custom + 顶层 custom_response 的重复
    pub async fn try_handle_text_for_pending_ask(&self, session_id: &str, content: &str) -> bool {
        let pending = match self.ask_registry.peek(session_id).await {
            Some(p) => p,
            None => return false,
        };
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return false;
        }
        // Phase 5c: 只设顶层 custom_response，不再 per-question 重复 Custom(note)
        let submission = AskSubmission {
            cancelled: false,
            custom_response: Some(content.to_string()),
            answers: Default::default(),
        };
        // 用原子化的 submit_validated 完成校验 + 写入 + notify。
        // 失败（已经被首决议/校验失败/pending 已清空）→ 必须返回 false，
        // 调用方继续走普通 send_message 路径，绝不静默吞掉用户的输入（issue 5）。
        match self
            .ask_registry
            .submit_validated(session_id, &pending.ask_id, &pending.request, submission)
            .await
        {
            Ok(()) => true,
            Err(errs) => {
                tracing::warn!(
                    "ask_user text-submit failed for session {} ask {}: {}",
                    session_id,
                    pending.ask_id,
                    errs.iter().map(|e| e.as_str()).collect::<Vec<_>>().join("; ")
                );
                false
            }
        }
    }

    // ==================== 新增 RPC 方法实现 ====================

    /// 设计文档 §5.4 / §5.5 / §5.6: session.attach
    /// - offset=None: 返回全部历史消息（首次 attach）
    /// - offset=Some(n): 仅返回第 n 条之后的消息（断线重连补推）
    ///
    /// Phase 2: 改为统一返回 SessionSnapshot；messages 仍然遵守 offset 语义
    /// （增量），其它字段始终为 session 当前的全量最新值。
    pub async fn attach_session_with_offset(
        self: &Arc<Self>,
        session_id: &str,
        offset: Option<usize>,
    ) -> Result<SessionSnapshot> {
        // 先检查内存中是否存在
        let in_memory = self.sessions.read().await.contains_key(session_id);
        if !in_memory {
            // 设计文档 §5.5: 新 client attach 时先读 jsonl 重放历史
            self.load_session_from_jsonl(session_id).await?;
        }
        let all = self.get_messages(session_id).await?;
        let messages = match offset {
            Some(n) if n > 0 => all.into_iter().skip(n).collect(),
            _ => all,
        };
        self.build_snapshot(session_id, messages).await
    }

    /// 兼容旧接口：不带 offset 的 attach
    pub async fn attach_session(self: &Arc<Self>, session_id: &str) -> Result<SessionSnapshot> {
        self.attach_session_with_offset(session_id, None).await
    }

    /// Phase 2: 构造 SessionSnapshot
    ///
    /// 不调用模型；仅聚合：
    /// - 内存 AgentSession（role / messages / project_path / model / tokens）
    /// - session_state SQLite（loop_state / stop_reason）
    /// - todo store（per-session todos）
    /// - plan 文件（项目级 .mcoder/plans/plan.json；保留项目级）
    /// - ask_registry（in-memory pending Ask）
    /// - task_manager（best-effort 快照；Phase 5 完善 session 隔离）
    ///
    /// `messages` 由调用方按 offset 规则裁剪后传入。
    async fn build_snapshot(
        self: &Arc<Self>,
        session_id: &str,
        messages: Vec<Message>,
    ) -> Result<SessionSnapshot> {
        // 1. 基础元数据：role / project_path / model / tokens（来自 AgentSession）
        // 终审修复 #6：snapshot build 不在 agent lock 内 await ask registry。
        // 先在锁内 clone 必要字段（role / project_path / model / tokens / context_window / cumulative_usage），
        // 释放锁后再 async peek ask registry；避免 agent lock 持有期间跨 await 导致死锁/竞态。
        let (role, project_path_str, model_name, tokens, context_window, loop_running, cumulative_usage) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .context("session not found")?
                .clone();
            let agent = entry.session.lock().await;
            let meta = agent.session.meta();
            (
                agent.current_role.clone(),
                meta.project_path.to_string_lossy().to_string(),
                meta.model.clone(),
                agent.estimate_total_tokens(),
                agent.model_config().context_window as usize,
                entry.loop_running.load(std::sync::atomic::Ordering::SeqCst),
                agent.cumulative_usage.clone(),
            )
        };
        // 锁外 await ask registry（非阻塞读）
        let pending_ask_raw = self.ask_registry.peek(session_id).await;
        // 终审修复 #15：从 DB 复原 role（user 已切换到的 plan/goal/loop 在服务重启后保持）。
        // 注意：若内存 role 与 DB 不同且内存是 default，说明是首次 attach，按 DB 复原即可。
        // 当前实现：内存若为 default 才覆盖为 DB（避免每帧都覆盖）。
        let role = if role == "default" {
            if let Some(store) = SessionStateStore::for_session(session_id).await {
                if let Ok(Some(persisted)) = store.get_kv(session_id, "role").await {
                    persisted
                } else {
                    role
                }
            } else {
                role
            }
        } else {
            role
        };

        // 2. session_state SQLite：loop_state / stop_reason
        let (db_loop_state, db_stop_reason) = match SessionStateStore::for_session(session_id).await {
            Some(store) => store.get_session_state(session_id).await,
            None => ("idle".to_string(), None),
        };
        // 内存 loop_running 与 DB loop_state 取并集：
        // - DB 状态来自上次结束（持久化）
        // - 内存 loop_running 是当前实时状态
        // 真实显示：memory_running → "running"；否则 DB
        let (loop_state, stop_reason) = if loop_running {
            ("running".to_string(), None)
        } else {
            (db_loop_state, db_stop_reason)
        };

        // 3. todos（per-session，来自 SessionStateStore）
        let todos = match SessionStateStore::for_session(session_id).await {
            Some(store) => store.list_todos(session_id).await.unwrap_or_default(),
            None => Vec::new(),
        };

        // 4. plan：Phase 4 改为 per-session SQLite pending_plan（不再读项目级 plan.json）
        //    - 内存无 plan → 查 DB
        //    - 终态（approved/rejected/edited）也展示在 snapshot 中，让 client 能看到最近一次决议
        //    - 旧版 .mcoder/plans/plan.json 不再作为 snapshot source（不兼容）
        let plan = if let Some(store) = SessionStateStore::for_session(session_id).await {
            match store.get_pending_plan(session_id).await {
                Some(rec) => {
                    use crate::persistence::session_state::PendingPlanState;
                    match rec.state {
                        PendingPlanState::Pending
                        | PendingPlanState::Approved
                        | PendingPlanState::Edited => Some(rec.content),
                        PendingPlanState::Rejected => None, // rejected 不再展示卡片
                    }
                }
                _ => None,
            }
        } else {
            None
        };

        // 5. pending ask（Phase 4: 优先内存，fallback DB）
        let pending_ask = if let Some(p) = pending_ask_raw {
            Some(SessionSnapshotPendingAsk {
                ask_id: p.ask_id,
                tool_call_id: p.tool_call_id,
                session_id: p.session_id,
                request: p.request,
                created_at_ms: p.created_at_ms,
            })
        } else if let Some(store) = SessionStateStore::for_session(session_id).await {
            use crate::persistence::session_state::PendingAskState;
            match store.get_pending_ask(session_id).await {
                Some(rec) if rec.state == PendingAskState::Pending => {
                    let req: AskRequest = serde_json::from_value(rec.request.clone())
                        .unwrap_or(AskRequest { questions: vec![] });
                    Some(SessionSnapshotPendingAsk {
                        ask_id: rec.ask_id,
                        tool_call_id: rec.tool_call_id,
                        session_id: rec.session_id,
                        request: req,
                        created_at_ms: rec.created_at_ms,
                    })
                }
                _ => None,
            }
        } else {
            None
        };

        // 6. tasks（Phase 5: per-session，从 DB + in-memory 合并）
        let tasks = self
            .list_tasks_for_session(session_id)
            .await
            .into_iter()
            .map(|t| SessionSnapshotTask {
                task_id: t["task_id"].as_str().unwrap_or("").to_string(),
                tool_name: t["tool_name"].as_str().unwrap_or("").to_string(),
                status: t["status"].as_str().unwrap_or("Running").to_string(),
                args_json: t.get("args_json").cloned(),
                output_json: t.get("output_json").cloned(),
                error: t.get("error").and_then(|v| v.as_str().map(|s| s.to_string())),
                created_at_ms: t.get("created_at_ms").and_then(|v| v.as_i64()).unwrap_or(0),
                updated_at_ms: t.get("updated_at_ms").and_then(|v| v.as_i64()).unwrap_or(0),
            })
            .collect();

        // 7. context tokens + cost（Phase 2 cost=0.0；Phase 4 接入 model pricing）
        // tokens 用 estimate_total_tokens（基于当前分支消息长度估算）；
        // 注：cumulative_usage 是跨轮累加值，不能用作当前上下文占用（会严重偏高）。
        let context = SessionSnapshotContext {
            tokens,
            cost: 0.0,
            context_window,
            usage: cumulative_usage,
        };

        // 8. can_resume：loop_state == "running" 时禁止 send（避免并发 loop）
        let can_resume = loop_state != "running";

        // 9. session meta block：title 来自 SessionMeta
        let title = JsonlSession::load(session_id)
            .map(|s| s.meta().title.clone())
            .unwrap_or_default();

        Ok(SessionSnapshot {
            session: SessionSnapshotSession {
                session_id: session_id.to_string(),
                title,
                project_path: project_path_str,
                role,
                model: model_name,
                loop_state,
                stop_reason,
            },
            messages,
            todos,
            plan,
            pending_ask,
            tasks,
            context,
            can_resume,
        })
    }

    /// 设计文档 §5.5: 从 jsonl 重放加载 session 到内存
    /// 用于 server 重启后 client attach 场景
    async fn load_session_from_jsonl(self: &Arc<Self>, session_id: &str) -> Result<()> {
        let jsonl = JsonlSession::load(session_id)
            .context("loading session from jsonl")?;
        let meta = jsonl.meta().clone();
        // 获取或创建该 session 所属项目的 per-project 资源
        self.get_or_create_resources(&meta.project_path).await?;
        let model_config = Arc::new(self.resolve_model(Some(&meta.model))?);
        let llm = create_adapter(&model_config)?;
        let max_iters = self.config.loop_max_iters;
        let agent = AgentSession::new(jsonl, model_config, llm, self.tools.clone(), max_iters, self.role_registry.clone());

        // Phase 5: 为重放的 session 分配 per-session TaskManager（get_or_create
        // 会原子标记任何 queued/running 任务为 interrupted，绝不自动重跑）
        let task_manager = self.get_or_create_task_manager(session_id).await?;

        let entry = Arc::new(SessionEntry {
            session: Mutex::new(agent),
            cancellation: CancellationToken::new(),
            client_count: std::sync::atomic::AtomicU32::new(0),
            loop_running: std::sync::atomic::AtomicBool::new(false),
            generation: std::sync::atomic::AtomicU64::new(0),
            last_unfinished_todo_fingerprint: Mutex::new(None),
            todo_gate_strikes: std::sync::atomic::AtomicU32::new(0),
            task_manager: task_manager.clone(),
            pending_injections: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        });

        self.sessions
            .write()
            .await
            .insert(session_id.to_string(), entry);
        tracing::info!("session {} reloaded from jsonl", session_id);
        Ok(())
    }

    /// session.close - 关闭会话（从内存移除，jsonl 保留）
    ///
    /// 关闭时必须清理：取消 CancellationToken、清理 pending Ask（避免悬挂 notify）、
    /// 唤醒 waiter（issue 5）
    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        let entry = self
            .sessions
            .write()
            .await
            .remove(session_id)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        // 1. 触发取消 token → agent loop 和 ask_user.execute 会退出
        entry.cancellation.cancel();
        // 2. 取消 pending Ask（同时唤醒 waiter + 广播 AskCancelled）
        if let Some(p) = self.ask_registry.cancel(session_id).await {
            let _ = self.event_tx.send(ServerEvent::AskCancelled {
                session_id: session_id.to_string(),
                ask_id: p.ask_id.clone(),
                tool_call_id: p.tool_call_id.clone(),
            });
        }
        // 3. 设计文档 §8.3.3: 触发 OnSessionEnd hook
        let _ = self.plugins.run_hooks(
            crate::plugin::HookPoint::OnSessionEnd,
            crate::plugin::HookContext::new(crate::plugin::HookPoint::OnSessionEnd, session_id),
        ).await;
        Ok(())
    }

    /// session.delete - 删除会话（内存 + jsonl 文件）
    ///
    /// 同 close_session：清理 pending + 取消 token（issue 5）
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let entry = self.sessions.write().await.remove(session_id);
        if let Some(entry) = entry {
            entry.cancellation.cancel();
        }
        // 取消 pending Ask（唤醒 waiter + 广播 AskCancelled）
        if let Some(p) = self.ask_registry.cancel(session_id).await {
            let _ = self.event_tx.send(ServerEvent::AskCancelled {
                session_id: session_id.to_string(),
                ask_id: p.ask_id.clone(),
                tool_call_id: p.tool_call_id.clone(),
            });
        }
        // 设计文档 §8.3.3: 触发 OnSessionEnd hook
        let _ = self.plugins.run_hooks(
            crate::plugin::HookPoint::OnSessionEnd,
            crate::plugin::HookContext::new(crate::plugin::HookPoint::OnSessionEnd, session_id),
        ).await;
        JsonlSession::delete(session_id)?;
        Ok(())
    }

    /// session.cancel - 取消正在运行的 agent loop
    /// 设计文档 §3.9: 触发 CancellationToken，agent loop 检测到后退出
    pub async fn cancel_session(&self, session_id: &str) -> Result<()> {
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            entry.cancellation.cancel();
            // 同时发系统消息记录取消事件（持久化）
            let mut agent = entry.session.lock().await;
            let msg = Message::system("[cancelled by user]");
            let _ = agent.add_message(msg);
            tracing::info!("session {} cancel triggered via CancellationToken", session_id);
        }
        // 取消任何 pending Ask（cancelled=true 通知 + 唤醒 waiter）
        let pending = self.ask_registry.cancel(session_id).await;
        if let Some(p) = pending {
            let _ = self.event_tx.send(ServerEvent::AskCancelled {
                session_id: session_id.to_string(),
                ask_id: p.ask_id.clone(),
                tool_call_id: p.tool_call_id.clone(),
            });
        }
        // Phase 2: cancel 时立即持久化 loop_state=stopped, stop_reason=cancelled
        self.persist_loop_state(session_id, "stopped", Some("cancelled")).await;
        Ok(())
    }

    /// attach_session 时增加 client 计数
    /// 设计文档 §5.5: 多 client 同步 - 记录订阅数
    pub async fn attach_client(&self, session_id: &str) -> Result<()> {
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            entry.client_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tracing::debug!("session {} client_count++", session_id);
        }
        Ok(())
    }

    /// close_session 时减少 client 计数
    pub async fn detach_client(&self, session_id: &str) -> Result<()> {
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            entry.client_count.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            tracing::debug!("session {} client_count--", session_id);
        }
        Ok(())
    }

    /// 获取 session 的 CancellationToken（供 agent loop 使用）
    pub async fn get_cancellation(&self, session_id: &str) -> Result<CancellationToken> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).context("session not found")?;
        Ok(entry.cancellation.clone())
    }

    /// 广播 role 切换事件
    pub async fn broadcast_role_changed(&self, session_id: &str, role: &str) -> Result<()> {
        let _ = self.event_tx.send(ServerEvent::RoleChanged {
            session_id: session_id.to_string(),
            role: role.to_string(),
        });
        Ok(())
    }

    /// 设计文档 §3.6: plan approve/reject/edit
    /// approve → 切换到 execute role，恢复 agent loop
    /// reject → 切换回 default role，告知 LLM plan 被拒绝
    /// edit → 用 edited_plan 替换 plan 内容，然后 approve
    ///
    /// Phase 4: plan 内容存到 per-session SQLite pending_plan；service restart 后
    /// 仍能取到 plan → 决议路径不依赖项目级 plan.json（不兼容）。
    pub async fn approve_plan(
        self: &Arc<Self>,
        session_id: &str,
        action: &str,
        edited_plan: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        // Phase 4: 决议写到 DB（per-session）；project_path 不再用于 plan 存储
        let store = SessionStateStore::for_session(session_id)
            .await
            .context("opening session_state store for plan approval")?;
        let now = chrono::Utc::now().timestamp_millis();

        match action {
            "approve" => {
                store
                    .approve_pending_plan(session_id, None, now)
                    .await
                    .context("db.approve_pending_plan")?;
                // loop_state=stopped（终态合理；用户可显式 resume_session）
                store
                    .set_session_state(session_id, "stopped", Some("plan_approved"))
                    .await
                    .context("db.set_session_state")?;
                self.set_role(session_id, "execute").await?;
                Ok(serde_json::json!({
                    "action": "approved",
                    "next_role": "execute",
                    "can_resume": true,
                }))
            }
            "reject" => {
                store
                    .reject_pending_plan(session_id, now)
                    .await
                    .context("db.reject_pending_plan")?;
                store
                    .set_session_state(session_id, "stopped", Some("plan_rejected"))
                    .await
                    .context("db.set_session_state")?;
                self.set_role(session_id, "default").await?;
                // 通知 agent plan 被拒绝
                let entry = self.sessions.read().await.get(session_id).cloned()
                    .context("session not found")?;
                let mut agent = entry.session.lock().await;
                let _ = agent.add_message(Message::system("[plan rejected by user]"));
                Ok(serde_json::json!({
                    "action": "rejected",
                    "next_role": "default",
                    "can_resume": true,
                }))
            }
            "edit" => {
                let edited = edited_plan
                    .context("plan.edit requires edited_plan payload")?;
                store
                    .approve_pending_plan(session_id, Some(edited), now)
                    .await
                    .context("db.approve_pending_plan(edit)")?;
                store
                    .set_session_state(session_id, "stopped", Some("plan_edited"))
                    .await
                    .context("db.set_session_state")?;
                self.set_role(session_id, "execute").await?;
                Ok(serde_json::json!({
                    "action": "edited_and_approved",
                    "next_role": "execute",
                    "can_resume": true,
                }))
            }
            other => anyhow::bail!("unknown approve action: {} (use approve|reject|edit)", other),
        }
    }

    /// task.list - 列出指定 session 的异步任务（Phase 5: per-session 隔离）
    /// - 不传 session_id → 返回空（强制 session-scoped 视图）
    /// - 返回值：包含 args_json / output_json / error / 时间戳的完整元数据
    ///
    /// Phase 5c：与 SessionStateStore 共享同一 SqlitePool 缓存；不再
    /// 用独立 `async_tasks.db` 路径。
    pub async fn list_tasks_for_session(&self, session_id: &str) -> Vec<serde_json::Value> {
        // 优先从 in-memory TaskManager 取（包含 in-flight 任务）
        let mut entries: Vec<serde_json::Value> = Vec::new();
        if let Ok(mgr) = self.get_or_create_task_manager(session_id).await {
            for t in mgr.list().await {
                entries.push(serde_json::json!({
                    "task_id": t.id,
                    "session_id": session_id,
                    "tool_name": t.name,
                    "status": format!("{:?}", t.status),
                    "result": t.result,
                    "error": t.error,
                }));
            }
        }
        // 从 DB 取（包括已终态 / interrupted 任务）
        // Phase 5c: 复用 SessionStateStore 共享池（同一 db_path → 同一 pool）
        if let Some(state_store) = crate::persistence::session_state::SessionStateStore::for_session(session_id).await {
            let store = crate::persistence::async_task_store::AsyncTaskStore::new(state_store.pool().clone());
            if let Ok(records) = store.list_tasks_for_session(session_id).await {
                for r in records {
                    // 避免与 in-memory 重复（用 task_id 去重）
                    if !entries.iter().any(|e| e["task_id"] == r.task_id) {
                        entries.push(serde_json::json!({
                            "task_id": r.task_id,
                            "session_id": r.session_id,
                            "tool_name": r.tool_name,
                            "status": format!("{:?}", r.status),
                            "args_json": r.args_json,
                            "output_json": r.output_json,
                            "error": r.error,
                            "created_at_ms": r.created_at_ms,
                            "updated_at_ms": r.updated_at_ms,
                        }));
                    }
                }
            }
        }
        entries
    }

    /// 兼容旧 API：返回所有 session 的 task（不推荐；保留以兼容旧前端）
    pub async fn list_tasks(&self) -> Vec<serde_json::Value> {
        // Phase 5: 仅返回已注册 session 的任务（不再全局）
        let mut all = Vec::new();
        let map = self.task_managers.read().await;
        for (sid, _mgr) in map.iter() {
            let list = self.list_tasks_for_session(sid).await;
            all.extend(list);
        }
        all
    }

    /// task.cancel - 取消指定 session 的 task（防跨会话）
    /// Phase 5: 严格 session 隔离；非 caller session 的 task 拒绝取消
    pub async fn cancel_task_for_session(&self, session_id: &str, task_id: &str) -> Result<()> {
        // 1. 先校验 task 属于该 session
        let mgr = match self.get_or_create_task_manager(session_id).await {
            Ok(m) => m,
            Err(_) => {
                anyhow::bail!("cannot open task store for session {}", session_id);
            }
        };
        let store: Arc<crate::persistence::async_task_store::AsyncTaskStore> = mgr.store();
        if store.get_task_for_session(session_id, task_id).await.is_none() {
            anyhow::bail!(
                "task {} not found in session {} (cross-session cancel denied)",
                task_id,
                session_id
            );
        }
        // 2. 取消内存中的 task（如果存在）
        let cancelled = mgr
            .cancel(task_id)
            .await
            .map_err(|e| anyhow::anyhow!("db cancel_task: {}", e))?;
        if !cancelled {
            anyhow::bail!("task {} is already terminal", task_id);
        }
        Ok(())
    }

    /// 兼容旧 API：默认取第一个 session 的 TaskManager 取消
    pub async fn cancel_task(&self, task_id: &str) -> Result<()> {
        // 没有 session_id 时，从 DB 全局搜 + 用对应 session 的 store 取消
        for (sid, _mgr) in self.task_managers.read().await.iter() {
            if let Ok(mgr) = self.get_or_create_task_manager(sid).await {
                let store = mgr.store();
                if store.get_task_for_session(sid, task_id).await.is_some() {
                    return self.cancel_task_for_session(sid, task_id).await;
                }
            }
        }
        anyhow::bail!("task not found or already completed: {}", task_id)
    }

    /// config.get - 读取配置项
    /// 设计文档 §8.3: 优先返回 runtime override（config.set 设置的值），其次返回静态配置
    /// 这样保证用户通过 config.set 修改的值在 config.get 时能看到，保持一致性
    pub async fn get_config(&self, key: Option<&str>) -> serde_json::Value {
        // 设计文档 §8.3: 先查 runtime override
        let overrides = config_overrides().await.read().await;
        if let Some(k) = key {
            if let Some(v) = overrides.get(k) {
                return v.clone();
            }
        }
        match key {
            Some("default_model") => serde_json::json!(self.config.default_model),
            Some("loop_max_iters") => serde_json::json!(self.config.loop_max_iters),
            Some("compact") => serde_json::json!(self.config.compact),
            Some("compact.threshold") => serde_json::json!(self.config.compact.threshold),
            Some("compact.keep_recent") => serde_json::json!(self.config.compact.keep_recent),
            Some("roles") => serde_json::json!(self.config.roles),
            Some("models") => serde_json::json!(self.config.models),
            Some("tui") => serde_json::json!(self.config.tui),
            Some("tui.compact") => serde_json::json!(self.config.tui.compact),
            Some("memory") => serde_json::json!(self.config.memory),
            Some("memory.auto_recall") => serde_json::json!(self.config.memory.auto_recall),
            Some("memory.auto_capture") => serde_json::json!(self.config.memory.auto_capture),
            Some("server") => serde_json::json!(self.config.server),
            Some(_) | None => serde_json::json!({
                "default_model": self.config.default_model,
                "loop_max_iters": self.config.loop_max_iters,
                "server": self.config.server,
                "tui": self.config.tui,
                "memory": self.config.memory,
            }),
        }
    }

    /// config.set - 设置配置项（运行时，不持久化）
    /// 设计文档 §8.3: 支持运行时修改配置，仅在内存生效，不写回 config.toml
    pub async fn set_config(self: &Arc<Self>, key: &str, value: serde_json::Value) -> Result<()> {
        tracing::info!("config.set requested: {} = {} (runtime, not persisted)", key, value);
        // 支持的点状路径：compact.threshold / tui.compact / loop_max_iters / memory.auto_recall
        // 用 RwLock 保护整个 AppConfig 写入
        let mut new_config = (*self.config).clone();
        match key {
            "loop_max_iters" => {
                if let Some(v) = value.as_u64() {
                    new_config.loop_max_iters = v as u32;
                } else {
                    anyhow::bail!("loop_max_iters expects u32");
                }
            }
            "tui.compact" => {
                if let Some(v) = value.as_bool() {
                    new_config.tui.compact = v;
                }
            }
            "compact.threshold" => {
                if let Some(v) = value.as_f64() {
                    new_config.compact.threshold = v as f32;
                }
            }
            "compact.keep_recent" => {
                if let Some(v) = value.as_u64() {
                    new_config.compact.keep_recent = v as u32;
                }
            }
            "memory.auto_recall" => {
                if let Some(v) = value.as_bool() {
                    new_config.memory.auto_recall = v;
                }
            }
            "memory.auto_capture" => {
                if let Some(v) = value.as_bool() {
                    new_config.memory.auto_capture = v;
                }
            }
            _ => {
                tracing::warn!("config.set: unsupported key '{}' (supported: loop_max_iters/tui.compact/compact.threshold/compact.keep_recent/memory.auto_recall/memory.auto_capture)", key);
                anyhow::bail!("unsupported config key: {}", key);
            }
        }
        // 替换全局 config（用 Arc<RwLock> 才能修改，这里只能记录到 override）
        // 由于 config 是 Arc<AppConfig> 不可变，这里用全局 override map
        config_overrides().await.write().await.insert(key.to_string(), value);
        tracing::info!("config.set: applied '{}' (note: runtime override, not persisted)", key);
        Ok(())
    }

    /// 设计文档 §8.3: 读取运行时覆盖的配置值
    pub async fn get_config_override(&self, key: &str) -> Option<serde_json::Value> {
        config_overrides().await.read().await.get(key).cloned()
    }

    /// config.list_models - 列出所有已配置的模型
    pub fn list_models(&self) -> Vec<serde_json::Value> {
        self.config.models.iter().map(|(name, m)| serde_json::json!({
            "name": name,
            "protocol": format!("{:?}", m.protocol),
            "model": m.name,
            "context_window": m.context_window,
        })).collect()
    }

    /// server.stats - 服务器统计信息
    pub async fn server_stats(&self) -> serde_json::Value {
        let session_count = self.sessions.read().await.len();
        // Phase 5: per-session task 数（累加所有 session）
        let mut task_count = 0usize;
        for (_sid, mgr) in self.task_managers.read().await.iter() {
            task_count += mgr.list().await.len();
        }
        serde_json::json!({
            "sessions_active": session_count,
            "tasks_total": task_count,
            "tools_registered": self.tools.list_schemas().len(),
            "roles_available": self.role_registry.list().len(),
        })
    }
}

// ==================== 工具调用分类与并发执行 ====================

/// 设计文档 §3.9: 只读工具白名单
/// 这些工具无副作用，可安全并发执行
///
/// **Phase 5c 校对**：只保留真实注册到 ToolRegistry 的工具名。
/// 移除历史遗留的 `task_status` / `task_list` / `plan_get` 等占位
/// （实际只有 `task` 工具）；新增 `plan_query`（Phase 5c 新增工具）。
///
/// **m14 校对**：graph 工具已经过合并。
/// `graph_query` + `graph_find` 合并为 `graph_search`；
/// `graph_callers` + `graph_callees` + `graph_references` 合并为 `graph_relations`。
///
/// **m8 校对**：`image` 工具不在此白名单中——其 `view` action 无副作用但 `send` action
/// 会向会话插入新消息（display-only）；runtime 无法按 action 区分，因此走默认路径
/// （与其它写工具一起串行，但读路径仍可并发），这是保守但安全的行为。
pub const READONLY_TOOLS: &[&str] = &[
    "read",
    "ls", "grep",
    "graph_search", "graph_file_symbols", "graph_index",
    "graph_relations",
    "memory_search", "memory_list",
    "sandbox_read",
    "task", // Phase 5: 单 task 工具（query/get/cancel 都通过 op 参数）
    "plan_query", // Phase 5c: 新增 plan 读取工具
    "workflow_query",
    "subagent", // Phase 5: 单 subagent 工具（status/result/list 都通过 op 参数）
    // 设计文档 §8.4.2: LSP 只读工具（诊断/hover/定义/引用查找）
    // lsp_rename / lsp_format 是写工具，不在此列表
    "lsp_diagnose", "lsp_hover", "lsp_definition", "lsp_references",
    // 设计文档 §8.4.3 / P2-6: debug_get_state 是只读操作
    "debug_get_state",
];

/// 设计文档 §3.9 / P1-10: 把工具调用拆成 只读组 + 写组
fn split_tool_calls(calls: &[crate::types::ToolCall]) -> (Vec<crate::types::ToolCall>, Vec<crate::types::ToolCall>) {
    let mut readonly = Vec::new();
    let mut writeonly = Vec::new();
    for tc in calls {
        if READONLY_TOOLS.contains(&tc.name.as_str()) {
            readonly.push(tc.clone());
        } else {
            writeonly.push(tc.clone());
        }
    }
    (readonly, writeonly)
}

/// 设计文档 §3.9 / P1-10: 并发执行只读工具
/// 所有工具共享同一 cancellation token 和 ToolContext
async fn execute_readonly_concurrent(
    mgr: &Arc<SessionManager>,
    session_id: &str,
    entry: &Arc<SessionEntry>,
    calls: &[crate::types::ToolCall],
    cancel_token: &CancellationToken,
    ctx: &crate::tools::ToolContext,
) -> Vec<(crate::types::ToolCall, Message)> {
    // 先发 ToolCallStart 事件 + 检查 role 白名单
    let mut handles: Vec<(crate::types::ToolCall, tokio::task::JoinHandle<Message>)> = Vec::new();
    for tc in calls {
        let _ = mgr.event_tx.send(ServerEvent::ToolCallStart {
            session_id: session_id.to_string(),
            name: tc.name.clone(),
        });

        // 检查 role 白名单
        let role_allowed = {
            let agent = entry.session.lock().await;
            agent.is_tool_allowed(&tc.name)
        };
        if !role_allowed {
            // 不允许的工具：直接 spawn 同步返回错误消息
            let id = tc.id.clone();
            let name = tc.name.clone();
            let handle = tokio::spawn(async move {
                Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                        id,
                        output: crate::types::ToolOutput::Error {
                            message: format!("tool '{}' is not allowed in current role", name),
                        },
                    }])
            });
            handles.push((tc.clone(), handle));
            continue;
        }

        // 取 clone 以便 spawn
        let tc_clone = tc.clone();
        let tools = mgr.tools.clone();
        let ct = cancel_token.clone();
        let ctx_clone = ctx.clone();
        let handle = tokio::spawn(async move {
            // 工具执行可被取消
            let output = tokio::select! {
                r = tools.execute(&tc_clone, &ctx_clone) => match r {
                    Ok(o) => o,
                    Err(e) => crate::types::ToolOutput::Error { message: e.to_string() },
                },
                _ = ct.cancelled() => {
                    crate::types::ToolOutput::Error {
                        message: format!("tool '{}' cancelled by user", tc_clone.name),
                    }
                }
            };
            Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                    id: tc_clone.id.clone(),
                    output,
                }])
        });
        handles.push((tc.clone(), handle));
    }

    // 并发等待所有工具完成
    let mut results = Vec::new();
    for (tc, handle) in handles {
        let msg = match handle.await {
            Ok(m) => m,
            Err(e) => Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                    id: tc.id.clone(),
                    output: crate::types::ToolOutput::Error {
                        message: format!("tool panicked: {}", e),
                    },
                }]),
        };
        // 广播消息
        let _ = mgr.event_tx.send(ServerEvent::Message {
            session_id: session_id.to_string(),
            message: msg.clone(),
        });
        // 持久化到 session
        {
            let mut agent = entry.session.lock().await;
            let _ = agent.add_message(msg.clone());
        }
        results.push((tc, msg));
    }
    results
}

// ==================== Phase 5 helpers ====================
//
// Phase 5c: 不再保留 `async_task_db_path` 独立路径——所有 per-session 状态
// 统一在 `<project>/.mcoder/session_state.db`（或全局 fallback），
// 由 `crate::persistence::session_state::session_state_db_path` 唯一计算。
//
// 旧 `todos.db` / `async_tasks.db` 路径已废弃（用户已确认：不兼容旧 DB）。
