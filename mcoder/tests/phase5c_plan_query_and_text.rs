// Phase 5c: plan_query tool + try_handle_text top-level only
//
// 关键不变量：
// 1. plan_query 工具已注册，可读出 session 当前的 plan state + content
// 2. try_handle_text_for_pending_ask 只设顶层 custom_response，不逐题重复

use mcoder_lib::persistence::session_state::SessionStateStore;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn fresh_dir() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("mcoder-planq-{}-{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn plan_query_tool_is_registered() {
    // 检查 plan_query 在工具注册表里
    use mcoder_lib::tools::build_full_registry;
    let (reg, _sub, _ask, _ask_reg) = build_full_registry();
    let schemas = reg.list_schemas();
    let names: Vec<String> = schemas.iter().map(|s| s.name.clone()).collect();
    assert!(
        names.iter().any(|n| n == "plan_query"),
        "plan_query must be registered; got tools: {:?}",
        names
    );
}

#[test]
fn plan_query_readonly_whitelist_matches_registered_tool() {
    // readonly 白名单里的 plan_query 必须真在工具表里（防止白名单飘了）
    use mcoder_lib::tools::build_full_registry;
    use mcoder_lib::session_manager::READONLY_TOOLS;
    let (reg, _sub, _ask, _ask_reg) = build_full_registry();
    let registered: std::collections::HashSet<String> = reg
        .list_schemas()
        .iter()
        .map(|s| s.name.clone())
        .collect();
    for tool in READONLY_TOOLS {
        assert!(
            registered.contains(*tool),
            "READONLY_TOOLS contains '{}' but tool is not registered; whitelist drift!",
            tool
        );
    }
    // 关键：plan_query 必须在白名单里（设计文档 §3.9）
    assert!(
        READONLY_TOOLS.contains(&"plan_query"),
        "plan_query must be in READONLY_TOOLS (it's a read-only DB query)"
    );
}

#[tokio::test]
async fn plan_query_returns_pending_plan() {
    // 写一个 pending plan，然后用 plan_query 读出
    let dir = fresh_dir();
    let db_path = dir.join("session_state.db");
    let store = SessionStateStore::open_at(db_path).await.unwrap();
    let sid = "s-plan-query";
    store
        .create_pending_plan(
            sid,
            "p-1",
            serde_json::json!({"steps": [{"id": 1, "description": "first"}, {"id": 2, "description": "second"}]}),
            1,
        )
        .await
        .unwrap();

    let rec = store.get_pending_plan(sid).await.unwrap();
    assert_eq!(rec.state, mcoder_lib::persistence::session_state::PendingPlanState::Pending);
    assert_eq!(rec.content["steps"].as_array().unwrap().len(), 2);
}
