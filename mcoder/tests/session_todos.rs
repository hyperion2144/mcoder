use mcoder_lib::persistence::{init_sqlite, session_state::{SessionStateStore, TodoInput}};

fn temp_db(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mcoder-{name}-{}.db", uuid::Uuid::new_v4()))
}

#[tokio::test]
async fn todos_are_session_scoped_sorted_and_enforce_one_in_progress() {
    let path = temp_db("todos");
    let store = SessionStateStore::new(init_sqlite(&path).await.unwrap());

    store.replace_todos("s1", vec![
        TodoInput::new("low pending", "pending", "low"),
        TodoInput::new("done", "completed", "high"),
        TodoInput::new("high pending", "pending", "high"),
        TodoInput::new("working", "in_progress", "medium"),
    ]).await.unwrap();
    store.add_todo("s2", TodoInput::new("other", "pending", "high")).await.unwrap();

    let s1 = store.list_todos("s1").await.unwrap();
    assert_eq!(s1.iter().map(|t| t.content.as_str()).collect::<Vec<_>>(), vec![
        "working", "high pending", "low pending", "done"
    ]);
    assert_eq!(store.list_todos("s2").await.unwrap().len(), 1);

    let err = store.add_todo("s1", TodoInput::new("second working", "in_progress", "high"))
        .await.unwrap_err();
    assert!(err.to_string().contains("in_progress"));

    let pending_id = s1.iter().find(|t| t.content == "high pending").unwrap().id.clone();
    store.update_todo("s1", &pending_id, None, Some("cancelled"), None).await.unwrap();
    store.clear_completed_todos("s1").await.unwrap();
    let terminal = store.list_todos("s1").await.unwrap();
    assert!(terminal.iter().any(|t| t.status == "cancelled"));
    assert!(!terminal.iter().any(|t| t.status == "completed"));

    store.remove_todo("s1", &pending_id).await.unwrap();
    assert!(!store.list_todos("s1").await.unwrap().iter().any(|t| t.id == pending_id));

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn replace_rejects_invalid_status_and_multiple_in_progress() {
    let path = temp_db("todos-validation");
    let store = SessionStateStore::new(init_sqlite(&path).await.unwrap());

    assert!(store.replace_todos("s", vec![
        TodoInput::new("one", "in_progress", "medium"),
        TodoInput::new("two", "in_progress", "medium"),
    ]).await.is_err());
    assert!(store.add_todo("s", TodoInput::new("bad", "done", "medium")).await.is_err());

    let _ = std::fs::remove_file(path);
}
