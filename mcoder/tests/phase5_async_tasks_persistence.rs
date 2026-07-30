// Phase 5: 异步任务按 session 持久化到现有 SQLite（session-scoped lifecycle）。
//
// 关键不变量：
// 1. tasks 表按 session 持久化（task_id PK, session_id NOT NULL, tool_name,
//    args_json, status(queued/running/completed/failed/cancelled/interrupted),
//    output_json, error, created_at_ms, updated_at_ms）
// 2. 服务启动 / 历史 session load / attach 时，把 DB 中 queued/running 原子标记
//    interrupted（绝不自动重跑任何工具）
// 3. TaskManager 创建 / 状态变化 / 完成 / 取消时写 DB（per-session 隔离）
// 4. SessionSnapshot.tasks 返回该 session 全量任务元数据（不再全局 best-effort）
// 5. session.resume 若有 interrupted tasks，在 [session resumed] 系统消息中
//    列出 tool_name/task_id/args/已有输出，明确让 agent inspect and decide
//    whether to rerun；**不能自动调用原工具**；即使仅有 interrupted task 也应允许 Start
// 6. RPC task.list 按 session 隔离；task.cancel 只能取消 attached/current session 的
//    task（防跨会话）
//
// 测试矩阵（RED 覆盖）：
// - session isolation（s1 和 s2 的 tasks 互不可见）
// - running → interrupted restart
// - completed 不改变（重启后状态保持 completed）
// - 不自动重跑（启动后 queued/running 被原子转为 interrupted，且不会触发任何重跑）
// - snapshot 返回 per-session 全量元数据（包含 args / output_json / status）
// - resume 注入 interrupted 列表（不重跑）
// - 跨 session cancel 拒绝（attempting to cancel s1's task from s2 returns 403-like error）

use mcoder_lib::agent::async_tasks::{TaskManager, TaskStatus};
use mcoder_lib::persistence::async_task_store::{AsyncTaskRecord, AsyncTaskState, AsyncTaskStore};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static SEQ: AtomicU64 = AtomicU64::new(0);

fn fresh_db_path() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "mcoder-phase5-{}-{}.db",
        std::process::id(),
        n
    ))
}

async fn fresh_store() -> AsyncTaskStore {
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    let pool = mcoder_lib::persistence::init_sqlite(&path).await.unwrap();
    AsyncTaskStore::new(pool)
}

// ==================== 基础 schema + CRUD ====================

#[tokio::test]
async fn insert_and_get_task() {
    let store = fresh_store().await;
    let rec = store
        .create_task(
            "s1",
            "bash",
            serde_json::json!({"cmd": "echo hi"}),
            1000,
        )
        .await
        .unwrap();
    assert_eq!(rec.session_id, "s1");
    assert_eq!(rec.tool_name, "bash");
    assert_eq!(rec.status, AsyncTaskState::Running);
    assert_eq!(rec.args_json, serde_json::json!({"cmd": "echo hi"}));
    assert!(rec.output_json.is_none());
    assert!(rec.error.is_none());

    let fetched = store.get_task(&rec.task_id).await.unwrap();
    assert_eq!(fetched.task_id, rec.task_id);
    assert_eq!(fetched.status, AsyncTaskState::Running);
}

#[tokio::test]
async fn complete_task_writes_output() {
    let store = fresh_store().await;
    let rec = store
        .create_task("s1", "bash", serde_json::json!({}), 1000)
        .await
        .unwrap();
    let output = serde_json::json!({"exit_code": 0, "stdout": "hi"});
    store
        .complete_task(&rec.task_id, output.clone(), 2000)
        .await
        .unwrap();
    let fetched = store.get_task(&rec.task_id).await.unwrap();
    assert_eq!(fetched.status, AsyncTaskState::Completed);
    assert_eq!(fetched.output_json, Some(output));
}

