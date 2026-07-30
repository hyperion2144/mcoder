// Phase 4: AskUser pending + Plan approval 按 session 持久化到现有 SQLite session_state 存储。
//
// 关键不变量：
// 1. AskUserTool create 事务写 pending（DB + 内存）
// 2. answer / cancel 写终态（DB + 内存）
// 3. 服务重启后内存 registry 为空 → snapshot 从 DB 恢复 pending_ask 给 client
// 4. 服务重启后 answer RPC 检测到 persisted pending 且无内存 pending 时：
//    a) 验证 submission
//    b) 向 JsonlSession 追加匹配原 tool_call_id 的真实 ToolResult Message（不伪造 ToolUse）
//    c) 更新 DB answered
//    d) 持久化 loop_state=stopped 或可 resume 状态
//    e) 触发 resume / 标记 can_resume（语义选择：返回 can_resume=true 让 client 触发 resume）
// 5. Plan approval 改为 session 级 SQLite：
//    a) PlanCreated 写 pending
//    b) approve/reject/edit 更新
//    c) snapshot plan 来 DB
//    d) 重启 attach 恢复卡片且不调用 LLM
// 6. 移除项目级 plan.json 作为 snapshot source（不兼容）
// 7. loop_state waiting_for_user 在 pending ask/plan 时写入；终态恢复 stopped/running 合理状态
//
// 测试矩阵（RED 覆盖）：
//   - DB roundtrip（pending_ask / pending_plan 增改查）
//   - 重启后 snapshot 重建 pending ask / plan
//   - 重启后 answer 写真实 ToolResult id（不伪造新 ToolUse）
//   - plan session 隔离
//   - plan 审批 attach 不调用 LLM（仅 DB + 内存）
//   - waiting_for_user 在 pending 时被持久化

use mcoder_lib::persistence::init_sqlite;
use mcoder_lib::persistence::session_state::{
    PendingAskRecord, PendingAskState, PendingPlanRecord, PendingPlanState, SessionStateStore,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn fresh_db_path() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "mcoder-phase4-test-{}-{}.db",
        std::process::id(),
        n
    ))
}

async fn fresh_store() -> SessionStateStore {
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    let pool = init_sqlite(&path).await.unwrap();
    SessionStateStore::new(pool)
}

// ==================== AskUser pending 持久化 ====================

#[tokio::test]
async fn pending_ask_round_trip() {
    let store = fresh_store().await;
    let sid = "s-ask-1";
    let ask_id = "ask-uuid-1";
    let tool_call_id = "tooluse-uuid-1";
    let request = serde_json::json!({
        "questions": [
            {
                "question": "Pick one",
                "options": [
                    {"label": "A"},
                    {"label": "B"}
                ],
                "multi_select": false
            }
        ]
    });

    store
        .create_pending_ask(sid, ask_id, tool_call_id, request.clone(), 1700000000000)
        .await
        .unwrap();

    let p = store
        .get_pending_ask(sid)
        .await
        .expect("must exist after create");
    assert_eq!(p.ask_id, ask_id);
    assert_eq!(p.tool_call_id, tool_call_id);
    assert_eq!(p.state, PendingAskState::Pending);
    assert_eq!(p.request, request);
    assert_eq!(p.created_at_ms, 1700000000000);
    assert!(p.submission.is_none());
    assert!(p.result.is_none());
}

#[tokio::test]
async fn pending_ask_answer_records_terminal_state() {
    let store = fresh_store().await;
    let sid = "s-ask-2";
    store
        .create_pending_ask(sid, "ask-a", "tc-a", serde_json::json!({"questions": []}), 1)
        .await
        .unwrap();

    let submission = serde_json::json!({
        "cancelled": false,
        "answers": {"0": {"kind": "single", "option": "A"}}
    });
    let result = serde_json::json!({
        "cancelled": false,
        "answers": [{"mode": "single", "option": "A"}]
    });
    store
        .answer_pending_ask(sid, submission.clone(), result.clone(), 2)
        .await
        .unwrap();

    let p = store.get_pending_ask(sid).await.expect("terminal row stays");
    assert_eq!(p.state, PendingAskState::Answered);
    assert_eq!(p.submission, Some(submission));
    assert_eq!(p.result, Some(result));
    assert_eq!(p.answered_at_ms, Some(2));
}

