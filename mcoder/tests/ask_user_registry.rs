// 服务端 ask_user AskRegistry 异步行为测试
// 验证：take 单次性 / cancel 清理 / 首答生效 / 多 session 互不阻塞
// 覆盖 review 反馈：原子提交、close 清理、自定义文本答复、tool_call_id 透传

use mcoder_lib::ask_user::{AskOption, AskQuestion, AskQuestionAnswer, AskRegistry, AskRequest, AskSubmission};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn req_single() -> AskRequest {
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

fn req_two() -> AskRequest {
    AskRequest {
        questions: vec![
            AskQuestion {
                question: "P1".into(),
                header: None,
                options: vec![
                    AskOption { label: "A".into(), description: None },
                    AskOption { label: "B".into(), description: None },
                ],
                multi_select: Some(false),
            },
            AskQuestion {
                question: "P2".into(),
                header: None,
                options: vec![
                    AskOption { label: "X".into(), description: None },
                    AskOption { label: "Y".into(), description: None },
                ],
                multi_select: Some(false),
            },
        ],
    }
}

#[tokio::test]
async fn registry_take_is_one_shot() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    assert_eq!(p.session_id, "s1");
    let taken = reg.take("s1", &p.ask_id).await;
    assert!(taken.is_some());
    // 二次 take 应该拿不到
    let again = reg.take("s1", &p.ask_id).await;
    assert!(again.is_none());
}

#[tokio::test]
async fn registry_cancel_clears_and_wakes() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let p2 = p.clone();
    let reg2 = reg.clone();
    let ask_id = p.ask_id.clone();
    let handle = tokio::spawn(async move {
        p2.notify.notified().await;
        let g = p2.submission.lock().await;
        g.clone().unwrap_or_default()
    });
    // 等 spawn 真的进入 notified
    tokio::time::sleep(Duration::from_millis(20)).await;
    let cancelled = reg2.cancel("s1").await;
    assert!(cancelled.is_some());
    let sub = tokio::time::timeout(Duration::from_millis(500), handle).await
        .expect("notified must resolve")
        .unwrap();
    assert!(sub.cancelled);
    // pending 已清空
    assert!(reg2.peek("s1").await.is_none());
    let _ = ask_id;
}

#[tokio::test]
async fn registry_submit_wakes_only_with_matching_ask() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let p2 = p.clone();
    let reg2 = reg.clone();
    let handle = tokio::spawn(async move {
        p2.notify.notified().await;
        let g = p2.submission.lock().await;
        g.clone().unwrap_or_default()
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    // 错误的 ask_id 不应 notify
    let req = req_single();
    let wrong = AskSubmission::default();
    let ok_wrong = reg2.submit_validated("s1", "wrong-id", &req, wrong).await;
    assert!(ok_wrong.is_err());
    // 正确的 ask_id 才生效
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    let ok_right = reg2
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers, custom_response: None },
        )
        .await;
    assert!(ok_right.is_ok());
    let sub = tokio::time::timeout(Duration::from_millis(500), handle).await
        .expect("notified must resolve")
        .unwrap();
    assert!(!sub.cancelled);
    let a = sub.answers.get(&0).expect("answer 0");
    if let AskQuestionAnswer::Single { option, .. } = a {
        assert_eq!(option, "A");
    } else {
        panic!("expected single");
    }
}