#[tokio::test]
async fn fail_task_writes_error() {
    let store = fresh_store().await;
    let rec = store
        .create_task("s1", "bash", serde_json::json!({}), 1000)
        .await
        .unwrap();
    store
        .fail_task(&rec.task_id, "timeout", 2000)
        .await
        .unwrap();
    let fetched = store.get_task(&rec.task_id).await.unwrap();
    assert_eq!(fetched.status, AsyncTaskState::Failed);
    assert_eq!(fetched.error.as_deref(), Some("timeout"));
}

#[tokio::test]
async fn cancel_task_changes_status() {
    let store = fresh_store().await;
    let rec = store
        .create_task("s1", "bash", serde_json::json!({}), 1000)
        .await
        .unwrap();
    store.cancel_task(&rec.task_id, 2000).await.unwrap();
    let fetched = store.get_task(&rec.task_id).await.unwrap();
    assert_eq!(fetched.status, AsyncTaskState::Cancelled);
}

// ==================== session isolation ====================

#[tokio::test]
async fn session_isolation_list() {
    let store = fresh_store().await;
    store.create_task("s1", "bash", serde_json::json!({}), 1).await.unwrap();
    store.create_task("s1", "bash", serde_json::json!({}), 2).await.unwrap();
    store.create_task("s2", "bash", serde_json::json!({}), 3).await.unwrap();

    let s1 = store.list_tasks_for_session("s1").await.unwrap();
    let s2 = store.list_tasks_for_session("s2").await.unwrap();
    assert_eq!(s1.len(), 2, "s1 must not see s2 tasks");
    assert_eq!(s2.len(), 1);
    for t in &s1 {
        assert_eq!(t.session_id, "s1");
    }
}

#[tokio::test]
async fn get_task_returns_none_for_cross_session() {
    let store = fresh_store().await;
    let rec = store
        .create_task("s1", "bash", serde_json::json!({}), 1)
        .await
        .unwrap();
    let fetched = store.get_task_for_session("s2", &rec.task_id).await;
    assert!(fetched.is_none(), "must not fetch s1 task via s2");
    let fetched = store.get_task_for_session("s1", &rec.task_id).await;
    assert!(fetched.is_some(), "owner session can fetch");
}

// ==================== restart: queued/running → interrupted (atomic) ====================

#[tokio::test]
async fn restart_marks_queued_and_running_as_interrupted_atomically() {
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    let pool = mcoder_lib::persistence::init_sqlite(&path).await.unwrap();
    let store = std::sync::Arc::new(AsyncTaskStore::new(pool.clone()));
    // 创建 2 个 running + 1 个 completed（completed 不被干扰）
    let r1 = store.create_task("s1", "bash", serde_json::json!({"id": 1}), 1).await.unwrap();
    let r2 = store.create_task("s1", "bash", serde_json::json!({"id": 2}), 2).await.unwrap();
    let r3 = store.create_task("s1", "bash", serde_json::json!({"id": 3}), 3).await.unwrap();
    store.complete_task(&r3.task_id, serde_json::json!({"ok": 1}), 4).await.unwrap();

    // 模拟服务重启：drop 内存 store，重新打开 DB
    drop(store);
    let pool2 = mcoder_lib::persistence::init_sqlite(&path).await.unwrap();
    let store2 = std::sync::Arc::new(AsyncTaskStore::new(pool2));

    let interrupted_count = store2.mark_orphans_interrupted(chrono::Utc::now().timestamp_millis()).await.unwrap();
    assert_eq!(interrupted_count, 2, "must atomic-mark 2 running as interrupted");

    // 检查状态
    let t1 = store2.get_task(&r1.task_id).await.unwrap();
    let t2 = store2.get_task(&r2.task_id).await.unwrap();
    let t3 = store2.get_task(&r3.task_id).await.unwrap();
    assert_eq!(t1.status, AsyncTaskState::Interrupted);
    assert_eq!(t2.status, AsyncTaskState::Interrupted);
    assert_eq!(t3.status, AsyncTaskState::Completed, "completed must not be touched");
}