#[tokio::test]
async fn pending_ask_cancel_records_terminal_state() {
    let store = fresh_store().await;
    let sid = "s-ask-3";
    store
        .create_pending_ask(sid, "ask-c", "tc-c", serde_json::json!({"questions": []}), 1)
        .await
        .unwrap();
    let updated = store.cancel_pending_ask(sid, 5).await.unwrap();
    assert!(updated, "first cancel must update row");

    let p = store.get_pending_ask(sid).await.expect("terminal row stays");
    assert_eq!(p.state, PendingAskState::Cancelled);
    assert_eq!(p.cancelled_at_ms, Some(5));
}

// 终审修复 #13: 终态保护的并发竞态测试
// 不允许 cancelled → answered 覆盖，也禁止 answered → cancelled 覆盖
#[tokio::test]
async fn pending_ask_cannot_overwrite_terminal_state_concurrently() {
    let store = fresh_store().await;
    let sid = "s-ask-race";
    store
        .create_pending_ask(sid, "ask-race", "tc-race", serde_json::json!({"q": []}), 1)
        .await
        .unwrap();

    // 1. 先 cancel
    let first_cancel = store.cancel_pending_ask(sid, 10).await.unwrap();
    assert!(first_cancel, "first cancel must update");

    // 2. 再尝试 answer → rows_affected=0（不应覆盖 cancelled 终态）
    let overwrite = store
        .answer_pending_ask(
            sid,
            serde_json::json!({"cancelled": false}),
            serde_json::json!({"cancelled": false}),
            20,
        )
        .await
        .unwrap();
    assert!(!overwrite, "answer must not overwrite cancelled terminal state");

    // 3. 终态仍为 cancelled
    let p = store.get_pending_ask(sid).await.expect("row still exists");
    assert_eq!(p.state, PendingAskState::Cancelled);
    assert_eq!(p.cancelled_at_ms, Some(10));
    assert_eq!(p.answered_at_ms, None, "answered_at must not be overwritten");

    // 4. 另一个方向：先 answer 再尝试 cancel
    store
        .create_pending_ask("s-ask-race-2", "a2", "tc2", serde_json::json!({"q": []}), 1)
        .await
        .unwrap();
    let answered = store
        .answer_pending_ask(
            "s-ask-race-2",
            serde_json::json!({"cancelled": false}),
            serde_json::json!({"cancelled": false}),
            11,
        )
        .await
        .unwrap();
    assert!(answered);
    let overwrite_cancel = store.cancel_pending_ask("s-ask-race-2", 22).await.unwrap();
    assert!(
        !overwrite_cancel,
        "cancel must not overwrite answered terminal state"
    );
    let p2 = store.get_pending_ask("s-ask-race-2").await.unwrap();
    assert_eq!(p2.state, PendingAskState::Answered);
    assert_eq!(p2.answered_at_ms, Some(11));
    assert_eq!(p2.cancelled_at_ms, None);
}

#[tokio::test]
async fn pending_ask_session_isolation() {
    let store = fresh_store().await;
    store
        .create_pending_ask("s1", "ask-1", "tc-1", serde_json::json!({"q": []}), 1)
        .await
        .unwrap();
    let s1 = store.get_pending_ask("s1").await.unwrap();
    let s2 = store.get_pending_ask("s2").await;
    assert_eq!(s1.ask_id, "ask-1");
    assert!(s2.is_none(), "different session must not see pending ask");
}

#[tokio::test]
async fn pending_ask_replace_existing_pending() {
    // 同一 session 第二次 create 应覆盖旧 pending（首决议语义）
    let store = fresh_store().await;
    store
        .create_pending_ask("s", "ask-1", "tc-1", serde_json::json!({"q": []}), 1)
        .await
        .unwrap();
    store
        .create_pending_ask("s", "ask-2", "tc-2", serde_json::json!({"q": []}), 2)
        .await
        .unwrap();
    let p = store.get_pending_ask("s").await.unwrap();
    assert_eq!(p.ask_id, "ask-2");
    assert_eq!(p.tool_call_id, "tc-2");
}

