pub mod async_tasks;
pub mod role;

use crate::llm::{LLMResponse, SharedLLM};
use crate::persistence::jsonl::JsonlSession;
use crate::tools::ToolRegistry;
use crate::types::{CompactConfig, ContentBlock, Message, ModelConfig, Role, ToolCall, ToolOutput};
use anyhow::{Context, Result};
use std::sync::Arc;

/// 压缩单条消息中的大 ToolResult
/// 设计文档 §3.5: tool_results 策略
///   - "summarize": 截断为前 200 + 后 100 字符
///   - "drop": 替换为占位文本
///   - "keep" / 其他: 原样保留
fn compact_one(msg: &Message, cfg: &CompactConfig) -> Message {
    let strategy = cfg.tool_results.as_str();
    let mut new_content: Vec<ContentBlock> = Vec::with_capacity(msg.content.len());
    for b in &msg.content {
        match b {
            ContentBlock::ToolResult { id, output } => {
                let output_str = serde_json::to_string(output).unwrap_or_default();
                if output_str.chars().count() <= 800 {
                    new_content.push(b.clone());
                    continue;
                }
                match strategy {
                    "summarize" => {
                        // 统一按字符计算，避免 UTF-8 多字节截断错位
                        let chars: Vec<char> = output_str.chars().collect();
                        let total_chars = chars.len();
                        let head: String = chars.iter().take(200).collect();
                        let tail_start = total_chars.saturating_sub(100);
                        let tail: String = chars[tail_start..].iter().collect();
                        let truncated_chars = total_chars.saturating_sub(300);
                        let summarized = format!(
                            "{}...[truncated {} chars during compaction]...{}",
                            head, truncated_chars, tail
                        );
                        new_content.push(ContentBlock::ToolResult {
                            id: id.clone(),
                            output: ToolOutput::Sync {
                                result: serde_json::Value::String(summarized),
                            },
                        });
                    }
                    "drop" => {
                        new_content.push(ContentBlock::ToolResult {
                            id: id.clone(),
                            output: ToolOutput::Sync {
                                result: serde_json::Value::String(
                                    "[tool result dropped during context compaction]".into()
                                ),
                            },
                        });
                    }
                    _ => {
                        new_content.push(b.clone());
                    }
                }
            }
            _ => {
                new_content.push(b.clone());
            }
        }
    }
    Message { role: msg.role, content: new_content }
}

pub struct AgentSession {
    pub session: JsonlSession,
    pub messages: Vec<Message>,
    pub model_config: ModelConfig,
    pub llm: SharedLLM,
    pub tools: Arc<ToolRegistry>,
    pub max_iters: u32,
    /// 设计文档 §3.4: 当前 role（默认 "default"）
    pub current_role: String,
    /// role 注册表引用（用于切换 role 和获取 system prompt）
    pub role_registry: Arc<role::RoleRegistry>,
}

impl AgentSession {
    pub fn new(
        session: JsonlSession,
        model_config: ModelConfig,
        llm: SharedLLM,
        tools: Arc<ToolRegistry>,
        max_iters: u32,
        role_registry: Arc<role::RoleRegistry>,
    ) -> Self {
        let messages = session.read_all().unwrap_or_default();
        Self {
            session,
            messages,
            model_config,
            llm,
            tools,
            max_iters,
            current_role: "default".into(),
            role_registry,
        }
    }

    pub fn add_message(&mut self, msg: Message) -> Result<()> {
        self.session.append(&msg)?;
        self.messages.push(msg);
        Ok(())
    }

    /// 只读访问 model_config（用于 compact 阈值计算）
    pub fn model_config(&self) -> &ModelConfig {
        &self.model_config
    }

    /// 切换 role（设计文档 §3.4: /mode plan, /mode goal 等）
    pub fn switch_role(&mut self, role_name: &str) -> Result<()> {
        if self.role_registry.get(role_name).is_none() {
            anyhow::bail!("unknown role: {} (available: {:?})", role_name, role::builtin_role_names());
        }
        self.current_role = role_name.into();
        // 重新注入 system prompt
        self.refresh_system_prompt();
        Ok(())
    }

    /// 确保会话以 system prompt 开头
    /// 根据 current_role 选择对应的 system prompt
    pub fn ensure_system_prompt(&mut self) {
        let has_system = self.messages.first()
            .map(|m| m.role == Role::System)
            .unwrap_or(false);
        if !has_system {
            self.refresh_system_prompt();
        }
    }

