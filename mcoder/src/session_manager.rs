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
use serde::{Deserialize, Serialize};
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
    /// mcoder version (from CARGO_PKG_VERSION)
    pub version: String,
    /// Active LSP server language names (e.g., ["rust", "typescript"])
    pub lsp_servers: Vec<String>,
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
    /// 设计文档 §provider: 配置用 RwLock 包裹，支持运行时 add/update/delete_provider
    /// - 读路径（list_models / list_providers）：blocking_read（这些 RPC 是同步）
    /// - 写路径（add/update/delete_provider）：async write（在 write lock 内 read-modify-write）
    /// - AgentSession 持有自己的 model_config Arc clone，不受配置替换影响
    config: Arc<RwLock<AppConfig>>,
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
    /// 设计文档 §8.8: 权限审批 pending 池（per-session）
    permission_registry: Arc<crate::permission::PermissionRegistry>,
    /// LSP 异步诊断 pending 队列（per-session）
    /// write/edit 后后台 LSP 任务把诊断 push 进来，
    /// 下次 tool call 执行前 drain 出来拼成 ToolResult 注入
    lsp_diag_store: Arc<crate::lsp::PendingDiagnosticsStore>,
    /// launch 工具：全局后台进程管理器（跨 session 共享）
    launch_manager: crate::tools::launch::LaunchManager,
    /// S2 修复: session-level thinking_depth override（不持久化）
    /// key = session_id, value = 覆盖的 ThinkingDepth
    /// 每次 LLM 调用前由 build_session_model_config 应用
    session_thinking_overrides: RwLock<HashMap<String, crate::types::ThinkingDepth>>,
}

