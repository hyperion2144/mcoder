// 服务端 ask_user 单元测试：validate_request / validate_submission
// 与客户端 mcoder-tui/src/ask/validation.ts 行为保持一致

use mcoder_lib::ask_user::{
    build_tool_result, validate_request, validate_submission, AskOption, AskQuestion,
    AskQuestionAnswer, AskRequest, AskSubmission, ASK_MAX_QUESTIONS, ASK_MAX_OPTIONS,
    ASK_MIN_QUESTIONS, ASK_MIN_OPTIONS,
};
use serde_json::json;
use std::collections::HashMap;

fn q(label: &str, opts: &[&str], multi: bool) -> AskQuestion {
    AskQuestion {
        question: format!("Pick {}", label),
        header: None,
        options: opts
            .iter()
            .map(|s| AskOption { label: s.to_string(), description: None })
            .collect(),
        multi_select: if multi { Some(true) } else { Some(false) },
    }
}

#[test]
fn validate_request_rejects_non_object() {
    let v = json!("not an object");
    let err = validate_request(&v).unwrap_err();
    assert!(err.iter().any(|e| e.contains("must be an object")));
}

#[test]
fn validate_request_rejects_missing_questions() {
    let v = json!({});
    let err = validate_request(&v).unwrap_err();
    assert!(err.iter().any(|e| e.contains("questions must be an array")));
}

#[test]
fn validate_request_rejects_too_few_questions() {
    let v = json!({ "questions": [] });
    let err = validate_request(&v).unwrap_err();
    assert!(err.iter().any(|e| e.contains(&format!("{}-{}", ASK_MIN_QUESTIONS, ASK_MAX_QUESTIONS))));
}

#[test]
fn validate_request_rejects_too_many_questions() {
    let mut qs = Vec::new();
    for i in 0..(ASK_MAX_QUESTIONS + 1) {
        qs.push(json!({
            "question": format!("Q{}", i),
            "options": [{"label": "A"}, {"label": "B"}],
        }));
    }
    let v = json!({ "questions": qs });
    let err = validate_request(&v).unwrap_err();
    assert!(err.iter().any(|e| e.contains(&format!("{}-{}", ASK_MIN_QUESTIONS, ASK_MAX_QUESTIONS))));
}

#[test]
fn validate_request_rejects_too_few_options() {
    let v = json!({
        "questions": [{"question": "q", "options": [{"label": "A"}]}]
    });
    let err = validate_request(&v).unwrap_err();
    assert!(err.iter().any(|e| e.contains(&format!("{}-{}", ASK_MIN_OPTIONS, ASK_MAX_OPTIONS))));
}

#[test]
fn validate_request_rejects_too_many_options() {
    let opts: Vec<_> = (0..(ASK_MAX_OPTIONS + 1)).map(|i| json!({"label": format!("O{}", i)})).collect();
    let v = json!({
        "questions": [{"question": "q", "options": opts}]
    });
    let err = validate_request(&v).unwrap_err();
    assert!(err.iter().any(|e| e.contains(&format!("{}-{}", ASK_MIN_OPTIONS, ASK_MAX_OPTIONS))));
}

#[test]
fn validate_request_rejects_empty_question_text() {
    let v = json!({
        "questions": [{
            "question": "   ",
            "options": [{"label": "A"}, {"label": "B"}]
        }]
    });
    let err = validate_request(&v).unwrap_err();
    assert!(err.iter().any(|e| e.contains("non-empty string")));
}

#[test]
fn validate_request_rejects_empty_label() {
    let v = json!({
        "questions": [{
            "question": "q",
            "options": [{"label": "A"}, {"label": " "}]
        }]
    });
    let err = validate_request(&v).unwrap_err();
    assert!(err.iter().any(|e| e.contains("non-empty string")));
}

#[test]
fn validate_request_rejects_duplicate_labels() {
    let v = json!({
        "questions": [{
            "question": "q",
            "options": [{"label": "A"}, {"label": "A"}]
        }]
    });
    let err = validate_request(&v).unwrap_err();
    assert!(err.iter().any(|e| e.contains("duplicate")));
}

#[test]
fn validate_request_accepts_valid_4q_4o() {
    let mut qs = Vec::new();
    for i in 0..ASK_MAX_QUESTIONS {
        let opts: Vec<_> = (0..ASK_MAX_OPTIONS).map(|j| json!({"label": format!("O{}{}", i, j)})).collect();
        qs.push(json!({
            "question": format!("Q{}", i),
            "header": format!("H{}", i),
            "options": opts,
            "multi_select": i % 2 == 0,
        }));
    }
    let v = json!({ "questions": qs });
    let req = validate_request(&v).expect("valid");
    assert_eq!(req.questions.len(), ASK_MAX_QUESTIONS);
    for (i, qst) in req.questions.iter().enumerate() {
        assert_eq!(qst.options.len(), ASK_MAX_OPTIONS);
        assert_eq!(qst.header.as_deref(), Some(format!("H{}", i).as_str()));
        assert_eq!(qst.multi_select, Some(i % 2 == 0));
    }
}

