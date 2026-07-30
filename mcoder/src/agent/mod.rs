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
    Message { id: msg.id.clone(), parent_id: msg.parent_id.clone(), role: msg.role, content: new_content, usage: msg.usage.clone(), display_only: msg.display_only }
}

pub struct AgentSession {
    pub session: JsonlSession,
    pub messages: Vec<Message>,
    pub model_config: Arc<ModelConfig>,
    pub llm: SharedLLM,
    pub tools: Arc<ToolRegistry>,
    pub max_iters: u32,
    /// 设计文档 §3.4: 当前 role（默认 "default"）
    pub current_role: String,
    /// role 注册表引用（用于切换 role 和获取 system prompt）
    pub role_registry: Arc<role::RoleRegistry>,
    /// 累计 usage（跨轮次累加，用于 context 占用估算与 cost 展示）
    pub cumulative_usage: crate::llm::Usage,
    /// 当前消息树分支末端消息 id（None=空会话；发新消息时作为 parent_id）
    pub current_head_id: Option<String>,
}

impl AgentSession {
    pub fn new(
        session: JsonlSession,
        model_config: Arc<ModelConfig>,
        llm: SharedLLM,
        tools: Arc<ToolRegistry>,
        max_iters: u32,
        role_registry: Arc<role::RoleRegistry>,
    ) -> Self {
        let messages = session.read_all().unwrap_or_default();
        let current_head_id = session.current_head_id().map(|s| s.to_string());
        Self {
            session,
            messages,
            model_config,
            llm,
            tools,
            max_iters,
            current_role: "default".into(),
            role_registry,
            cumulative_usage: crate::llm::Usage::default(),
            current_head_id,
        }
    }

    pub fn add_message(&mut self, mut msg: Message) -> Result<()> {
        // 消息树：parent_id 未显式设置时，用 current_head_id 作为上游
        if msg.parent_id.is_none() {
            msg.parent_id = self.current_head_id.clone();
        }
        let msg_id = msg.id.clone();
        // 先持久化 JSONL（失败则整体回滚）
        self.session.append(&msg)?;
        // 再持久化 head_id（失败则 meta.json 仍为旧值，与内存一致）
        self.session.update_head_id(&msg_id)?;
        // 两步持久化都成功后才更新内存
        self.messages.push(msg);
        self.current_head_id = Some(msg_id);
        Ok(())
    }

