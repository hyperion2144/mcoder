// 设计文档 §3.4/§3.6: SessionList/PlanCreated 事件和 get_cancellation 为 forward-looking API
#![allow(dead_code)]

use crate::agent::async_tasks::TaskManager;
use crate::agent::role::RoleRegistry;
use crate::agent::AgentSession;
use crate::llm::create_adapter;
use crate::memory::MemoryStore;
use crate::persistence::jsonl::{JsonlSession, SessionMeta};
use crate::plugin::PluginManager;
use crate::tools::ToolRegistry;
use crate::types::{AppConfig, CancellationToken, ContentBlock, Message, ModelConfig, Role};
use crate::workflow::extract_spawn_subagent;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};

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
    task_manager: Arc<TaskManager>,
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
    SessionDone {
        session_id: String,
        reason: String,
    },
    Error {
        message: String,
    },
}

impl SessionManager {
    pub fn new(
        tools: Arc<ToolRegistry>,
        config: Arc<AppConfig>,
        plugins: Arc<PluginManager>,
        task_manager: Arc<TaskManager>,
        role_registry: Arc<RoleRegistry>,
        experience_store: Arc<MemoryStore>,
        mcp_manager: Arc<crate::plugin::mcp::McpManager>,
        command_dispatcher: Arc<crate::commands::CommandDispatcher>,
    ) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(1024);
        Arc::new(Self {
            sessions: RwLock::new(HashMap::new()),
            tools,
            config,
            plugins,
            task_manager,
            role_registry,
            experience_store,
            mcp_manager,
            project_resources: RwLock::new(HashMap::new()),
            command_dispatcher,
            event_tx,
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
        let project_path = {
            let agent = entry.session.lock().await;
            agent.session.project_path().to_path_buf()
        };
        let resources = self.get_or_create_resources(&project_path).await?;
        let project_dir = project_path.join(".mcoder");
        let project_hash = crate::persistence::jsonl::escape_project_path(&project_path);
        Ok(crate::tools::ToolContext {
            session_id: session_id.to_string(),
            project_path,
            project_dir,
            project_hash,
            journal: resources.journal.clone(),
            memory_store: resources.memory_store.clone(),
            experience_store: self.experience_store.clone(),
            code_graph: resources.code_graph.clone(),
            lsp_manager: resources.lsp_manager.clone(),
            debug_manager: resources.debug_manager.clone(),
            task_manager: self.task_manager.clone(),
            workflow: resources.workflow.clone(),
            cancellation: entry.cancellation.clone(),
        })
    }

    pub async fn create_session(
        self: &Arc<Self>,
        project: &Path,
        title: &str,
        model_name: Option<&str>,
    ) -> Result<String> {
        let model_config = self.resolve_model(model_name)?;
        let jsonl = JsonlSession::create(project, title, &model_config.name)?;
        let session_id = jsonl.id().to_string();

        // 获取或创建该项目的 per-project 资源
        let _resources = self.get_or_create_resources(project).await?;

        let llm = create_adapter(&model_config)?;
        let max_iters = self.config.loop_max_iters;
        let agent = AgentSession::new(jsonl, model_config, llm, self.tools.clone(), max_iters, self.role_registry.clone());

        let entry = Arc::new(SessionEntry {
            session: Mutex::new(agent),
            cancellation: CancellationToken::new(),
            client_count: std::sync::atomic::AtomicU32::new(0),
            loop_running: std::sync::atomic::AtomicBool::new(false),
        });

        self.sessions
            .write()
            .await
            .insert(session_id.clone(), entry);

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

    /// 设计文档 §3.4: 切换 session 的 role（/mode 命令）
    pub async fn set_role(&self, session_id: &str, role_name: &str) -> Result<()> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).context("session not found")?;
        let mut agent = entry.session.lock().await;
        agent.switch_role(role_name)?;
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

        // 设计文档 §3.7: loop 已结束后完成的任务结果仍需追加，下次用户消息时模型可见
        // 在添加用户消息前，先 drain 上一轮 loop 结束后完成的异步任务
        self.inject_completed_tasks(session_id, &entry).await?;

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

        let mgr = self.clone();
        let sid = session_id.to_string();
        let entry_clone = entry.clone();
        tokio::spawn(async move {
            let result = mgr.run_agent_loop(&sid).await;
            // 无论成功失败都要重置 loop_running
            entry_clone.loop_running.store(false, std::sync::atomic::Ordering::SeqCst);
            if let Err(e) = result {
                let _ = mgr.event_tx.send(ServerEvent::Error {
                    message: format!("agent loop error: {}", e),
                });
            }
        });

        Ok(())
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

    /// 将已完成的异步任务结果作为 system 消息注入 session
    /// 设计文档 §3.5: 每轮 LLM 调用前注入已完成的后台任务结果
    async fn inject_completed_tasks(
        &self,
        session_id: &str,
        entry: &Arc<SessionEntry>,
    ) -> Result<()> {
        let completed = self.task_manager.drain_completed().await;
        if completed.is_empty() {
            return Ok(());
        }
        let mut agent = entry.session.lock().await;
        for t in completed {
            let text = format!(
                "[async task completed] id={} name={} status={:?}{}{}",
                t.id,
                t.name,
                t.status,
                t.result.map(|r| format!("\nresult: {}", r)).unwrap_or_default(),
                t.error.map(|e| format!("\nerror: {}", e)).unwrap_or_default(),
            );
            let msg = Message::system(text);
            agent.add_message(msg.clone())?;
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
                let _ = self.event_tx.send(ServerEvent::SessionDone {
                    session_id: session_id.to_string(),
                    reason: "cancelled".to_string(),
                });
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
                agent.inject_role_context().await?;
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
                let _ = self.event_tx.send(ServerEvent::SessionDone {
                    session_id: session_id.to_string(),
                    reason: "hook_blocked".to_string(),
                });
                break_reason = Some("hook_blocked".to_string());
                break;
            }

            // 设计文档 §3.9: LLM 调用可被取消（select between LLM 和 cancellation）
            let assistant_msg = {
                let mut agent = entry.session.lock().await;
                tokio::select! {
                    r = agent.run_once() => r?,
                    _ = cancel_token.cancelled() => {
                        tracing::info!("LLM call cancelled at iter {}", iter);
                        let _ = agent.add_message(Message::system("[LLM call cancelled by user]"));
                        let _ = self.event_tx.send(ServerEvent::SessionDone {
                            session_id: session_id.to_string(),
                            reason: "cancelled".to_string(),
                        });
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
                        if self.task_manager.has_running().await {
                            tracing::info!("no tool calls but background tasks still running, waiting...");
                            // 等待时也响应取消信号
                            tokio::select! {
                                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
                                _ = cancel_token.cancelled() => {
                                    tracing::info!("cancelled while waiting for background tasks");
                                    let _ = self.event_tx.send(ServerEvent::SessionDone {
                                        session_id: session_id.to_string(),
                                        reason: "cancelled".to_string(),
                                    });
                                    return Ok(());
                                }
                            }
                            continue;
                        }
                        let _ = self.event_tx.send(ServerEvent::SessionDone {
                            session_id: session_id.to_string(),
                            reason: "completed".to_string(),
                        });
                        break_reason = Some("completed".to_string());
                        break;
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
                            let _ = self.event_tx.send(ServerEvent::SessionDone {
                                session_id: session_id.to_string(),
                                reason: "cancelled".to_string(),
                            });
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
                            let block_msg = Message {
                                role: Role::Tool,
                                content: vec![ContentBlock::ToolResult {
                                    id: tc.id.clone(),
                                    output: crate::types::ToolOutput::Error {
                                        message: format!("tool '{}' is not allowed in current role", tc.name),
                                    },
                                }],
                            };
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
                                let block_msg = Message {
                                    role: Role::Tool,
                                    content: vec![ContentBlock::ToolResult {
                                        id: tc.id.clone(),
                                        output: crate::types::ToolOutput::Error {
                                            message: format!(
                                                "tool '{}' is a dangerous operation (browser/screen/app control) and requires user confirmation. Add it to [tools] auto_approve in config to allow automatically.",
                                                tc.name
                                            ),
                                        },
                                    }],
                                };
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
                            // 设计文档 §3.9: 工具执行可被取消
                            tokio::select! {
                                r = agent.execute_tool(tc, &ctx) => r,
                                _ = cancel_token.cancelled() => {
                                    tracing::info!("tool {} cancelled during execution", tc.name);
                                    Ok(Message {
                                        role: Role::Tool,
                                        content: vec![ContentBlock::ToolResult {
                                            id: tc.id.clone(),
                                            output: crate::types::ToolOutput::Error {
                                                message: format!("tool '{}' cancelled by user", tc.name),
                                            },
                                        }],
                                    })
                                }
                            }
                        };

                        let success = result.is_ok();
                        if let Ok(result_msg) = &result {
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

                            // 自动调度子代理：workflow_update phase_next 返回 spawn_subagent 时
                            // 自动构造 subagent 工具调用并执行，无需 LLM 主动调用
                            if tc.name == "workflow_update" {
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
                                    if let Ok(sub_msg) = sub_result {
                                        let _ = self.event_tx.send(ServerEvent::Message {
                                            session_id: session_id.to_string(),
                                            message: sub_msg,
                                        });
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
                            let _ = self.event_tx.send(ServerEvent::SessionDone {
                                session_id: session_id.to_string(),
                                reason: "loop_condition_met".to_string(),
                            });
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
                    let _ = self.event_tx.send(ServerEvent::SessionDone {
                        session_id: session_id.to_string(),
                        reason: "empty_response".to_string(),
                    });
                    break_reason = Some("empty_response".to_string());
                    break;
                }
            }
        }

        // 设计文档 §3.5: 循环自然结束（达到 max_iters）才发送 done
        // 若是 break 退出（completed/hook_blocked/empty_response）则已在循环内发送过，不重复发送
        if break_reason.is_none() {
            let _ = self.event_tx.send(ServerEvent::SessionDone {
                session_id: session_id.to_string(),
                reason: "max_iters_reached".to_string(),
            });
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

    // ==================== 新增 RPC 方法实现 ====================

    /// 设计文档 §5.4 / §5.5 / §5.6: session.attach
    /// - offset=None: 返回全部历史消息（首次 attach）
    /// - offset=Some(n): 仅返回第 n 条之后的消息（断线重连补推）
    pub async fn attach_session_with_offset(
        self: &Arc<Self>,
        session_id: &str,
        offset: Option<usize>,
    ) -> Result<Vec<Message>> {
        // 先检查内存中是否存在
        let in_memory = self.sessions.read().await.contains_key(session_id);
        if !in_memory {
            // 设计文档 §5.5: 新 client attach 时先读 jsonl 重放历史
            self.load_session_from_jsonl(session_id).await?;
        }
        let all = self.get_messages(session_id).await?;
        match offset {
            Some(n) if n > 0 => Ok(all.into_iter().skip(n).collect()),
            _ => Ok(all),
        }
    }

    /// 兼容旧接口：不带 offset 的 attach
    pub async fn attach_session(self: &Arc<Self>, session_id: &str) -> Result<Vec<Message>> {
        self.attach_session_with_offset(session_id, None).await
    }

    /// 设计文档 §5.5: 从 jsonl 重放加载 session 到内存
    /// 用于 server 重启后 client attach 场景
    async fn load_session_from_jsonl(self: &Arc<Self>, session_id: &str) -> Result<()> {
        let jsonl = JsonlSession::load(session_id)
            .context("loading session from jsonl")?;
        let meta = jsonl.meta().clone();
        // 获取或创建该 session 所属项目的 per-project 资源
        self.get_or_create_resources(&meta.project_path).await?;
        let model_config = self.resolve_model(Some(&meta.model))?;
        let llm = create_adapter(&model_config)?;
        let max_iters = self.config.loop_max_iters;
        let agent = AgentSession::new(jsonl, model_config, llm, self.tools.clone(), max_iters, self.role_registry.clone());

        let entry = Arc::new(SessionEntry {
            session: Mutex::new(agent),
            cancellation: CancellationToken::new(),
            client_count: std::sync::atomic::AtomicU32::new(0),
            loop_running: std::sync::atomic::AtomicBool::new(false),
        });

        self.sessions
            .write()
            .await
            .insert(session_id.to_string(), entry);
        tracing::info!("session {} reloaded from jsonl", session_id);
        Ok(())
    }

    /// session.close - 关闭会话（从内存移除，jsonl 保留）
    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        self.sessions.write().await.remove(session_id)
            .map(|_| ()).ok_or_else(|| anyhow::anyhow!("session not found"))?;
        // 设计文档 §8.3.3: 触发 OnSessionEnd hook
        let _ = self.plugins.run_hooks(
            crate::plugin::HookPoint::OnSessionEnd,
            crate::plugin::HookContext::new(crate::plugin::HookPoint::OnSessionEnd, session_id),
        ).await;
        Ok(())
    }

    /// session.delete - 删除会话（内存 + jsonl 文件）
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        self.sessions.write().await.remove(session_id);
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
    /// edit → 用 edited_plan 替换 plan.json，然后 approve
    pub async fn approve_plan(
        self: &Arc<Self>,
        session_id: &str,
        action: &str,
        edited_plan: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        // 从 session 获取 project_path，再定位 plans_dir
        let project_path = {
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id).context("session not found")?;
            let agent = entry.session.lock().await;
            agent.session.project_path().to_path_buf()
        };
        let plans_dir = crate::config::project_config_dir(&project_path).join("plans");
        let plan_path = plans_dir.join("plan.json");

        match action {
            "approve" => {
                self.set_role(session_id, "execute").await?;
                Ok(serde_json::json!({"action": "approved", "next_role": "execute"}))
            }
            "reject" => {
                self.set_role(session_id, "default").await?;
                // 通知 agent plan 被拒绝
                let entry = self.sessions.read().await.get(session_id).cloned()
                    .context("session not found")?;
                let mut agent = entry.session.lock().await;
                let _ = agent.add_message(Message::system("[plan rejected by user]"));
                Ok(serde_json::json!({"action": "rejected", "next_role": "default"}))
            }
            "edit" => {
                if let Some(plan) = edited_plan {
                    tokio::fs::write(&plan_path, serde_json::to_string_pretty(&plan)?).await?;
                }
                self.set_role(session_id, "execute").await?;
                Ok(serde_json::json!({"action": "edited_and_approved", "next_role": "execute"}))
            }
            other => anyhow::bail!("unknown approve action: {} (use approve|reject|edit)", other),
        }
    }

    /// task.list - 列出所有异步任务
    pub async fn list_tasks(&self) -> Vec<serde_json::Value> {
        self.task_manager.list().await.into_iter().map(|t| serde_json::json!({
            "id": t.id,
            "name": t.name,
            "status": format!("{:?}", t.status),
        })).collect()
    }

    /// task.cancel - 取消异步任务
    pub async fn cancel_task(&self, task_id: &str) -> Result<()> {
        if self.task_manager.cancel(task_id).await {
            Ok(())
        } else {
            anyhow::bail!("task not found or already completed: {}", task_id)
        }
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
        let task_count = self.task_manager.list().await.len();
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
const READONLY_TOOLS: &[&str] = &[
    "read", "read_more", "read_full", "read_original",
    "ls", "grep",
    "graph_query", "graph_file_symbols", "graph_index",
    // P2-8: 新增 graph 只读查询工具（graph_index 也算只读，因不修改源文件）
    "graph_find", "graph_callers", "graph_callees", "graph_references",
    "memory_search", "memory_list",
    "sandbox_read",
    "task_status", "task_list",
    "plan_query", "plan_get",
    "workflow_query",
    "subagent_status", "subagent_result", "subagent_list",
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
                Message {
                    role: Role::Tool,
                    content: vec![ContentBlock::ToolResult {
                        id,
                        output: crate::types::ToolOutput::Error {
                            message: format!("tool '{}' is not allowed in current role", name),
                        },
                    }],
                }
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
            Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    id: tc_clone.id.clone(),
                    output,
                }],
            }
        });
        handles.push((tc.clone(), handle));
    }

    // 并发等待所有工具完成
    let mut results = Vec::new();
    for (tc, handle) in handles {
        let msg = match handle.await {
            Ok(m) => m,
            Err(e) => Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    id: tc.id.clone(),
                    output: crate::types::ToolOutput::Error {
                        message: format!("tool panicked: {}", e),
                    },
                }],
            },
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