#[test]
fn validate_submission_rejects_unknown_option() {
    let req = AskRequest { questions: vec![q("a", &["A", "B"], false)] };
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Single { option: "C".into(), note: None });
    let sub = AskSubmission { cancelled: false, answers, custom_response: None };
    let err = validate_submission(&req, &sub).unwrap_err();
    assert!(err.iter().any(|e| e.contains("unknown option")));
}

#[test]
fn validate_submission_rejects_missing_answer() {
    let req = AskRequest {
        questions: vec![q("a", &["A", "B"], false), q("b", &["X", "Y"], false)],
    };
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    let sub = AskSubmission { cancelled: false, answers, custom_response: None };
    let err = validate_submission(&req, &sub).unwrap_err();
    assert!(err.iter().any(|e| e.contains("missing answer")));
}

#[test]
fn validate_submission_rejects_empty_multi() {
    let req = AskRequest { questions: vec![q("a", &["A", "B"], true)] };
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Multi { options: vec![], note: None });
    let sub = AskSubmission { cancelled: false, answers, custom_response: None };
    let err = validate_submission(&req, &sub).unwrap_err();
    assert!(err.iter().any(|e| e.contains("multi-select requires non-empty")));
}

#[test]
fn validate_submission_cancelled_is_always_valid() {
    let req = AskRequest { questions: vec![q("a", &["A", "B"], false)] };
    let sub = AskSubmission { cancelled: true, answers: HashMap::new(), custom_response: None };
    assert!(validate_submission(&req, &sub).is_ok());
}

#[test]
fn validate_submission_accepts_full_answers() {
    let req = AskRequest {
        questions: vec![q("a", &["A", "B"], false), q("b", &["X", "Y", "Z"], true)],
    };
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: Some("ok".into()) });
    answers.insert(1, AskQuestionAnswer::Multi {
        options: vec!["X".into(), "Z".into()],
        note: None,
    });
    let sub = AskSubmission { cancelled: false, answers, custom_response: None };
    assert!(validate_submission(&req, &sub).is_ok());
}

#[test]
fn build_tool_result_cancelled() {
    let req = AskRequest { questions: vec![q("a", &["A", "B"], false)] };
    let sub = AskSubmission { cancelled: true, answers: HashMap::new(), custom_response: None };
    let r = build_tool_result(&req, &sub);
    assert_eq!(r["cancelled"], json!(true));
    assert!(r["questions"].is_array());
}

#[test]
fn build_tool_result_full() {
    let req = AskRequest {
        questions: vec![q("a", &["A", "B"], false), q("b", &["X", "Y"], true)],
    };
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Single { option: "B".into(), note: Some("hi".into()) });
    answers.insert(1, AskQuestionAnswer::Multi { options: vec!["X".into()], note: None });
    let sub = AskSubmission { cancelled: false, answers, custom_response: None };
    let r = build_tool_result(&req, &sub);
    assert_eq!(r["cancelled"], json!(false));
    let ans = r["answers"].as_array().expect("answers array");
    assert_eq!(ans.len(), 2);
    assert_eq!(ans[0]["mode"], json!("single"));
    assert_eq!(ans[0]["option"], json!("B"));
    assert_eq!(ans[0]["note"], json!("hi"));
    assert_eq!(ans[1]["mode"], json!("multi"));
    assert_eq!(ans[1]["options"], json!(["X"]));
}

// ==================== review 新增测试 ====================

/// Custom(note) 单题作答：issue 3 — 不强制要求 option
#[test]
fn validate_submission_accepts_custom_single() {
    let req = AskRequest { questions: vec![q("a", &["A", "B"], false)] };
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Custom { note: "自由答复".into() });
    let sub = AskSubmission {
        cancelled: false,
        answers,
        custom_response: Some("自由答复".into()),
    };
    assert!(validate_submission(&req, &sub).is_ok());
}

/// Custom(note) 多题作答：每题独立 Custom + 整段 custom_response
#[test]
fn validate_submission_accepts_custom_multi() {
    let req = AskRequest {
        questions: vec![q("a", &["A", "B"], false), q("b", &["X", "Y"], true)],
    };
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Custom { note: "整段答复".into() });
    answers.insert(1, AskQuestionAnswer::Custom { note: "整段答复".into() });
    let sub = AskSubmission {
        cancelled: false,
        answers,
        custom_response: Some("整段答复".into()),
    };
    assert!(validate_submission(&req, &sub).is_ok());
}