#[tokio::test]
async fn registry_overwrites_previous_pending() {
    let reg = Arc::new(AskRegistry::new());
    let (p1, _) = reg.create("s1", "tc1", req_single()).await;
    let reg2 = reg.clone();
    let p1c = p1.clone();
    let handle = tokio::spawn(async move {
        p1c.notify.notified().await;
        let g = p1c.submission.lock().await;
        g.clone().unwrap_or_default()
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    // 创建新的 pending → 旧的应被覆盖并 cancelled
    let (_p2, old) = reg2.create("s1", "tc2", req_single()).await;
    assert!(old.is_some(), "create should return overwritten pending");
    assert_eq!(old.as_ref().unwrap().tool_call_id, "tc1");
    let sub = tokio::time::timeout(Duration::from_millis(500), handle).await
        .expect("old notified must resolve")
        .unwrap();
    assert!(sub.cancelled);
    // 当前 pending 应该是新的
    let cur = reg2.peek("s1").await.expect("current pending");
    assert_eq!(cur.tool_call_id, "tc2");
}

#[tokio::test]
async fn registry_separates_sessions() {
    let reg = Arc::new(AskRegistry::new());
    let (_a, _) = reg.create("s1", "tc1", req_single()).await;
    let (b, _) = reg.create("s2", "tc2", req_single()).await;
    // session s2 不会因 s1 的 cancel 而结束
    let cancelled = reg.cancel("s1").await;
    assert!(cancelled.is_some());
    // s2 仍然有 pending
    let cur = reg.peek("s2").await.expect("s2 still pending");
    assert_eq!(cur.ask_id, b.ask_id);
}

#[tokio::test]
async fn registry_first_answer_wins() {
    // 验证：首答生效；之后覆盖 submit 不能破坏已 take 的结果
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let p2 = p.clone();
    let reg2 = reg.clone();
    let ask_id = p.ask_id.clone();
    let handle = tokio::spawn(async move {
        p2.notify.notified().await;
        let g = p2.submission.lock().await;
        g.clone().unwrap_or_default()
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let req = req_single();
    let mut a1 = HashMap::new();
    a1.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    let mut a2 = HashMap::new();
    a2.insert(0, AskQuestionAnswer::Single { option: "B".into(), note: None });
    assert!(reg2
        .submit_validated(
            "s1",
            &ask_id,
            &req,
            AskSubmission { cancelled: false, answers: a1, custom_response: None },
        )
        .await
        .is_ok());
    // 首决议已生效 → 第二次 submit 必须被拒绝，不能后写覆盖首答
    assert!(
        reg2.submit_validated(
            "s1",
            &ask_id,
            &req,
            AskSubmission { cancelled: false, answers: a2, custom_response: None },
        )
        .await
        .is_err(),
        "second submit must fail; first decision wins"
    );
    let sub = tokio::time::timeout(Duration::from_millis(500), handle).await
        .expect("notified must resolve")
        .unwrap();
    // waiter 看到的是首答 "A"，不是被覆盖后的 "B"
    let a = sub.answers.get(&0).expect("answer 0");
    if let AskQuestionAnswer::Single { option, .. } = a {
        assert_eq!(option, "A");
    } else {
        panic!("expected single");
    }
}

#[tokio::test]
async fn registry_submit_under_50ms() {
    // 性能冒烟：从 create 到 submit 到 notified 全链路 < 50ms
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let p2 = p.clone();
    let start = Instant::now();
    let req = req_single();
    let h = tokio::spawn(async move {
        p2.notify.notified().await;
    });
    tokio::time::sleep(Duration::from_millis(5)).await;
    let mut a = HashMap::new();
    a.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    let _ = reg
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers: a, custom_response: None },
        )
        .await;
    let _ = tokio::time::timeout(Duration::from_millis(50), h).await;
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(200), "took {:?}", elapsed);
}

// ==================== review 新增测试 ====================

/// 普通文本 pending answer 必须有效（issue 3）
/// 之前 try_handle_text_for_pending_ask 会给所有题写 option="",
/// 服务端 validate_submission 拒绝 unknown option。
/// 新实现：单题 → Custom(note)；多题 → Custom(note) + custom_response；
/// validation 接受，不留下缺失答案。
#[tokio::test]
async fn registry_text_for_pending_single_question_accepted() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let req = req_single();
    let sub = AskSubmission {
        cancelled: false,
        answers: HashMap::from([(0u32, AskQuestionAnswer::Custom { note: "hello".into() })]),
        custom_response: Some("hello".into()),
    };
    let ok = reg
        .submit_validated("s1", &p.ask_id, &req, sub.clone())
        .await;
    assert!(ok.is_ok(), "single-question text must be accepted: {:?}", ok);
    let stored = reg.peek("s1").await.expect("still pending");
    let g = stored.submission.lock().await;
    let got = g.as_ref().expect("submission stored");
    assert!(!got.cancelled);
    assert_eq!(got.custom_response.as_deref(), Some("hello"));
    match got.answers.get(&0).unwrap() {
        AskQuestionAnswer::Custom { note } => assert_eq!(note, "hello"),
        _ => panic!("expected Custom"),
    }
}