    /// 刷新 system prompt（基于当前 role）
    fn refresh_system_prompt(&mut self) {
        let role_prompt = self.role_registry.get(&self.current_role)
            .map(|r| r.system_prompt.clone())
            .unwrap_or_default();
        let prompt = if role_prompt.is_empty() {
            default_system_prompt()
        } else {
            format!("{}\n\n{}", role_prompt, default_system_prompt())
        };

        // 移除旧的 system prompt（如果第一条是 system）
        if self.messages.first().map(|m| m.role == Role::System).unwrap_or(false) {
            self.messages.remove(0);
        }
        let msg = Message::system(prompt);
        if let Ok(()) = self.session.append(&msg) {
            self.messages.insert(0, msg);
        }
    }

    /// 设计文档 §3.5: 注入 role 特定上下文
    /// 例如 plan role 注入当前 plan 状态，execute role 注入 todo 状态
    /// Phase 4: plan 改用 SessionStateStore pending_plan（per-session），不再读项目级 plan.json
    pub async fn inject_role_context(
        &mut self,
        session_state: &crate::persistence::session_state::SessionStateStore,
    ) -> Result<()> {
        let session_id = self.session.id().to_string();
        match self.current_role.as_str() {
            "plan" | "execute" => {
                // 注入当前 plan 状态（per-session SQLite）
                if let Some(rec) = session_state.get_pending_plan(&session_id).await {
                    let body = serde_json::to_string_pretty(&rec.content).unwrap_or_default();
                    let state_label = format!("{:?}", rec.state);
                    let msg = Message::system(format!("[current plan: {}]\n{}", state_label, body));
                    self.add_message(msg)?;
                }
            }
            "goal" | "loop" => {
                // 注入 todo 状态（per-session，来源 SessionStateStore / SQLite）
                // 已废弃旧版 .mcoder/plans/todo.json 文件读取（不兼容旧数据）
                let items = session_state.list_todos(&session_id).await.unwrap_or_default();
                if !items.is_empty() {
                    let summary = crate::persistence::session_state::TodoSummary::from_items(&items);
                    let body = serde_json::to_string_pretty(&items).unwrap_or_default();
                    let header = format!(
                        "[current todos] {} total · {} pending · {} in_progress · {} completed · {} cancelled",
                        summary.total, summary.pending, summary.in_progress, summary.completed, summary.cancelled,
                    );
                    let msg = Message::system(format!("{}\n{}", header, body));
                    self.add_message(msg)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 设计文档 §3.4: 检查当前 role 是否允许使用某工具
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        if let Some(r) = self.role_registry.get(&self.current_role) {
            r.is_tool_allowed(tool_name)
        } else {
            true
        }
    }

    /// 设计文档 §3.5: 计算当前 role 的 max_iters
    /// - role.max_iters = Some(n) → 用 n（0 视为无限，用一个大的兜底值）
    /// - role.max_iters = None → 用 self.max_iters（config 的 loop_max_iters）
    pub fn max_iters_for_current_role(&self) -> u32 {
        if let Some(r) = self.role_registry.get(&self.current_role) {
            match r.max_iters {
                Some(0) => u32::MAX, // 0 = 无限（loop role），用 u32::MAX 兜底
                Some(n) => n,
                None => self.max_iters,
            }
        } else {
            self.max_iters
        }
    }

    /// 设计文档 §3.5: 检查 loop 退出条件（公开给 session_manager 调用）
    pub async fn check_loop_condition(&self) -> bool {
        let role = match self.role_registry.get(&self.current_role) {
            Some(r) => r,
            None => return false,
        };
        // Phase 4: plan 来源改为 per-session SQLite pending_plan（不再读项目级 plan.json）
        match role.loop_condition.as_deref() {
            Some("plan_created") => {
                // plan role: 检查 pending_plan（DB）是否存在
                if let Some(store) = crate::persistence::session_state::SessionStateStore::for_session(self.session.id()).await {
                    if store.get_pending_plan(self.session.id()).await.is_some() {
                        return true;
                    }
                }
                false
            }
            Some("plan_all_done") => {
                // execute role: 检查所有 plan steps 是否 done
                if let Some(store) = crate::persistence::session_state::SessionStateStore::for_session(self.session.id()).await {
                    if let Some(rec) = store.get_pending_plan(self.session.id()).await {
                        if let Ok(plan) = serde_json::from_value::<serde_json::Value>(rec.content) {
                            if let Some(steps) = plan["steps"].as_array() {
                                return steps.iter().all(|s| s["status"] == "done" || s["status"] == "skipped");
                            }
                        }
                    }
                }
                false
            }
            Some("goal_achieved") | Some("task_complete") => {
                // 这些由用户或 LLM 主动结束，不在自动检查中
                false
            }
            _ => false,
        }
    }

    /// 设计文档 §3.5: 上下文压缩
    /// 策略：
    ///   1. 估算当前消息总 token 数（每 4 字符 ≈ 1 token）
    ///   2. 超过 context_window * threshold 时触发压缩
    ///   3. 保留 system prompt（开头 keep_first 条）+ 最近 keep_recent 条
    ///   4. 中间消息：按 cfg.tool_results 策略处理大的 ToolResult
    ///      - "summarize": 截断为前 200 + 后 100 字符的摘要
    ///      - "drop": 丢弃大的 ToolResult（保留 ToolUse 以维持对话连贯）
    ///      - "keep": 不压缩（默认）
    pub fn maybe_compact(&mut self, cfg: &CompactConfig) {
        if cfg.strategy == "off" || cfg.strategy == "none" {
            return;
        }

        let total = self.messages.len();
        if total < 10 {
            return;
        }

        // 估算当前 token 数
        let est_tokens: usize = self.messages.iter()
            .map(|m| Self::estimate_tokens(m))
            .sum();
        let token_threshold = (self.model_config.context_window as f32 * cfg.threshold) as usize;
        if est_tokens < token_threshold {
            return;
        }

        let keep_first = cfg.keep_first.max(1) as usize;
        let keep_recent = cfg.keep_recent as usize;
        if total <= keep_first + keep_recent {
            return;
        }

        // 策略1: 渐进式压缩 - 先压缩中间部分的大 ToolResult
        let mut new_messages: Vec<Message> = Vec::with_capacity(total);
        new_messages.extend(self.messages[..keep_first].iter().cloned());

        let middle_end = total - keep_recent;
        let mut compacted_count = 0usize;
        for msg in &self.messages[keep_first..middle_end] {
            let compacted = Self::compact_message(msg, cfg);
            if compacted {
                compacted_count += 1;
            }
            new_messages.push(compact_one(msg, cfg));
        }

        new_messages.extend(self.messages[middle_end..].iter().cloned());

        let new_tokens: usize = new_messages.iter()
            .map(|m| Self::estimate_tokens(m))
            .sum();
        tracing::info!(
            "context compacted: {} messages ({}→{} tokens, {} tool results compacted)",
            total, est_tokens, new_tokens, compacted_count
        );
        self.messages = new_messages;

        // 策略2: 如果压缩后仍超阈值，用摘要替代整个中间段
        let new_tokens_after = self.estimate_total_tokens();
        if new_tokens_after > token_threshold {
            self.aggressive_compact(cfg);
        }
    }

    /// 估算单条消息的 token 数（每 4 字符 ≈ 1 token）
    fn estimate_tokens(msg: &Message) -> usize {
        let chars: usize = msg.content.iter()
            .map(|b| match b {
                ContentBlock::Text { text } => text.len(),
                ContentBlock::ToolUse { name, args, .. } => {
                    name.len() + args.to_string().len()
                }
                ContentBlock::ToolResult { output, .. } => {
                    serde_json::to_string(output).map(|s| s.len()).unwrap_or(0)
                }
            })
            .sum();
        chars / 4 + 1 // 至少 1 token
    }

    /// 判断消息是否需要压缩（包含大的 ToolResult）
    fn compact_message(msg: &Message, _cfg: &CompactConfig) -> bool {
        msg.content.iter().any(|b| match b {
            ContentBlock::ToolResult { output, .. } => {
                serde_json::to_string(output).map(|s| s.len() > 800).unwrap_or(false)
            }
            _ => false,
        })
    }

    pub fn estimate_total_tokens(&self) -> usize {
        self.messages.iter().map(Self::estimate_tokens).sum()
    }

    /// 激进压缩：用一条摘要替代整个中间段
    fn aggressive_compact(&mut self, cfg: &CompactConfig) {
        let total = self.messages.len();
        let keep_first = cfg.keep_first.max(1) as usize;
        let keep_recent = cfg.keep_recent as usize;
        if total <= keep_first + keep_recent + 1 {
            return;
        }

        let middle_count = total - keep_first - keep_recent;
        let mut new_messages: Vec<Message> = Vec::with_capacity(keep_first + keep_recent + 1);
        new_messages.extend(self.messages[..keep_first].iter().cloned());

        let summary = Message::system(format!(
            "[context compacted: {} earlier messages summarized. \
             Tool results were replaced to save tokens.]",
            middle_count
        ));
        new_messages.push(summary);

        new_messages.extend(self.messages[total - keep_recent..].iter().cloned());

        let before_tokens = self.estimate_total_tokens();
        self.messages = new_messages;
        let after_tokens = self.estimate_total_tokens();
        tracing::info!(
            "aggressive compact: {} messages ({}→{} tokens)",
            total, before_tokens, after_tokens
        );
    }

    pub async fn run_once(&mut self) -> Result<Option<Message>> {
        // 设计文档 §3.5: 根据 role 的 allowed_tools 过滤工具列表
        let all_schemas = self.tools.list_schemas();
        let schemas: Vec<_> = if let Some(role) = self.role_registry.get(&self.current_role) {
            if role.allowed_tools.is_empty() {
                all_schemas
            } else {
                all_schemas.into_iter()
                    .filter(|s| role.allowed_tools.contains(&s.name))
                    .collect()
            }
        } else {
            all_schemas
        };

        let resp = self.llm.chat(&self.messages, &schemas, &self.model_config)
            .await
            .context("LLM call failed")?;

        self.process_response(resp)
    }

    fn process_response(&mut self, resp: LLMResponse) -> Result<Option<Message>> {
        let LLMResponse { content, tool_calls, .. } = resp;

        let mut blocks: Vec<ContentBlock> = Vec::new();
        if let Some(text) = &content {
            if !text.is_empty() {
                blocks.push(ContentBlock::Text { text: text.clone() });
            }
        }
        for tc in &tool_calls {
            blocks.push(ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                args: tc.args.clone(),
            });
        }

        if blocks.is_empty() {
            return Ok(None);
        }

        let msg = Message { role: Role::Assistant, content: blocks };
        self.add_message(msg.clone())?;
        Ok(Some(msg))
    }

    pub async fn execute_tool(&mut self, call: &ToolCall, ctx: &crate::tools::ToolContext) -> Result<Message> {
        let output = match self.tools.execute(call, ctx).await {
            Ok(out) => out,
            Err(e) => ToolOutput::Error { message: e.to_string() },
        };

        let msg = Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                id: call.id.clone(),
                output,
            }],
        };
        self.add_message(msg.clone())?;
        Ok(msg)
    }
}

