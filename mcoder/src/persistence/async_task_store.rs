// Phase 5: 异步任务（AsyncTask）按 session 持久化到现有 SQLite。
//
// 设计目标：
// 1. 每条 async task 都持久化到 per-session 数据库的 tasks 表：
//    task_id PK, session_id, tool_name, args_json, status, output_json, error,
//    created_at_ms, updated_at_ms
// 2. 服务启动 / 历史 session load / attach 时，把 DB 中 queued/running 原子
//    标记为 interrupted（绝不自动重跑）
// 3. TaskManager 创建 / 状态变化 / 完成 / 取消时同步写 DB
// 4. session.isolation 严格：list / get / cancel 都按 session_id 过滤
// 5. RPC task.list 按 session 隔离；task.cancel 只允许取消 caller 所属 session
//    的 task（防跨会话）
//
// 不变量：
// - status 枚举：queued | running | completed | failed | cancelled | interrupted
// - interrupted 终态：表明服务重启时被原子标记，agent inspect 后决定是否重跑
// - task_id 是 string（UUIDv4），由 TaskManager spawn 时分配

use crate::persistence::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::Row;

/// 合法状态枚举（与 SPEC 文档一致）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsyncTaskState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    /// 服务重启时被原子标记（queued/running → interrupted）；不自动重跑
    Interrupted,
}

impl AsyncTaskState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsyncTaskRecord {
    pub task_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub status: AsyncTaskState,
    pub args_json: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_json: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// AsyncTaskStore: per-session task 持久化
pub struct AsyncTaskStore {
    pool: DbPool,
}

impl AsyncTaskStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// 写入一条 task（status=running）
    /// 返回 AsyncTaskRecord（含 DB 写入的 task_id）
    pub async fn create_task(
        &self,
        session_id: &str,
        tool_name: &str,
        args: serde_json::Value,
        now_ms: i64,
    ) -> Result<AsyncTaskRecord, sqlx::Error> {
        let task_id = format!("at-{}", uuid::Uuid::new_v4());
        sqlx::query(
            r#"INSERT INTO async_tasks (task_id, session_id, tool_name, status, args_json, created_at_ms, updated_at_ms)
               VALUES (?, ?, ?, 'running', ?, ?, ?)"#,
        )
        .bind(&task_id)
        .bind(session_id)
        .bind(tool_name)
        .bind(serde_json::to_string(&args).unwrap_or_else(|_| "{}".into()))
        .bind(now_ms)
        .bind(now_ms)
        .execute(&self.pool)
        .await?;
        Ok(AsyncTaskRecord {
            task_id,
            session_id: session_id.into(),
            tool_name: tool_name.into(),
            status: AsyncTaskState::Running,
            args_json: args,
            output_json: None,
            error: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }

    /// task 完成 → 写 output
    pub async fn complete_task(
        &self,
        task_id: &str,
        output: serde_json::Value,
        now_ms: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"UPDATE async_tasks
               SET status = 'completed', output_json = ?, error = NULL, updated_at_ms = ?
               WHERE task_id = ? AND status IN ('queued', 'running')"#,
        )
        .bind(serde_json::to_string(&output).unwrap_or_else(|_| "null".into()))
        .bind(now_ms)
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// task 失败 → 写 error
    pub async fn fail_task(
        &self,
        task_id: &str,
        error: &str,
        now_ms: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"UPDATE async_tasks
               SET status = 'failed', error = ?, output_json = NULL, updated_at_ms = ?
               WHERE task_id = ? AND status IN ('queued', 'running')"#,
        )
        .bind(error)
        .bind(now_ms)
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// 取消 task
    /// 取出未注入的 completed/failed task（服务启动 / attach 时补投用）
    pub async fn list_undelivered_terminal_tasks(
        &self,
        session_id: &str,
    ) -> Result<Vec<AsyncTaskRecord>, sqlx::Error> {
        let rows = sqlx::query(
            r#"SELECT a.task_id, a.session_id, a.tool_name, a.status, a.args_json,
                      a.output_json, a.error, a.created_at_ms, a.updated_at_ms
               FROM async_tasks a
               LEFT JOIN async_task_injections i ON i.task_id = a.task_id
               WHERE a.session_id = ?
                 AND a.status IN ('completed', 'failed', 'cancelled')
                 AND i.task_id IS NULL
               ORDER BY a.created_at_ms ASC"#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(rec) = parse_record(&row) {
                out.push(rec);
            }
        }
        Ok(out)
    }

    /// 标记 task 已注入。重复调用幂等。
    pub async fn mark_task_injected(
        &self,
        task_id: &str,
        injected_at_ms: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"INSERT INTO async_task_injections (task_id, session_id, injected_at_ms)
               SELECT ?, session_id, ? FROM async_tasks WHERE task_id = ?
               ON CONFLICT(task_id) DO NOTHING"#,
        )
        .bind(task_id)
        .bind(injected_at_ms)
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// 任务注入前：把终态结果再写一次（保证 DB 与内存一致），幂等。
    pub async fn complete_terminal_state(
        &self,
        task_id: &str,
        status: AsyncTaskState,
        result: Option<serde_json::Value>,
        error: Option<String>,
    ) -> Result<bool, sqlx::Error> {
        let result_json = result
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "null".into()));
        let result = sqlx::query(
            r#"UPDATE async_tasks
               SET status = ?,
                   output_json = COALESCE(?, output_json),
                   error = COALESCE(?, error),
                   updated_at_ms = ?
               WHERE task_id = ? AND status IN ('completed', 'failed', 'cancelled')"#,
        )
        .bind(status.as_str())
        .bind(result_json)
        .bind(error)
        .bind(chrono::Utc::now().timestamp_millis())
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn cancel_task(&self, task_id: &str, now_ms: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"UPDATE async_tasks
               SET status = 'cancelled', updated_at_ms = ?
               WHERE task_id = ? AND status IN ('queued', 'running')"#,
        )
        .bind(now_ms)
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// 把所有 queued/running 标记为 interrupted（服务启动时调用）
    /// 返回被标记的 task 数；不会触发任何自动重跑
    pub async fn mark_orphans_interrupted(&self, now_ms: i64) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"UPDATE async_tasks
               SET status = 'interrupted', error = 'interrupted by service restart', updated_at_ms = ?
               WHERE status IN ('queued', 'running')"#,
        )
        .bind(now_ms)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// 单 task 查询（不做 session 过滤；用于内部 hydration / TaskManager 内部）
    pub async fn get_task(&self, task_id: &str) -> Option<AsyncTaskRecord> {
        self.get_task_filtered(task_id, None).await
    }

    /// 按 session 过滤取 task（防跨会话读取）
    pub async fn get_task_for_session(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Option<AsyncTaskRecord> {
        self.get_task_filtered(task_id, Some(session_id)).await
    }

    async fn get_task_filtered(
        &self,
        task_id: &str,
        session_id: Option<&str>,
    ) -> Option<AsyncTaskRecord> {
        let row = match session_id {
            Some(sid) => {
                sqlx::query(
                    r#"SELECT task_id, session_id, tool_name, status, args_json,
                              output_json, error, created_at_ms, updated_at_ms
                       FROM async_tasks WHERE task_id = ? AND session_id = ?"#,
                )
                .bind(task_id)
                .bind(sid)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten()
            }
            None => {
                sqlx::query(
                    r#"SELECT task_id, session_id, tool_name, status, args_json,
                              output_json, error, created_at_ms, updated_at_ms
                       FROM async_tasks WHERE task_id = ?"#,
                )
                .bind(task_id)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten()
            }
        }?;
        parse_record(&row)
    }

    /// 列出某 session 的所有 tasks（按 created_at_ms ASC）
    pub async fn list_tasks_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<AsyncTaskRecord>, sqlx::Error> {
        let rows = sqlx::query(
            r#"SELECT task_id, session_id, tool_name, status, args_json,
                      output_json, error, created_at_ms, updated_at_ms
               FROM async_tasks
               WHERE session_id = ?
               ORDER BY created_at_ms ASC"#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            if let Some(rec) = parse_record(&r) {
                out.push(rec);
            }
        }
        Ok(out)
    }
}