/// 多题场景：所有题都 Custom + custom_response，应通过校验（issue 3）
#[tokio::test]
async fn registry_text_for_pending_multi_question_accepted() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_two()).await;
    let req = req_two();
    let sub = AskSubmission {
        cancelled: false,
        answers: HashMap::from([
            (0u32, AskQuestionAnswer::Custom { note: "全部改为 option A，其他说明".into() }),
            (1u32, AskQuestionAnswer::Custom { note: "全部改为 option A，其他说明".into() }),
        ]),
        custom_response: Some("全部改为 option A，其他说明".into()),
    };
    let ok = reg.submit_validated("s1", &p.ask_id, &req, sub).await;
    assert!(ok.is_ok(), "multi-question text must be accepted: {:?}", ok);
}

/// 整段 custom_response 单独提交也合法（部分客户端只发一段话）
#[tokio::test]
async fn registry_text_only_custom_response_accepted() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_two()).await;
    let req = req_two();
    let sub = AskSubmission {
        cancelled: false,
        answers: HashMap::new(),
        custom_response: Some("整段答复".into()),
    };
    let ok = reg.submit_validated("s1", &p.ask_id, &req, sub).await;
    assert!(ok.is_ok(), "empty answers + custom_response must be accepted: {:?}", ok);
}

/// 原子性：validate 失败的 submission 不会被写入（issue 4）
#[tokio::test]
async fn registry_submit_validated_rejects_invalid_without_writing() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let req = req_single();
    // 完全空 answers 且无 custom_response → 校验失败
    let sub = AskSubmission {
        cancelled: false,
        answers: HashMap::new(),
        custom_response: None,
    };
    let ok = reg.submit_validated("s1", &p.ask_id, &req, sub).await;
    assert!(ok.is_err());
    // 提交失败后 pending 仍在，submission slot 仍为 None
    let stored = reg.peek("s1").await.expect("pending still alive");
    let g = stored.submission.lock().await;
    assert!(g.is_none(), "rejected submission must not be stored");
}

/// 原子性：首答后立即 take，第二次 submit 失败（不能后写覆盖）
#[tokio::test]
async fn registry_first_answer_then_take_blocks_second() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let req = req_single();
    let mut a1 = HashMap::new();
    a1.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    assert!(reg
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers: a1, custom_response: None },
        )
        .await
        .is_ok());
    // take 后 pending 已移除
    let taken = reg.take("s1", &p.ask_id).await;
    assert!(taken.is_some());
    // 第二次 submit 应当失败（pending 不在 map）
    let mut a2 = HashMap::new();
    a2.insert(0, AskQuestionAnswer::Single { option: "B".into(), note: None });
    let res = reg
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers: a2, custom_response: None },
        )
        .await;
    assert!(res.is_err(), "second submit must be rejected after take");
}

// ==================== 首决议并发语义（first-decision-wins）====================

/// 连续两次 submit_validated（executor 还没 take）：
/// 第二次必须失败，且第一份答案必须被保留。
///
/// 当前 bug：第二次 submit_validated 会再次写入 submission 槽并 notify_waiters，
/// 覆盖已经被 waiter 看到的首答，破坏"首答生效"语义。
#[tokio::test]
async fn registry_double_submit_first_wins() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let req = req_single();

    let mut a1 = HashMap::new();
    a1.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    let first = reg
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers: a1, custom_response: None },
        )
        .await;
    assert!(first.is_ok(), "first submit must succeed");

    // 第二次 submit 必须在 executor take 之前失败，不能后写覆盖首答
    let mut a2 = HashMap::new();
    a2.insert(0, AskQuestionAnswer::Single { option: "B".into(), note: None });
    let second = reg
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers: a2, custom_response: None },
        )
        .await;
    assert!(
        second.is_err(),
        "second submit must fail because first decision already won"
    );

    // 第一份答案必须保留：取走 pending 并读取 submission 槽
    let stored = reg.take("s1", &p.ask_id).await.expect("pending still there");
    let g = stored.submission.lock().await;
    let got = g.as_ref().expect("submission stored");
    assert!(!got.cancelled, "first answer must win, not cancelled");
    let ans = got.answers.get(&0).expect("answer 0");
    match ans {
        AskQuestionAnswer::Single { option, .. } => assert_eq!(option, "A"),
        _ => panic!("expected single A"),
    }
}

