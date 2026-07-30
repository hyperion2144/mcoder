// 设计文档 §2.2: per-session state (todos) — 取代项目级 .mcoder/plans/todo.json
//
// 不兼容旧数据：旧 todo.json 文件将被忽略，todos 表从空开始。
//
// 数据模型：
//   - session_id: 严格会话隔离，模型不可跨 session 访问（ToolContext 自动绑定 session_id）
//   - status: pending | in_progress | completed | cancelled
//   - priority: high | medium | low
//   - order: 用户在 replace 时提供的稳定排序键（同 priority 内也按 order 排序）
//
// 不变量：
//   - 每 session 至多一个 in_progress
//   - status 必须是合法枚举
//   - list 排序: in_progress → pending → (terminal: completed/cancelled)，再 priority/order
//
// 所有变更走事务 + 返回更新后的 todos + summary，调用方负责广播 ServerEvent::TodoUpdated

use crate::persistence::DbPool;
use serde::{Deserialize, Serialize};
use sqlx::Row;

/// 合法 status
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_IN_PROGRESS: &str = "in_progress";
pub const STATUS_COMPLETED: &str = "completed";
pub const STATUS_CANCELLED: &str = "cancelled";

/// 合法 priority
pub const PRIORITY_HIGH: &str = "high";
pub const PRIORITY_MEDIUM: &str = "medium";
pub const PRIORITY_LOW: &str = "low";

pub const VALID_STATUSES: &[&str] = &[STATUS_PENDING, STATUS_IN_PROGRESS, STATUS_COMPLETED, STATUS_CANCELLED];
pub const VALID_PRIORITIES: &[&str] = &[PRIORITY_HIGH, PRIORITY_MEDIUM, PRIORITY_LOW];

/// 写入输入（add / replace 都使用）
#[derive(Debug, Clone)]
pub struct TodoInput {
    pub content: String,
    pub status: String,
    pub priority: String,
}

impl TodoInput {
    pub fn new(content: impl Into<String>, status: impl Into<String>, priority: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            status: status.into(),
            priority: priority.into(),
        }
    }
}

/// 存储的 todo 完整结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoRecord {
    pub id: String,
    pub session_id: String,
    pub content: String,
    pub status: String,
    pub priority: String,
    pub order: i64,
    pub created_at: String,
    pub updated_at: String,
}

// ==================== Phase 4: per-session Pending Ask / Pending Plan ====================
//
// 设计：
// - 每个 session 至多一条 pending_ask（按 session_id PRIMARY KEY）；
//   同一 session 第二次 create 直接覆盖旧行（ask_registry 已保证）
// - 每个 session 至多一条 pending_plan（同上）
// - state 字段语义：
//     ask:  pending → answered / cancelled
//     plan: pending → approved / rejected / edited
// - 时间戳全部 ms（与 AskRegistry.created_at_ms / 现有 snapshot 一致）
// - 终态保留（不删除），attach 时由 SessionManager 判定是否展示
//
// 不变量：
// - pending_ask / pending_plan 表 schema 由 `create_schema` 在 per-session store
//   首次打开时自动建（兼容旧 DB：IF NOT EXISTS）
// - 不破坏现有 todos / session_state 表

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingAskState {
    Pending,
    Answered,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingPlanState {
    Pending,
    Approved,
    Edited,
    Rejected,
}

/// pending_ask 单行（per session）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAskRecord {
    pub session_id: String,
    pub ask_id: String,
    pub tool_call_id: String,
    pub request: serde_json::Value,
    pub state: PendingAskState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled_at_ms: Option<i64>,
}

/// pending_plan 单行（per session）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPlanRecord {
    pub session_id: String,
    pub plan_id: String,
    pub content: serde_json::Value,
    pub state: PendingPlanState,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at_ms: Option<i64>,
}

/// 摘要（前端 + session.done.unfinished_todos 用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoSummary {
    pub total: usize,
    pub pending: usize,
    pub in_progress: usize,
    pub completed: usize,
    pub cancelled: usize,
}

impl TodoSummary {
    pub fn from_items(items: &[TodoRecord]) -> Self {
        let mut s = Self {
            total: items.len(),
            pending: 0,
            in_progress: 0,
            completed: 0,
            cancelled: 0,
        };
        for t in items {
            match t.status.as_str() {
                STATUS_PENDING => s.pending += 1,
                STATUS_IN_PROGRESS => s.in_progress += 1,
                STATUS_COMPLETED => s.completed += 1,
                STATUS_CANCELLED => s.cancelled += 1,
                _ => {}
            }
        }
        s
    }
}

/// 写入/读取的统一入口。线程安全（pool 内部 mutex）。
///
/// **Phase 5c 统一**：SessionStateStore 现在**只**负责单一 `session_state.db`
/// 数据库（取代旧版的 `todos.db` / `async_tasks.db` 分散路径），包含以下表：
/// - `todos`（per-session todo 列表）
/// - `session_state`（loop_state / stop_reason）
/// - `pending_ask`（per-session pending ask）
/// - `pending_plan`（per-session pending plan）
/// - `session_attrs`（per-session key-value attrs）
/// - `tasks`（Phase 5: per-session async task 持久化）
/// - `async_tasks`（Phase 5 完整 task 元数据表，与 `tasks` 共享同一 DB）
///
/// 不再保留对旧 `todos.db` / `async_tasks.db` 文件路径的兼容（用户已确认：
/// 不兼容旧 DB）。
pub struct SessionStateStore {
    pool: DbPool,
}