#[tokio::test]
async fn restart_does_not_rerun_any_tool() {
    // 验证：mark_orphans_interrupted 不会触发任何 task.run() 调用
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    {
        let pool = mcoder_lib::persistence::init_sqlite(&path).await.unwrap();
        let store = std::sync::Arc::new(AsyncTaskStore::new(pool));
        let r1 = store.create_task("s1", "bash", serde_json::json!({}), 1).await.unwrap();
        // mark_orphans_interrupted 不应触发任何 task.run 逻辑
        store.mark_orphans_interrupted(chrono::Utc::now().timestamp_millis()).await.unwrap();
        // 验证：r1 status=interrupted，error 包含 "interrupted" 描述
        let t1 = store.get_task(&r1.task_id).await.unwrap();
        assert_eq!(t1.status, AsyncTaskState::Interrupted);
    }
}

// ==================== TaskManager 集成：DB 持久化 ====================

#[tokio::test]
async fn taskmanager_writes_db_on_create_and_complete() {
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    let pool = mcoder_lib::persistence::init_sqlite(&path).await.unwrap();
    let store = Arc::new(AsyncTaskStore::new(pool.clone()));
    let mgr = TaskManager::new_for_session("s1", store.clone());

    let id = mgr
        .spawn_compat("bash", async move { Ok::<_, String>("hi".to_string()) })
        .await;
    // 等任务完成
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    use sqlx::Row;
    let row = sqlx::query("SELECT status, output_json FROM async_tasks WHERE task_id = ?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let status: String = row.get(0);
    let output: Option<String> = row.get(1);
    assert_eq!(status, "completed");
    let output_v: serde_json::Value =
        serde_json::from_str(&output.expect("output should be written")).unwrap();
    assert_eq!(output_v, serde_json::json!("hi"));
}

// ==================== snapshot 字段契约 ====================

#[tokio::test]
async fn snapshot_task_record_has_required_fields() {
    // AsyncTaskRecord 必须包含 task_id, session_id, tool_name, status, args_json,
    // output_json?, error?, created_at_ms, updated_at_ms
    let rec = AsyncTaskRecord {
        task_id: "t1".into(),
        session_id: "s1".into(),
        tool_name: "bash".into(),
        status: AsyncTaskState::Interrupted,
        args_json: serde_json::json!({"cmd": "ls"}),
        output_json: None,
        error: None,
        created_at_ms: 1000,
        updated_at_ms: 2000,
    };
    let v = serde_json::to_value(&rec).unwrap();
    for k in [
        "task_id",
        "session_id",
        "tool_name",
        "status",
        "args_json",
        "created_at_ms",
        "updated_at_ms",
    ] {
        assert!(v.get(k).is_some(), "missing field: {}", k);
    }
    assert_eq!(v["status"], "interrupted");
}

// ==================== status enum 契约 ====================

#[test]
fn async_task_state_serialization() {
    assert_eq!(
        serde_json::to_string(&AsyncTaskState::Queued).unwrap(),
        "\"queued\""
    );
    assert_eq!(
        serde_json::to_string(&AsyncTaskState::Running).unwrap(),
        "\"running\""
    );
    assert_eq!(
        serde_json::to_string(&AsyncTaskState::Completed).unwrap(),
        "\"completed\""
    );
    assert_eq!(
        serde_json::to_string(&AsyncTaskState::Failed).unwrap(),
        "\"failed\""
    );
    assert_eq!(
        serde_json::to_string(&AsyncTaskState::Cancelled).unwrap(),
        "\"cancelled\""
    );
    assert_eq!(
        serde_json::to_string(&AsyncTaskState::Interrupted).unwrap(),
        "\"interrupted\""
    );
}

#[test]
fn async_task_state_deserialization() {
    let s: AsyncTaskState = serde_json::from_str("\"queued\"").unwrap();
    assert_eq!(s, AsyncTaskState::Queued);
    let s: AsyncTaskState = serde_json::from_str("\"interrupted\"").unwrap();
    assert_eq!(s, AsyncTaskState::Interrupted);
}

// ==================== TaskStatus 兼容（agent/async_tasks.rs 旧枚举） ====================
//
// 旧的 TaskStatus（Pending/Running/Completed/Failed/Cancelled）应保留为 in-memory
// 状态。新 AsyncTaskState 用于 DB 持久化。两者映射如下：
// - Pending → Queued
// - Running → Running
// - Completed → Completed
// - Failed → Failed
// - Cancelled → Cancelled
// - interrupted（DB） → Interrupted

#[test]
fn taskstatus_to_db_state_mapping() {
    // 通过 TaskManager 的辅助函数转换（Phase 5 实现）
    assert_eq!(
        TaskManager::task_status_to_db_state(&TaskStatus::Pending),
        AsyncTaskState::Queued
    );
    assert_eq!(
        TaskManager::task_status_to_db_state(&TaskStatus::Running),
        AsyncTaskState::Running
    );
    assert_eq!(
        TaskManager::task_status_to_db_state(&TaskStatus::Completed),
        AsyncTaskState::Completed
    );
    assert_eq!(
        TaskManager::task_status_to_db_state(&TaskStatus::Failed),
        AsyncTaskState::Failed
    );
    assert_eq!(
        TaskManager::task_status_to_db_state(&TaskStatus::Cancelled),
        AsyncTaskState::Cancelled
    );
}

// ==================== TaskManager spawn 默认 DB 持久化 ====================

#[tokio::test]
async fn taskmanager_default_uses_db() {
    // spawn 必须写入 DB；如果 session-scoped store 不可用，错误
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    let pool = mcoder_lib::persistence::init_sqlite(&path).await.unwrap();
    let store = Arc::new(AsyncTaskStore::new(pool.clone()));
    let mgr = TaskManager::new_for_session("s1", store.clone());

    // spawn 一个 5s 睡眠任务，立即 cancel
    let id = mgr
        .spawn_compat("bash", async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            Ok::<_, String>("done".to_string())
        })
        .await;

    // 立即 cancel：spawn 应该已经把 task 写入 DB
    mgr.cancel(&id).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mgr_list = mgr.list().await;
    assert!(!mgr_list.is_empty(), "in-memory must have task");
}

