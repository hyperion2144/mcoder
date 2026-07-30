// Test: 终审修复 #1 — restart ask answer 追加 ToolResult 前校验 JSONL 存在真实 matching ToolUse
//
// 不变量（终审修复 #1）：
// 1. 服务重启后 ask.answer 若找到 DB 中 persisted pending_ask 但 JSONL 里
//    找不到匹配的 ToolUse(id=tool_call_id)：
//    - **不能**追加 ToolResult Message（避免 LLM 见到 无主 tool_result）
//    - DB 写终态 cancelled（不是 answered）
//    - 广播 AskCancelled 事件
//    - 不进入 resume（can_resume=false）
// 2. JSONL 找到匹配的 ToolUse：
//    - 追加 ToolResult Message（id 匹配 ToolUse.id）
//    - DB answered
//    - 广播 AskAnswered
//    - can_resume=true（让 client 触发 resume_session）
//
// 这是纯逻辑测试，用 messaging schema 直接构造 tool_use 块以保证测试与 LLM adapter
// 解耦。验证逻辑集中在纯函数 `verify_or_cancel_restart_pending_ask` 上。

use mcoder_lib::ask_user::{
    verify_or_cancel_restart_pending_ask, RestartAskDecision, VerifyMode,
};
use mcoder_lib::types::{ContentBlock, Message, Role, ToolOutput};
use serde_json::json;

fn tool_use_msg(id: &str) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: "ask_user".into(),
            args: json!({"questions": []}),
        }],
    }
}

fn tool_result_msg(id: &str, output: serde_json::Value) -> Message {
    Message {
        role: Role::Tool,
        content: vec![ContentBlock::ToolResult {
            id: id.to_string(),
            output: ToolOutput::Sync { result: output },
        }],
    }
}

fn text_msg(role: Role, text: &str) -> Message {
    Message { role, content: vec![ContentBlock::Text { text: text.into() }] }
}

#[test]
fn restart_with_matching_tool_use_returns_proceed() {
    // JSONL 存在与 tool_call_id 匹配的 ToolUse → Proceed
    let tc = "toolcall-uuid-match";
    let messages = vec![
        text_msg(Role::System, "sys"),
        text_msg(Role::User, "hi"),
        tool_use_msg(tc),
        tool_result_msg(tc, json!({"placeholder": true})),
    ];
    let d = verify_or_cancel_restart_pending_ask(&messages, tc, VerifyMode::ToolUse);
    assert!(
        matches!(d, RestartAskDecision::Proceed { .. }),
        "matching ToolUse must yield Proceed; got {:?}",
        d
    );
}

#[test]
fn restart_without_matching_tool_use_yields_cancel_not_proceed() {
    // JSONL 找不到匹配 ToolUse → Cancel（绝不能 Proceed）
    let tc = "toolcall-uuid-missing";
    let messages = vec![
        text_msg(Role::User, "hi"),
        // 不同的 tool_use，id 不匹配
        tool_use_msg("toolcall-other-id"),
        text_msg(Role::Assistant, "done"),
    ];
    let d = verify_or_cancel_restart_pending_ask(&messages, tc, VerifyMode::ToolUse);
    assert!(
        matches!(d, RestartAskDecision::Cancel { .. }),
        "missing ToolUse must yield Cancel; got {:?}",
        d
    );
    // 必须有理由/细节
    if let RestartAskDecision::Cancel { reason } = d {
        assert!(!reason.is_empty(), "cancel decision must include non-empty reason");
    }
}

#[test]
fn restart_with_empty_messages_yields_cancel() {
    // 防御性：空消息列表也必须 Cancel（不能 Proceed）
    let tc = "toolcall-uuid-1";
    let messages: Vec<Message> = Vec::new();
    let d = verify_or_cancel_restart_pending_ask(&messages, tc, VerifyMode::ToolUse);
    assert!(
        matches!(d, RestartAskDecision::Cancel { .. }),
        "empty messages must yield Cancel; got {:?}",
        d
    );
}

#[test]
fn restart_tool_use_id_exact_match_required() {
    let tc = "exact-id-1";
    let messages = vec![
        // id 只有前/后缀匹配，不算精确匹配
        tool_use_msg(&format!("{}-suffix", tc)),
        tool_use_msg(&format!("prefix-{}", tc)),
    ];
    let d = verify_or_cancel_restart_pending_ask(&messages, tc, VerifyMode::ToolUse);
    assert!(
        matches!(d, RestartAskDecision::Cancel { .. }),
        "non-exact id must not yield Proceed; got {:?}",
        d
    );
}

#[test]
fn restart_tool_use_mode_ignores_tool_result_only() {
    // VerifyMode::ToolResult 用作回退：仅看 JSONL 中是否有匹配 ToolResult
    // 这是为一些 adapter / restart 场景；如果 tool_use 已存在则优先 ToolUse 模式
    let tc = "toolcall-fallback";
    let messages = vec![tool_result_msg(tc, json!({"foo": 1}))];
    let d = verify_or_cancel_restart_pending_ask(&messages, tc, VerifyMode::ToolResult);
    assert!(matches!(d, RestartAskDecision::Proceed { .. }));
    let d2 = verify_or_cancel_restart_pending_ask(&messages, tc, VerifyMode::ToolUse);
    assert!(matches!(d2, RestartAskDecision::Cancel { .. }));
}