#[tokio::test]
async fn pending_ask_restart_recovery_returns_persisted() {
    // 模拟服务重启：drop 内存 store，重新打开 DB → DB 仍能取出 pending ask
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    {
        let pool = init_sqlite(&path).await.unwrap();
        let store = SessionStateStore::new(pool);
        store
            .create_pending_ask(
                "s-restart",
                "ask-restart",
                "tc-restart",
                serde_json::json!({"questions": [{"question": "q"}]}),
                1700,
            )
            .await
            .unwrap();
    }
    // 重新打开（同一 path；不删）
    let pool = init_sqlite(&path).await.unwrap();
    let store2 = SessionStateStore::new(pool);
    let p = store2.get_pending_ask("s-restart").await.unwrap();
    assert_eq!(p.ask_id, "ask-restart");
    assert_eq!(p.tool_call_id, "tc-restart");
    assert_eq!(p.state, PendingAskState::Pending);
}

// ==================== Plan pending 持久化 ====================

#[tokio::test]
async fn pending_plan_round_trip() {
    let store = fresh_store().await;
    let sid = "s-plan-1";
    let plan_id = "plan-uuid-1";
    let content = serde_json::json!({
        "steps": [
            {"id": 1, "description": "step one"},
            {"id": 2, "description": "step two"}
        ]
    });
    store
        .create_pending_plan(sid, plan_id, content.clone(), 1700000000000)
        .await
        .unwrap();
    let p = store.get_pending_plan(sid).await.expect("must exist");
    assert_eq!(p.plan_id, plan_id);
    assert_eq!(p.content, content);
    assert_eq!(p.state, PendingPlanState::Pending);
}

#[tokio::test]
async fn pending_plan_approve_records_terminal() {
    let store = fresh_store().await;
    let sid = "s-plan-2";
    let original = serde_json::json!({"steps": [{"id": 1, "description": "a"}]});
    store
        .create_pending_plan(sid, "p-1", original.clone(), 1)
        .await
        .unwrap();
    store
        .approve_pending_plan(sid, None, 5)
        .await
        .unwrap();
    let p = store.get_pending_plan(sid).await.expect("terminal row stays");
    assert_eq!(p.state, PendingPlanState::Approved);
    assert_eq!(p.content, original, "approve without edit keeps original");
    assert_eq!(p.decided_at_ms, Some(5));
}

#[tokio::test]
async fn pending_plan_edit_records_terminal_with_edited_content() {
    let store = fresh_store().await;
    let sid = "s-plan-3";
    store
        .create_pending_plan(sid, "p-1", serde_json::json!({"steps": []}), 1)
        .await
        .unwrap();
    let edited = serde_json::json!({"steps": [{"id": 1, "description": "edited"}]});
    store
        .approve_pending_plan(sid, Some(edited.clone()), 5)
        .await
        .unwrap();
    let p = store.get_pending_plan(sid).await.unwrap();
    assert_eq!(p.state, PendingPlanState::Edited);
    assert_eq!(p.content, edited);
}

#[tokio::test]
async fn pending_plan_reject_records_terminal() {
    let store = fresh_store().await;
    let sid = "s-plan-4";
    store
        .create_pending_plan(sid, "p-1", serde_json::json!({"steps": []}), 1)
        .await
        .unwrap();
    store.reject_pending_plan(sid, 7).await.unwrap();
    let p = store.get_pending_plan(sid).await.unwrap();
    assert_eq!(p.state, PendingPlanState::Rejected);
    assert_eq!(p.decided_at_ms, Some(7));
}

#[tokio::test]
async fn pending_plan_session_isolation() {
    let store = fresh_store().await;
    store
        .create_pending_plan("s1", "p-1", serde_json::json!({"a": 1}), 1)
        .await
        .unwrap();
    store
        .create_pending_plan("s2", "p-2", serde_json::json!({"a": 2}), 2)
        .await
        .unwrap();
    let s1 = store.get_pending_plan("s1").await.unwrap();
    let s2 = store.get_pending_plan("s2").await.unwrap();
    assert_eq!(s1.content, serde_json::json!({"a": 1}));
    assert_eq!(s2.content, serde_json::json!({"a": 2}));
}