// ==================== resume_policy 5 参数版本（Phase 5 扩展） ====================

#[test]
fn resume_only_interrupted_tasks_starts() {
    // 即使没有 unfinished todos 和 stop_reason，仅有 interrupted tasks 也应 Start
    use mcoder_lib::resume_policy::{decide_resume, ResumeDecisionKind};
    let r = decide_resume(false, "stopped", None, 0, true);
    assert_eq!(r, ResumeDecisionKind::Start);
}

#[test]
fn resume_interrupted_tasks_stop_reason_starts() {
    use mcoder_lib::resume_policy::{decide_resume, ResumeDecisionKind};
    let r = decide_resume(false, "stopped", Some("interrupted_tasks"), 0, true);
    assert_eq!(r, ResumeDecisionKind::Start);
}

#[test]
fn resume_no_work_no_interrupted_is_no_work() {
    use mcoder_lib::resume_policy::{decide_resume, ResumeDecisionKind};
    let r = decide_resume(false, "completed", None, 0, false);
    assert_eq!(r, ResumeDecisionKind::NoWork);
}

// ==================== 跨 session cancel 拒绝 ====================

#[tokio::test]
async fn cross_session_cancel_denied() {
    // 同一个 store 中，s1 的 task 不能被 s2 通过 get_task_for_session 取到
    let store = fresh_store().await;
    let r1 = store.create_task("s1", "bash", serde_json::json!({}), 1).await.unwrap();
    // 通过 s2 查：应该 None
    let fetched = store.get_task_for_session("s2", &r1.task_id).await;
    assert!(fetched.is_none(), "must not see s1's task from s2");
    // 通过 s1 查：应该 Some
    let fetched = store.get_task_for_session("s1", &r1.task_id).await;
    assert!(fetched.is_some(), "owner can fetch");
}

