// AskUser 工具 - 服务端实现
//
// 设计目标（与设计文档 §6 / §8 对齐）：
// 1. 服务端把 ask_user 作为**结构化普通工具**注册到 ToolRegistry。
// 2. 调用时，当前 session 的 agent loop 在 ask_user.execute() 中 await 用户回答。
//    其他 session 不阻塞（ask 池是 per-session 的）。
// 3. 多客户端同步：pending / answered / cancelled 通过 ServerEvent 广播给订阅此 session 的 client。
// 4. 客户端通过 WS RPC（ask.pending / ask.answer / ask.cancel）提交/查询。
// 5. 校验：1-4 题、每题 2-4 选项、单选/多选/其他自由文本/取消。
// 6. 答案返回给 LLM 时：cancelled → 简短取消说明；否则按 question 顺序格式化为结构化 + 自由文本。
// 7. pending 期间，sessions.send 输入的纯文本视为 note（增强版答案），不创建新 loop。
//
// 客户端类型定义见 mcoder-tui/src/ask/types.ts
// 客户端共享纯逻辑见 mcoder-tui/src/ask/{validation,summary}.ts（与本文件的校验/摘要规则保持一致）
//
// Phase 4: pending ask 通过 SessionStateStore 持久化（per session）；服务重启后
//   memory registry 为空时，从 DB 恢复 pending_ask 供 snapshot 用。
//   ask.answer RPC 在内存无 pending 但 DB 有 pending 时走 restart 路径：
//   1) 验证 submission
//   2) 向 JsonlSession 追加匹配原 tool_call_id 的真实 ToolResult Message
//      （绝不伪造新 ToolUse）
//   3) DB answered
//   4) loop_state 置 stopped / waiting_for_user 终态合理状态
//   5) 返回 can_resume=true 让 client 触发 resume_session

use crate::persistence::session_state::SessionStateStore;
use crate::session_manager::ServerEvent;
use crate::tools::{Tool, ToolContext};
use crate::types::{ToolOutput, ToolSchema};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, Notify};

/// 工具名常量（与客户端 ASK_USER_TOOL 对应）
pub const ASK_USER_TOOL: &str = "ask_user";

/// 约束：1-4 题、每题 2-4 选项
pub const ASK_MIN_QUESTIONS: usize = 1;
pub const ASK_MAX_QUESTIONS: usize = 4;
pub const ASK_MIN_OPTIONS: usize = 2;
pub const ASK_MAX_OPTIONS: usize = 4;

// ==================== 类型定义 ====================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AskMode {
    Single,
    Multi,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskQuestion {
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub options: Vec<AskOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_select: Option<bool>,
}