/// **Phase 5c 池缓存**：按 db_path 缓存已打开的 SqlitePool，避免重复
/// `connect_with` 产生多个独立 pool。多 Store 共享同一 pool，确保写锁
/// 不会因不同 Store 持有不同连接而失序。
type PoolCell = std::sync::Arc<tokio::sync::OnceCell<DbPool>>;
static POOL_CACHE: tokio::sync::OnceCell<
    tokio::sync::Mutex<std::collections::HashMap<std::path::PathBuf, PoolCell>>,
> = tokio::sync::OnceCell::const_new();

async fn pool_cache() -> &'static tokio::sync::Mutex<std::collections::HashMap<std::path::PathBuf, PoolCell>> {
    POOL_CACHE
        .get_or_init(|| async {
            tokio::sync::Mutex::new(std::collections::HashMap::new())
        })
        .await
}

fn normalize_absolute_db_path(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = absolute.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "database path has no parent")
    })?;
    std::fs::create_dir_all(parent)?;
    let parent = std::fs::canonicalize(parent)?;
    let file_name = absolute.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "database path has no file name")
    })?;
    Ok(parent.join(file_name))
}

/// 计算指定 session 的统一 session_state.db 路径。
///
/// - 反查 JsonlSession meta → project_path
/// - 优先 per-project `<project>/.mcoder/session_state.db`
/// - fallback 全局 `~/.mcoder/session_state.db`
///
/// 旧版的 `todos.db` / `async_tasks.db` 路径不再使用（不兼容旧 DB）。
pub fn session_state_db_path(session_id: &str) -> std::path::PathBuf {
    let project_path = crate::persistence::jsonl::JsonlSession::load(session_id)
        .ok()
        .map(|s| s.meta().project_path.clone());
    match project_path {
        Some(p) => session_state_db_path_for_project(&p),
        None => crate::config::global_config_dir().join("session_state.db"),
    }
}

/// 仅按项目路径计算 session_state.db（启动期 orphan 扫描用，不依赖 session_id）。
pub fn session_state_db_path_for_project(project: &std::path::Path) -> std::path::PathBuf {
    crate::config::project_config_dir(project).join("session_state.db")
}

