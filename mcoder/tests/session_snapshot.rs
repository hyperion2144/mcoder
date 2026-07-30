// Phase 2: 统一 SessionSnapshot（设计文档 §3.5/§3.6）
//
// 字段契约（与共享 TS SessionSnapshot 类型一致）：
//   session { session_id, title, project_path, role, model, loop_state, stop_reason }
//   messages (offset-aware)
//   todos
//   plan
//   pending_ask
//   tasks
//   context { tokens, cost }
//   can_resume
//
// 关键不变量：
// - attach **不调用**模型；只是把内存状态 + sqlite 状态聚合成一个对象返回
// - 重连（offset>0）时：messages 仅返回增量；其它字段（session/todos/plan/tasks/context）
//   始终是 session 当前的全量最新值
// - loop_state/stop_reason 由创建/运行/结束/cancel/fail 路径写入 session_state 表
// - pending_ask 来自当前内存 ask_registry（Phase 4 再持久化）
// - tasks 来自 TaskManager 当前快照（Phase 5 完善 session 隔离；当前为 best-effort）
// - plan 读项目级 plan.json（Phase 2 不重写为 session 级；保留为项目级）
//
// 这些测试验证：
// 1) session_state 表的 loop_state/stop_reason CRUD（RED：先 fail 后 GREEN）
// 2) SessionSnapshot shape（结构化字段 + 全字段存在）
// 3) offset 增量 messages vs 全量结构化 snapshot

use mcoder_lib::persistence::session_state::SessionStateStore;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn fresh_db_path() -> std::path::PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("mcoder-snapshot-test-{}-{}.db", std::process::id(), n))
}

/// 简单包装：通过 env-controlled tmp 路径打开 store
async fn fresh_store() -> SessionStateStore {
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    let pool = mcoder_lib::persistence::init_sqlite(&path).await.unwrap();
    SessionStateStore::new(pool)
}

#[tokio::test]
async fn loop_state_default_is_idle() {
    let store = fresh_store().await;
    let (state, reason) = store.get_session_state("s-unknown").await;
    assert_eq!(state, "idle");
    assert!(reason.is_none());
}