    /// 返回 root->current_head_id 路径上的消息（消息树分支隔离）。
    /// 用于 run_once 调用 LLM 前筛选当前分支；messages 保留全树供 tree 视图/兄弟分支 checkout。
    /// m12: 过滤掉 `display_only=true` 的消息（仅展示给 UI，不送入 LLM 上下文）
    pub fn messages_along_head_path(&self) -> Vec<Message> {
        let head_id = match &self.current_head_id {
            Some(id) => id.clone(),
            None => return Vec::new(),
        };
        let by_id: std::collections::HashMap<&str, &Message> =
            self.messages.iter().map(|m| (m.id.as_str(), m)).collect();
        let target = match by_id.get(head_id.as_str()) {
            Some(m) => *m,
            None => return self.messages.clone(), // head 不在内存（异常），兜底返回全部
        };
        let mut path: Vec<&Message> = Vec::new();
        let mut cur = Some(target);
        while let Some(m) = cur {
            path.push(m);
            cur = m.parent_id.as_deref().and_then(|pid| by_id.get(pid).copied());
        }
        path.reverse();
        path.into_iter()
            .filter(|m| !m.display_only)
            .cloned()
            .collect()
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

    /// Switch the LLM model at runtime. Updates model_config and recreates the LLM adapter.
    /// The next run_once() call will use the new model.
    pub fn set_model(&mut self, model_config: Arc<ModelConfig>, llm: SharedLLM) -> Result<()> {
        self.model_config = model_config;
        self.llm = llm;
        // Persist new model to JSONL meta so server restart restores the correct model
        if let Err(e) = self.session.update_model(&self.model_config.name) {
            tracing::warn!("failed to persist model to session meta: {}", e);
        }
        // Refresh system prompt since it depends on model_config (model name, context window)
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
    /// 结构：静态段（Identity + Principles + Extensions + AGENTS.md）+ 会话段（Date/Platform/CWD/Git）
    /// 插入两条 system 消息：静态段和会话段，便于 LLM 适配器对静态段做 cache_control
    fn refresh_system_prompt(&mut self) {
        let role_prompt = self.role_registry.get(&self.current_role)
            .map(|r| r.system_prompt.clone())
            .unwrap_or_default();

        let project_path = self.session.project_path();
        let agents_md = load_agents_md(project_path);
        let static_segment = default_system_prompt(&self.model_config, agents_md.as_deref());
        let session_segment = build_session_segment(project_path);

        // role_prompt 合并到静态段前面（role_prompt 也属于可缓存的静态内容）
        let static_text = if role_prompt.is_empty() {
            static_segment
        } else {
            format!("{}\n\n{}", role_prompt, static_segment)
        };

        // 移除旧的 system prompt（可能有多条 system 消息在开头）
        while self.messages.first().map(|m| m.role == Role::System).unwrap_or(false) {
            self.messages.remove(0);
        }
        // 插入静态段和会话段两条 system 消息
        let static_msg = Message::system(static_text);
        let session_msg = Message::system(session_segment);
        if let Ok(()) = self.session.append(&static_msg) {
            self.messages.insert(0, static_msg);
        }
        if let Ok(()) = self.session.append(&session_msg) {
            self.messages.insert(1, session_msg);
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

        // After compaction, re-inject workflow context
        let project_path = self.session.project_path();
        let workflow_config = project_path.join(".mcoder").join("workflow").join("config.yaml");
        if workflow_config.exists() {
            if let Some(compact) = crate::workflow::context::build_compact_context(project_path) {
                if let Err(e) = self.add_message(Message::system(compact)) {
                    tracing::warn!("post-compaction workflow context re-injection failed: {}", e);
                }
            }
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
                ContentBlock::Image { .. } => 1000, // 图片估算 1000 token
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

    pub async fn run_once(&mut self) -> Result<(Option<Message>, Option<crate::llm::Usage>)> {
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

        // 非视觉模型图片过滤：若当前模型不支持图片输入，将 ContentBlock::Image 替换为
        // 包含文件路径的文本块，让模型知道图片存在并可调用 view_image 工具理解图片。
        // 仅取 root->current_head_id 路径上的消息（消息树分支隔离）
        let path_messages = self.messages_along_head_path();
        let mut messages_for_llm: Vec<Message> = if !self.model_config.supports_image() {
            path_messages.iter().map(|m| {
                let needs_filter = m.content.iter().any(|b| matches!(b, ContentBlock::Image { .. }));
                if !needs_filter {
                    return m.clone();
                }
                let mut new_content = Vec::with_capacity(m.content.len());
                for b in &m.content {
                    match b {
                        ContentBlock::Image { path, media_type } => {
                            new_content.push(ContentBlock::Text {
                                text: format!(
                                    "[image: {} ({}). Use the image tool with action=view and this path to analyze the image.]",
                                    path, media_type
                                ),
                            });
                        }
                        _ => new_content.push(b.clone()),
                    }
                }
                Message { id: m.id.clone(), parent_id: m.parent_id.clone(), role: m.role, content: new_content, usage: m.usage.clone(), display_only: m.display_only }
            }).collect()
        } else {
            path_messages
        };

        // Per-turn workflow state injection
        let project_path = self.session.project_path();
        let workflow_config = project_path.join(".mcoder").join("workflow").join("config.yaml");
        if workflow_config.exists() {
            if let Some(state) = crate::workflow::context::read_workflow_state(project_path) {
                if state.has_config {
                    let state_line = format!(
                        "[workflow state] milestone={} phase={} active_change={} next={}",
                        state.milestone.as_ref().map(|(_, n)| n.as_str()).unwrap_or("-"),
                        state.phase.as_ref().map(|(_, n)| n.as_str()).unwrap_or("-"),
                        state.active_change.as_ref().map(|(n, s)| format!("{}({})", n, s)).unwrap_or("-".to_string()),
                        state.next_action.as_deref().unwrap_or("-"),
                    );
                    messages_for_llm.push(Message::system(state_line));
                }
            }
        }

        let resp = self.llm.chat(&messages_for_llm, &schemas, &self.model_config)
            .await
            .context("LLM call failed")?;

        self.process_response(resp)
    }

    /// 检测 image 工具（action=send）调用结果，若返回 image_sent 标记，
    /// 则创建并追加一条含 ContentBlock::Image 的 assistant 消息到会话，
    /// 返回该消息供调用方广播给客户端展示。
    pub fn maybe_create_image_message(&mut self, call: &ToolCall, result_msg: &Message) -> Option<Message> {
        if call.name != "image" {
            return None;
        }
        // 从 ToolResult 中提取 image_sent 标记
        for block in &result_msg.content {
            if let ContentBlock::ToolResult { output, .. } = block {
                if let crate::types::ToolOutput::Sync { result } = output {
                    if result.get("type").and_then(|v| v.as_str()) == Some("image_sent") {
                        let image_path = result.get("image_path").and_then(|v| v.as_str()).unwrap_or("");
                        let media_type = result.get("media_type").and_then(|v| v.as_str()).unwrap_or("image/png");
                        let caption = result.get("caption").and_then(|v| v.as_str()).unwrap_or("");

                        let mut blocks = Vec::new();
                        if !caption.is_empty() {
                            blocks.push(ContentBlock::Text { text: caption.to_string() });
                        }
                        blocks.push(ContentBlock::Image {
                            path: image_path.to_string(),
                            media_type: media_type.to_string(),
                        });

                        // m12: 这条消息只用于 UI 展示图片，LLM 不应该再看到
                        // （否则下一轮 LLM 会以为这是它自己输出的图片，进入死循环）
                        let mut img_msg = Message::new(Role::Assistant, blocks);
                        img_msg.display_only = true;
                        if self.add_message(img_msg.clone()).is_ok() {
                            return Some(img_msg);
                        }
                    }
                }
            }
        }
        None
    }

    fn process_response(&mut self, resp: LLMResponse) -> Result<(Option<Message>, Option<crate::llm::Usage>)> {
        let LLMResponse { content, tool_calls, usage } = resp;

        // 累加 usage（若存在）
        let last_usage = usage.clone();
        if let Some(u) = &usage {
            self.cumulative_usage.prompt_tokens = self.cumulative_usage.prompt_tokens.saturating_add(u.prompt_tokens);
            self.cumulative_usage.completion_tokens = self.cumulative_usage.completion_tokens.saturating_add(u.completion_tokens);
            self.cumulative_usage.total_tokens = self.cumulative_usage.total_tokens.saturating_add(u.total_tokens);
            self.cumulative_usage.cache_read_input_tokens = self.cumulative_usage.cache_read_input_tokens.saturating_add(u.cache_read_input_tokens);
            self.cumulative_usage.cache_creation_input_tokens = self.cumulative_usage.cache_creation_input_tokens.saturating_add(u.cache_creation_input_tokens);
        }

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
            return Ok((None, last_usage));
        }

        let msg = Message::new(Role::Assistant, blocks);
        let msg = msg.with_usage(last_usage.clone());
        self.add_message(msg.clone())?;
        Ok((Some(msg), last_usage))
    }

    pub async fn execute_tool(&mut self, call: &ToolCall, ctx: &crate::tools::ToolContext) -> Result<Vec<Message>> {
        // C3: 在执行前检查取消，避免半提交状态
        if ctx.cancellation.is_cancelled() {
            let msg = Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                id: call.id.clone(),
                output: ToolOutput::Error { message: "cancelled before execution".into() },
            }]);
            return Ok(vec![msg]);
        }

        let output = match self.tools.execute(call, ctx).await {
            Ok(out) => out,
            Err(e) => ToolOutput::Error { message: e.to_string() },
        };

        // C3: 工具执行后再次检查取消
        if ctx.cancellation.is_cancelled() {
            let msg = Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                id: call.id.clone(),
                output: ToolOutput::Error { message: "cancelled after execution".into() },
            }]);
            self.add_message(msg.clone())?;
            return Ok(vec![msg]);
        }

        // 检测图片 read 结果：若工具返回的 Sync result 中含 {"type":"image", ...}，
        // 则根据主模型是否支持图片输入走不同分支。
        // 注：data_url 已移除（C1），ContentBlock::Image 只需 path + media_type，
        // 各 LLM adapter 会自己读文件再 base64 编码。
        let mut image_info: Option<(String, String, Option<String>)> = None;
        if let ToolOutput::Sync { result } = &output {
            if result.get("type").and_then(|v| v.as_str()) == Some("image") {
                let path = result.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let media_type = result.get("media_type").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let description = result.get("description")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                if !path.is_empty() && !media_type.is_empty() {
                    image_info = Some((path, media_type, description));
                }
            }
        }

        // 非图片工具走原路径
        let Some((path, media_type, description)) = image_info else {
            let msg = Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                id: call.id.clone(),
                output,
            }]);
            self.add_message(msg.clone())?;
            return Ok(vec![msg]);
        };

        if self.model_config.supports_image() {
            // M4: 主模型支持图片时 description 永远为 None（read_image 跳过了视觉模型调用），
            // 所以 compact_result 不包含 description，img_msg 也不带描述文本。
            // C2: 先构造两条消息，用单次 add_message 分别提交；
            // 若第二条失败，第一条已持久化，此时追加一条 error tool_result 作为补偿，
            // 让下一轮 LLM 调用知道图片未附加。
            let compact_result = serde_json::json!({
                "type": "image",
                "path": path,
                "media_type": media_type,
                "width": output.get_field("width"),
                "height": output.get_field("height"),
                "size_bytes": output.get_field("size_bytes"),
                "note": "image displayed in next user message",
            });
            let tool_msg = Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                id: call.id.clone(),
                output: ToolOutput::Sync { result: compact_result },
            }]);

            let img_msg = Message::new(Role::User, vec![
                ContentBlock::Text {
                    text: format!("[image from read: {}]", path),
                },
                ContentBlock::Image {
                    path: path.clone(),
                    media_type: media_type.clone(),
                },
            ]);

            // C2: 原子性 -- 先提交 tool_msg，再提交 img_msg。
            // 若 img_msg 失败，追加补偿消息。
            self.add_message(tool_msg.clone())?;
            if let Err(e) = self.add_message(img_msg.clone()) {
                tracing::error!("failed to persist image message for {}: {}", path, e);
                let compensation = Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                    id: call.id.clone(),
                    output: ToolOutput::Error {
                        message: format!("image attachment failed: {}", e),
                    },
                }]);
                let _ = self.add_message(compensation);
                return Ok(vec![tool_msg]);
            }
            Ok(vec![tool_msg, img_msg])
        } else if let Some(desc) = description {
            // 主模型不支持但视觉模型有描述：把描述作为工具结果文本返回
            let new_output = ToolOutput::Sync {
                result: serde_json::json!({
                    "type": "image",
                    "description": desc,
                    "note": "vision model described this image; main model cannot see images"
                }),
            };
            let msg = Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                id: call.id.clone(),
                output: new_output,
            }]);
            self.add_message(msg.clone())?;
            Ok(vec![msg])
        } else {
            // M5: 都没法处理 -- 保留元信息 + 提示主模型可调用 image 工具重试
            let new_output = ToolOutput::Sync {
                result: serde_json::json!({
                    "type": "image",
                    "path": path,
                    "media_type": media_type,
                    "note": "image read but no vision model available. You cannot see this image. Use image tool action=view with this path to retry, or skip this image."
                }),
            };
            let msg = Message::new(Role::Tool, vec![ContentBlock::ToolResult {
                id: call.id.clone(),
                output: new_output,
            }]);
            self.add_message(msg.clone())?;
            Ok(vec![msg])
        }
    }
}