impl SessionStateStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// 共享 pool 句柄（供外部构造 Store 用；不创建新连接）
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Phase 5c：按 session_id 打开（**共享池缓存**）session_state.db，
    /// 不再使用 `todos.db` / `async_tasks.db` 旧路径。
    pub async fn for_session(session_id: &str) -> Option<Self> {
        let db_path = session_state_db_path(session_id);
        Self::open_at(db_path).await
    }

    /// Phase 5c：在指定路径打开 session_state.db，共享 SqlitePool 缓存。
    /// 所有 Store（SessionStateStore / AsyncTaskStore）都用同一池，保证
    /// 同一 DB 文件的写锁串行。
    pub async fn open_at(db_path: std::path::PathBuf) -> Option<Self> {
        use sqlx::sqlite::SqlitePoolOptions;
        use std::time::Duration;

        let db_path = normalize_absolute_db_path(&db_path).ok()?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }

        let cell = {
            let mut cache = pool_cache().await.lock().await;
            cache
                .entry(db_path.clone())
                .or_insert_with(|| std::sync::Arc::new(tokio::sync::OnceCell::new()))
                .clone()
        };
        let pool = cell
            .get_or_try_init(|| async {
                SqlitePoolOptions::new()
                    .max_connections(2)
                    .connect_with(
                        sqlx::sqlite::SqliteConnectOptions::new()
                            .filename(&db_path)
                            .create_if_missing(true)
                            .busy_timeout(Duration::from_secs(5)),
                    )
                    .await
            })
            .await
            .ok()?
            .clone();

        // 3. 在共享 pool 上跑 schema（idempotent；IF NOT EXISTS）
        if let Err(e) = sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS todos (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                priority TEXT NOT NULL DEFAULT 'medium',
                "order" INTEGER NOT NULL DEFAULT 0,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_todos_session ON todos(session_id);

            CREATE TABLE IF NOT EXISTS session_state (
                session_id TEXT PRIMARY KEY,
                loop_state TEXT NOT NULL DEFAULT 'idle',
                stop_reason TEXT,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS pending_ask (
                session_id TEXT PRIMARY KEY,
                ask_id TEXT NOT NULL,
                tool_call_id TEXT NOT NULL,
                request_json TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending',
                submission_json TEXT,
                result_json TEXT,
                created_at_ms INTEGER NOT NULL,
                answered_at_ms INTEGER,
                cancelled_at_ms INTEGER,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS pending_plan (
                session_id TEXT PRIMARY KEY,
                plan_id TEXT NOT NULL,
                content_json TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending',
                created_at_ms INTEGER NOT NULL,
                decided_at_ms INTEGER,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS session_attrs (
                session_id TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (session_id, key)
            );

            -- Phase 5: per-session async task 持久化（与 session_state 共用一库）
            CREATE TABLE IF NOT EXISTS async_tasks (
                task_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'running',
                args_json TEXT NOT NULL,
                output_json TEXT,
                error TEXT,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_async_tasks_session ON async_tasks(session_id);
            CREATE INDEX IF NOT EXISTS idx_async_tasks_status ON async_tasks(status);

            -- Phase 5 / P1-8: async task 结果幂等投递
            CREATE TABLE IF NOT EXISTS async_task_injections (
                task_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                injected_at_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_async_task_injections_session ON async_task_injections(session_id);
            ALTER TABLE async_tasks ADD COLUMN injected_at_ms INTEGER;
            ALTER TABLE async_tasks ADD COLUMN result_json TEXT;
            "#,
        )
        .execute(&pool)
        .await
        {
            tracing::warn!("session_state schema init failed at {}: {}", db_path.display(), e);
            return None;
        }

        Some(Self { pool })
    }

    /// 列出 session 的所有 todos（按稳定顺序：in_progress → pending → terminal，再 priority/order）
    pub async fn list_todos(&self, session_id: &str) -> Result<Vec<TodoRecord>, sqlx::Error> {
        let rows = sqlx::query(
            r#"SELECT id, session_id, content, status, priority, "order", created_at, updated_at
               FROM todos
               WHERE session_id = ?
               ORDER BY
                 CASE status
                   WHEN 'in_progress' THEN 0
                   WHEN 'pending' THEN 1
                   WHEN 'completed' THEN 2
                   WHEN 'cancelled' THEN 3
                   ELSE 4
                 END,
                 CASE priority WHEN 'high' THEN 0 WHEN 'medium' THEN 1 WHEN 'low' THEN 2 ELSE 3 END,
                 "order" ASC,
                 created_at ASC"#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(TodoRecord {
                id: r.get(0),
                session_id: r.get(1),
                content: r.get(2),
                status: r.get(3),
                priority: r.get(4),
                order: r.get(5),
                created_at: r.get::<String, _>(6),
                updated_at: r.get::<String, _>(7),
            });
        }
        Ok(out)
    }

    /// 添加一个 todo；保持稳定 order（追加到当前 pending 末尾）
    pub async fn add_todo(
        &self,
        session_id: &str,
        input: TodoInput,
    ) -> Result<TodoRecord, sqlx::Error> {
        if !VALID_STATUSES.contains(&input.status.as_str()) {
            return Err(sqlx::Error::Protocol(format!("invalid status: {}", input.status)));
        }
        if !VALID_PRIORITIES.contains(&input.priority.as_str()) {
            return Err(sqlx::Error::Protocol(format!("invalid priority: {}", input.priority)));
        }

        let mut tx = self.pool.begin().await?;

        // 若要设为 in_progress，先确保没有别的 in_progress
        if input.status == STATUS_IN_PROGRESS {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM todos WHERE session_id = ? AND status = 'in_progress'",
            )
            .bind(session_id)
            .fetch_one(&mut *tx)
            .await?;
            if count > 0 {
                return Err(sqlx::Error::Protocol(
                    "another todo is already in_progress; only one in_progress allowed".into(),
                ));
            }
        }

        // 计算 order：取当前 max(order)+1
        let next_order: i64 = sqlx::query_scalar(
            r#"SELECT COALESCE(MAX("order"), -1) + 1 FROM todos WHERE session_id = ?"#,
        )
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;

        let id = format!("td-{}", uuid::Uuid::new_v4());
        sqlx::query(
            r#"INSERT INTO todos (id, session_id, content, status, priority, "order")
               VALUES (?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(session_id)
        .bind(&input.content)
        .bind(&input.status)
        .bind(&input.priority)
        .bind(next_order)
        .execute(&mut *tx)
        .await?;

        let record = sqlx::query(
            r#"SELECT id, session_id, content, status, priority, "order", created_at, updated_at
               FROM todos WHERE id = ?"#,
        )
        .bind(&id)
        .fetch_one(&mut *tx)
        .await
        .map(|r| TodoRecord {
            id: r.get(0),
            session_id: r.get(1),
            content: r.get(2),
            status: r.get(3),
            priority: r.get(4),
            order: r.get(5),
            created_at: r.get::<String, _>(6),
            updated_at: r.get::<String, _>(7),
        })?;

        tx.commit().await?;
        Ok(record)
    }

    /// 更新 todo 的 content / status / priority（id 必须存在）
    /// 参数顺序与测试期望一致：content, status, priority
    pub async fn update_todo(
        &self,
        session_id: &str,
        id: &str,
        new_content: Option<&str>,
        new_status: Option<&str>,
        new_priority: Option<&str>,
    ) -> Result<TodoRecord, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        // 读取当前记录，校验 status / priority 合法性
        let current = sqlx::query(
            r#"SELECT id, session_id, content, status, priority, "order", created_at, updated_at
               FROM todos WHERE id = ? AND session_id = ?"#,
        )
        .bind(id)
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| sqlx::Error::Protocol(format!("todo not found: {}", id)))?;

        let cur_status: String = current.get(3);
        let cur_priority: String = current.get(4);
        let cur_content: String = current.get(2);

        let target_status = new_status.unwrap_or(&cur_status).to_string();
        let target_priority = new_priority.unwrap_or(&cur_priority).to_string();
        let target_content = new_content.unwrap_or(&cur_content).to_string();

        if !VALID_STATUSES.contains(&target_status.as_str()) {
            return Err(sqlx::Error::Protocol(format!("invalid status: {}", target_status)));
        }
        if !VALID_PRIORITIES.contains(&target_priority.as_str()) {
            return Err(sqlx::Error::Protocol(format!("invalid priority: {}", target_priority)));
        }

        // 若要把状态切换为 in_progress，确保当前 session 没有别的 in_progress
        if target_status == STATUS_IN_PROGRESS && cur_status != STATUS_IN_PROGRESS {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM todos WHERE session_id = ? AND status = 'in_progress' AND id != ?",
            )
            .bind(session_id)
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            if count > 0 {
                return Err(sqlx::Error::Protocol(
                    "another todo is already in_progress; only one in_progress allowed".into(),
                ));
            }
        }

        sqlx::query(
            r#"UPDATE todos
               SET status = ?, priority = ?, content = ?, updated_at = CURRENT_TIMESTAMP
               WHERE id = ? AND session_id = ?"#,
        )
        .bind(&target_status)
        .bind(&target_priority)
        .bind(&target_content)
        .bind(id)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;

        let record = sqlx::query(
            r#"SELECT id, session_id, content, status, priority, "order", created_at, updated_at
               FROM todos WHERE id = ?"#,
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map(|r| TodoRecord {
            id: r.get(0),
            session_id: r.get(1),
            content: r.get(2),
            status: r.get(3),
            priority: r.get(4),
            order: r.get(5),
            created_at: r.get::<String, _>(6),
            updated_at: r.get::<String, _>(7),
        })?;

        tx.commit().await?;
        Ok(record)
    }

    /// 删除 todo
    pub async fn remove_todo(&self, session_id: &str, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM todos WHERE id = ? AND session_id = ?")
            .bind(id)
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// 替换整个 session 的 todo 列表（用于 LLM 想重新规划的批量替换）
    /// - inputs 按用户提供的顺序写入 order 0, 1, 2, ...
    /// - 同一 session 已有 todo 全部删除
    /// - 校验：至多一个 in_progress、status/priority 合法
    pub async fn replace_todos(
        &self,
        session_id: &str,
        inputs: Vec<TodoInput>,
    ) -> Result<Vec<TodoRecord>, sqlx::Error> {
        // 校验所有 input
        let mut in_progress_count = 0usize;
        for (i, it) in inputs.iter().enumerate() {
            if !VALID_STATUSES.contains(&it.status.as_str()) {
                return Err(sqlx::Error::Protocol(format!(
                    "input[{}] invalid status: {}",
                    i, it.status
                )));
            }
            if !VALID_PRIORITIES.contains(&it.priority.as_str()) {
                return Err(sqlx::Error::Protocol(format!(
                    "input[{}] invalid priority: {}",
                    i, it.priority
                )));
            }
            if it.status == STATUS_IN_PROGRESS {
                in_progress_count += 1;
                if in_progress_count > 1 {
                    return Err(sqlx::Error::Protocol(
                        "multiple in_progress todos in replace; only one allowed".into(),
                    ));
                }
            }
            if it.content.trim().is_empty() {
                return Err(sqlx::Error::Protocol(format!(
                    "input[{}] content must be non-empty",
                    i
                )));
            }
        }

        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM todos WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;

        let mut records: Vec<TodoRecord> = Vec::with_capacity(inputs.len());
        for (i, it) in inputs.iter().enumerate() {
            let id = format!("td-{}", uuid::Uuid::new_v4());
            let order = i as i64;
            sqlx::query(
                r#"INSERT INTO todos (id, session_id, content, status, priority, "order")
                   VALUES (?, ?, ?, ?, ?, ?)"#,
            )
            .bind(&id)
            .bind(session_id)
            .bind(&it.content)
            .bind(&it.status)
            .bind(&it.priority)
            .bind(order)
            .execute(&mut *tx)
            .await?;

            let r = sqlx::query(
                r#"SELECT id, session_id, content, status, priority, "order", created_at, updated_at
                   FROM todos WHERE id = ?"#,
            )
            .bind(&id)
            .fetch_one(&mut *tx)
            .await?;
            records.push(TodoRecord {
                id: r.get(0),
                session_id: r.get(1),
                content: r.get(2),
                status: r.get(3),
                priority: r.get(4),
                order: r.get(5),
                created_at: r.get::<String, _>(6),
                updated_at: r.get::<String, _>(7),
            });
        }

        tx.commit().await?;
        Ok(records)
    }

    /// 清除已 completed 的 todos（cancelled 不动）
    pub async fn clear_completed_todos(
        &self,
        session_id: &str,
    ) -> Result<usize, sqlx::Error> {
        let result = sqlx::query(
            "DELETE FROM todos WHERE session_id = ? AND status = ?",
        )
        .bind(session_id)
        .bind(STATUS_COMPLETED)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() as usize)
    }

    /// 取出未完成（pending + in_progress）的 todos，用于 agent loop gate / session.done.unfinished_todos
    pub async fn list_unfinished_todos(
        &self,
        session_id: &str,
    ) -> Result<Vec<TodoRecord>, sqlx::Error> {
        let rows = sqlx::query(
            r#"SELECT id, session_id, content, status, priority, "order", created_at, updated_at
               FROM todos
               WHERE session_id = ? AND status IN ('pending', 'in_progress')
               ORDER BY
                 CASE status WHEN 'in_progress' THEN 0 WHEN 'pending' THEN 1 ELSE 2 END,
                 CASE priority WHEN 'high' THEN 0 WHEN 'medium' THEN 1 WHEN 'low' THEN 2 ELSE 3 END,
                 "order" ASC,
                 created_at ASC"#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(TodoRecord {
                id: r.get(0),
                session_id: r.get(1),
                content: r.get(2),
                status: r.get(3),
                priority: r.get(4),
                order: r.get(5),
                created_at: r.get::<String, _>(6),
                updated_at: r.get::<String, _>(7),
            });
        }
        Ok(out)
    }

    // ==================== Phase 2: loop_state / stop_reason ====================

    /// 读取 per-session 的 loop_state（默认 idle）+ stop_reason
    pub async fn get_session_state(
        &self,
        session_id: &str,
    ) -> (String, Option<String>) {
        let row = sqlx::query(
            "SELECT loop_state, stop_reason FROM session_state WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        match row {
            Some(r) => (
                r.get::<String, _>(0),
                r.get::<Option<String>, _>(1),
            ),
            None => ("idle".to_string(), None),
        }
    }

    /// 写入 per-session 的 loop_state + stop_reason（upsert）
    pub async fn set_session_state(
        &self,
        session_id: &str,
        loop_state: &str,
        stop_reason: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"INSERT INTO session_state (session_id, loop_state, stop_reason, updated_at)
               VALUES (?, ?, ?, CURRENT_TIMESTAMP)
               ON CONFLICT(session_id) DO UPDATE SET
                 loop_state = excluded.loop_state,
                 stop_reason = excluded.stop_reason,
                 updated_at = CURRENT_TIMESTAMP"#,
        )
        .bind(session_id)
        .bind(loop_state)
        .bind(stop_reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ==================== Phase 5b: per-session attrs (key-value) ====================
    //
    // 终审修复 #15：role 持久化与 snapshot 恢复。

    /// 写入 session 维度的 key-value attr（upsert）
    pub async fn set_kv(
        &self,
        session_id: &str,
        key: &str,
        value: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"INSERT INTO session_attrs (session_id, key, value, updated_at)
               VALUES (?, ?, ?, CURRENT_TIMESTAMP)
               ON CONFLICT(session_id, key) DO UPDATE SET
                 value = excluded.value,
                 updated_at = CURRENT_TIMESTAMP"#,
        )
        .bind(session_id)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 读取 session 维度的 attr（None 表示未设置）
    pub async fn get_kv(
        &self,
        session_id: &str,
        key: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row = sqlx::query("SELECT value FROM session_attrs WHERE session_id = ? AND key = ?")
            .bind(session_id)
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    // ==================== Phase 4: pending_ask / pending_plan CRUD ====================
    //
    // 关键不变量：
    // - create_*_pending 是 upsert（覆盖旧行）；同一 session 同时只有一个 pending
    // - answer / cancel / approve / reject / edit 把 state 写为终态并保留行
    // - get_*_pending 返回 Option，None 表示没有该 session 的任何记录
    // - 写 pending 的同时由调用方负责把 loop_state=waiting_for_user 持久化
    //   （避免在 store 内引入对外部状态的依赖；store 只管表）

    /// 写入 pending ask（upsert）。同一 session 第二次写入覆盖旧行。
    pub async fn create_pending_ask(
        &self,
        session_id: &str,
        ask_id: &str,
        tool_call_id: &str,
        request: serde_json::Value,
        created_at_ms: i64,
    ) -> Result<(), sqlx::Error> {
        let req_str = serde_json::to_string(&request)
            .map_err(|e| sqlx::Error::Protocol(format!("serialize request: {}", e)))?;
        sqlx::query(
            r#"INSERT INTO pending_ask (session_id, ask_id, tool_call_id, request_json, state, created_at_ms, updated_at)
               VALUES (?, ?, ?, ?, 'pending', ?, CURRENT_TIMESTAMP)
               ON CONFLICT(session_id) DO UPDATE SET
                 ask_id = excluded.ask_id,
                 tool_call_id = excluded.tool_call_id,
                 request_json = excluded.request_json,
                 state = 'pending',
                 submission_json = NULL,
                 result_json = NULL,
                 created_at_ms = excluded.created_at_ms,
                 answered_at_ms = NULL,
                 cancelled_at_ms = NULL,
                 updated_at = CURRENT_TIMESTAMP"#,
        )
        .bind(session_id)
        .bind(ask_id)
        .bind(tool_call_id)
        .bind(req_str)
        .bind(created_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn create_pending_ask_waiting(
        &self,
        session_id: &str,
        ask_id: &str,
        tool_call_id: &str,
        request: serde_json::Value,
        created_at_ms: i64,
    ) -> Result<(), sqlx::Error> {
        let req_str = serde_json::to_string(&request)
            .map_err(|e| sqlx::Error::Protocol(format!("serialize request: {}", e)))?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"INSERT INTO pending_ask (session_id, ask_id, tool_call_id, request_json, state, created_at_ms, updated_at)
               VALUES (?, ?, ?, ?, 'pending', ?, CURRENT_TIMESTAMP)
               ON CONFLICT(session_id) DO UPDATE SET
                 ask_id = excluded.ask_id,
                 tool_call_id = excluded.tool_call_id,
                 request_json = excluded.request_json,
                 state = 'pending',
                 submission_json = NULL,
                 result_json = NULL,
                 created_at_ms = excluded.created_at_ms,
                 answered_at_ms = NULL,
                 cancelled_at_ms = NULL,
                 updated_at = CURRENT_TIMESTAMP"#,
        )
        .bind(session_id)
        .bind(ask_id)
        .bind(tool_call_id)
        .bind(req_str)
        .bind(created_at_ms)
        .execute(&mut *tx)
        .await?;
        upsert_session_state_tx(&mut tx, session_id, "waiting_for_user", Some("ask_pending")).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn answer_pending_ask_and_stop(
        &self,
        session_id: &str,
        ask_id: &str,
        submission: serde_json::Value,
        result: serde_json::Value,
        answered_at_ms: i64,
        stop_reason: &str,
    ) -> Result<bool, sqlx::Error> {
        let sub_str = serde_json::to_string(&submission)
            .map_err(|e| sqlx::Error::Protocol(format!("serialize submission: {}", e)))?;
        let res_str = serde_json::to_string(&result)
            .map_err(|e| sqlx::Error::Protocol(format!("serialize result: {}", e)))?;
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            r#"UPDATE pending_ask
               SET state = 'answered', submission_json = ?, result_json = ?,
                   answered_at_ms = ?, cancelled_at_ms = NULL, updated_at = CURRENT_TIMESTAMP
               WHERE session_id = ? AND ask_id = ? AND state = 'pending'"#,
        )
        .bind(sub_str)
        .bind(res_str)
        .bind(answered_at_ms)
        .bind(session_id)
        .bind(ask_id)
        .execute(&mut *tx)
        .await?;
        if rows.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        upsert_session_state_tx(&mut tx, session_id, "stopped", Some(stop_reason)).await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn cancel_pending_ask_and_stop(
        &self,
        session_id: &str,
        ask_id: &str,
        cancelled_at_ms: i64,
        stop_reason: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query(
            r#"UPDATE pending_ask
               SET state = 'cancelled', cancelled_at_ms = ?, answered_at_ms = NULL,
                   updated_at = CURRENT_TIMESTAMP
               WHERE session_id = ? AND ask_id = ? AND state = 'pending'"#,
        )
        .bind(cancelled_at_ms)
        .bind(session_id)
        .bind(ask_id)
        .execute(&mut *tx)
        .await?;
        if rows.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        upsert_session_state_tx(&mut tx, session_id, "stopped", Some(stop_reason)).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// 把 pending ask 标记为 answered，写入 submission 与 result。
    /// 终审修复 #13：仅当 `state='pending'` 时才允许 UPDATE
    ///   （已有 answered/cancelled 终态时 rows_affected=0，返回 `Ok(false)` 给调用方判定）。
    pub async fn answer_pending_ask(
        &self,
        session_id: &str,
        submission: serde_json::Value,
        result: serde_json::Value,
        answered_at_ms: i64,
    ) -> Result<bool, sqlx::Error> {
        let sub_str = serde_json::to_string(&submission)
            .map_err(|e| sqlx::Error::Protocol(format!("serialize submission: {}", e)))?;
        let res_str = serde_json::to_string(&result)
            .map_err(|e| sqlx::Error::Protocol(format!("serialize result: {}", e)))?;
        let rows = sqlx::query(
            r#"UPDATE pending_ask
               SET state = 'answered',
                   submission_json = ?,
                   result_json = ?,
                   answered_at_ms = ?,
                   cancelled_at_ms = NULL,
                   updated_at = CURRENT_TIMESTAMP
               WHERE session_id = ? AND state = 'pending'"#,
        )
        .bind(sub_str)
        .bind(res_str)
        .bind(answered_at_ms)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(rows.rows_affected() > 0)
    }

    /// 把 pending ask 标记为 cancelled。
    /// 终审修复 #13：仅当 `state='pending'` 时才允许 UPDATE
    ///   （已有 answered/cancelled 终态时 rows_affected=0，返回 `Ok(false)` 给调用方判定）。
    pub async fn cancel_pending_ask(
        &self,
        session_id: &str,
        cancelled_at_ms: i64,
    ) -> Result<bool, sqlx::Error> {
        let rows = sqlx::query(
            r#"UPDATE pending_ask
               SET state = 'cancelled',
                   cancelled_at_ms = ?,
                   answered_at_ms = NULL,
                   updated_at = CURRENT_TIMESTAMP
               WHERE session_id = ? AND state = 'pending'"#,
        )
        .bind(cancelled_at_ms)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(rows.rows_affected() > 0)
    }

    /// 取出 pending ask（包括终态行；调用方按 state 自行决定展示）。
    pub async fn get_pending_ask(&self, session_id: &str) -> Option<PendingAskRecord> {
        let row = sqlx::query(
            r#"SELECT session_id, ask_id, tool_call_id, request_json, state,
                      submission_json, result_json,
                      created_at_ms, answered_at_ms, cancelled_at_ms
               FROM pending_ask WHERE session_id = ?"#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()?;
        let request_json: String = row.get(3);
        let state: String = row.get(4);
        let submission_json: Option<String> = row.get(5);
        let result_json: Option<String> = row.get(6);
        let request = match serde_json::from_str(&request_json) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("pending_ask.request_json parse failed: {}", e);
                serde_json::Value::Null
            }
        };
        let submission = submission_json.and_then(|s| match serde_json::from_str(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("pending_ask.submission_json parse failed: {}", e);
                None
            }
        });
        let result = result_json.and_then(|s| match serde_json::from_str(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("pending_ask.result_json parse failed: {}", e);
                None
            }
        });
        Some(PendingAskRecord {
            session_id: row.get(0),
            ask_id: row.get(1),
            tool_call_id: row.get(2),
            request,
            state: parse_ask_state(&state),
            submission,
            result,
            created_at_ms: row.get::<i64, _>(7),
            answered_at_ms: row.get::<Option<i64>, _>(8),
            cancelled_at_ms: row.get::<Option<i64>, _>(9),
        })
    }

    /// 写入 pending plan（upsert）。
    pub async fn create_pending_plan(
        &self,
        session_id: &str,
        plan_id: &str,
        content: serde_json::Value,
        created_at_ms: i64,
    ) -> Result<(), sqlx::Error> {
        let content_str = serde_json::to_string(&content)
            .map_err(|e| sqlx::Error::Protocol(format!("serialize content: {}", e)))?;
        sqlx::query(
            r#"INSERT INTO pending_plan (session_id, plan_id, content_json, state, created_at_ms, updated_at)
               VALUES (?, ?, ?, 'pending', ?, CURRENT_TIMESTAMP)
               ON CONFLICT(session_id) DO UPDATE SET
                 plan_id = excluded.plan_id,
                 content_json = excluded.content_json,
                 state = 'pending',
                 decided_at_ms = NULL,
                 created_at_ms = excluded.created_at_ms,
                 updated_at = CURRENT_TIMESTAMP"#,
        )
        .bind(session_id)
        .bind(plan_id)
        .bind(content_str)
        .bind(created_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// approve 当前 pending plan。
    /// - `edited_content == None` → state = approved，content 保持原样
    /// - `edited_content == Some(v)` → state = edited，content 替换为 v
    pub async fn approve_pending_plan(
        &self,
        session_id: &str,
        edited_content: Option<serde_json::Value>,
        decided_at_ms: i64,
    ) -> Result<(), sqlx::Error> {
        match edited_content {
            None => {
                sqlx::query(
                    r#"UPDATE pending_plan
                       SET state = 'approved', decided_at_ms = ?, updated_at = CURRENT_TIMESTAMP
                       WHERE session_id = ?"#,
                )
                .bind(decided_at_ms)
                .bind(session_id)
                .execute(&self.pool)
                .await?;
            }
            Some(v) => {
                let s = serde_json::to_string(&v)
                    .map_err(|e| sqlx::Error::Protocol(format!("serialize plan: {}", e)))?;
                sqlx::query(
                    r#"UPDATE pending_plan
                       SET state = 'edited',
                       content_json = ?,
                       decided_at_ms = ?,
                       updated_at = CURRENT_TIMESTAMP
                       WHERE session_id = ?"#,
                )
                .bind(s)
                .bind(decided_at_ms)
                .bind(session_id)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    /// reject 当前 pending plan。
    pub async fn reject_pending_plan(
        &self,
        session_id: &str,
        decided_at_ms: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"UPDATE pending_plan
               SET state = 'rejected', decided_at_ms = ?, updated_at = CURRENT_TIMESTAMP
               WHERE session_id = ?"#,
        )
        .bind(decided_at_ms)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Phase 4: 仅更新 plan 内容，保留 state 不变（plan_update 工具用）。
    pub async fn update_pending_plan_content(
        &self,
        session_id: &str,
        content: serde_json::Value,
    ) -> Result<(), sqlx::Error> {
        let s = serde_json::to_string(&content)
            .map_err(|e| sqlx::Error::Protocol(format!("serialize plan: {}", e)))?;
        sqlx::query(
            r#"UPDATE pending_plan
               SET content_json = ?, updated_at = CURRENT_TIMESTAMP
               WHERE session_id = ?"#,
        )
        .bind(s)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 取出 pending plan。
    pub async fn get_pending_plan(&self, session_id: &str) -> Option<PendingPlanRecord> {
        let row = sqlx::query(
            r#"SELECT session_id, plan_id, content_json, state, created_at_ms, decided_at_ms
               FROM pending_plan WHERE session_id = ?"#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()?;
        let content_json: String = row.get(2);
        let content = match serde_json::from_str(&content_json) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("pending_plan.content_json parse failed: {}", e);
                serde_json::Value::Null
            }
        };
        Some(PendingPlanRecord {
            session_id: row.get(0),
            plan_id: row.get(1),
            content,
            state: parse_plan_state(&row.get::<String, _>(3)),
            created_at_ms: row.get::<i64, _>(4),
            decided_at_ms: row.get::<Option<i64>, _>(5),
        })
    }
}

async fn upsert_session_state_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session_id: &str,
    loop_state: &str,
    stop_reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO session_state (session_id, loop_state, stop_reason, updated_at)
           VALUES (?, ?, ?, CURRENT_TIMESTAMP)
           ON CONFLICT(session_id) DO UPDATE SET
             loop_state = excluded.loop_state,
             stop_reason = excluded.stop_reason,
             updated_at = CURRENT_TIMESTAMP"#,
    )
    .bind(session_id)
    .bind(loop_state)
    .bind(stop_reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn parse_ask_state(s: &str) -> PendingAskState {
    match s {
        "answered" => PendingAskState::Answered,
        "cancelled" => PendingAskState::Cancelled,
        _ => PendingAskState::Pending,
    }
}

fn parse_plan_state(s: &str) -> PendingPlanState {
    match s {
        "approved" => PendingPlanState::Approved,
        "edited" => PendingPlanState::Edited,
        "rejected" => PendingPlanState::Rejected,
        _ => PendingPlanState::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::init_sqlite;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    async fn fresh_store() -> SessionStateStore {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("mcoder-session-state-test-{}-{}.db", std::process::id(), n));
        // 清理（若存在）
        let _ = std::fs::remove_file(&path);
        let pool = init_sqlite(&path).await.unwrap();
        SessionStateStore::new(pool)
    }

    #[tokio::test]
    async fn session_isolation() {
        let store = fresh_store().await;
        store.add_todo("s1", TodoInput::new("a", STATUS_PENDING, PRIORITY_HIGH)).await.unwrap();
        store.add_todo("s2", TodoInput::new("b", STATUS_PENDING, PRIORITY_HIGH)).await.unwrap();
        let s1 = store.list_todos("s1").await.unwrap();
        let s2 = store.list_todos("s2").await.unwrap();
        assert_eq!(s1.len(), 1);
        assert_eq!(s1[0].content, "a");
        assert_eq!(s2.len(), 1);
        assert_eq!(s2[0].content, "b");
    }

    #[tokio::test]
    async fn replace_sets_stable_order_and_rejects_multiple_in_progress() {
        let store = fresh_store().await;
        store.add_todo("s", TodoInput::new("old", STATUS_PENDING, PRIORITY_HIGH)).await.unwrap();
        let res = store.replace_todos("s", vec![
            TodoInput::new("a", STATUS_PENDING, PRIORITY_HIGH),
            TodoInput::new("b", STATUS_IN_PROGRESS, PRIORITY_HIGH),
            TodoInput::new("c", STATUS_PENDING, PRIORITY_LOW),
        ]).await.unwrap();
        assert_eq!(res.iter().map(|r| r.content.as_str()).collect::<Vec<_>>(), vec!["a", "b", "c"]);
        let all = store.list_todos("s").await.unwrap();
        // 排序后: in_progress(b) → pending(a high) → pending(c low)
        assert_eq!(all.iter().map(|t| t.content.as_str()).collect::<Vec<_>>(), vec!["b", "a", "c"]);

        let err = store.replace_todos("s", vec![
            TodoInput::new("x", STATUS_IN_PROGRESS, PRIORITY_HIGH),
            TodoInput::new("y", STATUS_IN_PROGRESS, PRIORITY_HIGH),
        ]).await.unwrap_err();
        assert!(err.to_string().contains("multiple in_progress"));
    }

    #[tokio::test]
    async fn unique_in_progress_invariant() {
        let store = fresh_store().await;
        store.add_todo("s", TodoInput::new("first", STATUS_IN_PROGRESS, PRIORITY_HIGH)).await.unwrap();
        let err = store.add_todo("s", TodoInput::new("second", STATUS_IN_PROGRESS, PRIORITY_HIGH)).await.unwrap_err();
        assert!(err.to_string().contains("in_progress"));
    }

    #[tokio::test]
    async fn clear_completed_only_removes_completed() {
        let store = fresh_store().await;
        store.add_todo("s", TodoInput::new("a", STATUS_COMPLETED, PRIORITY_HIGH)).await.unwrap();
        store.add_todo("s", TodoInput::new("b", STATUS_CANCELLED, PRIORITY_HIGH)).await.unwrap();
        store.add_todo("s", TodoInput::new("c", STATUS_PENDING, PRIORITY_HIGH)).await.unwrap();
        let n = store.clear_completed_todos("s").await.unwrap();
        assert_eq!(n, 1);
        let remaining = store.list_todos("s").await.unwrap();
        let status: Vec<&str> = remaining.iter().map(|t| t.status.as_str()).collect();
        assert!(status.contains(&STATUS_CANCELLED));
        assert!(status.contains(&STATUS_PENDING));
        assert!(!status.contains(&STATUS_COMPLETED));
    }

    #[tokio::test]
    async fn summary_counts_correctly() {
        let store = fresh_store().await;
        store.add_todo("s", TodoInput::new("a", STATUS_COMPLETED, PRIORITY_HIGH)).await.unwrap();
        store.add_todo("s", TodoInput::new("b", STATUS_IN_PROGRESS, PRIORITY_HIGH)).await.unwrap();
        store.add_todo("s", TodoInput::new("c", STATUS_PENDING, PRIORITY_HIGH)).await.unwrap();
        store.add_todo("s", TodoInput::new("d", STATUS_CANCELLED, PRIORITY_HIGH)).await.unwrap();
        let summary = TodoSummary::from_items(&store.list_todos("s").await.unwrap());
        assert_eq!(summary.total, 4);
        assert_eq!(summary.pending, 1);
        assert_eq!(summary.in_progress, 1);
        assert_eq!(summary.completed, 1);
        assert_eq!(summary.cancelled, 1);
    }
}