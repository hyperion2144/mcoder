// 设计文档 §4.1: ToolRegistry::get 为 forward-looking API（当前通过 execute 调用）
#![allow(dead_code)]

pub mod ast_edit;
pub mod bash;
pub mod code_exec;
pub mod file;
pub mod journal;
pub mod plan;
pub mod sandbox;
pub mod subagent;
pub mod task;
pub mod undo;
pub mod workflow;

// SkillUseTool / SkillListTool：让 LLM 能调用 skill
// skill 引擎定义在 crate::skills，这里只做工具适配
pub struct SkillUseTool {
    pub registry: std::sync::Arc<crate::skills::SkillRegistry>,
}
pub struct SkillListTool {
    pub registry: std::sync::Arc<crate::skills::SkillRegistry>,
}

use crate::types::{ToolCall, ToolOutput, ToolSchema};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// 设计文档 §4.2: 工具运行时上下文
/// 每次工具执行时由 SessionManager 构造，包含 per-session 和 per-project 的资源
/// 工具不再持有 project_dir 等状态，统一从 ctx 获取
#[derive(Clone)]
pub struct ToolContext {
    /// 当前会话 ID
    pub session_id: String,
    /// 当前 LLM ToolCall.id（由 session_manager 注入）
    /// 工具如 ask_user 需要把它转发给客户端，保证 tool_use.id ↔ ask tool_call_id 一致
    /// None 表示非 LLM 驱动的直接调用（如 tool.call RPC、test hook）
    pub tool_call_id: Option<String>,
    /// 会话的工作目录（项目根路径）
    pub project_path: std::path::PathBuf,
    /// 项目配置目录 = project_path/.mcoder
    pub project_dir: std::path::PathBuf,
    /// 项目 hash（用于记忆隔离）
    pub project_hash: String,
    /// 文件变更日志
    pub journal: Arc<journal::FileJournal>,
    /// 项目记忆存储
    pub memory_store: Arc<crate::memory::MemoryStore>,
    /// 全局经验库
    pub experience_store: Arc<crate::memory::MemoryStore>,
    /// 代码图谱
    pub code_graph: Arc<crate::code_graph::CodeGraph>,
    /// LSP 管理器
    pub lsp_manager: Arc<crate::lsp::LspManager>,
    /// DAP 调试管理器
    pub debug_manager: Arc<crate::debug::DebugManager>,
    /// 异步任务管理器
    pub task_manager: Arc<crate::agent::async_tasks::TaskManager>,
    /// 工作流存储
    pub workflow: Arc<crate::workflow::WorkflowStore>,
    /// 会话状态存储（todo 等 per-session state，绑 session_id，模型不可跨 session）
    pub session_state: Arc<crate::persistence::session_state::SessionStateStore>,
    /// 服务端事件总线（用于工具广播 TodoUpdated 等）
    pub event_tx: tokio::sync::broadcast::Sender<crate::session_manager::ServerEvent>,
    /// 取消令牌
    pub cancellation: crate::types::CancellationToken,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn schema(&self) -> ToolSchema;
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput>;
}

pub type SharedTool = Arc<dyn Tool>;

pub struct ToolRegistry {
    tools: HashMap<String, SharedTool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: HashMap::new() }
    }

    pub fn register(&mut self, tool: SharedTool) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// 批量注册工具
    pub fn register_all(&mut self, tools: Vec<SharedTool>) {
        for t in tools {
            self.register(t);
        }
    }

    pub fn get(&self, name: &str) -> Option<SharedTool> {
        self.tools.get(name).cloned()
    }

    pub fn list_schemas(&self) -> Vec<ToolSchema> {
        self.tools.values().map(|t| t.schema()).collect()
    }

    pub async fn execute(&self, call: &ToolCall, ctx: &ToolContext) -> Result<ToolOutput> {
        let tool = self.tools.get(&call.name)
            .ok_or_else(|| anyhow::anyhow!("tool not found: {}", call.name))?;
        // 透传 call.id：让 ctx.tool_call_id 反映真实 LLM ToolCall.id
        let mut ctx_with_id = ctx.clone();
        if ctx_with_id.tool_call_id.is_none() {
            ctx_with_id.tool_call_id = Some(call.id.clone());
        }
        tool.execute(call.args.clone(), &ctx_with_id).await
    }
}