/// 辅助 trait：从 ToolOutput::Sync 中安全提取字段（不存在时返回 Null）
trait ToolOutputExt {
    fn get_field(&self, key: &str) -> serde_json::Value;
}

impl ToolOutputExt for ToolOutput {
    fn get_field(&self, key: &str) -> serde_json::Value {
        match self {
            ToolOutput::Sync { result } => result.get(key).cloned().unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        }
    }
}

/// 默认 system prompt 的静态段（可缓存）：Identity + Principles + Extensions + Project Context
/// 参数：model_config 提供模型信息，agents_md 为 AGENTS.md 文件内容（若存在）
fn default_system_prompt(model_config: &ModelConfig, agents_md: Option<&str>) -> String {
    let protocol_str = match model_config.protocol {
        crate::types::ModelProtocol::OpenaiChat => "OpenAI Chat",
        crate::types::ModelProtocol::OpenaiCompatible => "OpenAI Compatible",
        crate::types::ModelProtocol::OpenaiResponses => "OpenAI Responses",
        crate::types::ModelProtocol::Anthropic => "Anthropic",
        crate::types::ModelProtocol::Gemini => "Gemini",
    };
    let provider = extract_provider(&model_config.base_url);

    let mut prompt = format!(
        r#"# Identity & Model
You are mcoder, a self-hosted coding agent running on {model_name} ({provider} {protocol}).
Context window: {context_window}K tokens.

You assist users with software engineering tasks: reading/writing code, running commands,
debugging, refactoring, and project management.

# Operating Principles
- Read before writing. Understand existing code before modifying it.
- Be concise. Act, don't narrate.
- If something fails, read the error and fix it. Don't retry blindly.
- Don't re-output file contents you've already read; reference by path.
- Prefer batch operations (bash commands array, sed) over repeated single calls.
- When unsure, ask the user (ask_user tool).

# Extensions
- Use skill_use(action=list) to discover available skills, skill_use(action=use, name=...) to activate one.
- Use mcp_list to discover external tools from connected MCP servers, mcp_call(server, tool, args) to invoke them."#,
        model_name = model_config.name,
        provider = provider,
        protocol = protocol_str,
        context_window = model_config.context_window / 1000,
    );

    if let Some(md) = agents_md {
        if !md.trim().is_empty() {
            prompt.push_str("\n\n# Project Context\n");
            prompt.push_str(md.trim());
        }
    }

    prompt
}