/// 默认 system prompt：告诉模型它是 mcoder，工具调用规则等
fn default_system_prompt() -> String {
    r#"You are mcoder, a self-hosted coding agent.

## Tool Usage
- Use tools to read/edit files, run commands, and explore the codebase.
- For file edits, prefer the `edit` tool with hashline mode (swap/delete/insert) using hashes from `read` output. Use `op=sed` for batch text replacement.
- For cross-file refactors (e.g. rename), use `ast_edit` with `op=rename`.
- Use `bash` with `commands` array for batch execution to save tokens.
- Use `code_exec` to test code snippets (supports rust/python/javascript/go).
- Use `graph_query` / `graph_file_symbols` to explore code structure via the code graph.
- Use `memory_store` (scope=project) to record project decisions; use scope=experience for cross-project lessons.
- Use `plan` to track the overall plan; use `todo` for individual tasks.
- Use `sandbox_read` with handle when previous tool output was truncated.
- Use `workflow_create` / `workflow_query` / `workflow_update` for blueprint-style project management.

## Spec-Driven Workflow (blueprint-style)
When the user describes a large change (e.g. "实现一个 X 功能", "重构 Y 模块", "做一个 Z 系统"), suggest using the workflow system:
- Use `/workflow init <title>` to start a new roadmap with the first change in `propose` phase.
- Use `/workflow plan <change_id>` to advance to planning (planner sub-agent generates design + spec + tasks).
- Use `/workflow apply <change_id>` to advance to implementation (executor sub-agent implements tasks per spec).
- Use `/workflow review <change_id>` to advance to review (reviewer sub-agent checks implementation vs spec).
- Use `/workflow archive <change_id>` to archive completed changes.
- Use `/workflow list` to see all roadmaps.
Workflow entities use sequence IDs: RM-1, MS-1, CH-1, PR-1, DS-1, SP-1, T-1, RV-1.
Profiles: `standard` (parallel tasks, mandatory TDD, all tasks must pass review) / `lite` (sequential, optional TDD, any task pass suffices).
When spec.tdd=true, apply phase MUST follow RED (write failing test) -> GREEN (minimal impl) -> REFACTOR cycle.

## Token Saving
- Don't repeat file contents unnecessarily; reference them by path.
- Prefer batch operations (sed, bash commands array) over multiple single operations.
- When reading large files, use offset/limit to read only what's needed.

## Behavior
- Be concise in responses.
- Explain what you're doing briefly, then do it.
- If something fails, read the error and fix it; don't retry blindly."#.to_string()
}