/// 仅 custom_response，无 answers：多题场景下只给一段话也应通过
#[test]
fn validate_submission_accepts_custom_response_only() {
    let req = AskRequest {
        questions: vec![q("a", &["A", "B"], false), q("b", &["X", "Y"], true)],
    };
    let sub = AskSubmission {
        cancelled: false,
        answers: HashMap::new(),
        custom_response: Some("整段答复".into()),
    };
    assert!(validate_submission(&req, &sub).is_ok());
}

/// 完全空 answers + 无 custom_response → 必须拒绝（防止静默丢失答案）
#[test]
fn validate_submission_rejects_completely_empty() {
    let req = AskRequest { questions: vec![q("a", &["A", "B"], false)] };
    let sub = AskSubmission {
        cancelled: false,
        answers: HashMap::new(),
        custom_response: None,
    };
    let err = validate_submission(&req, &sub).unwrap_err();
    assert!(err.iter().any(|e| e.contains("custom_response") || e.contains("custom")));
}

/// Skipped + custom_response 组合合法
#[test]
fn validate_submission_accepts_skipped_with_custom_response() {
    let req = AskRequest {
        questions: vec![q("a", &["A", "B"], false), q("b", &["X", "Y"], false)],
    };
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Skipped);
    answers.insert(1, AskQuestionAnswer::Custom { note: "只回答了第二题".into() });
    let sub = AskSubmission {
        cancelled: false,
        answers,
        custom_response: Some("只回答了第二题".into()),
    };
    assert!(validate_submission(&req, &sub).is_ok());
}

/// build_tool_result 必须把 custom_response 透传给 LLM
#[test]
fn build_tool_result_includes_custom_response() {
    let req = AskRequest { questions: vec![q("a", &["A", "B"], false)] };
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Custom { note: "free text".into() });
    let sub = AskSubmission {
        cancelled: false,
        answers,
        custom_response: Some("free text".into()),
    };
    let r = build_tool_result(&req, &sub);
    assert_eq!(r["cancelled"], json!(false));
    assert_eq!(r["custom_response"], json!("free text"));
    assert_eq!(r["answers"][0]["mode"], json!("custom"));
    assert_eq!(r["answers"][0]["note"], json!("free text"));
}

/// build_tool_result 必须把 Skipped 渲染出来
#[test]
fn build_tool_result_renders_skipped() {
    let req = AskRequest { questions: vec![q("a", &["A", "B"], false)] };
    let mut answers = HashMap::new();
    answers.insert(0, AskQuestionAnswer::Skipped);
    let sub = AskSubmission {
        cancelled: false,
        answers,
        custom_response: Some("xxx".into()),
    };
    let r = build_tool_result(&req, &sub);
    assert_eq!(r["answers"][0]["mode"], json!("skipped"));
}

// ==================== 二次 review 新增测试 ====================

/// 模式不匹配：multi_select=false (Single) 的题收到 Multi 答案，必须被服务端拒绝（issue 4）
#[test]
fn validate_submission_rejects_mode_mismatch_single_question_with_multi_answer() {
    let req = AskRequest {
        questions: vec![q("a", &["A", "B"], false)], // multi_select=false (Single)
    };
    let mut answers = HashMap::new();
    // 故意给 Multi 答案（题是 Single）
    answers.insert(0, AskQuestionAnswer::Multi {
        options: vec!["A".into()],
        note: None,
    });
    let sub = AskSubmission {
        cancelled: false,
        answers,
        custom_response: None,
    };
    let err = validate_submission(&req, &sub).unwrap_err();
    assert!(
        err.iter().any(|e| e.contains("mode") || e.contains("multi") || e.contains("single")),
        "error must mention mode mismatch, got: {:?}",
        err
    );
}

/// 模式不匹配：multi_select=true (Multi) 的题收到 Single 答案，必须被服务端拒绝（issue 4）
#[test]
fn validate_submission_rejects_mode_mismatch_multi_question_with_single_answer() {
    let req = AskRequest {
        questions: vec![q("a", &["A", "B"], true)], // multi_select=true (Multi)
    };
    let mut answers = HashMap::new();
    // 故意给 Single 答案（题是 Multi）
    answers.insert(0, AskQuestionAnswer::Single { option: "A".into(), note: None });
    let sub = AskSubmission {
        cancelled: false,
        answers,
        custom_response: None,
    };
    let err = validate_submission(&req, &sub).unwrap_err();
    assert!(
        err.iter().any(|e| e.contains("mode") || e.contains("multi") || e.contains("single")),
        "error must mention mode mismatch, got: {:?}",
        err
    );
}