impl AskQuestion {
    pub fn mode(&self) -> AskMode {
        if self.multi_select.unwrap_or(false) {
            AskMode::Multi
        } else {
            AskMode::Single
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskRequest {
    pub questions: Vec<AskQuestion>,
}

/// 单题答案：single / multi / note-only (custom) / skipped
///
/// 设计目的：
/// 1. 普通文本输入作为 note 提交时，不能强制给所有题写空 option
///    （之前 `try_handle_text_for_pending_ask` 会给所有题写 `option: ""`，导致 `validate_submission` 报 unknown option）
/// 2. 多题场景下用户可能只回答其中几题，其他题允许 Custom(none) / 留空
/// 3. single / multi 仍是结构化首选；note-only / skipped 用于"完全自由回答 / 跳过"
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskQuestionAnswer {
    /// 自由文本答复；用户没选任何 option，仅给出 note
    Custom { note: String },
    /// 结构化单选 + 可选 note
    Single {
        option: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// 结构化多选 + 可选 note
    Multi {
        options: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// 该题明确跳过（用户未回答）
    Skipped,
}

/// 一次提交（cancelled=true 时 answers 为空）
///
/// - `cancelled`：取消整个 Ask
/// - `answers`：question 索引 → 答案；cancelled 时可为空
/// - `custom_response`：跨题整段自由文本（如多题场景下仅给一段话作答）
///   仅当 answers 为空 / 全 Skipped / 全 Custom 时设置；服务端校验语义
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AskSubmission {
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub answers: HashMap<u32, AskQuestionAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_response: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingAsk {
    pub ask_id: String,
    pub tool_call_id: String,
    pub session_id: String,
    pub request: AskRequest,
    pub created_at_ms: i64,
    /// 提交答案的 oneshot（tool execute 在此 await）
    pub notify: Arc<Notify>,
    /// 提交的答案（首决议占位后槽内一定有 `Some(AskSubmission)`；
    ///   cancelled 决议时 `cancelled = true`、`answers` 为空）—— Arc 共享。
    /// 注：execute 路径"先检查槽再 await notified()"协议下，await 醒来后槽必为
    ///   `Some`：决议路径（create 旧 pending / cancel / submit_validated）一律
    ///   `notify_one()` 后才写槽，不可能再出现"未决议"状态。
    pub submission: Arc<Mutex<Option<AskSubmission>>>,
}

// ==================== 校验（与客户端 validation.ts 行为一致）====================

pub fn validate_request(raw: &Value) -> Result<AskRequest, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();
    let obj = match raw.as_object() {
        Some(o) => o,
        None => return Err(vec!["ask_user args must be an object".into()]),
    };
    let questions_raw = match obj.get("questions") {
        Some(v) => v,
        None => return Err(vec!["ask_user.questions must be an array".into()]),
    };
    let questions_arr = match questions_raw.as_array() {
        Some(a) => a,
        None => return Err(vec!["ask_user.questions must be an array".into()]),
    };
    if questions_arr.len() < ASK_MIN_QUESTIONS || questions_arr.len() > ASK_MAX_QUESTIONS {
        errors.push(format!(
            "ask_user.questions length must be {}-{}, got {}",
            ASK_MIN_QUESTIONS,
            ASK_MAX_QUESTIONS,
            questions_arr.len()
        ));
    }
    let mut questions: Vec<AskQuestion> = Vec::new();
    for (i, q) in questions_arr.iter().enumerate() {
        let qo = match q.as_object() {
            Some(o) => o,
            None => {
                errors.push(format!("questions[{}] must be an object", i));
                continue;
            }
        };
        let question_text = match qo.get("question").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => {
                errors.push(format!("questions[{}].question must be a non-empty string", i));
                String::new()
            }
        };
        let options_raw = match qo.get("options").and_then(|v| v.as_array()) {
            Some(a) => a,
            None => {
                errors.push(format!("questions[{}].options must be an array", i));
                continue;
            }
        };
        if options_raw.len() < ASK_MIN_OPTIONS || options_raw.len() > ASK_MAX_OPTIONS {
            errors.push(format!(
                "questions[{}].options length must be {}-{}, got {}",
                i,
                ASK_MIN_OPTIONS,
                ASK_MAX_OPTIONS,
                options_raw.len()
            ));
        }
        let mut options: Vec<AskOption> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (j, opt) in options_raw.iter().enumerate() {
            let oo = match opt.as_object() {
                Some(o) => o,
                None => {
                    errors.push(format!("questions[{}].options[{}] must be an object", i, j));
                    continue;
                }
            };
            let label = match oo.get("label").and_then(|v| v.as_str()) {
                Some(s) if !s.trim().is_empty() => s.to_string(),
                _ => {
                    errors.push(format!(
                        "questions[{}].options[{}].label must be a non-empty string",
                        i, j
                    ));
                    continue;
                }
            };
            if !seen.insert(label.clone()) {
                errors.push(format!(
                    "questions[{}].options[{}].label duplicate: \"{}\"",
                    i, j, label
                ));
            }
            let description = oo
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            options.push(AskOption { label, description });
        }
        let multi_select = qo.get("multi_select").and_then(|v| v.as_bool());
        let header = qo
            .get("header")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        questions.push(AskQuestion {
            question: question_text,
            header,
            options,
            multi_select,
        });
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(AskRequest { questions })
}

pub fn validate_submission(req: &AskRequest, sub: &AskSubmission) -> Result<(), Vec<String>> {
    if sub.cancelled {
        return Ok(());
    }
    let mut errors: Vec<String> = Vec::new();
    let all_questions_count = req.questions.len();
    // 顶层 custom_response 一旦非空，所有题均可"未填"（视为整段答复覆盖整个 Ask）
    let has_custom_resp = sub
        .custom_response
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let mut structural_present = 0usize;
    for (i, q) in req.questions.iter().enumerate() {
        let idx = i as u32;
        let a = match sub.answers.get(&idx) {
            Some(a) => a,
            None => {
                // 无单题答复：要求有 custom_response 兜底
                if !has_custom_resp {
                    errors.push(format!("missing answer for question {}", i));
                }
                continue;
            }
        };
        let labels: std::collections::HashSet<&str> =
            q.options.iter().map(|o| o.label.as_str()).collect();
        // 模式校验（issue 4）：question.mode 与 answer kind 必须一致
        let expected_mode = q.mode();
        match a {
            AskQuestionAnswer::Single { option, .. } => {
                if expected_mode == AskMode::Multi {
                    errors.push(format!(
                        "question {}: mode mismatch — question is multi-select but answer is single-select",
                        i
                    ));
                    continue;
                }
                if option.is_empty() {
                    errors.push(format!("question {}: single-select requires non-empty option", i));
                    continue;
                }
                if !labels.contains(option.as_str()) {
                    errors.push(format!("question {}: unknown option \"{}\"", i, option));
                }
                structural_present += 1;
            }
            AskQuestionAnswer::Multi { options, .. } => {
                if expected_mode == AskMode::Single {
                    errors.push(format!(
                        "question {}: mode mismatch — question is single-select but answer is multi-select",
                        i
                    ));
                    continue;
                }
                if options.is_empty() {
                    errors.push(format!(
                        "question {}: multi-select requires non-empty options[]",
                        i
                    ));
                    continue;
                }
                for opt in options {
                    if !labels.contains(opt.as_str()) {
                        errors.push(format!("question {}: unknown option \"{}\"", i, opt));
                    }
                }
                structural_present += 1;
            }
            AskQuestionAnswer::Custom { .. } => {
                // note-only 答复本身合法；不计入 structural_present
            }
            AskQuestionAnswer::Skipped => {
                // 显式跳过：合法
            }
        }
    }
    // 兜底校验：所有题既无结构化答复也无 Custom/Skipped → 必须有 custom_response
    let all_skipped_or_empty = structural_present == 0
        && sub.answers.values().all(|a| matches!(a, AskQuestionAnswer::Skipped | AskQuestionAnswer::Custom { .. }));
    if all_questions_count > 0 && all_skipped_or_empty && !has_custom_resp && sub.answers.is_empty() {
        errors.push(
            "submission must contain at least one structural answer, a per-question Custom, or a top-level custom_response".into(),
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ==================== Pending 池（per-session）====================

type StoreFuture = Pin<
    Box<dyn Future<Output = Result<Option<Arc<SessionStateStore>>>> + Send + 'static>,
>;
type StoreResolver = dyn Fn(String) -> StoreFuture + Send + Sync;

pub struct AskRegistry {
    /// session_id → 当前 pending Ask（每个 session 同时只允许一个 pending）
    pending: Mutex<HashMap<String, PendingAsk>>,
    /// 按 session_id 解析其项目级 SessionStateStore。None 仅用于纯内存测试。
    store_resolver: Option<Arc<StoreResolver>>,
}

impl Default for AskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AskRegistry {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            store_resolver: None,
        }
    }

    pub fn with_store_resolver<F, Fut>(resolver: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Arc<SessionStateStore>>> + Send + 'static,
    {
        Self {
            pending: Mutex::new(HashMap::new()),
            store_resolver: Some(Arc::new(move |session_id| {
                let fut = resolver(session_id);
                Box::pin(async move { fut.await.map(Some) })
            })),
        }
    }

    pub async fn store_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<Arc<SessionStateStore>>> {
        match &self.store_resolver {
            Some(resolve) => resolve(session_id.to_string()).await,
            None => Ok(None),
        }
    }

    /// 创建 pending ask。
    ///
    /// - 同一 session 同时只允许一个 pending；
    /// - 若已有旧 pending，**原子地**把它标记为 cancelled 并唤醒等待者，
    ///   同时把新 pending 写入 map（避免后写覆盖）；
    /// - 返回 (new_pending, Option<old_pending>)：old_pending 用于广播 AskCancelled 事件（issue 8）
    pub async fn create(
        &self,
        session_id: &str,
        tool_call_id: &str,
        request: AskRequest,
    ) -> (PendingAsk, Option<PendingAsk>) {
        let pending = PendingAsk {
            ask_id: format!("ask-{}", uuid::Uuid::new_v4()),
            tool_call_id: tool_call_id.to_string(),
            session_id: session_id.to_string(),
            request,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            notify: Arc::new(Notify::new()),
            submission: Arc::new(Mutex::new(None)),
        };
        let old = self
            .pending
            .lock()
            .await
            .insert(session_id.to_string(), pending.clone());
        if let Some(prev) = &old {
            decide_first(
                &prev.submission,
                AskSubmission { cancelled: true, ..Default::default() },
            )
            .await;
            prev.notify.notify_one();
        }
        (pending, old)
    }

    pub async fn create_persisted(
        &self,
        session_id: &str,
        tool_call_id: &str,
        request: AskRequest,
    ) -> Result<(PendingAsk, Option<PendingAsk>)> {
        let ask_id = format!("ask-{}", uuid::Uuid::new_v4());
        let now = chrono::Utc::now().timestamp_millis();
        let submission_slot = Arc::new(Mutex::new(None));
        let pending = PendingAsk {
            ask_id: ask_id.clone(),
            tool_call_id: tool_call_id.to_string(),
            session_id: session_id.to_string(),
            request,
            created_at_ms: now,
            notify: Arc::new(Notify::new()),
            submission: submission_slot,
        };
        if let Some(store) = self.store_for_session(session_id).await? {
            store
                .create_pending_ask_waiting(
                    session_id,
                    &ask_id,
                    tool_call_id,
                    serde_json::to_value(&pending.request).unwrap_or(serde_json::Value::Null),
                    now,
                )
                .await?;
        }

        // DB 成功后才修改内存并唤醒被覆盖的 waiter。
        let old = {
            let mut map = self.pending.lock().await;
            map.insert(session_id.to_string(), pending.clone())
        };
        let old_ret = if let Some(prev) = old {
            tracing::warn!(
                "ask_user: overwriting pending ask {} for session {}",
                prev.ask_id,
                session_id
            );
            decide_first(
                &prev.submission,
                AskSubmission { cancelled: true, ..Default::default() },
            )
            .await;
            prev.notify.notify_one();
            Some(prev)
        } else {
            None
        };

        Ok((pending, old_ret))
    }

    /// 取出并移除 pending（answers 时调用；保证一次性）
    pub async fn take(&self, session_id: &str, ask_id: &str) -> Option<PendingAsk> {
        let mut map = self.pending.lock().await;
        if let Some(p) = map.get(session_id) {
            if p.ask_id == ask_id {
                return map.remove(session_id);
            }
        }
        None
    }

    /// 取出当前 session 的 pending（不校验 ask_id），用于取消
    pub async fn take_by_session(&self, session_id: &str) -> Option<PendingAsk> {
        let mut map = self.pending.lock().await;
        map.remove(session_id)
    }

    /// 仅查看（不取出）
    pub async fn peek(&self, session_id: &str) -> Option<PendingAsk> {
        self.pending.lock().await.get(session_id).cloned()
    }

    /// 取消并清理（语义同 take_by_session + 标记 cancelled + notify）
    ///
    /// **首决议语义**：若已被首答决议占位（submission 槽非空），cancel 不覆盖、返回 None。
    ///   这是为了保证"answer 之后 cancel 不能篡改答案"（issue: 首决议并发）。
    pub async fn cancel(&self, session_id: &str) -> Option<PendingAsk> {
        self.cancel_persisted(session_id).await.ok().flatten()
    }

    pub async fn cancel_persisted(&self, session_id: &str) -> Result<Option<PendingAsk>> {
        let pending = match self.peek(session_id).await {
            Some(p) => p,
            None => return Ok(None),
        };

        if let Some(store) = self.store_for_session(session_id).await? {
            let updated = store
                .cancel_pending_ask_and_stop(
                    session_id,
                    &pending.ask_id,
                    chrono::Utc::now().timestamp_millis(),
                    "ask_cancelled",
                )
                .await?;
            if !updated {
                return Ok(None);
            }
        }

        let won = decide_first(
            &pending.submission,
            AskSubmission { cancelled: true, ..Default::default() },
        )
        .await;
        if !won {
            return Ok(None);
        }

        {
            let mut map = self.pending.lock().await;
            if map
                .get(session_id)
                .is_some_and(|current| current.ask_id == pending.ask_id)
            {
                map.remove(session_id);
            }
        }
        pending.notify.notify_one();
        Ok(Some(pending))
    }

    /// 原子提交答案：把校验 + 写入 + notify 全部放在同一锁区间，
    /// 防止"先 validate 通过但 pending 已被取走"的竞态（issue 4）
    ///
    /// **首决议语义**：若已被 cancel / 早 submit 占位（submission 槽非空），
    ///   第二次 submit 返回错误，不覆盖已决议答案（issue: 首决议并发）。
    ///
    /// 成功 → `Some(())`；失败 → `Err(errors)`
    pub async fn submit_validated(
        &self,
        session_id: &str,
        ask_id: &str,
        req: &AskRequest,
        sub: AskSubmission,
    ) -> Result<(), Vec<String>> {
        // 校验在锁外做（只读 req / sub），避免锁内阻塞
        if let Err(errs) = validate_submission(req, &sub) {
            return Err(errs);
        }
        // 1. 锁内只匹配 ask_id 并 clone 出 PendingAsk 的 Arc handle；
        //    **绝不在持有 registry.pending 锁的同时锁 submission**（避免锁序死锁）
        let pending = {
            let map = self.pending.lock().await;
            match map.get(session_id) {
                Some(p) if p.ask_id == ask_id => p.clone(),
                _ => {
                    return Err(vec![format!(
                        "no pending ask {} for session {}",
                        ask_id, session_id
                    )]);
                }
            }
        };
        if let Some(store) = self
            .store_for_session(session_id)
            .await
            .map_err(|e| vec![format!("ask persistence failed: {}", e)])?
        {
            let result_json = build_tool_result(req, &sub);
            let updated = store
                .answer_pending_ask_and_stop(
                    session_id,
                    ask_id,
                    serde_json::to_value(&sub).unwrap_or(serde_json::Value::Null),
                    result_json,
                    chrono::Utc::now().timestamp_millis(),
                    "ask_answered",
                )
                .await
                .map_err(|e| vec![format!("ask persistence failed: {}", e)])?;
            if !updated {
                return Err(vec![format!(
                    "ask {} for session {} already decided; first decision wins",
                    ask_id, session_id
                )]);
            }
        }

        let won = decide_first(&pending.submission, sub.clone()).await;
        if !won {
            return Err(vec![format!(
                "ask {} for session {} already decided; first decision wins",
                ask_id, session_id
            )]);
        }
        pending.notify.notify_one();

        Ok(())
    }
}

// ==================== 终审修复 #1: restart ask answer JSONL 校验 ====================
//
// 服务重启 / memory registry 被清空后，ask.answer 走 DB 路径追加 ToolResult Message。
// 此时必须校验 JSONL 中存在真实匹配的 ToolUse（id == tool_call_id），否则：
// - **绝不能**追加无主 ToolResult（LLM 看到 unmatched tool_result 会导致错误）
// - DB 写终态 cancelled（不是 answered）
// - 广播 AskCancelled（client 移除 ask 卡片）
// - 不进入 resume（can_resume=false）
//
// 这是一个纯逻辑判定函数（不动 DB / 文件 I/O），方便 unit test 与 SessionManager 调用分离。

/// 校验模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    /// 要求 JSONL 里有匹配 id 的 ToolUse 块（默认首选）
    ToolUse,
    /// 要求 JSONL 里有匹配 id 的 ToolResult 块（罕见回退）
    ToolResult,
}

/// restart ask 验证决策
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartAskDecision {
    /// JSONL 存在匹配的块 → append_tool_result_message 后 DB answered + AskAnswered + can_resume=true
    Proceed { mode: VerifyMode },
    /// JSONL 没有匹配 → 拒答，DB cancelled + AskCancelled + can_resume=false
    Cancel { reason: String },
}

/// 纯函数：验证 restart ask answer 是否安全追加 ToolResult Message。
///
/// 输入：
/// - `messages`：JsonlSession 已读出的消息列表（已按顺序）
/// - `tool_call_id`：DB 中 pending_ask 的 tool_call_id
/// - `mode`：校验模式（默认 ToolUse）
///
/// 输出：
/// - Proceed：存在匹配 JSONL 块 → 走 append_tool_result_message + DB answered
/// - Cancel：不存在匹配 → 走 DB cancelled + AskCancelled
///
/// 安全约束：
/// - id 严格相等（不做前缀/后缀匹配）
/// - 匹配即发现即返回（不取最后一次）
/// - 空 messages 列表 → Cancel（防止 LLM 见到无主 tool_result）
pub fn verify_or_cancel_restart_pending_ask(
    messages: &[crate::types::Message],
    tool_call_id: &str,
    mode: VerifyMode,
) -> RestartAskDecision {
    use crate::types::ContentBlock;
    if tool_call_id.is_empty() {
        return RestartAskDecision::Cancel {
            reason: "empty tool_call_id is invalid".into(),
        };
    }
    for m in messages {
        for b in &m.content {
            match (mode, b) {
                (VerifyMode::ToolUse, ContentBlock::ToolUse { id, .. }) if id == tool_call_id => {
                    return RestartAskDecision::Proceed { mode: VerifyMode::ToolUse };
                }
                (VerifyMode::ToolResult, ContentBlock::ToolResult { id, .. })
                    if id == tool_call_id =>
                {
                    return RestartAskDecision::Proceed { mode: VerifyMode::ToolResult };
                }
                _ => {}
            }
        }
    }
    let reason = match mode {
        VerifyMode::ToolUse => format!(
            "no ToolUse block with id={} found in JSONL history; refusing to append orphan ToolResult",
            tool_call_id
        ),
        VerifyMode::ToolResult => format!(
            "no ToolResult block with id={} found in JSONL history; refusing to append orphan ToolResult",
            tool_call_id
        ),
    };
    RestartAskDecision::Cancel { reason }
}

/// 把 `sub` 写入 `slot` 仅当其当前为 None（set-if-empty）。
///
/// 返回 `true` 表示本次写入生效（首决议），`false` 表示槽位已被其他决议占位。
///
/// **锁序约束**：本函数**绝不在调用方持有 AskRegistry.pending 锁时被调用**，
/// 否则会与 cancel / create 内"先释放 registry 锁、再锁 submission"的路径产生锁序循环。
async fn decide_first(slot: &Arc<Mutex<Option<AskSubmission>>>, sub: AskSubmission) -> bool {
    let mut g = slot.lock().await;
    if g.is_some() {
        // 已被首决议占位：不覆盖
        false
    } else {
        *g = Some(sub);
        true
    }
}

// ==================== 工具实现 ====================

/// AskUser 工具：把 args 解析为 AskRequest，await 用户回答，返回结构化结果
pub struct AskUserTool {
    pub registry: Arc<AskRegistry>,
    /// Late binding 注入：在 ToolRegistry 构建时还没有 event_tx
    event_tx: std::sync::Mutex<Option<broadcast::Sender<ServerEvent>>>,
}

impl AskUserTool {
    pub fn new(registry: Arc<AskRegistry>) -> Self {
        Self {
            registry,
            event_tx: std::sync::Mutex::new(None),
        }
    }

    pub fn set_event_tx(&self, tx: broadcast::Sender<ServerEvent>) {
        if let Ok(mut g) = self.event_tx.lock() {
            *g = Some(tx);
        }
    }

    fn tx(&self) -> Option<broadcast::Sender<ServerEvent>> {
        self.event_tx.lock().ok().and_then(|g| g.clone())
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        ASK_USER_TOOL
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: ASK_USER_TOOL.into(),
            description: "Ask the user structured questions (1-4) with 2-4 options each. Blocks the current session's agent loop until the user answers or cancels. Other sessions are not blocked. Use this when you need a real decision from the user (e.g. clarify ambiguous requirements, choose between approaches, confirm scope).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": ASK_MIN_QUESTIONS,
                        "maxItems": ASK_MAX_QUESTIONS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": { "type": "string", "description": "The question to ask" },
                                "header": { "type": "string", "description": "Short label for UI (max ~12 chars)" },
                                "options": {
                                    "type": "array",
                                    "minItems": ASK_MIN_OPTIONS,
                                    "maxItems": ASK_MAX_OPTIONS,
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": { "type": "string" },
                                            "description": { "type": "string" }
                                        },
                                        "required": ["label"]
                                    }
                                },
                                "multi_select": { "type": "boolean", "default": false }
                            },
                            "required": ["question", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        // 1. 校验
        let request = validate_request(&args).map_err(|errs| {
            anyhow::anyhow!(
                "invalid ask_user args: {}",
                errs.iter().map(|e| e.as_str()).collect::<Vec<_>>().join("; ")
            )
        })?;

        // 2. tool_call_id 必须使用真实 LLM ToolCall.id（issue 1）；
        //    仅在直接调用（tool.call RPC / 测试）时退化为 ask-{uuid}，便于区分。
        let tool_call_id = ctx
            .tool_call_id
            .clone()
            .unwrap_or_else(|| format!("ask-{}", uuid::Uuid::new_v4()));

        // 3. 创建 pending；若覆盖旧 pending 则广播 AskCancelled（issue 8）
        let (pending, overwritten) = self
            .registry
            .create_persisted(&ctx.session_id, &tool_call_id, request.clone())
            .await?;

        let tx = self.tx();

        if let (Some(tx), Some(old)) = (&tx, overwritten.as_ref()) {
            let _ = tx.send(ServerEvent::AskCancelled {
                session_id: ctx.session_id.clone(),
                ask_id: old.ask_id.clone(),
                tool_call_id: old.tool_call_id.clone(),
            });
        }

        // 4. 广播 pending 事件给所有订阅此 session 的 client
        if let Some(tx) = &tx {
            let _ = tx.send(ServerEvent::AskPending {
                session_id: ctx.session_id.clone(),
                ask_id: pending.ask_id.clone(),
                tool_call_id: tool_call_id.clone(),
                request: request.clone(),
            });
        }

        // 5. await 用户回答（同时响应 cancellation）
        //
        // 唤醒协议（issue 1：notify-before-wait 不丢 permit）：
        // - 所有决议路径（create 旧 pending / cancel / submit_validated）一律
        //   `notify_one()`，保留 permit：即使 waiter 还没注册，permit 也不会丢；
        // - execute 走"先检查 submission 槽 → 若空则 `notified().await` 注册
        //   permit → 醒来后再查一次"的循环协议；permission 可能在 first_check
        //   之后、await 之前到达（notify_one 会让 next notified() 立刻返回）；
        // - 醒来后若槽仍为空（理论不应发生），**绝不**用 `unwrap_or_default()` 当
        //   作空答案，而是继续等待或响应 cancellation，避免把"未知"塞成"默认取消
        //   否"。
        let submission: AskSubmission = {
            let notify = pending.notify.clone();
            let submission_arc = pending.submission.clone();
            let sid = ctx.session_id.clone();
            let registry = self.registry.clone();
            let ctx_cancellation = ctx.cancellation.clone();
            // 循环：先检查槽，命中则退出；否则 await notified()，被唤醒后再
            // 检查一次。permit 由 notify_one 累加，本循环最坏 O(2) 次等待。
            loop {
                if let Some(s) = submission_arc.lock().await.take() {
                    break s;
                }
                tokio::select! {
                    biased;
                    // 取消优先：ctx.cancellation 一旦触发，立刻走 cancel 路径，
                    // 避免闭锁的 cancel-during-await 还要再等一次 notify_one。
                    _ = ctx_cancellation.cancelled() => {
                        // 整个 session 被取消 → 走 AskRegistry::cancel（首决议语义），
                        // 广播 AskCancelled **仅由 SessionManager.cancel/close 单点
                        // 负责**，这里不再广播（issue 3 防止双广播）。
                        let _ = registry.cancel(&sid).await;
                        // cancel 之后槽一定被占位（cancelled）；再读一次必命中。
                        break submission_arc
                            .lock()
                            .await
                            .take()
                            .expect("cancel must populate submission slot");
                    }
                    _ = notify.notified() => {
                        // notify_one 会保留 permit；这里 permit 已消耗，
                        // 写者若在两次 `take()` 之间写入，下一次 take 会立刻
                        // 拿到，否则下个 await notified() 拿下一个 permit。
                        continue;
                    }
                }
            }
        };

        // 6. 清理 pending
        let _ = self.registry.take(&ctx.session_id, &pending.ask_id).await;

        // 7. 构造返回结果 + 广播 answered 事件
        let result_json = build_tool_result(&request, &submission);
        // 终态唯一性（issue 3）：cancelled 由 SessionManager 单点广播 AskCancelled，
        // 这里 **绝不能** 再广播 AskAnswered（防止 cancelled → Answered 双广播）。
        if !submission.cancelled {
            if let Some(tx) = &tx {
                let _ = tx.send(ServerEvent::AskAnswered {
                    session_id: ctx.session_id.clone(),
                    ask_id: pending.ask_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    submission: submission.clone(),
                    result: result_json.clone(),
                });
            }
        }

        Ok(ToolOutput::Sync { result: result_json })
    }
}

/// 把 AskSubmission 格式化为给 LLM 的 tool result JSON
pub fn build_tool_result(req: &AskRequest, sub: &AskSubmission) -> Value {
    if sub.cancelled {
        return serde_json::json!({
            "cancelled": true,
            "questions": req.questions,
        });
    }
    let mut answers_out: Vec<Value> = Vec::with_capacity(req.questions.len());
    for (i, q) in req.questions.iter().enumerate() {
        let idx = i as u32;
        let a = sub.answers.get(&idx);
        let answer = match a {
            Some(AskQuestionAnswer::Single { option, note }) => {
                serde_json::json!({
                    "question": q.question,
                    "header": q.header,
                    "mode": "single",
                    "option": option,
                    "note": note,
                })
            }
            Some(AskQuestionAnswer::Multi { options, note }) => {
                serde_json::json!({
                    "question": q.question,
                    "header": q.header,
                    "mode": "multi",
                    "options": options,
                    "note": note,
                })
            }
            Some(AskQuestionAnswer::Custom { note }) => {
                serde_json::json!({
                    "question": q.question,
                    "header": q.header,
                    "mode": "custom",
                    "note": note,
                })
            }
            Some(AskQuestionAnswer::Skipped) => {
                serde_json::json!({
                    "question": q.question,
                    "header": q.header,
                    "mode": "skipped",
                })
            }
            None => {
                serde_json::json!({
                    "question": q.question,
                    "header": q.header,
                    "answer": null,
                })
            }
        };
        answers_out.push(answer);
    }
    // 自定义整段自由文本答复（多题场景下整段作答）
    let mut payload = serde_json::json!({
        "cancelled": false,
        "answers": answers_out,
    });
    if let Some(cr) = &sub.custom_response {
        if !cr.trim().is_empty() {
            payload["custom_response"] = serde_json::Value::String(cr.clone());
        }
    }
    payload
}