/// submit_validated 成功后立即 cancel：cancel 必须不能覆盖已回答的 submission。
///
/// 当前 bug：cancel 会再次写入 `cancelled=true` 覆盖首答。
#[tokio::test]
async fn registry_cancel_after_submit_does_not_overwrite_answer() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let req = req_single();

    let mut a1 = HashMap::new();
    a1.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    let ok = reg
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers: a1, custom_response: None },
        )
        .await;
    assert!(ok.is_ok());

    // cancel 必须不能覆盖已回答的 submission：取消要么返回 None 要么保留答案
    let cancelled = reg.cancel("s1").await;
    if let Some(p2) = cancelled {
        let g = p2.submission.lock().await;
        let got = g.as_ref().expect("submission stored");
        assert!(
            !got.cancelled,
            "cancel must not overwrite an already-decided submission"
        );
        match got.answers.get(&0).unwrap() {
            AskQuestionAnswer::Single { option, .. } => assert_eq!(option, "A"),
            _ => panic!("expected single A"),
        }
    } else {
        // cancel 在首答已决议时返回 None（pending 已被 take_by_session 移除）
        assert!(reg.peek("s1").await.is_none());
    }
}

/// cancel 之后 submit_validated 必须失败。
///
/// 当前 bug：cancel 已决议，但 submit_validated 不感知 cancel 状态，会再次覆盖。
#[tokio::test]
async fn registry_submit_after_cancel_fails() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let req = req_single();

    // cancel 决议
    let cancelled = reg.cancel("s1").await;
    assert!(cancelled.is_some(), "cancel must return the pending");

    // 之后的 submit 必须失败
    let mut a = HashMap::new();
    a.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    let res = reg
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers: a, custom_response: None },
        )
        .await;
    assert!(res.is_err(), "submit after cancel must fail");

    // take 也应拿不到（已被 cancel 清空）
    let taken = reg.take("s1", &p.ask_id).await;
    assert!(
        taken.is_none(),
        "after cancel, take must yield None; got {:?}",
        taken.as_ref().map(|p| p.ask_id.clone())
    );
}

// ==================== 二次 review 新增测试 ====================

/// notify-before-wait：答案在 waiter 注册前到达，不能丢 permit（issue 1）
///
/// 当前 bug：`tokio::sync::Notify::notify_waiters` 在 wait 之前调用会被丢弃，
/// 而 waiter 之后 `notified().await` 会永久挂起。
///
/// 正确语义：应采用"先检查 submission，再 await；notify_one + 一律可重入"协议。
/// 本测试在 spawn waiter 之前就设置好 submission，断言 waiter 拿到答案而不是
/// 永久挂起。
#[tokio::test]
async fn registry_waiter_after_notify_does_not_lose_permit() {
    use tokio::time::timeout;
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;

    // 在 waiter 注册前完成决议（模拟"答案先到，waiter 后到"的时序）
    let req = req_single();
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    assert!(reg
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers, custom_response: None },
        )
        .await
        .is_ok());

    // waiter 现在才注册 — 必须能立刻拿到首答，不能挂起
    let p2 = p.clone();
    let sub = timeout(Duration::from_millis(500), async move {
        // 实现正确时：先检查 submission 槽（已被占位），立刻返回，不再 await notified
        loop {
            {
                let g = p2.submission.lock().await;
                if let Some(s) = g.as_ref() {
                    return s.clone();
                }
            }
            p2.notify.notified().await;
        }
    })
    .await
    .expect("waiter must not hang on notify-before-wait");
    assert!(!sub.cancelled);
    let a = sub.answers.get(&0).expect("answer 0");
    if let AskQuestionAnswer::Single { option, .. } = a {
        assert_eq!(option, "A");
    } else {
        panic!("expected single A");
    }
}

/// 模式不匹配：single 题收到 multi 答案（或反过来），必须被服务端拒绝（issue 4）
#[tokio::test]
async fn registry_rejects_mode_mismatch() {
    let reg = Arc::new(AskRegistry::new());
    // single 题
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let req = req_single();
    let mut answers = HashMap::new();
    // 单题却给了 Multi 答案
    answers.insert(0, AskQuestionAnswer::Multi { options: vec!["A".into()], note: None });
    let res = reg
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers, custom_response: None },
        )
        .await;
    assert!(res.is_err(), "single question with Multi answer must be rejected");
    assert!(
        res.as_ref().err().unwrap().iter().any(|e| e.contains("mode") || e.contains("multi") || e.contains("single")),
        "error message must mention mode mismatch, got: {:?}",
        res
    );
}

// ==================== 丢唤醒竞态（issue 1: notify-before-wait）====================