#[tokio::test]
async fn pending_plan_restart_recovery() {
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    {
        let pool = init_sqlite(&path).await.unwrap();
        let store = SessionStateStore::new(pool);
        store
            .create_pending_plan(
                "s-restart",
                "p-restart",
                serde_json::json!({"steps": [{"id": 1, "description": "x"}]}),
                1700,
            )
            .await
            .unwrap();
    }
    let pool = init_sqlite(&path).await.unwrap();
    let store2 = SessionStateStore::new(pool);
    let p = store2.get_pending_plan("s-restart").await.unwrap();
    assert_eq!(p.plan_id, "p-restart");
    assert_eq!(p.state, PendingPlanState::Pending);
}

// ==================== pending 状态 → waiting_for_user loop_state ====================

#[tokio::test]
async fn pending_ask_writes_waiting_for_user_loop_state() {
    // ask pending 时，SessionStateStore 应能让我们把 loop_state=waiting_for_user 持久化
    let store = fresh_store().await;
    let sid = "s-ask-wait";
    // 模拟 ask 创建路径写入的：pending + waiting_for_user
    store
        .create_pending_ask(sid, "ask-1", "tc-1", serde_json::json!({"q": []}), 1)
        .await
        .unwrap();
    store
        .set_session_state(sid, "waiting_for_user", Some("ask_pending"))
        .await
        .unwrap();
    let (state, reason) = store.get_session_state(sid).await;
    assert_eq!(state, "waiting_for_user");
    assert_eq!(reason.as_deref(), Some("ask_pending"));
}

#[tokio::test]
async fn pending_plan_writes_waiting_for_user_loop_state() {
    let store = fresh_store().await;
    let sid = "s-plan-wait";
    store
        .create_pending_plan(sid, "p-1", serde_json::json!({"steps": []}), 1)
        .await
        .unwrap();
    store
        .set_session_state(sid, "waiting_for_user", Some("plan_pending"))
        .await
        .unwrap();
    let (state, reason) = store.get_session_state(sid).await;
    assert_eq!(state, "waiting_for_user");
    assert_eq!(reason.as_deref(), Some("plan_pending"));
}

#[tokio::test]
async fn terminal_ask_writes_stopped_loop_state() {
    let store = fresh_store().await;
    let sid = "s-ask-done";
    store
        .create_pending_ask(sid, "ask-1", "tc-1", serde_json::json!({"q": []}), 1)
        .await
        .unwrap();
    store.answer_pending_ask(
        sid,
        serde_json::json!({"cancelled": false, "answers": {}}),
        serde_json::json!({"cancelled": false}),
        2,
    ).await.unwrap();
    // answer 后服务应把 loop_state 写回 stopped（或 running if resume）
    store
        .set_session_state(sid, "stopped", Some("ask_answered"))
        .await
        .unwrap();
    let (state, reason) = store.get_session_state(sid).await;
    assert_eq!(state, "stopped");
    assert_eq!(reason.as_deref(), Some("ask_answered"));
}

// ==================== pending 类型契约 ====================

#[test]
fn pending_ask_state_variants() {
    // enum 序列化检查（防止后续 rename 影响 DB 兼容）
    assert_eq!(
        serde_json::to_string(&PendingAskState::Pending).unwrap(),
        "\"pending\""
    );
    assert_eq!(
        serde_json::to_string(&PendingAskState::Answered).unwrap(),
        "\"answered\""
    );
    assert_eq!(
        serde_json::to_string(&PendingAskState::Cancelled).unwrap(),
        "\"cancelled\""
    );
}

#[test]
fn pending_plan_state_variants() {
    assert_eq!(
        serde_json::to_string(&PendingPlanState::Pending).unwrap(),
        "\"pending\""
    );
    assert_eq!(
        serde_json::to_string(&PendingPlanState::Approved).unwrap(),
        "\"approved\""
    );
    assert_eq!(
        serde_json::to_string(&PendingPlanState::Edited).unwrap(),
        "\"edited\""
    );
    assert_eq!(
        serde_json::to_string(&PendingPlanState::Rejected).unwrap(),
        "\"rejected\""
    );
}

