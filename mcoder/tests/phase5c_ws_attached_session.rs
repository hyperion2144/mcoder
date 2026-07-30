// Phase 5c: ws_server attached_session 校验
//
// 关键不变量：
// 1. tool.call / ask.pending / task.list / task.cancel 校验 attached_session
//    == params.session_id（防止越权访问 / 跨会话读取）
// 2. 不满足时返回 -32602 Invalid Params
//
// **实现说明**：本测试为 RED 覆盖；本测试要求 ws_server 层 handle_request
// 能感知 attached_session。当前实现已通过 `attached_session: &Option<String>`
// 参数传入；本测试仅做"行为契约"验证，需要构造一个最小 ws server 实例。
//
// 由于直接构造 WsServer + 启动 WS 端口较重，本测试改用**纯函数单元**覆盖：
// 把 handle_request 内部的"是否越权"判定抽成纯函数 check_attached_session。

use mcoder_lib::transport::ws_server::check_attached_session;

/// 纯函数校验：attached_session 是否匹配 params.session_id
/// 单独抽出便于单元测试（不依赖 WsServer 实例）
#[test]
fn rejects_when_not_attached() {
    // caller 未 attach → params.session_id 任意值都应被拒
    let r = check_attached_session(&None, "any-session");
    assert!(!r.is_ok(), "must reject when caller is not attached");
}

#[test]
fn rejects_cross_session_access() {
    // caller attach 到 s1，但 params.session_id = s2 → 拒绝
    let r = check_attached_session(&Some("s1".to_string()), "s2");
    assert!(!r.is_ok(), "must reject cross-session param mismatch");
}

#[test]
fn accepts_same_session() {
    let r = check_attached_session(&Some("s1".to_string()), "s1");
    assert!(r.is_ok(), "must accept same session");
}

#[test]
fn accepts_when_param_empty() {
    // 客户端 RPC 路径通常 params.session_id 与 attached 一致；
    // 空字符串 session_id 仍可调用 ping 等 RPC（不是 session-scoped）
    let r = check_attached_session(&Some("s1".to_string()), "");
    // 空 param 不应被该 helper 拦（这些 RPC 走其他分支）
    assert!(r.is_ok());
}