/// 构建无状态工具集（所有依赖通过 ToolContext 注入，不再在构造时绑定 project）
/// 返回 (registry, subagent_tool, ask_user_tool, ask_registry) - subagent_tool 需在 registry 构建后调用 set_dependencies
/// ask_user_tool 需在 SessionManager 创建后调用 set_event_tx
/// ask_registry 由 SessionManager 持有，用于 RPC 端 ask.pending/ask.answer/ask.cancel
/// 设计文档 §8.5: SubagentTool 使用 late binding 解决循环依赖
pub fn build_full_registry() -> (
    ToolRegistry,
    Arc<subagent::SubagentTool>,
    Arc<crate::ask_user::AskUserTool>,
    Arc<crate::ask_user::AskRegistry>,
) {
    let mut reg = ToolRegistry::new();

    // ===== 文件工具族（无状态，从 ctx 取 project_dir/journal）=====
    reg.register(Arc::new(file::ReadTool));
    reg.register(Arc::new(file::ReadMoreTool));
    reg.register(Arc::new(file::ReadFullTool));
    reg.register(Arc::new(file::ReadOriginalTool));
    reg.register(Arc::new(file::WriteTool));
    reg.register(Arc::new(file::EditTool));
    reg.register(Arc::new(file::LsTool));
    reg.register(Arc::new(file::GrepTool));

    // ===== Bash 工具族（无状态，从 ctx 取）=====
    reg.register(Arc::new(bash::BashTool));
    reg.register(Arc::new(bash::BashBatchTool));

    // ===== 代码图谱工具（无状态，从 ctx 取 graph）=====
    reg.register(Arc::new(crate::code_graph::tools::GraphQueryTool));
    reg.register(Arc::new(crate::code_graph::tools::GraphFileSymbolsTool));
    reg.register(Arc::new(crate::code_graph::tools::GraphIndexTool));
    reg.register(Arc::new(crate::code_graph::tools::GraphFindTool));
    reg.register(Arc::new(crate::code_graph::tools::GraphCallersTool));
    reg.register(Arc::new(crate::code_graph::tools::GraphCalleesTool));
    reg.register(Arc::new(crate::code_graph::tools::GraphReferencesTool));

    // ===== 记忆工具（无状态，从 ctx 取 store/project_hash）=====
    reg.register(Arc::new(crate::memory::tools::MemoryStoreTool));
    reg.register(Arc::new(crate::memory::tools::MemorySearchTool));
    reg.register(Arc::new(crate::memory::tools::MemoryListTool));

    // ===== AST 工具集（无状态，从 ctx 取 graph/journal/lsp）=====
    reg.register(Arc::new(ast_edit::AstRenameTool));
    reg.register(Arc::new(ast_edit::AstExtractTool));
    reg.register(Arc::new(ast_edit::AstInlineTool));

    // ===== Plan / Todo（无状态，从 ctx 取 project_dir）=====
    reg.register(Arc::new(plan::PlanCreateTool));
    reg.register(Arc::new(plan::PlanUpdateTool));
    reg.register(Arc::new(plan::PlanQueryTool));
    reg.register(Arc::new(plan::TodoTool));

    // ===== 代码执行（无状态，从 ctx 取）=====
    reg.register(Arc::new(code_exec::CodeExecTool));

    // ===== Sandbox 续读（无状态，从 ctx 取）=====
    reg.register(Arc::new(sandbox::SandboxReadTool));

    // ===== Task 管理（无状态，从 ctx 取 task_manager）=====
    reg.register(Arc::new(task::TaskTool));

    // ===== Undo（无状态，从 ctx 取 journal）=====
    reg.register(Arc::new(undo::UndoTool));

    // ===== Subagent（late binding）=====
    // 注意: SubagentTool 仍需 task_manager，通过 ctx 获取
    let subagent_tool = Arc::new(subagent::SubagentTool::new());
    reg.register(subagent_tool.clone());

    // ===== Workflow 工具集（无状态，从 ctx 取 store）=====
    reg.register(Arc::new(workflow::WorkflowCreateTool));
    reg.register(Arc::new(workflow::WorkflowQueryTool));
    reg.register(Arc::new(workflow::WorkflowUpdateTool));

    // ===== DAP 调试工具集（无状态，从 ctx 取）=====
    reg.register_all(crate::debug::tools::build_debug_tools());

    // ===== LSP 工具集（无状态，从 ctx 取）=====
    reg.register_all(crate::lsp::tools::build_lsp_tools());

    // ===== 浏览器工具集（设计文档 §8.7 M5，无状态）=====
    let browser_manager = crate::browser::BrowserManager::new();
    reg.register_all(crate::browser::tools::build_browser_tools(browser_manager));

    // ===== Computer Use 工具集（设计文档 §8.7 M5，无状态）=====
    reg.register_all(crate::computer_use::build_computer_use_tools());

    // ===== AskUser 工具（late binding：event_tx 在 SessionManager 创建后注入）=====
    let ask_registry = Arc::new(crate::ask_user::AskRegistry::with_store_resolver(
        |session_id: String| async move {
            crate::persistence::session_state::SessionStateStore::for_session(&session_id)
                .await
                .map(Arc::new)
                .ok_or_else(|| anyhow::anyhow!("cannot open session_state for {}", session_id))
        },
    ));
    let ask_user_tool = Arc::new(crate::ask_user::AskUserTool::new(ask_registry.clone()));
    reg.register(ask_user_tool.clone() as SharedTool);

    (reg, subagent_tool, ask_user_tool, ask_registry)
}

// ==================== Skill 工具实现 ====================

#[async_trait]
impl Tool for SkillUseTool {
    fn name(&self) -> &str {
        "skill_use"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "skill_use".into(),
            description: "Activate a skill by name. Returns the expanded prompt to guide subsequent actions. Use when the user's request matches a skill's description, or after the user invokes /skill-name.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Skill name (e.g. 'tdd', 'commit', 'review')"
                    },
                    "args": {
                        "type": "string",
                        "description": "Arguments to substitute into the skill template ($ARGUMENTS)"
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let name = args.get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'name' field"))?;
        let skill_args = args.get("args")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let prompt = self.registry.activate(name, skill_args).await
            .map_err(|e| anyhow::anyhow!("skill activation failed: {}", e))?;

        Ok(ToolOutput::Sync {
            result: serde_json::json!({
                "skill": name,
                "prompt": prompt,
            }),
        })
    }
}

#[async_trait]
impl Tool for SkillListTool {
    fn name(&self) -> &str {
        "skill_list"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "skill_list".into(),
            description: "List all available skills with their descriptions.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn execute(&self, _args: serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let skills = self.registry.list().await;
        let list: Vec<serde_json::Value> = skills.iter().map(|s| serde_json::json!({
            "name": s.name,
            "description": s.description,
            "user_invocable": s.user_invocable,
            "disable_model_invocation": s.disable_model_invocation,
        })).collect();
        Ok(ToolOutput::Sync { result: serde_json::Value::Array(list) })
    }
}