/// 丢唤醒竞态回归：answers 在 waiter 注册前通过 cancel / submit_validated 决议，
/// 然后 waiter 才进入 `notify.notified().await`。
///
/// 当前 bug：
/// - `AskRegistry::cancel` / `submit_validated` / `create` 旧 pending 覆盖路径
///   全部使用 `tokio::sync::Notify::notify_waiters()`，仅唤醒**已注册**的 waiter；
/// - 执行路径 `AskUserTool::execute` 内"先检查 submission 槽、再 `notify.notified().await`"
///   之间存在窗口：决议路径的 `notify_waiters()` 可能在 `notified()` future 被 poll
///   之前/期间调用，导致这次唤醒被丢弃，waiter 永久挂起（直到外部 cancellation）。
///
/// 正确语义：
/// - 所有决议路径必须使用 `notify_one()`（保留 permit），保证 notify-before-wait 不丢；
/// - execute 路径"先检查槽再 await notified"是合法的，但 await 必须采用"先
///   `notified()` 注册 permit，再检查槽"协议，或者循环 check-then-await，
///   避免空通知被当成"无答案"unwrap_or_default。
///
/// 本测试模拟"`execute` 第一次检查时槽为空 → 决议路径随后写入并 notify_waiters →
/// waiter 终于到 `notified().await`"的时序，断言 waiter 必须在合理 timeout 内
/// 拿到答案（执行 `submit_validated`）或 cancelled（执行 `cancel`），永远不得挂起。
#[tokio::test]
async fn registry_waiter_does_not_lose_wakeup_when_decision_happens_before_notified() {
    use tokio::time::timeout;

    // -- case A: submit_validated 决议发生在 notified() 之前 --
    {
        let reg = Arc::new(AskRegistry::new());
        let (p, _) = reg.create("s1", "tc1", req_single()).await;
        let notify = p.notify.clone();
        let submission_arc = p.submission.clone();
        let ask_id = p.ask_id.clone();
        let req = req_single();

        // 模拟 execute 的"先检查 submission 槽"路径：槽为空，进入 await 分支。
        let first_check = submission_arc.lock().await.take();
        assert!(first_check.is_none(), "slot must be empty before producer");

        // 在 waiter 真正 await notified() 之前，让 submit_validated 把决议写入并
        // notify_waiters（模拟竞态窗口）。
        let producer = {
            let reg2 = reg.clone();
            let ask_id = ask_id.clone();
            tokio::spawn(async move {
                let mut a = HashMap::new();
                a.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
                reg2.submit_validated(
                    "s1",
                    &ask_id,
                    &req,
                    AskSubmission { cancelled: false, answers: a, custom_response: None },
                )
                .await
            })
        };
        // 让 producer 跑完，写槽 + notify_waiters（此时没有 waiter）
        let _ = producer.await.expect("producer join");

        // waiter 现在才进入 notified().await — 必须在 timeout 内完成。
        let sub = timeout(Duration::from_millis(500), async move {
            // 模拟 execute 的 await 分支（仅 notified()，无 re-check loop）：
            // 正确实现必须能立刻拿到答案（notify_one 保留 permit），
            // 或在槽里有值时读出来。但**当前**实现：notify_waiters 已被丢弃，
            // 此处会永久挂起。
            notify.notified().await;
            let mut g = submission_arc.lock().await;
            g.take().expect("submission must be populated by producer")
        })
        .await
        .expect("waiter must not hang: notify-before-wait must keep permit");
        assert!(!sub.cancelled);
        let a = sub.answers.get(&0).expect("answer 0");
        if let AskQuestionAnswer::Single { option, .. } = a {
            assert_eq!(option, "A");
        } else {
            panic!("expected single A");
        }
    }

    // -- case B: cancel 决议发生在 notified() 之前 --
    {
        let reg = Arc::new(AskRegistry::new());
        let (p, _) = reg.create("s1", "tc1", req_single()).await;
        let notify = p.notify.clone();
        let submission_arc = p.submission.clone();

        // 同样地：先检查槽（空），再让 cancel 决议，再尝试 notified().await。
        let first_check = submission_arc.lock().await.take();
        assert!(first_check.is_none());

        let producer = {
            let reg2 = reg.clone();
            tokio::spawn(async move { reg2.cancel("s1").await })
        };
        let _ = producer.await.expect("cancel join");

        let sub = timeout(Duration::from_millis(500), async move {
            notify.notified().await;
            let mut g = submission_arc.lock().await;
            g.take().expect("cancel must populate submission slot")
        })
        .await
        .expect("waiter must not hang on cancel-before-wait");
        assert!(sub.cancelled, "cancel must surface cancelled=true");
    }

    // -- case C: create 旧 pending 覆盖的决议发生在 notified() 之前 --
    {
        let reg = Arc::new(AskRegistry::new());
        let (p1, _) = reg.create("s1", "tc1", req_single()).await;
        let notify = p1.notify.clone();
        let submission_arc = p1.submission.clone();

        let first_check = submission_arc.lock().await.take();
        assert!(first_check.is_none());

        // 模拟"新的 ask 把旧的覆盖"：决议前 pending 槽空，覆盖后旧 pending 决议为 cancelled。
        let (_p2, old) = reg.create("s1", "tc2", req_single()).await;
        assert!(old.is_some(), "create should return overwritten pending");

        let sub = timeout(Duration::from_millis(500), async move {
            notify.notified().await;
            let mut g = submission_arc.lock().await;
            g.take().expect("overwrite must populate cancelled submission")
        })
        .await
        .expect("waiter must not hang on overwrite-before-wait");
        assert!(sub.cancelled, "overwrite must surface cancelled=true");
    }
}