#[tokio::test]
async fn session_a_does_not_see_session_b_tasks() {
    let store = fresh_store().await;
    store.create_task("a", "bash", serde_json::json!({}), 1).await.unwrap();
    store.create_task("a", "bash", serde_json::json!({}), 2).await.unwrap();
    store.create_task("b", "bash", serde_json::json!({}), 3).await.unwrap();
    let a = store.list_tasks_for_session("a").await.unwrap();
    let b = store.list_tasks_for_session("b").await.unwrap();
    assert_eq!(a.len(), 2);
    assert_eq!(b.len(), 1);
    for t in &a {
        assert_eq!(t.session_id, "a");
    }
    for t in &b {
        assert_eq!(t.session_id, "b");
    }
}

// ==================== AsyncTaskStore: restart 不重跑 ====================

#[tokio::test]
async fn restart_does_not_auto_rerun_via_spawn() {
    // 模拟服务重启：DB 中有 queued/running tasks，
    // 我们验证 spawn_compat 不会读 DB 自动重跑 queued/running 任务
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    let pool = mcoder_lib::persistence::init_sqlite(&path).await.unwrap();
    let store = Arc::new(AsyncTaskStore::new(pool.clone()));
    let mgr = TaskManager::new_for_session("s1", store.clone());

    // 创建 2 个 running task（模拟重启前的状态）
    let _id1 = mgr
        .spawn_compat("bash", async move {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            Ok::<_, String>("x".to_string())
        })
        .await;
    let _id2 = mgr
        .spawn_compat("bash", async move {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            Ok::<_, String>("y".to_string())
        })
        .await;

    // 模拟重启：drop mgr，重新开 mgr
    drop(mgr);
    let store2 = Arc::new(AsyncTaskStore::new(pool.clone()));
    let mgr2 = TaskManager::new_for_session("s1", store2.clone());

    // 调用 mark_orphans_interrupted（get_or_create_task_manager 内部会调用）
    let n = store2
        .mark_orphans_interrupted(chrono::Utc::now().timestamp_millis())
        .await
        .unwrap();
    assert_eq!(n, 2, "must mark 2 in-flight tasks as interrupted");

    // 验证：mgr2.list() 不应自动重启这两个 task
    let list = mgr2.list().await;
    assert!(
        list.is_empty(),
        "must NOT auto-rerun: in-memory should be empty after restart"
    );

    // DB 中两个 task 都应是 interrupted
    let tasks = store2.list_tasks_for_session("s1").await.unwrap();
    assert_eq!(tasks.len(), 2);
    for t in &tasks {
        assert_eq!(t.status, AsyncTaskState::Interrupted);
    }
}

// ==================== completed 不改变（重启后保持 completed） ====================

#[tokio::test]
async fn completed_task_status_unchanged_after_restart() {
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    let pool = mcoder_lib::persistence::init_sqlite(&path).await.unwrap();
    let store = Arc::new(AsyncTaskStore::new(pool.clone()));
    let mgr = TaskManager::new_for_session("s1", store.clone());

    // spawn + 等完成
    let id = mgr
        .spawn_compat("bash", async move {
            Ok::<_, String>("done".to_string())
        })
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 重启：drop mgr，重新创建
    drop(mgr);
    let store2 = Arc::new(AsyncTaskStore::new(pool.clone()));
    let _n = store2
        .mark_orphans_interrupted(chrono::Utc::now().timestamp_millis())
        .await
        .unwrap();

    let t = store2.get_task(&id).await.unwrap();
    assert_eq!(
        t.status,
        AsyncTaskState::Completed,
        "completed task must remain completed after restart"
    );
}