fn parse_record(r: &sqlx::sqlite::SqliteRow) -> Option<AsyncTaskRecord> {
    let task_id: String = r.get(0);
    let session_id: String = r.get(1);
    let tool_name: String = r.get(2);
    let status_str: String = r.get(3);
    let args_str: String = r.get(4);
    let output_str: Option<String> = r.get(5);
    let error: Option<String> = r.get(6);
    let created_at_ms: i64 = r.get(7);
    let updated_at_ms: i64 = r.get(8);

    Some(AsyncTaskRecord {
        task_id,
        session_id,
        tool_name,
        status: parse_state(&status_str),
        args_json: serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null),
        output_json: output_str.and_then(|s| serde_json::from_str(&s).ok()),
        error,
        created_at_ms,
        updated_at_ms,
    })
}

fn parse_state(s: &str) -> AsyncTaskState {
    match s {
        "queued" => AsyncTaskState::Queued,
        "running" => AsyncTaskState::Running,
        "completed" => AsyncTaskState::Completed,
        "failed" => AsyncTaskState::Failed,
        "cancelled" => AsyncTaskState::Cancelled,
        "interrupted" => AsyncTaskState::Interrupted,
        _ => AsyncTaskState::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::init_sqlite;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    async fn fresh_store() -> AsyncTaskStore {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "mcoder-async-task-store-{}-{}.db",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_file(&path);
        let pool = init_sqlite(&path).await.unwrap();
        AsyncTaskStore::new(pool)
    }

    #[tokio::test]
    async fn round_trip() {
        let store = fresh_store().await;
        let rec = store
            .create_task("s1", "bash", serde_json::json!({"cmd": "ls"}), 1)
            .await
            .unwrap();
        assert_eq!(rec.status, AsyncTaskState::Running);
        let fetched = store.get_task(&rec.task_id).await.unwrap();
        assert_eq!(fetched.task_id, rec.task_id);
        assert_eq!(fetched.tool_name, "bash");
    }

    #[tokio::test]
    async fn mark_orphans_only_touches_queued_running() {
        let store = fresh_store().await;
        let r1 = store.create_task("s1", "bash", serde_json::json!({}), 1).await.unwrap();
        let r2 = store.create_task("s1", "bash", serde_json::json!({}), 2).await.unwrap();
        let r3 = store.create_task("s1", "bash", serde_json::json!({}), 3).await.unwrap();
        store.complete_task(&r3.task_id, serde_json::json!({"ok": 1}), 4).await.unwrap();
        let n = store.mark_orphans_interrupted(100).await.unwrap();
        assert_eq!(n, 2);
        let t1 = store.get_task(&r1.task_id).await.unwrap();
        let t2 = store.get_task(&r2.task_id).await.unwrap();
        let t3 = store.get_task(&r3.task_id).await.unwrap();
        assert_eq!(t1.status, AsyncTaskState::Interrupted);
        assert_eq!(t2.status, AsyncTaskState::Interrupted);
        assert_eq!(t3.status, AsyncTaskState::Completed);
    }
}