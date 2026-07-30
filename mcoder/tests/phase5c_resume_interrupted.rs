// Phase 5c: Resume 5 参数 + TaskStatus::Interrupted 集成测试
//
// 关键不变量：
// 1. TaskStatus 现在有 `Interrupted` 变体（不再映射为 Failed）
// 2. resume_policy::decide_resume 第 5 参数 has_interrupted_tasks=true
//    触发 Start（即使无 unfinished + 无 stop_reason）
// 3. AsyncTaskState::Interrupted → TaskStatus::Interrupted（不再丢）
// 4. AsyncTaskState::Interrupted → "Interrupted" 字符串（用于 snapshot）

use mcoder_lib::agent::async_tasks::TaskStatus;
use mcoder_lib::persistence::async_task_store::AsyncTaskState;

#[test]
fn task_status_interrupted_variant_exists() {
    // 之前 TaskStatus 没有 Interrupted 变体；现在必须存在
    let s = TaskStatus::Interrupted;
    assert_eq!(format!("{:?}", s), "Interrupted");
}

#[test]
fn task_status_to_db_state_interrupted() {
    assert_eq!(
        TaskStatus::Interrupted.to_db_state(),
        AsyncTaskState::Interrupted
    );
}

#[test]
fn task_status_from_db_state_interrupted() {
    // **关键回归点**：之前 AsyncTaskState::Interrupted 被映射为 Failed；
    // 现在必须是 TaskStatus::Interrupted（这样 has_interrupted_tasks 才能正确识别）
    assert_eq!(
        TaskStatus::from_db_state(AsyncTaskState::Interrupted),
        TaskStatus::Interrupted
    );
    // 同时验证其它状态未受影响
    assert_eq!(
        TaskStatus::from_db_state(AsyncTaskState::Running),
        TaskStatus::Running
    );
    assert_eq!(
        TaskStatus::from_db_state(AsyncTaskState::Failed),
        TaskStatus::Failed
    );
    assert_eq!(
        TaskStatus::from_db_state(AsyncTaskState::Cancelled),
        TaskStatus::Cancelled
    );
}

#[test]
fn task_status_round_trip_all_variants() {
    // 完整 round-trip：to_db_state → from_db_state == identity
    for s in [
        TaskStatus::Pending,
        TaskStatus::Running,
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
        TaskStatus::Interrupted,
    ] {
        let db = s.to_db_state();
        let back = TaskStatus::from_db_state(db);
        assert_eq!(s, back, "round-trip failed for {:?}", s);
    }
}

#[test]
fn decide_resume_has_interrupted_tasks_triggers_start() {
    use mcoder_lib::resume_policy::{decide_resume, ResumeDecisionKind};
    // 即使 loop_state=stopped, 无 unfinished, 无 stop_reason,
    // 仅 has_interrupted_tasks=true → 必须是 Start
    let r = decide_resume(false, "stopped", None, 0, true);
    assert_eq!(r, ResumeDecisionKind::Start);
}

#[test]
fn decide_resume_no_interrupted_no_work_is_no_work() {
    use mcoder_lib::resume_policy::{decide_resume, ResumeDecisionKind};
    let r = decide_resume(false, "completed", None, 0, false);
    assert_eq!(r, ResumeDecisionKind::NoWork);
}

#[test]
fn decide_resume_with_interrupted_tasks_stop_reason_starts() {
    use mcoder_lib::resume_policy::{decide_resume, ResumeDecisionKind};
    // stop_reason=interrupted_tasks + has_interrupted=true → Start
    let r = decide_resume(false, "stopped", Some("interrupted_tasks"), 0, true);
    assert_eq!(r, ResumeDecisionKind::Start);
}