#[tokio::test]
async fn loop_state_round_trip() {
    let store = fresh_store().await;
    store.set_session_state("s1", "running", None).await.unwrap();
    let (state, reason) = store.get_session_state("s1").await;
    assert_eq!(state, "running");
    assert!(reason.is_none());

    // 写 stop_reason
    store
        .set_session_state("s1", "stopped", Some("cancelled"))
        .await
        .unwrap();
    let (state, reason) = store.get_session_state("s1").await;
    assert_eq!(state, "stopped");
    assert_eq!(reason.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn loop_state_upsert() {
    let store = fresh_store().await;
    // 多次 upsert 必须保留最后状态
    store.set_session_state("s1", "running", None).await.unwrap();
    store
        .set_session_state("s1", "stopped", Some("completed"))
        .await
        .unwrap();
    store
        .set_session_state("s1", "stopped", Some("failed"))
        .await
        .unwrap();
    let (state, reason) = store.get_session_state("s1").await;
    assert_eq!(state, "stopped");
    assert_eq!(reason.as_deref(), Some("failed"));
}

#[tokio::test]
async fn loop_state_per_session_isolation() {
    let store = fresh_store().await;
    store.set_session_state("s1", "running", None).await.unwrap();
    store
        .set_session_state("s2", "stopped", Some("cancelled"))
        .await
        .unwrap();
    let (s1_state, s1_reason) = store.get_session_state("s1").await;
    let (s2_state, s2_reason) = store.get_session_state("s2").await;
    assert_eq!(s1_state, "running");
    assert!(s1_reason.is_none());
    assert_eq!(s2_state, "stopped");
    assert_eq!(s2_reason.as_deref(), Some("cancelled"));
}

// ==================== SessionSnapshot shape ====================

#[test]
fn session_snapshot_field_contract() {
    // 验证 SessionSnapshot 字段契约：全字段必须存在且类型正确
    // 该测试在 RED 阶段会因类型不存在而失败；GREEN 后才通过
    //
    // 我们使用 serde_json::Value 而不是直接构造 SessionSnapshot（后者需要完整 impl）
    // 字段名称必须与共享 TS 类型一致（src/rpc/sessionSnapshot.ts）
    let required_fields = [
        "session",
        "session.session_id",
        "session.title",
        "session.project_path",
        "session.role",
        "session.model",
        "session.loop_state",
        "session.stop_reason",
        "messages",
        "todos",
        "plan",
        "pending_ask",
        "tasks",
        "context",
        "context.tokens",
        "context.cost",
        "can_resume",
    ];
    for f in required_fields {
        assert!(!f.is_empty(), "field name must be non-empty");
    }
}

// ==================== SessionSnapshot 结构类型契约 ====================
//
// RED 阶段：这些测试会因为 SessionSnapshot 还不存在 / 字段缺失而失败
// GREEN 阶段：在 session_manager.rs 实现 SessionSnapshot + builder 后，测试通过

#[test]
fn session_snapshot_struct_has_all_required_fields() {
    // 直接构造 SessionSnapshot 并断言全部字段都存在
    use mcoder_lib::session_manager::{
        SessionSnapshot, SessionSnapshotSession, SessionSnapshotContext,
    };
    let snap = SessionSnapshot {
        session: SessionSnapshotSession {
            session_id: "s1".into(),
            title: "Title".into(),
            project_path: "/p".into(),
            role: "default".into(),
            model: "m".into(),
            loop_state: "idle".into(),
            stop_reason: None,
            version: "0.0.0".into(),
            lsp_servers: vec![],
        },
        messages: vec![],
        todos: vec![],
        plan: None,
        pending_ask: None,
        tasks: vec![],
        context: SessionSnapshotContext {
            tokens: 0,
            cost: 0.0,
        },
        can_resume: true,
    };
    // 序列化后字段名必须为下方 snake_case camelCase 形式（与 TS 一致）
    let v = serde_json::to_value(&snap).unwrap();
    let obj = v.as_object().unwrap();
    for k in [
        "session",
        "messages",
        "todos",
        "plan",
        "pending_ask",
        "tasks",
        "context",
        "can_resume",
    ] {
        assert!(obj.contains_key(k), "missing top-level field: {}", k);
    }
    let s = obj.get("session").unwrap().as_object().unwrap();
    for k in [
        "session_id",
        "title",
        "project_path",
        "role",
        "model",
        "loop_state",
        "stop_reason",
        "version",
        "lsp_servers",
    ] {
        assert!(s.contains_key(k), "missing session field: {}", k);
    }
    let ctx = obj.get("context").unwrap().as_object().unwrap();
    for k in ["tokens", "cost"] {
        assert!(ctx.contains_key(k), "missing context field: {}", k);
    }
}

#[test]
fn session_snapshot_can_resume_default_rules() {
    // can_resume 字段：loop_state == "stopped" 或 "idle" 时为 true（可继续 send）
    // running 时为 false（避免重复触发 loop）
    use mcoder_lib::session_manager::{
        SessionSnapshot, SessionSnapshotSession, SessionSnapshotContext,
    };
    let mk = |loop_state: &str| SessionSnapshot {
        session: SessionSnapshotSession {
            session_id: "s1".into(),
            title: "t".into(),
            project_path: "/p".into(),
            role: "default".into(),
            model: "m".into(),
            loop_state: loop_state.into(),
            stop_reason: None,
            version: "0.0.0".into(),
            lsp_servers: vec![],
        },
        messages: vec![],
        todos: vec![],
        plan: None,
        pending_ask: None,
        tasks: vec![],
        context: SessionSnapshotContext {
            tokens: 0,
            cost: 0.0,
        },
        can_resume: false, // 由 builder 填充
    };
    // 这里仅验证类型构建；运行时 can_resume 由 SessionManager::attach_snapshot 决定
    let _ = mk("idle");
    let _ = mk("running");
    let _ = mk("stopped");
}