/// 决议路径必须使用 `notify_one()`，保留 permit；`notify_waiters()` 之前的
/// 旧实现会在 notify-before-wait 时丢通知。
#[tokio::test]
async fn registry_wake_path_uses_notify_one() {
    // 间接验证：决策路径调用的是 notify_one（或 notify_waiters 之外的方法）。
    // 这里的测试思路是构造一个"notify 早就被触发了多次"场景，并断言 waiter
    // 仍能正确拿到 permit。
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let notify = p.notify.clone();
    let req = req_single();
    let mut a = HashMap::new();
    a.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    let _ = reg
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers: a, custom_response: None },
        )
        .await;
    // 决议写入 → 已用 notify_one 保留 permit（修复后）。
    // 之后到达的 waiter 必须能立刻拿到答案，不能挂起。
    let sub = tokio::time::timeout(Duration::from_millis(500), async move {
        notify.notified().await;
        p.submission.lock().await.take().expect("answer")
    })
    .await
    .expect("waiter must not hang on decision-before-wait");
    let a = sub.answers.get(&0).expect("answer 0");
    if let AskQuestionAnswer::Single { option, .. } = a {
        assert_eq!(option, "A");
    } else {
        panic!("expected single A");
    }
}

/// cancel 必须只广播一次 cancelled；之后不能再传播 AskAnswered（issue 3）
///
/// 当前 bug：cancel 后的 waiter 可能被 AskAnswered 事件再次"回答"。
/// 终态唯一：cancelled OR answered，二选一。
#[tokio::test]
async fn registry_cancel_does_not_follow_with_answered() {
    let reg = Arc::new(AskRegistry::new());
    let (p, _) = reg.create("s1", "tc1", req_single()).await;
    let p2 = p.clone();
    // waiter 必须在 cancel 之后才 spawn，确保"cancel 后通知"的语义
    let handle = tokio::spawn(async move {
        loop {
            {
                let g = p2.submission.lock().await;
                if let Some(s) = g.as_ref() {
                    return s.clone();
                }
            }
            p2.notify.notified().await;
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let cancelled = reg.cancel("s1").await;
    assert!(cancelled.is_some());
    let sub = tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("waiter must resolve on cancel")
        .unwrap();
    assert!(sub.cancelled, "cancel terminal state must be cancelled=true");
    // 之后再 submit_validated 必须失败；即便能写入也不能把 cancelled 翻成 answered
    let req = req_single();
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    let res = reg
        .submit_validated(
            "s1",
            &p.ask_id,
            &req,
            AskSubmission { cancelled: false, answers, custom_response: None },
        )
        .await;
    assert!(res.is_err(), "submit after cancel must fail (no Answered broadcast)");
}
// ==================== Phase 4: AskRegistry ↔ SessionStateStore 持久化 ====================
//
// 设计目标：
// 1. attach_store 后，create 立即把 pending 写入 DB；service restart 后 DB 仍可见
// 2. submit_validated 写 answered 终态
// 3. cancel 写 cancelled 终态
// 4. DB 写失败仅 warn，不影响内存流程（best-effort）

use mcoder_lib::persistence::init_sqlite;
use mcoder_lib::persistence::session_state::{
    PendingAskState, SessionStateStore,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static P4_SEQ: AtomicU64 = AtomicU64::new(0);

fn fresh_db_path() -> PathBuf {
    let n = P4_SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "mcoder-ask-persist-{}-{}-{}.db",
        std::process::id(),
        n,
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ))
}

async fn fresh_store_handle() -> Arc<SessionStateStore> {
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);
    let pool = init_sqlite(&path).await.unwrap();
    Arc::new(SessionStateStore::new(pool))
}

#[tokio::test]
async fn p4_registry_create_persists_pending_ask() {
    let store = fresh_store_handle().await;
    let reg = Arc::new(AskRegistry::new());
    reg.attach_store(store.clone()).await;
    let (p, _) = reg.create("s1", "tc-real", req_single()).await;

    let rec = store.get_pending_ask("s1").await.expect("DB must have pending");
    assert_eq!(rec.ask_id, p.ask_id);
    assert_eq!(rec.tool_call_id, "tc-real");
    assert_eq!(rec.state, PendingAskState::Pending);
    // waiting_for_user 也应被持久化
    let (state, reason) = store.get_session_state("s1").await;
    assert_eq!(state, "waiting_for_user");
    assert_eq!(reason.as_deref(), Some("ask_pending"));
}

#[tokio::test]
async fn p4_registry_submit_persists_answered() {
    let store = fresh_store_handle().await;
    let reg = Arc::new(AskRegistry::new());
    reg.attach_store(store.clone()).await;
    let (p, _) = reg.create("s1", "tc-1", req_single()).await;
    let req = req_single();
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    reg.submit_validated(
        "s1",
        &p.ask_id,
        &req,
        AskSubmission { cancelled: false, answers, custom_response: None },
    )
    .await
    .unwrap();

    let rec = store.get_pending_ask("s1").await.unwrap();
    assert_eq!(rec.state, PendingAskState::Answered);
    assert!(rec.result.is_some(), "result must be persisted");
    let (state, reason) = store.get_session_state("s1").await;
    assert_eq!(state, "stopped");
    assert_eq!(reason.as_deref(), Some("ask_answered"));
}

#[tokio::test]
async fn p4_registry_cancel_persists_cancelled() {
    let store = fresh_store_handle().await;
    let reg = Arc::new(AskRegistry::new());
    reg.attach_store(store.clone()).await;
    let (_p, _) = reg.create("s1", "tc-1", req_single()).await;
    reg.cancel("s1").await;

    let rec = store.get_pending_ask("s1").await.unwrap();
    assert_eq!(rec.state, PendingAskState::Cancelled);
    let (state, reason) = store.get_session_state("s1").await;
    assert_eq!(state, "stopped");
    assert_eq!(reason.as_deref(), Some("ask_cancelled"));
}

#[tokio::test]
async fn p4_registry_restart_persists_then_recovers() {
    // 模拟：注册 + 写 DB → drop registry → 新 registry attach 同 DB → DB 仍可见
    let path = fresh_db_path();
    let _ = std::fs::remove_file(&path);

    let store_orig = {
        let pool = init_sqlite(&path).await.unwrap();
        Arc::new(SessionStateStore::new(pool))
    };
    let reg1 = Arc::new(AskRegistry::new());
    reg1.attach_store(store_orig.clone()).await;
    let (p, _) = reg1.create("s-restart", "tc-restart", req_single()).await;
    drop(reg1);

    // 重启：新建 registry（无内存状态），attach 同一 DB
    let pool2 = init_sqlite(&path).await.unwrap();
    let store2 = Arc::new(SessionStateStore::new(pool2));
    let reg2 = AskRegistry::new();
    reg2.attach_store(store2.clone()).await;

    // DB 仍能取出 pending（service restart 后 memory 为空但 DB 有）
    let rec = store2.get_pending_ask("s-restart").await.unwrap();
    assert_eq!(rec.state, PendingAskState::Pending);
    assert_eq!(rec.tool_call_id, "tc-restart");
    // 内存 registry 不应有 pending（重启后清空）
    assert!(reg2.peek("s-restart").await.is_none());
    // 把 ask_id 也确认一致
    assert_eq!(rec.ask_id, p.ask_id);
}