/// 从 base_url 提取 provider 名称（域名部分）
fn extract_provider(base_url: &str) -> String {
    let url = base_url.trim_end_matches('/');
    // 去掉协议前缀
    let host = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    // 取第一个 / 之前的部分
    let host = host.split('/').next().unwrap_or(host);
    // 去掉 api. 前缀，取主域名
    let host = host.strip_prefix("api.").unwrap_or(host);
    host.to_string()
}

/// 构建 system prompt 的会话段（不可缓存）：日期、平台、CWD、Git 信息
fn build_session_segment(project_path: &std::path::Path) -> String {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let platform = std::env::consts::OS;
    let cwd = project_path.display();

    let git_info = git_summary(project_path);

    format!(
        r#"
# Session
Date: {date}
Platform: {platform}
CWD: {cwd}{git_info}"#,
        date = date,
        platform = platform,
        cwd = cwd,
        git_info = git_info,
    )
}

/// 获取 git 分支和状态摘要
fn git_summary(project_path: &std::path::Path) -> String {
    let branch = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(project_path)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    match branch {
        Some(b) => format!("\nGit: {}", b),
        None => String::new(),
    }
}

/// 加载 AGENTS.md 文件（优先项目根目录，其次 .mcoder/AGENTS.md）
fn load_agents_md(project_path: &std::path::Path) -> Option<String> {
    let candidates = [
        project_path.join("AGENTS.md"),
        project_path.join(".mcoder").join("AGENTS.md"),
    ];
    for path in &candidates {
        if let Ok(content) = std::fs::read_to_string(path) {
            return Some(content);
        }
    }
    None
}