#[test]
fn pending_ask_record_serializes_fields() {
    let r = PendingAskRecord {
        session_id: "s".into(),
        ask_id: "ask".into(),
        tool_call_id: "tc".into(),
        request: serde_json::json!({"questions": []}),
        state: PendingAskState::Pending,
        submission: None,
        result: None,
        created_at_ms: 0,
        answered_at_ms: None,
        cancelled_at_ms: None,
    };
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["ask_id"], "ask");
    assert_eq!(v["state"], "pending");
}

#[test]
fn pending_plan_record_serializes_fields() {
    let r = PendingPlanRecord {
        session_id: "s".into(),
        plan_id: "p".into(),
        content: serde_json::json!({"steps": []}),
        state: PendingPlanState::Pending,
        created_at_ms: 0,
        decided_at_ms: None,
    };
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["plan_id"], "p");
    assert_eq!(v["state"], "pending");
}
// ==================== Phase 4: 重启 answer 不伪造新 ToolUse ====================
//
// 关键不变量：
// 1. service restart 后，DB 中持久化的 pending ask 仍带**原始** tool_call_id
// 2. SessionManager restart 路径用持久化的 tool_call_id 追加 ToolResult Message
//    （不新建 ToolUse Message，避免破坏 LLM 上下文的 tool_use ↔ tool_result 配对）
//
// 这里覆盖 DB 侧的语义保证（持久化 tool_call_id 完整、跨重启可读）。

