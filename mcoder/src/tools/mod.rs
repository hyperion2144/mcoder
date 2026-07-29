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
        tool.execute(call.args.clone(), ctx).await
    }
}

/// 构建无状态工具集（所有依赖通过 ToolContext 注入，不再在构造时绑定 project）
/// 返回 (registry, subagent_tool) - subagent_tool 需在 registry 构建后调用 set_dependencies
/// 设计文档 §8.5: SubagentTool 使用 late binding 解决循环依赖
pub fn build_full_registry() -> (ToolRegistry, Arc<subagent::SubagentTool>) {
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

    (reg, subagent_tool)
}