/// handoff / handoff_back 结果结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffResult {
    pub handoff_doc: String,
    pub new_session_id: String,
    pub original_session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffBackResult {
    pub back_doc: String,
    pub to_session_id: String,
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
    /// 模型切换后广播（/model set 或 session.model.set RPC）
    ModelChanged {
        session_id: String,
        model: String,
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
    /// 设计文档 §8.8: 权限审批 - 新请求（client 渲染审批卡片）
    PermissionPending {
        session_id: String,
        request: crate::permission::PermissionRequest,
    },
    /// 设计文档 §8.8: 权限审批 - 已决议
    PermissionResolved {
        session_id: String,
        request_id: String,
        decision: crate::permission::PermissionDecision,
    },
    /// LLM usage 报告：每轮 LLM 调用后广播 delta + cumulative + context_window
    /// 客户端用于更新上下文使用率（圆环/百分比）与 cost 展示
    UsageUpdated {
        session_id: String,
        delta: crate::llm::Usage,
        cumulative: crate::llm::Usage,
        context_window: usize,
    },
    /// write/edit 后 LSP 异步诊断结果（用于前端 inline 显示）
    /// 不强制注入 LLM context，由 session_manager 在下次 tool call 前批量注入
    LspDiagnostics {
        session_id: String,
        /// 关联到触发诊断的 write/edit tool_call_id（前端可 inline 显示）
        tool_call_id: Option<String>,
        file: String,
        language: String,
        wait_ms: u64,
        diagnostics: Vec<crate::lsp::LspDiagnostic>,
        ts: i64,
    },
    /// launch 工具：后台进程 stdout/stderr 单行输出（前端 inline 显示）
    LaunchOutput {
        session_id: String,
        id: String,
        name: Option<String>,
        stream: String,
        text: String,
        ts: i64,
    },
    /// launch 工具：后台进程退出
    LaunchExited {
        session_id: String,
        id: String,
        name: Option<String>,
        exit_code: Option<i32>,
        signal: Option<i32>,
        ts: i64,
    },
    Error {
        message: String,
    },
    /// 配置变更（provider/model/default 改完）：广播给所有 client 刷新 UI
    ConfigUpdated {
        op: String,
        providers: Vec<serde_json::Value>,
        models: Vec<serde_json::Value>,
        default_model: String,
        default_provider: Option<String>,
    },
    /// 自定义通知：用于 session.state_changed 等无需新增 enum 变体的事件
    Custom {
        method: String,
        params: serde_json::Value,
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
        self.broadcast_state_changed(session_id, "stopped").await;
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
        // launch_manager 需要 event_tx clone + launch config，先取出
        let launch_event_tx = event_tx.clone();
        let launch_config = config.launch.clone();
        // 设计文档 §8.8: 权限审批 pending 池（直接用主 event_tx 转发 ServerEvent::Permission*）
        let permission_registry = crate::permission::PermissionRegistry::new();
        let perm_event_tx = event_tx.clone();
        let perm_event_tx_for_set = perm_event_tx.clone();
        let perm_sink = Box::new(move |event: crate::permission::PermissionEvent| {
            use crate::permission::PermissionEvent as PE;
            match event {
                PE::Pending { session_id, request } => {
                    let _ = perm_event_tx_for_set.send(ServerEvent::PermissionPending {
                        session_id,
                        request,
                    });
                }
                PE::Resolved { session_id, request_id, decision } => {
                    let _ = perm_event_tx_for_set.send(ServerEvent::PermissionResolved {
                        session_id,
                        request_id,
                        decision,
                    });
                }
                PE::Cancelled { .. } => {
                    // cancel 在 session_manager 的 cancel_session 路径中独立处理
                }
            }
        });
        // 在 new() 内同步注册：PermissionRegistry 内部用 tokio::sync::Mutex，
        // 但 set_event_tx_boxed 是 async，不能在 Arc::new(Self {...}) 同步构造内 await
        // 设计：先构造 Arc<Self>，再异步注册 event sink
        let registry_clone = permission_registry.clone();
        let perm_sink_for_async = perm_sink;
        tokio::spawn(async move {
            registry_clone.set_event_tx_boxed(perm_sink_for_async).await;
        });
        let _ = perm_event_tx;
        // 设计文档 §provider: new() 接 Arc<AppConfig>（来自 load_config），
        // 内部解包成 AppConfig 用 RwLock 包裹（运行时支持 add/update/delete_provider）。
        // 这是边界：旧 API 兼容 + 新内存可溶性。
        let cfg_arc: Arc<AppConfig> = config;
        let cfg = Arc::try_unwrap(cfg_arc)
            .unwrap_or_else(|arc| (*arc).clone());
        Arc::new(Self {
            sessions: RwLock::new(HashMap::new()),
            tools,
            config: Arc::new(RwLock::new(cfg)),
            plugins,
            task_managers: RwLock::new(HashMap::new()),
            role_registry,
            experience_store,
            mcp_manager,
            project_resources: RwLock::new(HashMap::new()),
            command_dispatcher,
            event_tx: event_tx.clone(),
            ask_registry,
            permission_registry,
            lsp_diag_store: crate::lsp::PendingDiagnosticsStore::new(),
            launch_manager: crate::tools::launch::LaunchManager::new(
                launch_event_tx,
                launch_config,
            ),
            session_thinking_overrides: RwLock::new(HashMap::new()),
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
        // launch 工具：关闭所有后台进程（避免进程泄漏为孤儿进程）
        // 遍历所有 session 的所有进程，逐个 stop
        self.shutdown_all_launches().await;
        tracing::info!("session manager shutdown complete");
    }

    /// 优雅关闭所有 launch 启动的后台进程
    /// 每个进程用 default_stop_timeout_ms 等待，超时后强杀
    async fn shutdown_all_launches(&self) {
        // 收集所有 (session_id, id_or_name) 元组
        let procs = self.launch_manager.all_processes_snapshot().await;
        if procs.is_empty() {
            return;
        }
        tracing::info!("shutting down {} background processes...", procs.len());
        for (session_id, id, name, timeout_ms) in procs {
            let target = name.unwrap_or(id);
            let timeout = timeout_ms.unwrap_or(3000);
            if let Err(e) = self.launch_manager.stop(&target, &session_id, timeout).await {
                tracing::warn!("failed to stop launch process '{}': {}", target, e);
            }
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.event_tx.subscribe()
    }

    /// 暴露 event_tx 给 ask_user 工具做 late binding
    pub fn event_tx(&self) -> broadcast::Sender<ServerEvent> {
        self.event_tx.clone()
    }

    /// 解析模型配置：先按名称查 config.models，找不到则查 providers，
/// 从 ProviderConfig + model name 合成 ModelConfig。
/// S1 修复: 之前只查 cfg.models，用户通过 ProviderPanel 添加 provider 后
/// 无法 /model set 到该 provider 的模型。
fn resolve_model(&self, model_name: Option<&str>) -> Result<ModelConfig> {
        let cfg = self.current_config();
        let name = model_name.unwrap_or(&cfg.default_model);

        // 1. 优先查 cfg.models（旧式扁平配置）
        if let Some(m) = cfg.models.get(name) {
            return Ok(m.clone());
        }

        // 2. 查 default_model 的扁平配置
        if model_name.is_none() || name == cfg.default_model {
            if let Some(m) = cfg.models.get(&cfg.default_model) {
                return Ok(m.clone());
            }
        }

        // 3. S1 修复: 查 providers -- 遍历所有 provider 的 models 列表
        //    匹配规则：纯 model name（如 "gpt-4o"）或 "provider/model" 形式
        for (pname, p) in &cfg.providers {
            for mname in &p.models {
                let full = format!("{pname}/{mname}");
                if mname.as_str() == name || full == name {
                    return Ok(self.synthesize_model_from_provider(p, mname));
                }
            }
            // 也检查 name 是否为 "provider/model" 形式
            if let Some(bare) = name.strip_prefix(&format!("{pname}/")) {
                for mname in &p.models {
                    if mname.as_str() == bare {
                        return Ok(self.synthesize_model_from_provider(p, mname));
                    }
                }
            }
        }

        // 4. M7 修复: 用户明确请求的模型不存在时直接报错，不静默 fallback 到 default_model
        //    （仅当 model_name 为 None 时才用 default_model，那已经在步骤 1-3 处理了）
        if model_name.is_some() {
            anyhow::bail!(
                "model '{}' not found in config.models or providers",
                name
            );
        }

        anyhow::bail!(
            "default_model '{}' not found in config.models or providers",
            cfg.default_model
        );
    }

    /// 从 ProviderConfig + model name 合成 ModelConfig
    /// M11 修复: 委托给 ProviderConfig::synthesize_model_config 共享方法
    fn synthesize_model_from_provider(&self, p: &crate::types::ProviderConfig, model_name: &str) -> ModelConfig {
        p.synthesize_model_config(model_name)
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

        // 设计文档 §provider: 读 cfg 一次用于 ToolContext（避免每字段访问 RwLock）
        let cfg = self.current_config();
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
            // M3 修复: ToolContext.app_config 是构造时的不可变快照。
            // 已运行的 tool 调用继续用旧 cfg；新 tool 调用通过 current_config() 拿当前 cfg。
            app_config: Arc::new(cfg),
            mcp_manager: Some(self.mcp_manager.clone()),
            current_model,  // already Arc<ModelConfig> from agent.model_config.clone()
            lsp_diag_store: self.lsp_diag_store.clone(),
            launch_manager: self.launch_manager.clone(),
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
        let cfg_snapshot = self.current_config();
        let max_iters = cfg_snapshot.loop_max_iters;
        let agent = AgentSession::new(
            jsonl,
            model_config,
            llm,
            self.tools.clone(),
            max_iters,
            self.role_registry.clone(),
            Arc::new(cfg_snapshot.models.clone()),
        );
        // P1: 启动期预热 tiktoken
        AgentSession::prewarm_token_estimator();

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
        self.broadcast_state_changed(&session_id, "idle").await;

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
        // LSP 异步诊断：tool 执行前 drain pending 队列，拼成 ToolResult 注入到 messages
        // 这样 LLM 在看到本次工具结果时也能看到之前 write/edit 的 LSP 反馈
        self.inject_pending_lsp_diagnostics(session_id).await;
        let ctx = self.build_tool_context(session_id, &entry).await?;
        let call = crate::types::ToolCall {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            args,
        };
        self.tools.execute(&call, &ctx).await
    }

    /// 取出 session 的 pending LSP 诊断，拼接成 user message 注入到 messages
/// 作为下一次 tool call 的 sibling，让 LLM 看到之前的 LSP 反馈
pub async fn inject_pending_lsp_diagnostics(&self, session_id: &str) {
        let diags = self.lsp_diag_store.drain(session_id).await;
        if diags.is_empty() {
            return;
        }
        let cfg = self.current_config();
        let cfg = &cfg.tools.lsp_diagnostics;
        let text = crate::lsp::diagnostics_store::format_for_context(&diags, cfg.max_results);
        if text.is_empty() {
            return;
        }
        // 用 User role 包装，前缀标明是历史 LSP 反馈（不污染 assistant 上下文）
        let msg = crate::types::Message::new(
            crate::types::Role::User,
            vec![crate::types::ContentBlock::Text { text }],
        );
        // 写 jsonl + 注入内存
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            let mut agent = entry.session.lock().await;
            let _ = agent.add_message(msg);
        }
        tracing::debug!(
            "injected {} LSP diagnostic batch(es) for session {}",
            diags.len(),
            session_id
        );
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

    /// 切换 session 的 model（/model set 命令 / session.model.set RPC）
    /// 解析新模型配置，重建 LLM adapter，替换 agent 的 model_config + llm，
    /// 并广播 ModelChanged 事件让所有订阅 client 同步更新 UI。
    pub async fn set_model(&self, session_id: &str, model_name: &str) -> Result<()> {
        let new_config = Arc::new(self.resolve_model(Some(model_name))?);
        let new_llm = create_adapter(&new_config)?;
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).context("session not found")?;
        let mut agent = entry.session.lock().await;
        agent.set_model(new_config, new_llm)?;
        drop(agent);
        // 持久化 model 到 session_state（"key_value" 表），重启后 snapshot/attach 复原
        if let Some(store) = SessionStateStore::for_session(session_id).await {
            if let Err(e) = store.set_kv(session_id, "model", model_name).await {
                tracing::warn!("session_state set model persist failed: {}", e);
            }
        }
        // 广播 ModelChanged 事件（确保所有订阅 client 同步看到）
        let _ = self.event_tx.send(ServerEvent::ModelChanged {
            session_id: session_id.to_string(),
            model: model_name.to_string(),
        });
        tracing::info!("session {} switched to model: {}", session_id, model_name);
        Ok(())
    }

    /// 获取当前 model 名称
    pub async fn current_model(&self, session_id: &str) -> Result<String> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).context("session not found")?;
        let agent = entry.session.lock().await;
        Ok(agent.model_config.name.clone())
    }

    /// 广播 model 切换事件
    pub async fn broadcast_model_changed(&self, session_id: &str, model: &str) -> Result<()> {
        let _ = self.event_tx.send(ServerEvent::ModelChanged {
            session_id: session_id.to_string(),
            model: model.to_string(),
        });
        Ok(())
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
        if self.current_config().memory.auto_recall {
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
        mgr.broadcast_state_changed(&sid, "running").await;
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

        if self.current_config().memory.auto_recall {
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
        mgr.broadcast_state_changed(&sid, "running").await;
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
        self.broadcast_state_changed(session_id, "running").await;

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
                mgr.broadcast_state_changed(&sid, "stopped").await;
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

        let limit = self.current_config().memory.recall_limit.max(1);
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

                        // 设计文档 §8.8: 权限审批（write 工具之前）
                        // PermissionConfig.requires_approval 决定：yolo 跳过；standard 写工具需批；strict 更严
                        if let Some(reason) = self.current_config().permission.requires_approval(&tc.name) {
                            match self
                                .permission_registry
                                .check_and_wait(&self.current_config().permission, session_id, tc)
                                .await
                            {
                                Ok(()) => {
                                    tracing::debug!(
                                        "tool {} approved for session {}",
                                        tc.name, session_id
                                    );
                                }
                                Err(e) => {
                                    tracing::info!(
                                        "tool {} denied by permission gate: {}",
                                        tc.name, e
                                    );
                                    let block_msg = Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                                        id: tc.id.clone(),
                                        output: crate::types::ToolOutput::Error {
                                            message: format!("permission denied: {}", e),
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
                            }
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
                            && !self.current_config().tools.is_auto_approved(&tc.name) {
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
                        let cfg = self.current_config();
                        let token_threshold = (agent.model_config().context_window as f32
                            * cfg.compact.threshold) as usize;
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
                        agent.maybe_compact(&cfg.compact).await;
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
        if self.current_config().memory.auto_capture {
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

    // ==================== permission RPC（设计文档 §8.8）====================

    /// permission.submit - 客户端提交权限审批决议
    /// RPC 路径：ws 收到 client 推送的 decision → 唤醒对应 pending → tool 执行继续
    pub async fn submit_permission(
        &self,
        session_id: &str,
        response: crate::permission::PermissionResponse,
    ) -> Result<()> {
        self.permission_registry.submit(session_id, response).await
    }

    /// permission.peek - 客户端 attach 时查询当前 pending 审批（用于断线重连恢复）
    pub async fn peek_permission(&self, session_id: &str) -> Option<serde_json::Value> {
        // 简化：仅查内存中的 pending；复杂持久化留给后续
        None
    }

    /// permission.set_level - 切换权限级别（runtime 修改）
    pub fn set_permission_level(&self, level: crate::types::PermissionLevel) {
        // 设计文档 §provider: 当前 level 在 cfg 里；改完后必须 save_config + write lock 才生效
        // 为简化起见这里仅记日志；UI 应通过 config.set + config 持久化路径生效
        tracing::warn!(
            "set_permission_level({:?}) called; for full effect use /config set or restart with new ~/.mcoder/config.toml",
            level
        );
    }

    // ==================== ask_user text submit ====================
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
                agent.model_config.name.clone(),
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

        // 10. lsp_servers：从 per-project LspManager 获取已启动的语言服务器
        let lsp_servers = match self.get_or_create_resources(Path::new(&project_path_str)).await {
            Ok(res) => res.lsp_manager.active_languages().await,
            Err(_) => Vec::new(),
        };

        Ok(SessionSnapshot {
            session: SessionSnapshotSession {
                session_id: session_id.to_string(),
                title,
                project_path: project_path_str,
                role,
                model: model_name,
                loop_state,
                stop_reason,
                version: env!("CARGO_PKG_VERSION").to_string(),
                lsp_servers,
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
        let cfg_snapshot = self.current_config();
        let max_iters = cfg_snapshot.loop_max_iters;
        let agent = AgentSession::new(
            jsonl,
            model_config,
            llm,
            self.tools.clone(),
            max_iters,
            self.role_registry.clone(),
            Arc::new(cfg_snapshot.models.clone()),
        );

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
        self.broadcast_state_changed(session_id, "stopped").await;
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
        // 一次 blocking_read 拿 cfg，再按 key 分发；避免每字段访问都 RwLock
        let cfg = self.current_config();
        match key {
            Some("default_model") => serde_json::json!(cfg.default_model),
            Some("loop_max_iters") => serde_json::json!(cfg.loop_max_iters),
            Some("compact") => serde_json::json!(cfg.compact),
            Some("compact.threshold") => serde_json::json!(cfg.compact.threshold),
            Some("compact.keep_recent") => serde_json::json!(cfg.compact.keep_recent),
            Some("roles") => serde_json::json!(cfg.roles),
            Some("models") => serde_json::json!(cfg.models),
            Some("tui") => serde_json::json!(cfg.tui),
            Some("tui.compact") => serde_json::json!(cfg.tui.compact),
            Some("memory") => serde_json::json!(cfg.memory),
            Some("memory.auto_recall") => serde_json::json!(cfg.memory.auto_recall),
            Some("memory.auto_capture") => serde_json::json!(cfg.memory.auto_capture),
            Some("server") => serde_json::json!(cfg.server),
            Some(_) | None => serde_json::json!({
                "default_model": cfg.default_model,
                "loop_max_iters": cfg.loop_max_iters,
                "server": cfg.server,
                "tui": cfg.tui,
                "memory": cfg.memory,
            }),
        }
    }

    /// config.set - 设置配置项（运行时，不持久化）
    /// 设计文档 §8.3: 支持运行时修改配置，仅在内存生效，不写回 config.toml
    pub async fn set_config(self: &Arc<Self>, key: &str, value: serde_json::Value) -> Result<()> {
        tracing::info!("config.set requested: {} = {} (runtime, not persisted)", key, value);
        // 校验 key 合法性（不实际修改 self.config；走全局 override map）
        match key {
            "loop_max_iters" => {
                if !value.is_u64() { anyhow::bail!("loop_max_iters expects u32"); }
            }
            "tui.compact" => {
                if !value.is_boolean() { anyhow::bail!("tui.compact expects bool"); }
            }
            "compact.threshold" => {
                if !value.is_f64() { anyhow::bail!("compact.threshold expects f64"); }
            }
            "compact.keep_recent" => {
                if !value.is_u64() { anyhow::bail!("compact.keep_recent expects u32"); }
            }
            "memory.auto_recall" => {
                if !value.is_boolean() { anyhow::bail!("memory.auto_recall expects bool"); }
            }
            "memory.auto_capture" => {
                if !value.is_boolean() { anyhow::bail!("memory.auto_capture expects bool"); }
            }
            _ => {
                tracing::warn!("config.set: unsupported key '{}' (supported: loop_max_iters/tui.compact/compact.threshold/compact.keep_recent/memory.auto_recall/memory.auto_capture)", key);
                anyhow::bail!("unsupported config key: {}", key);
            }
        }
        // S3 修复: 不再 read-modify-write self.config（之前 new_config 构造后根本没用）；
        // override map 自身有 RwLock 保护，原子写入即可
        config_overrides().await.write().await.insert(key.to_string(), value);
        tracing::info!("config.set: applied '{}' (note: runtime override, not persisted)", key);
        Ok(())
    }

    /// 设计文档 §8.3: 读取运行时覆盖的配置值
    pub async fn get_config_override(&self, key: &str) -> Option<serde_json::Value> {
        config_overrides().await.read().await.get(key).cloned()
    }

    /// config.list_models - 列出所有已配置的模型（含从 providers 展开）
    pub fn list_models(&self) -> Vec<serde_json::Value> {
        let cfg = self.current_config();
        let mut out: Vec<serde_json::Value> = cfg.models.iter().map(|(name, m)| serde_json::json!({
            "name": name,
            "display_name": m.name,
            "protocol": serde_json::to_string(&m.protocol).unwrap_or_default().trim_matches('"').to_string(),
            "context_window": m.context_window,
            "max_tokens": m.max_tokens,
            "source": "models",
        })).collect();
        for (pname, p) in &cfg.providers {
            for m in &p.models {
                let full = format!("{pname}/{m}");
                if cfg.models.contains_key(&full) { continue; }
                out.push(serde_json::json!({
                    "name": full,
                    "display_name": m,
                    "protocol": p.protocol,
                    "provider": pname,
                    "source": "provider",
                }));
            }
        }
        out.sort_by(|a, b| a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")));
        out
    }

    /// config.list_providers - 列出供应商
    pub fn list_providers(&self) -> Vec<serde_json::Value> {
        self.current_config().providers.iter().map(|(name, p)| {
            // S1 修复: has_api_key 展开env var后判断，避免 ${ENV_VAR} 永远非空误报
            let expanded_key = crate::config::expand_env_var(&p.api_key);
            serde_json::json!({
                "name": name,
                "display_name": p.name,
                "protocol": p.protocol,
                "base_url": p.base_url,
                "has_api_key": !expanded_key.is_empty(),
                "enabled": p.enabled,
                "models": p.models,
            })
        }).collect()
    }

    /// config.add_provider - 添加供应商
    /// S3 修复: read-modify-write 在单次 write lock 内完成，消除 TOCTOU
    /// M1 修复: 检查重复名
    /// 设计文档 §provider: 写 ~/.mcoder/config.toml，原子写（tmp + rename）
    pub async fn add_provider(
        self: &Arc<Self>,
        name: String,
        protocol: String,
        base_url: String,
        api_key: String,
        models: Vec<String>,
    ) -> Result<()> {
        // M8: 校验 name 非空
        if name.trim().is_empty() {
            anyhow::bail!("provider name cannot be empty");
        }
        // M8: 校验 protocol 合法
        let valid_protocols = ["openai", "openai_responses", "anthropic", "ollama", "gemini", "custom"];
        if !valid_protocols.contains(&protocol.as_str()) {
            anyhow::bail!("invalid protocol '{protocol}'; must be one of: {}", valid_protocols.join(", "));
        }
        let mut guard = self.config.write().await;
        // M1: 检查重复名
        if guard.providers.contains_key(&name) {
            anyhow::bail!("provider '{name}' already exists; use update_provider to modify");
        }
        let mut new_config = (*guard).clone();
        new_config.providers.insert(name.clone(), crate::types::ProviderConfig {
            name: name.clone(),
            protocol,
            base_url,
            api_key,
            models,
            enabled: true,
            model_params: Default::default(),
        });
        crate::config::save_config(&new_config)?;
        *guard = new_config;
        drop(guard);
        self.broadcast_config_updated("add_provider").await;
        Ok(())
    }

    /// config.update_provider - 改字段
    /// S3 修复: read-modify-write 在单次 write lock 内完成
    pub async fn update_provider(
        self: &Arc<Self>,
        name: String,
        protocol: Option<String>,
        base_url: Option<String>,
        api_key: Option<String>,
        models: Option<Vec<String>>,
        enabled: Option<bool>,
    ) -> Result<()> {
        let mut guard = self.config.write().await;
        let mut new_config = (*guard).clone();
        {
            let p = new_config.providers.get_mut(&name)
                .ok_or_else(|| anyhow::anyhow!("provider '{name}' not found"))?;
            if let Some(v) = protocol { p.protocol = v; }
            if let Some(v) = base_url { p.base_url = v; }
            if let Some(v) = api_key { p.api_key = v; }
            if let Some(v) = models { p.models = v; }
            if let Some(v) = enabled { p.enabled = v; }
        }
        crate::config::save_config(&new_config)?;
        *guard = new_config;
        drop(guard);
        self.broadcast_config_updated("update_provider").await;
        Ok(())
    }

    /// config.delete_provider - 删除供应商
    /// S3 修复: read-modify-write 在单次 write lock 内完成
    /// M2 修复: 清理悬空 default_model（若 default_model 在被删 provider 的 models 列表中）
    pub async fn delete_provider(self: &Arc<Self>, name: String) -> Result<()> {
        let mut guard = self.config.write().await;
        let mut new_config = (*guard).clone();
        let removed = new_config.providers.remove(&name)
            .ok_or_else(|| anyhow::anyhow!("provider '{name}' not found"))?;
        // 清理 default_provider
        if new_config.default_provider.as_deref() == Some(&name) {
            new_config.default_provider = None;
        }
        // M2: 清理悬空 default_model（S2: 支持 "provider/model" 形式）
        if !new_config.default_model.is_empty() && removed.has_model(&name, &new_config.default_model) {
            tracing::warn!(
                "default_model '{}' was in deleted provider '{}'; clearing default_model",
                new_config.default_model, name
            );
            new_config.default_model = String::new();
        }
        crate::config::save_config(&new_config)?;
        *guard = new_config;
        drop(guard);
        self.broadcast_config_updated("delete_provider").await;
        Ok(())
    }

    /// config.set_default - 设置默认模型
    /// S3 修复: read-modify-write 在单次 write lock 内完成
    /// S2 修复: 支持 "provider/model" 形式
    pub async fn set_default(self: &Arc<Self>, model: String, provider: Option<String>) -> Result<()> {
        let mut guard = self.config.write().await;
        let mut new_config = (*guard).clone();
        // S2: 校验 model 在 providers/models 中能找到（支持纯名和 "provider/model" 形式）
        let model_exists = new_config.models.contains_key(&model)
            || new_config.providers.iter().any(|(pname, p)| p.has_model(pname, &model));
        if !model_exists && !model.is_empty() {
            anyhow::bail!("model '{model}' not found in providers/models");
        }
        // 校验 provider 参数存在
        if let Some(ref pname) = provider {
            if !new_config.providers.contains_key(pname) {
                anyhow::bail!("provider '{pname}' not found");
            }
        }
        new_config.default_model = model;
        new_config.default_provider = provider;
        crate::config::save_config(&new_config)?;
        *guard = new_config;
        drop(guard);
        self.broadcast_config_updated("set_default").await;
        Ok(())
    }

    /// config.test_provider - 测试供应商连通性
    /// S3 修复: ollama 测试 /v1/models（与实际推理路径一致）
    /// M14 修复: 用 normalized_protocol 做大小写不敏感匹配
    /// S2 修复: 此处展开 ${ENV_VAR}
    pub async fn test_provider(&self, name: String) -> Result<serde_json::Value> {
        let provider = self.current_config().providers.get(&name)
            .ok_or_else(|| anyhow::anyhow!("provider '{name}' not found"))?
            .clone();
        // S2: 展开 ${ENV_VAR}
        let api_key = crate::config::expand_env_var(&provider.api_key);
        // M14: 用 normalized_protocol 做大小写不敏感匹配
        let url = match provider.normalized_protocol() {
            "anthropic" => format!("{}/v1/models", provider.base_url.trim_end_matches('/')),
            "ollama" => format!("{}/v1/models", provider.base_url.trim_end_matches('/')),
            _ => format!("{}/models", provider.base_url.trim_end_matches('/')),
        };
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()?;
        let mut req = client.get(&url);
        if !api_key.is_empty() && provider.normalized_protocol() != "ollama" {
            req = req.bearer_auth(&api_key);
        }
        match req.send().await {
            Ok(r) if r.status().is_success() => Ok(serde_json::json!({
                "ok": true,
                "status": r.status().as_u16(),
                "url": url,
            })),
            Ok(r) => Ok(serde_json::json!({
                "ok": false,
                "status": r.status().as_u16(),
                "url": url,
                "hint": format!("HTTP {}; check base_url and api_key", r.status().as_u16()),
            })),
            Err(e) => Ok(serde_json::json!({
                "ok": false,
                "url": url,
                "error": e.to_string(),
                "hint": "cannot reach server; check network and base_url",
            })),
        }
    }

    /// config.set_model_params - 设置 per-model 参数
    /// 写入 ProviderConfig.model_params，持久化到 config.toml
    pub async fn set_model_params(
        self: &Arc<Self>,
        provider: String,
        model: String,
        params: crate::types::ModelParams,
    ) -> Result<()> {
        let mut guard = self.config.write().await;
        let mut new_config = (*guard).clone();
        let p = new_config.providers.get_mut(&provider)
            .ok_or_else(|| anyhow::anyhow!("provider '{provider}' not found"))?;
        p.model_params.insert(model, params);
        crate::config::save_config(&new_config)?;
        *guard = new_config;
        drop(guard);
        self.broadcast_config_updated("set_model_params").await;
        Ok(())
    }

    /// config.get_model_params - 读取 per-model 参数
    pub fn get_model_params(&self, provider: &str, model: &str) -> crate::types::ModelParams {
        self.current_config()
            .providers.get(provider)
            .and_then(|p| p.model_params.get(model))
            .cloned()
            .unwrap_or_default()
    }

    /// config.get_protocol_schema - 返回协议参数 schema（供 UI 渲染控件）
    pub fn get_protocol_schema(&self, protocol: &str) -> serde_json::Value {
        crate::types::protocol_schema(protocol)
    }

    /// config.quick_thinking - 快捷切换当前 session 模型的思考深度
/// S1 修复: 拒绝 streaming 中的切换（防止旧 task 与新 adapter 并存造成 tool_use id 撞车）
/// S2 修复: 临时覆盖存到 session-level override map，不污染 agent.model_config，
///         解决下次 hydrate 时丢失的问题（每次 LLM 调用都从 override 重新合成）
pub async fn quick_thinking(
        self: &Arc<Self>,
        session_id: &str,
        depth: crate::types::ThinkingDepth,
    ) -> Result<()> {
        let entry = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
                .ok_or_else(|| anyhow::anyhow!("session '{session_id}' not found"))?
        };
        // S1: 拒绝 streaming 中的切换（防止旧 adapter 与新 adapter 并存）
        if entry.loop_running.load(std::sync::atomic::Ordering::SeqCst) {
            anyhow::bail!("cannot change thinking depth while agent loop is running; wait for current request to finish");
        }
        // S2: 用 session-level override map 存，不修改 agent.model_config
        let mut overrides = self.session_thinking_overrides.write().await;
        if depth == crate::types::ThinkingDepth::None {
            overrides.remove(session_id);
        } else {
            overrides.insert(session_id.to_string(), depth);
        }
        // 重建 adapter 并 set_model，使下一次 LLM 调用使用新参数
        let mut agent = entry.session.lock().await;
        let new_config = self.build_session_model_config(&agent);
        let new_llm = create_adapter(&new_config)?;
        agent.set_model(std::sync::Arc::new(new_config), new_llm)?;
        Ok(())
    }

    // ==================== P1: 子代理 session 化 ====================

    /// 创建子 session（subagent 或 handoff）
    /// 复用 create_session 创建基础 session，然后设置 child meta
    pub async fn create_child_session(
        self: &Arc<Self>,
        parent_session_id: &str,
        title: &str,
        model_name: Option<&str>,
        source: crate::persistence::jsonl::SessionSource,
        subagent_role: Option<&str>,
        task_description: Option<&str>,
    ) -> Result<String> {
        // 获取父 session 的 project_path
        let parent_entry = {
            let sessions = self.sessions.read().await;
            sessions.get(parent_session_id).cloned()
                .ok_or_else(|| anyhow::anyhow!("parent session '{parent_session_id}' not found"))?
        };
        let project_path = {
            let parent_agent = parent_entry.session.lock().await;
            parent_agent.session.project_path().to_path_buf()
        };

        // 创建基础 session
        let child_id = self.create_session(&project_path, title, model_name).await?;

        // 设置 child meta（需要 load jsonl 再 set_child_meta）
        let mut jsonl = crate::persistence::jsonl::JsonlSession::load(&child_id)?;
        jsonl.set_child_meta(parent_session_id, source, subagent_role, task_description)?;

        Ok(child_id)
    }

    /// 注入消息到 session（不触发 agent loop）
    /// 用于 handoff 文档注入、子代理 system prompt 注入等
    pub async fn inject_message(
        self: &Arc<Self>,
        session_id: &str,
        message: crate::types::Message,
    ) -> Result<()> {
        let entry = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
                .ok_or_else(|| anyhow::anyhow!("session '{session_id}' not found"))?
        };
        let mut agent = entry.session.lock().await;
        agent.add_message(message.clone())?;
        // 广播消息事件（让 attached 客户端看到）
        let _ = self.event_tx.send(ServerEvent::Message {
            session_id: session_id.to_string(),
            message,
        });
        Ok(())
    }

    /// P2: 直接启动 session 的 agent loop（不加额外消息）
    /// 用于子代理 session：消息已通过 inject_message 注入，只需触发 loop。
    /// 调用方需保证此前没有并发 loop（内部 CAS loop_running false→true）。
    pub async fn start_session_loop(
        self: &Arc<Self>,
        session_id: &str,
    ) -> Result<()> {
        let entry = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
                .ok_or_else(|| anyhow::anyhow!("session '{session_id}' not found"))?
        };

        // CAS loop_running false -> true
        if entry.loop_running.compare_exchange(
            false, true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        ).is_err() {
            anyhow::bail!("agent loop already running for session {}", session_id);
        }

        self.persist_loop_state(session_id, "running", None).await;
        self.broadcast_state_changed(session_id, "running").await;

        self.spawn_run_loop(session_id.to_string(), entry);

        Ok(())
    }

    /// P2: 查询 session 的 agent loop 是否正在运行
    pub async fn is_loop_running(&self, session_id: &str) -> bool {
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            entry.loop_running.load(std::sync::atomic::Ordering::SeqCst)
        } else {
            false
        }
    }

    /// 列出指定父 session 的所有子代理 session
    /// 返回每个子 session 的实时状态
    pub async fn list_child_sessions(
        self: &Arc<Self>,
        parent_session_id: &str,
    ) -> Vec<serde_json::Value> {
        let all_sessions = crate::persistence::jsonl::JsonlSession::list(None)
            .unwrap_or_default();

        let mut children = Vec::new();
        for meta in all_sessions {
            if meta.parent_session_id.as_deref() == Some(parent_session_id) {
                // 获取实时状态
                let (loop_state, msg_count) = self.get_session_runtime_state(&meta.session_id).await;
                children.push(serde_json::json!({
                    "session_id": meta.session_id,
                    "title": meta.title,
                    "model": meta.model,
                    "source": meta.source,
                    "subagent_role": meta.subagent_role,
                    "task_description": meta.task_description,
                    "parent_session_id": meta.parent_session_id,
                    "loop_state": loop_state,
                    "message_count": msg_count,
                    "created_at": meta.created_at.to_rfc3339(),
                }));
            }
        }
        children
    }

    /// 获取 session 的实时 loop_state 和 message_count
    async fn get_session_runtime_state(&self, session_id: &str) -> (String, usize) {
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            let agent = entry.session.lock().await;
            let loop_running = entry.loop_running.load(std::sync::atomic::Ordering::SeqCst);
            let loop_state = if loop_running { "running" } else { "idle" };
            let msg_count = agent.messages.len();
            (loop_state.to_string(), msg_count)
        } else {
            // session 不在内存，从 DB 读 loop_state
            let (db_state, _) = match SessionStateStore::for_session(session_id).await {
                Some(store) => store.get_session_state(session_id).await,
                None => ("idle".to_string(), None),
            };
            (db_state, 0)
        }
    }

    /// 广播 session 状态变化通知
    pub async fn broadcast_state_changed(&self, session_id: &str, loop_state: &str) {
        let msg_count = {
            let sessions = self.sessions.read().await;
            if let Some(entry) = sessions.get(session_id) {
                let agent = entry.session.lock().await;
                agent.messages.len()
            } else {
                0
            }
        };
        let _ = self.event_tx.send(ServerEvent::Custom {
            method: "session.state_changed".into(),
            params: serde_json::json!({
                "session_id": session_id,
                "loop_state": loop_state,
                "message_count": msg_count,
            }),
        });
    }

    // ==================== P4: handoff / handoff_back ====================

    /// session.handoff - 生成交接文档 + 创建子 session + 注入文档
    pub async fn handoff(
        self: &Arc<Self>,
        session_id: &str,
        task_prompt: &str,
    ) -> Result<HandoffResult> {
        // 1. 获取当前 session 消息
        let entry = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
                .ok_or_else(|| anyhow::anyhow!("session '{session_id}' not found"))?
        };
        let (messages, model_config, project_path) = {
            let agent = entry.session.lock().await;
            (
                agent.messages.clone(),
                (*agent.model_config).clone(),
                agent.session.project_path().to_path_buf(),
            )
        };
        let _ = project_path; // project_path 由 create_child_session 内部从 parent 获取

        // 2. 用 LLM 生成 handoff 文档
        let handoff_doc = self.generate_handoff_doc(&messages, task_prompt, &model_config).await?;

        // 3. 创建子 session（source=Handoff）
        let title = if task_prompt.len() > 60 {
            format!("Handoff: {}", &task_prompt[..60])
        } else {
            format!("Handoff: {}", task_prompt)
        };
        let child_id = self.create_child_session(
            session_id,
            &title,
            Some(&model_config.name),
            crate::persistence::jsonl::SessionSource::Handoff,
            None,
            Some(task_prompt),
        ).await?;

        // 4. 注入 handoff 文档到子 session（不触发 agent loop）
        self.inject_message(&child_id, crate::types::Message::system(&handoff_doc)).await?;

        Ok(HandoffResult {
            handoff_doc,
            new_session_id: child_id,
            original_session_id: session_id.to_string(),
        })
    }

    /// session.handoff_back - 生成回传文档 + 注入回父 session
    pub async fn handoff_back(
        self: &Arc<Self>,
        from_session_id: &str,
    ) -> Result<HandoffBackResult> {
        // 1. 从 meta 读 parent_session_id
        let jsonl = crate::persistence::jsonl::JsonlSession::load(from_session_id)?;
        let parent_session_id = jsonl.meta().parent_session_id.clone()
            .ok_or_else(|| anyhow::anyhow!("session '{from_session_id}' has no parent (not a child session)"))?;

        // 2. 获取 from_session 的消息
        let entry = {
            let sessions = self.sessions.read().await;
            sessions.get(from_session_id).cloned()
                .ok_or_else(|| anyhow::anyhow!("session '{from_session_id}' not found"))?
        };
        let (messages, model_config) = {
            let agent = entry.session.lock().await;
            (agent.messages.clone(), (*agent.model_config).clone())
        };

        // 3. 用 LLM 生成回传文档
        let back_doc = self.generate_handoff_back_doc(&messages, &model_config).await?;

        // 4. 注入到父 session
        let inject_text = format!("## Handoff Back from {}\n\n{}", from_session_id, back_doc);
        self.inject_message(&parent_session_id, crate::types::Message::system(&inject_text)).await?;

        Ok(HandoffBackResult {
            back_doc,
            to_session_id: parent_session_id,
        })
    }

    /// LLM 生成 handoff 文档
    async fn generate_handoff_doc(
        self: &Arc<Self>,
        messages: &[crate::types::Message],
        task_prompt: &str,
        model_config: &crate::types::ModelConfig,
    ) -> Result<String> {
        // 序列化消息（复用 compaction 的序列化 + 限制长度）
        let serialized: String = messages.iter()
            .map(crate::agent::compaction::serialize_for_summary)
            .collect::<Vec<_>>()
            .join("\n");
        let truncated = if serialized.len() > 20000 {
            crate::agent::compaction::fallback_truncate(&serialized, 10000, 10000)
        } else {
            serialized
        };

        // 脱敏
        let sanitized = sanitize_for_handoff(&truncated);

        let prompt = format!(
            r#"你是一个交接文档生成器。根据以下对话历史，生成一份结构化的交接 Markdown 文档。

## 任务描述
{task_prompt}

## 对话历史
{sanitized}

## 输出格式（严格遵守）
# Handoff Document

## Project Context
- 项目路径、模型等元信息

## Task
{task_prompt}

## Key Decisions
- 从对话中提取的关键决策

## File Pointers
- 引用过的文件路径列表

## Pitfalls
- 踩过的坑、已排除的方案

## 规则
1. 不要复制代码内容
2. 不要包含 API key、密码
3. 保持简洁（< 500 词）
4. 聚焦于"下一个会话需要知道什么""#,
            task_prompt = task_prompt,
            sanitized = sanitized
        );

        let req = vec![crate::types::Message::user(&prompt)];
        let llm = crate::llm::create_adapter(model_config)?;
        let resp = llm.chat(&req, &[], model_config).await?;
        Ok(resp.content.unwrap_or_else(|| "handoff doc generation failed".into()))
    }

    /// LLM 生成回传文档
    async fn generate_handoff_back_doc(
        self: &Arc<Self>,
        messages: &[crate::types::Message],
        model_config: &crate::types::ModelConfig,
    ) -> Result<String> {
        let serialized: String = messages.iter()
            .map(crate::agent::compaction::serialize_for_summary)
            .collect::<Vec<_>>()
            .join("\n");
        let truncated = if serialized.len() > 20000 {
            crate::agent::compaction::fallback_truncate(&serialized, 10000, 10000)
        } else {
            serialized
        };
        let sanitized = sanitize_for_handoff(&truncated);

        let prompt = format!(
            r#"你是一个回传文档生成器。根据以下子代理的对话历史，生成一份简洁的回传 Markdown 文档。

## 对话历史
{sanitized}

## 输出格式
## Summary
- 完成了什么

## Key Learnings
- 学到了什么、哪些结论不是一眼能看出的

## Code Changes
- 改了哪些文件，一句话摘要

## 规则
1. 保持简洁（< 300 词）
2. 不要包含 API key、密码
3. 聚焦于"父会话需要知道什么""#,
            sanitized = sanitized
        );

        let req = vec![crate::types::Message::user(&prompt)];
        let llm = crate::llm::create_adapter(model_config)?;
        let resp = llm.chat(&req, &[], model_config).await?;
        Ok(resp.content.unwrap_or_else(|| "handoff back doc generation failed".into()))
    }

    /// S2 修复: 构造 session 当前生效的 ModelConfig（合并 provider 默认 + override）
    /// agent.model_config 是 provider 默认值；session override 只覆盖 thinking_depth。
    /// 这个函数把两者合并，确保 adapter 总是用最新配置。
    fn build_session_model_config(&self, agent: &crate::agent::AgentSession) -> crate::types::ModelConfig {
        let mut cfg = (*agent.model_config).clone();
        let sid = agent.session.id();
        if let Ok(overrides) = self.session_thinking_overrides.try_read() {
            if let Some(depth) = overrides.get(sid) {
                cfg.thinking_depth = Some(*depth);
            }
        }
        cfg
    }

    /// S2 修复: 读取 session 的思考深度 override
    pub async fn get_session_thinking(&self, session_id: &str) -> Option<crate::types::ThinkingDepth> {
        let overrides = self.session_thinking_overrides.read().await;
        overrides.get(session_id).copied()
    }

    /// config.list_models_protocols - 列出支持的协议
    pub fn list_protocols(&self) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({"id": "openai",           "name": "OpenAI / 兼容", "default_url": "https://api.openai.com/v1"}),
            serde_json::json!({"id": "openai_responses", "name": "OpenAI Responses", "default_url": "https://api.openai.com/v1"}),
            serde_json::json!({"id": "anthropic",        "name": "Anthropic Claude", "default_url": "https://api.anthropic.com"}),
            serde_json::json!({"id": "ollama",           "name": "Ollama (本地)", "default_url": "http://localhost:11434"}),
            serde_json::json!({"id": "gemini",           "name": "Google Gemini", "default_url": "https://generativelanguage.googleapis.com"}),
            serde_json::json!({"id": "custom",           "name": "Custom OpenAI-兼容", "default_url": ""}),
        ]
    }

    /// 读取当前配置快照（同步；用于 sync 读路径）
    /// 设计文档 §provider: AppConfig clone ≈ 几 KB，可忽略
    fn current_config(&self) -> AppConfig {
        self.config.blocking_read().clone()
    }

    /// 广播 config.updated 事件给所有连接的 client（async 以便 RPC 调用方保持 .await 风格）
    /// M4 修复: 只调一次 current_config()，复用快照给 list_providers / list_models / default 字段
    async fn broadcast_config_updated(&self, op: &str) {
        let cfg = self.current_config();
        // M4: 从同一份 cfg 快照内联构建 providers/models JSON，避免多次 blocking_read
        let providers: Vec<serde_json::Value> = cfg.providers.iter().map(|(name, p)| {
            let expanded_key = crate::config::expand_env_var(&p.api_key);
            serde_json::json!({
                "name": name,
                "display_name": p.name,
                "protocol": p.protocol,
                "base_url": p.base_url,
                "has_api_key": !expanded_key.is_empty(),
                "enabled": p.enabled,
                "models": p.models,
            })
        }).collect();
        let models = self.list_models_from_snapshot(&cfg);
        let default_model = cfg.default_model.clone();
        let default_provider = cfg.default_provider.clone();
        let _ = self.event_tx.send(ServerEvent::ConfigUpdated {
            op: op.to_string(),
            providers,
            models,
            default_model,
            default_provider,
        });
    }

    /// M4: 从给定 cfg 快照构建 models JSON（避免 list_models() 再调 current_config()）
    fn list_models_from_snapshot(&self, cfg: &AppConfig) -> Vec<serde_json::Value> {
        let mut out: Vec<serde_json::Value> = cfg.models.iter().map(|(name, m)| serde_json::json!({
            "name": name,
            "display_name": m.name,
            "protocol": serde_json::to_string(&m.protocol).unwrap_or_default().trim_matches('"').to_string(),
            "context_window": m.context_window,
            "max_tokens": m.max_tokens,
            "source": "models",
        })).collect();
        for (pname, p) in &cfg.providers {
            for m in &p.models {
                let full = format!("{pname}/{m}");
                if cfg.models.contains_key(&full) { continue; }
                out.push(serde_json::json!({
                    "name": full,
                    "display_name": m,
                    "protocol": p.protocol,
                    "provider": pname,
                    "source": "provider",
                }));
            }
        }
        out.sort_by(|a, b| a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")));
        out
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
    // LSP 异步诊断：tool 执行前注入之前 write/edit 的诊断结果到 messages
    mgr.inject_pending_lsp_diagnostics(session_id).await;

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

// ==================== handoff 脱敏 ====================

/// 脱敏：替换 API key、密码等敏感信息
fn sanitize_for_handoff(s: &str) -> String {
    // 替换 sk-xxx 格式的 API key
    let re1 = regex::Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap();
    let s = re1.replace_all(s, "[REDACTED]");
    // 替换 ${ENV_VAR} 格式
    let re2 = regex::Regex::new(r"\$\{[A-Z_][A-Z0-9_]*\}").unwrap();
    let s = re2.replace_all(&s, "${REDACTED}");
    // 替换 password=xxx
    let re3 = regex::Regex::new(r"(?i)(password|passwd|pwd)\s*[:=]\s*\S+").unwrap();
    let s = re3.replace_all(&s, "$1=[REDACTED]");
    s.to_string()
}
