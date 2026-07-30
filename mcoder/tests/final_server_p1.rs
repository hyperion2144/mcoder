use mcoder_lib::agent::async_tasks::{TaskManager, TaskStatus};
use mcoder_lib::ask_user::{
    AskOption, AskQuestion, AskQuestionAnswer, AskRegistry, AskRequest, AskSubmission,
};
use mcoder_lib::persistence::async_task_store::{AsyncTaskState, AsyncTaskStore};
use mcoder_lib::persistence::session_state::{PendingAskState, SessionStateStore};
use std::collections::HashMap;
use std::sync::Arc;

fn request() -> AskRequest {
    AskRequest {
        questions: vec![AskQuestion {
            question: "Pick".into(),
            header: None,
            options: vec![
                AskOption { label: "A".into(), description: None },
                AskOption { label: "B".into(), description: None },
            ],
            multi_select: Some(false),
        }],
    }
}

async fn store_at(path: &std::path::Path) -> Arc<SessionStateStore> {
    let pool = mcoder_lib::persistence::init_sqlite(path).await.unwrap();
    Arc::new(SessionStateStore::new(pool))
}

#[tokio::test]
async fn ask_registry_resolves_store_per_session_and_recovers_after_restart() {
    let root = std::env::temp_dir().join(format!(
        "mcoder-final-p1-ask-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let store_a = store_at(&root.join("project-a.db")).await;
    let store_b = store_at(&root.join("project-b.db")).await;

    let stores = Arc::new(HashMap::from([
        ("session-a".to_string(), store_a.clone()),
        ("session-b".to_string(), store_b.clone()),
    ]));
    let resolver = {
        let stores = stores.clone();
        move |session_id: String| {
            let stores = stores.clone();
            async move {
                stores
                    .get(&session_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("missing store for {session_id}"))
            }
        }
    };
    let registry = Arc::new(AskRegistry::with_store_resolver(resolver));

    let (ask_a, _) = registry
        .create_persisted("session-a", "tool-a", request())
        .await
        .unwrap();
    let (_ask_b, _) = registry
        .create_persisted("session-b", "tool-b", request())
        .await
        .unwrap();

    let mut answers = HashMap::new();
    answers.insert(
        0,
        AskQuestionAnswer::Single { option: "A".into(), note: None },
    );
    registry
        .submit_validated(
            "session-a",
            &ask_a.ask_id,
            &request(),
            AskSubmission { cancelled: false, answers, custom_response: None },
        )
        .await
        .unwrap();
    registry.cancel("session-b").await.unwrap();

    assert_eq!(
        store_a.get_pending_ask("session-a").await.unwrap().state,
        PendingAskState::Answered
    );
    assert!(store_a.get_pending_ask("session-b").await.is_none());
    assert_eq!(
        store_b.get_pending_ask("session-b").await.unwrap().state,
        PendingAskState::Cancelled
    );
    assert!(store_b.get_pending_ask("session-a").await.is_none());

    let stores_after_restart = stores.clone();
    let restarted = AskRegistry::with_store_resolver(move |session_id: String| {
        let stores = stores_after_restart.clone();
        async move {
            stores
                .get(&session_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing store for {session_id}"))
        }
    });
    let recovered_a = restarted
        .store_for_session("session-a")
        .await
        .unwrap()
        .unwrap()
        .get_pending_ask("session-a")
        .await
        .unwrap();
    let recovered_b = restarted
        .store_for_session("session-b")
        .await
        .unwrap()
        .unwrap()
        .get_pending_ask("session-b")
        .await
        .unwrap();
    assert_eq!(recovered_a.state, PendingAskState::Answered);
    assert_eq!(recovered_b.state, PendingAskState::Cancelled);
}

#[tokio::test]
async fn concurrent_pool_reuse_normalizes_path_and_initializes_once() {
    let root = std::env::temp_dir().join(format!(
        "mcoder-final-p1-pool-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(root.join("alias")).unwrap();
    let canonical = root.join("session_state.db");
    let alias = root.join("alias").join("..").join("session_state.db");

    let mut opens = Vec::new();
    for index in 0..24 {
        let path = if index % 2 == 0 {
            canonical.clone()
        } else {
            alias.clone()
        };
        opens.push(tokio::spawn(async move {
            SessionStateStore::open_at(path).await.expect("open shared pool")
        }));
    }
    let mut stores = Vec::new();
    for open in opens {
        stores.push(open.await.unwrap());
    }

    stores[0].pool().close().await;
    assert!(
        stores.iter().all(|store| store.pool().is_closed()),
        "all concurrent normalized opens must clone the same SqlitePool"
    );
}

#[tokio::test]
async fn resume_heal_stopped_when_waiting_has_no_pending_ask_or_plan() {
    use mcoder_lib::resume_policy::{decide_resume, ResumeDecisionKind};
    assert_eq!(
        decide_resume(false, "waiting_for_user", Some("ask_pending"), 0, false, true),
        ResumeDecisionKind::WaitingForUser
    );
    assert_eq!(
        decide_resume(false, "waiting_for_user", Some("ask_pending"), 0, false, false),
        ResumeDecisionKind::HealStopped
    );
}

#[tokio::test]
async fn ask_terminal_failure_does_not_wake_or_mutate_state() {
    let root = std::env::temp_dir().join(format!(
        "mcoder-final-p1-ask-terminal-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("session_state.db");
    let pool = mcoder_lib::persistence::init_sqlite(&db_path).await.unwrap();
    let store = Arc::new(SessionStateStore::new(pool));

    let registry = Arc::new(AskRegistry::with_store_resolver({
        let store = store.clone();
        move |session_id: String| {
            let store = store.clone();
            async move { Ok::<_, anyhow::Error>(store.clone()) }
        }
    }));
    let (pending, _) = registry
        .create_persisted("s", "tc", request())
        .await
        .unwrap();
    assert_eq!(
        store.get_pending_ask("s").await.unwrap().state,
        PendingAskState::Pending
    );

    // 关掉 store 模拟 IO 失败
    store.pool().close().await;
    let mut answers = HashMap::new();
    answers.insert(
        0,
        AskQuestionAnswer::Single { option: "A".into(), note: None },
    );
    let result = registry
        .submit_validated(
            "s",
            &pending.ask_id,
            &request(),
            AskSubmission { cancelled: false, answers, custom_response: None },
        )
        .await;
    assert!(result.is_err());
    // 内存 pending 仍在 → 不会被唤醒成首决议
    assert!(registry.peek("s").await.is_some());
}

#[tokio::test]
async fn startup_orphan_sweep_isolates_and_marks_only_running_tasks() {
    let dir = std::env::temp_dir().join(format!(
        "mcoder-final-p1-startup-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let project_a = dir.join("project-a");
    let project_b = dir.join("project-b");
    std::fs::create_dir_all(&project_a).unwrap();
    std::fs::create_dir_all(&project_b).unwrap();

    for project in [&project_a, &project_b] {
        let db_path = project.join(".mcoder").join("session_state.db");
        let _ = std::fs::remove_file(&db_path);
        let pool = mcoder_lib::persistence::init_sqlite(&db_path).await.unwrap();
        let store = AsyncTaskStore::new(pool);
        let running = store
            .create_task(
                "session",
                "bash",
                serde_json::json!({"cmd": "echo"}),
                1,
            )
            .await
            .unwrap();
        let completed = store
            .create_task("session", "bash", serde_json::json!({}), 2)
            .await
            .unwrap();
        store
            .complete_task(&completed.task_id, serde_json::json!({"ok": true}), 3)
            .await
            .unwrap();
        // 模拟上次周期未清理的 running task
        let _ = running;
    }

    // 通过 SessionStateStore 验证两条 session_state.db 都可达
    for project in [&project_a, &project_b] {
        let db_path = project.join(".mcoder").join("session_state.db");
        let pool = mcoder_lib::persistence::init_sqlite(&db_path).await.unwrap();
        let store = AsyncTaskStore::new(pool);
        let count = store
            .mark_orphans_interrupted(chrono::Utc::now().timestamp_millis())
            .await
            .unwrap();
        assert_eq!(count, 1, "each project must lose only its running task");
    }
}

#[tokio::test]
async fn task_spawn_db_failure_returns_error_without_running_future() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let manager = TaskManager::new_for_session("s-broken", Arc::new(AsyncTaskStore::new(pool)));
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ran_in_task = ran.clone();

    let result = manager
        .spawn_compat("bash", async move {
            ran_in_task.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, String>("should not run".into())
        })
        .await;

    assert!(result.is_err(), "missing async_tasks table must fail spawn");
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    assert!(manager.list().await.is_empty());
}

#[tokio::test]
async fn async_task_terminal_transition_is_first_writer_wins() {
    let path = std::env::temp_dir().join(format!(
        "mcoder-final-p1-task-race-{}-{}.db",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let pool = mcoder_lib::persistence::init_sqlite(&path).await.unwrap();
    let store = Arc::new(AsyncTaskStore::new(pool));
    let task = store
        .create_task("s1", "bash", serde_json::json!({}), 1)
        .await
        .unwrap();

    let completed_store = store.clone();
    let failed_store = store.clone();
    let completed_id = task.task_id.clone();
    let failed_id = task.task_id.clone();
    let (completed, failed) = tokio::join!(
        completed_store.complete_task(&completed_id, serde_json::json!("ok"), 2),
        failed_store.fail_task(&failed_id, "boom", 3),
    );
    let completed = completed.unwrap();
    let failed = failed.unwrap();
    assert_ne!(completed, failed, "exactly one terminal transition must win");

    let record = store.get_task(&task.task_id).await.unwrap();
    match record.status {
        AsyncTaskState::Completed => {
            assert!(completed);
            assert_eq!(record.output_json, Some(serde_json::json!("ok")));
        }
        AsyncTaskState::Failed => {
            assert!(failed);
            assert_eq!(record.error.as_deref(), Some("boom"));
        }
        other => panic!("unexpected terminal state: {other:?}"),
    }

    assert!(!store.cancel_task(&task.task_id, 4).await.unwrap());
    assert_eq!(store.get_task(&task.task_id).await.unwrap().status, record.status);
}

#[tokio::test]
async fn task_manager_memory_follows_database_terminal_decision() {
    let path = std::env::temp_dir().join(format!(
        "mcoder-final-p1-manager-race-{}-{}.db",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let pool = mcoder_lib::persistence::init_sqlite(&path).await.unwrap();
    let store = Arc::new(AsyncTaskStore::new(pool));
    let manager = TaskManager::new_for_session("s1", store.clone());
    let release = Arc::new(tokio::sync::Notify::new());
    let release_task = release.clone();
    let id = manager
        .spawn_compat("bash", async move {
            release_task.notified().await;
            Ok::<_, String>("late completion".into())
        })
        .await
        .unwrap();

    assert!(manager.cancel(&id).await.unwrap());
    release.notify_one();
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert_eq!(store.get_task(&id).await.unwrap().status, AsyncTaskState::Cancelled);
    assert_eq!(manager.get_status(&id).await, Some(TaskStatus::Cancelled));
}
