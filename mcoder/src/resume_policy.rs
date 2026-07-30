// Phase 3: Resume 策略纯逻辑
//
// 设计：
// - `decide_resume` 是纯函数（无副作用、无 DB / 文件 I/O）
// - SessionManager::resume_session 调用它做决策，然后做 CAS + 注入消息 +
//   spawn run_agent_loop
// - 客户端 TS 也实现同名决策（src/resume/state.ts），用于 UI gating；测试矩阵
//   一一对应，保证两边语义一致
//
// 测试位置：mcoder/tests/session_resume.rs
//
// 决策矩阵：
// 1. loop_running=true 或 loop_state ∈ {running, waiting_for_user} → Conflict
// 2. stop_reason ∈ {blocked, cancelled, failed, unfinished_todos, interrupted_tasks}
//    或 unfinished > 0 或 has_interrupted_tasks=true → Start
// 3. loop_state ∈ {completed, idle, stopped} 且 stop_reason=None 且
//    unfinished==0 且 has_interrupted_tasks=false → NoWork { requires_user_input: true }
// 4. loop_state == waiting_for_user → WaitingForUser（与 1 的 waiting 分支一致）

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeDecisionKind {
    /// 启动 loop（已注入 [session resumed] 系统消息；调用方负责 spawn run_agent_loop）
    Start,
    /// 拒绝：loop 已在运行（loop_running=true）或 loop_state=running/waiting_for_user
    Conflict,
    /// 不启动：completed/idle/stopped 且无未完成工作；等待用户消息
    NoWork,
    /// 不启动：loop_state=waiting_for_user，等待用户回答 ask
    WaitingForUser,
    /// 自愈：DB 仍标 waiting_for_user，但 ask/plan 已无 pending。
    /// 调用方应把 session_state 写回 stopped 并继续走 NoWork 路径。
    HealStopped,
}

/// 触发 Start 的 stop_reason 集合：
/// - blocked：被 hook 拦截，需要重试
/// - cancelled：被用户取消，可能需要续上
/// - failed：失败，重试
/// - unfinished_todos：未完成 todo，需要续上
/// - interrupted_tasks：Phase 5: 服务重启打断的 task，agent inspect 后决定重跑
const RESUME_REASONS: &[&str] = &[
    "blocked",
    "cancelled",
    "failed",
    "unfinished_todos",
    "interrupted_tasks",
];

/// 纯函数：resume 决策
///
/// 输入：
/// - `loop_running`: 当前内存 AtomicBool
/// - `loop_state`: 持久化 loop_state（idle | running | stopped | waiting_for_user | completed）
/// - `stop_reason`: None 或具体原因字符串
/// - `unfinished`: 未完成 todo 数量（pending + in_progress）
/// - `has_interrupted_tasks`: Phase 5: 当前 session 是否有 interrupted tasks
///   （即使没有 unfinished todo，只要存在 interrupted task 也应允许 Start）
pub fn decide_resume(
    loop_running: bool,
    loop_state: &str,
    stop_reason: Option<&str>,
    unfinished: usize,
    has_interrupted_tasks: bool,
    has_pending_ask_or_plan: bool,
) -> ResumeDecisionKind {
    // 1. running 状态：拒绝
    if loop_running || loop_state == "running" {
        return ResumeDecisionKind::Conflict;
    }
    // 2. waiting_for_user：单独分支 → WaitingForUser（不抢答）
    if loop_state == "waiting_for_user" {
        // 启动期/stale 场景：DB 标 waiting_for_user 但内存和 DB 都没 pending ask/plan。
        // 自愈为 stopped 后重新走默认 NoWork 路径，避免永远卡死。
        if !has_pending_ask_or_plan {
            return ResumeDecisionKind::HealStopped;
        }
        return ResumeDecisionKind::WaitingForUser;
    }
    // 3. 有未完成 todo：必须 Start
    if unfinished > 0 {
        return ResumeDecisionKind::Start;
    }
    // 4. stop_reason ∈ RESUME_REASONS：也 Start
    if let Some(r) = stop_reason {
        if RESUME_REASONS.contains(&r) {
            return ResumeDecisionKind::Start;
        }
    }
    // 5. Phase 5: 仅有 interrupted tasks（无 unfinished + 无 stop_reason）也 Start
    //    让 agent inspect 后决定是否重跑
    if has_interrupted_tasks {
        return ResumeDecisionKind::Start;
    }
    // 6. 其它（completed / idle / stopped + 无 stop_reason / 无 unfinished / 无 interrupted）：NoWork
    ResumeDecisionKind::NoWork
}

/// 兼容旧签名（4 参数；默认 has_interrupted_tasks=false, has_pending_ask_or_plan=true）
pub fn decide_resume_v0(
    loop_running: bool,
    loop_state: &str,
    stop_reason: Option<&str>,
    unfinished: usize,
) -> ResumeDecisionKind {
    decide_resume(loop_running, loop_state, stop_reason, unfinished, false, true)
}