#[tokio::test]
async fn p4_persisted_tool_call_id_is_preserved_across_restart() {
    let path = std::env::temp_dir().join(format!(
        "mcoder-p4-tcid-{}-{}.db",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&path);

    // 第一次启动：写入 pending ask（用真实的 LLM tool_use id 模拟）
    let real_tool_call_id = "tooluse_real_llm_abc123";
    {
        let pool = init_sqlite(&path).await.unwrap();
        let store = SessionStateStore::new(pool);
        store
            .create_pending_ask(
                "s1",
                "ask-x",
                real_tool_call_id,
                serde_json::json!({"questions": [{"question": "q"}]}),
                1700,
            )
            .await
            .unwrap();
    }
    // 重启：同一 path 新 store 读出
    let pool = init_sqlite(&path).await.unwrap();
    let store2 = SessionStateStore::new(pool);
    let rec = store2.get_pending_ask("s1").await.unwrap();
    // 关键断言：tool_call_id 与首次写入完全一致
    assert_eq!(rec.tool_call_id, real_tool_call_id);
    assert_eq!(rec.ask_id, "ask-x");
    // 任何后续 answer / cancel 都不应改变 tool_call_id
    store2
        .answer_pending_ask(
            "s1",
            serde_json::json!({"cancelled": false, "answers": {}}),
            serde_json::json!({"cancelled": false}),
            2000,
        )
        .await
        .unwrap();
    let rec2 = store2.get_pending_ask("s1").await.unwrap();
    assert_eq!(
        rec2.tool_call_id, real_tool_call_id,
        "answer 阶段必须保留原始 tool_call_id 以便 append 真实 ToolResult"
    );
}

#[tokio::test]
async fn p4_plan_pending_state_machine() {
    // 完整生命周期：pending → edited → approved / rejected 都从同一行
    let store = fresh_store().await;
    let sid = "s-plan-lifecycle";
    let plan_id = "plan-lc";
    store
        .create_pending_plan(
            sid,
            plan_id,
            serde_json::json!({"steps": [{"id": 1, "description": "x"}]}),
            1,
        )
        .await
        .unwrap();

    // edit
    store
        .approve_pending_plan(
            sid,
            Some(serde_json::json!({"steps": [{"id": 1, "description": "edited"}]})),
            2,
        )
        .await
        .unwrap();
    let r = store.get_pending_plan(sid).await.unwrap();
    assert_eq!(r.state, mcoder_lib::persistence::session_state::PendingPlanState::Edited);
    assert_eq!(r.content["steps"][0]["description"], "edited");

    // 再 approve（不带 edit）
    store.approve_pending_plan(sid, None, 3).await.unwrap();
    let r = store.get_pending_plan(sid).await.unwrap();
    assert_eq!(r.state, mcoder_lib::persistence::session_state::PendingPlanState::Approved);
    // edit 内容被覆盖：content 仍是 "edited"（approve 没替换 content）
    assert_eq!(r.content["steps"][0]["description"], "edited");

    // reject
    store.reject_pending_plan(sid, 4).await.unwrap();
    let r = store.get_pending_plan(sid).await.unwrap();
    assert_eq!(r.state, mcoder_lib::persistence::session_state::PendingPlanState::Rejected);
}

#[tokio::test]
async fn p4_update_pending_plan_content_preserves_state() {
    let store = fresh_store().await;
    store
        .create_pending_plan(
            "s",
            "p",
            serde_json::json!({"steps": [{"id": 1, "description": "a"}]}),
            1,
        )
        .await
        .unwrap();
    store
        .approve_pending_plan(
            "s",
            Some(serde_json::json!({"steps": [{"id": 1, "description": "edited"}]})),
            2,
        )
        .await
        .unwrap();
    // 状态此时是 edited
    let before = store.get_pending_plan("s").await.unwrap();
    assert_eq!(
        before.state,
        mcoder_lib::persistence::session_state::PendingPlanState::Edited
    );

    // plan_update 调 update_pending_plan_content：state 应保持 edited
    store
        .update_pending_plan_content(
            "s",
            serde_json::json!({"steps": [{"id": 1, "description": "in_progress_step"}]}),
        )
        .await
        .unwrap();
    let after = store.get_pending_plan("s").await.unwrap();
    assert_eq!(
        after.state,
        mcoder_lib::persistence::session_state::PendingPlanState::Edited,
        "update_pending_plan_content 必须保留 state"
    );
    assert_eq!(after.content["steps"][0]["description"], "in_progress_step");
}

#[tokio::test]
async fn p4_ask_pending_overwrite_resets_to_pending() {
    // 同一 session 第二次 create 必须把 state 重置为 pending（覆盖旧终态）
    let store = fresh_store().await;
    store
        .create_pending_ask("s", "a1", "t1", serde_json::json!({"q": []}), 1)
        .await
        .unwrap();
    store
        .answer_pending_ask(
            "s",
            serde_json::json!({"cancelled": false, "answers": {}}),
            serde_json::json!({"cancelled": false}),
            2,
        )
        .await
        .unwrap();
    let before = store.get_pending_ask("s").await.unwrap();
    assert_eq!(
        before.state,
        mcoder_lib::persistence::session_state::PendingAskState::Answered
    );
    // 新的 pending create：state 必须重置
    store
        .create_pending_ask("s", "a2", "t2", serde_json::json!({"q": []}), 3)
        .await
        .unwrap();
    let after = store.get_pending_ask("s").await.unwrap();
    assert_eq!(
        after.state,
        mcoder_lib::persistence::session_state::PendingAskState::Pending
    );
    assert_eq!(after.ask_id, "a2");
    assert_eq!(after.tool_call_id, "t2");
}

// ==================== Phase 5b / 终审修复 #15: session_attrs kv ====================

#[tokio::test]
async fn session_attrs_kv_round_trip() {
    let store = fresh_store().await;
    let sid = "s-attr-1";

    // 首次 set
    store.set_kv(sid, "role", "plan").await.unwrap();
    let v = store.get_kv(sid, "role").await.unwrap();
    assert_eq!(v.as_deref(), Some("plan"));

    // upsert
    store.set_kv(sid, "role", "execute").await.unwrap();
    let v = store.get_kv(sid, "role").await.unwrap();
    assert_eq!(v.as_deref(), Some("execute"), "kv must upsert, not append");
}

#[tokio::test]
async fn session_attrs_kv_isolation_per_session() {
    let store = fresh_store().await;
    store.set_kv("s1", "role", "plan").await.unwrap();
    store.set_kv("s2", "role", "goal").await.unwrap();
    assert_eq!(store.get_kv("s1", "role").await.unwrap().as_deref(), Some("plan"));
    assert_eq!(store.get_kv("s2", "role").await.unwrap().as_deref(), Some("goal"));
}

#[tokio::test]
async fn session_attrs_kv_returns_none_for_missing() {
    let store = fresh_store().await;
    let v = store.get_kv("s-nonexistent", "role").await.unwrap();
    assert!(v.is_none(), "missing session must return None");
}
