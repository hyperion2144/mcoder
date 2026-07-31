//! LSP 异步诊断状态：每个 session 维护一个 pending 队列
//!
//! 工作流程：
//! 1. write/edit 完成后，后台 LSP 任务计算诊断并 push_pending()
//! 2. SessionManager 在下次 tool call 执行前 drain_pending() 拼成 system message
//! 3. drain_pending() 清空队列，pending diagnostics 不重复注入
//!
//! 跨进程持久化（jsonl）：可选，避免 server restart 丢失未读诊断。
//! 简化方案：纯内存队列（session 在 server 进程内）

use crate::lsp::LspDiagnostic;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 单次诊断事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDiagnostic {
    pub file: String,
    pub language: String,
    pub tool_call_id: Option<String>,
    pub wait_ms: u64,
    pub diagnostics: Vec<LspDiagnostic>,
    pub ts: i64,
}

/// 格式化为可注入 LLM context 的文本
/// 每个文件一段，按 severity 计数
impl PendingDiagnostic {
    pub fn to_context_text(&self) -> String {
        if self.diagnostics.is_empty() {
            return String::new();
        }
        let mut errors = 0;
        let mut warnings = 0;
        let mut lines = Vec::new();
        for d in &self.diagnostics {
            match d.severity.as_str() {
                "error" => errors += 1,
                "warning" => warnings += 1,
                _ => {}
            }
            let code = d
                .code
                .as_ref()
                .map(|c| format!("[{}]", c))
                .unwrap_or_default();
            let src = d
                .source
                .as_ref()
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();
            // LSP line/column 是 0-based，转 1-based 方便 LLM 阅读
            lines.push(format!(
                "  [{}:{}] {}{}{}: {}",
                d.line + 1,
                d.column + 1,
                d.severity,
                code,
                src,
                d.message.lines().next().unwrap_or("").trim()
            ));
        }
        let summary = format!(
            "{} error{}, {} warning{}",
            errors,
            if errors == 1 { "" } else { "s" },
            warnings,
            if warnings == 1 { "" } else { "s" }
        );
        format!(
            "{} ({}, {}ms wait):\n{}",
            self.file,
            self.language,
            self.wait_ms,
            lines.join("\n")
        ) + &format!("\n  Summary: {}", summary)
    }
}

/// Per-session pending 队列
#[derive(Default)]
pub struct PendingDiagnosticsStore {
    by_session: RwLock<HashMap<String, Vec<PendingDiagnostic>>>,
}

impl PendingDiagnosticsStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 添加一条诊断到 session 队列
    pub async fn push(&self, session_id: &str, diag: PendingDiagnostic) {
        let mut map = self.by_session.write().await;
        map.entry(session_id.to_string())
            .or_insert_with(Vec::new)
            .push(diag);
    }

    /// 取出并清空 session 的所有 pending 诊断
    pub async fn drain(&self, session_id: &str) -> Vec<PendingDiagnostic> {
        let mut map = self.by_session.write().await;
        map.remove(session_id).unwrap_or_default()
    }

    /// 取出但不清空（peek）
    pub async fn peek(&self, session_id: &str) -> Vec<PendingDiagnostic> {
        let map = self.by_session.read().await;
        map.get(session_id).cloned().unwrap_or_default()
    }

    /// 同文件旧诊断替换为新诊断（去重）
    pub async fn replace_for_file(
        &self,
        session_id: &str,
        file: &str,
        new_diag: PendingDiagnostic,
    ) {
        let mut map = self.by_session.write().await;
        let entry = map.entry(session_id.to_string()).or_insert_with(Vec::new);
        // 移除该文件的旧诊断
        entry.retain(|d| d.file != file);
        entry.push(new_diag);
    }
}

/// 把多个 PendingDiagnostic 拼成一条 system message 的文本
pub fn format_for_context(diags: &[PendingDiagnostic], max_per_file: usize) -> String {
    if diags.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "[LSP diagnostics from previous edits - check for errors that need fixing]".to_string(),
        String::new(),
    ];
    let mut total = 0;
    for d in diags {
        let truncated: Vec<_> = d.diagnostics.iter().take(max_per_file).collect();
        let more = d.diagnostics.len().saturating_sub(truncated.len());
        let mut text = format!("{} ({}):\n", d.file, d.language);
        for td in truncated {
            let code = td
                .code
                .as_ref()
                .map(|c| format!("[{}]", c))
                .unwrap_or_default();
            // 保留完整 message 但限制 200 字符（多行诊断只取前几行）
            let msg = truncate_message(&td.message, 200);
            text.push_str(&format!(
                "  [L{}:{}] {}{}: {}\n",
                td.line + 1,
                td.column + 1,
                td.severity,
                code,
                msg
            ));
            total += 1;
        }
        if more > 0 {
            text.push_str(&format!("  ... and {} more\n", more));
        }
        lines.push(text);
    }
    lines.push(String::new());
    lines.push(format!(
        "Total: {} diagnostics across {} files.",
        total,
        diags.len()
    ));
    lines.join("\n")
}

/// 截断诊断消息：保留前 3 行或 200 字符（多行诊断如 rustc 的 expected x, found y 不会丢失）
fn truncate_message(s: &str, max_chars: usize) -> String {
    let truncated_by_line: String = s
        .lines()
        .take(3)
        .collect::<Vec<_>>()
        .join(" | ");
    if truncated_by_line.chars().count() <= max_chars {
        truncated_by_line
    } else {
        let chars: String = truncated_by_line.chars().take(max_chars).collect();
        format!("{}...", chars)
    }
}

/// 时间戳工具（避免在 file.rs 里直接 chrono）
pub fn now_ts() -> i64 {
    Utc::now().timestamp()
}