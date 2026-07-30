// 设计文档 §2.1: sqlx-based DbPool 为 forward-looking alternative
// 当前实现使用 rusqlite 直接连接；sqlx 池化实现保留供未来切换
#![allow(dead_code)]

pub mod async_task_store;
pub mod jsonl;
pub mod sandbox;
pub mod session_state;

use anyhow::{Context, Result};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::path::Path;

pub type DbPool = SqlitePool;

/// Phase 5c test helper: 返回当前 pool 的 size（已打开连接数）。
/// 用于测试判断两个 store 是否共享同一 SqlitePool 实例。
///
/// - 同池复用：size 计数共享 → 取连接后 size 增加
/// - 不同池实例：size 独立 → 互不影响
pub fn pool_size(pool: &DbPool) -> u32 {
    pool.size() as u32
}

pub async fn init_sqlite(db_path: &Path) -> Result<DbPool> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(db_path)
                .create_if_missing(true)
                .busy_timeout(std::time::Duration::from_secs(5)),
        )
        .await
        .with_context(|| format!("opening sqlite: {}", db_path.display()))?;

    run_migrations(&pool).await?;
    Ok(pool)
}

/// Phase 5: 在已打开的 pool 上补跑 migrations（用于 per-session DB 二次连接）
/// pool 必须是异步连接池
pub async fn init_sqlite_at(db_path: &Path, pool: &DbPool) -> Result<()> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    run_migrations(pool).await
}

async fn run_migrations(pool: &DbPool) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sandbox_outputs (
            handle TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            content TEXT NOT NULL,
            content_type TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX IF NOT EXISTS idx_sandbox_session ON sandbox_outputs(session_id);

        CREATE TABLE IF NOT EXISTS journal_entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            tool TEXT NOT NULL,
            file_path TEXT NOT NULL,
            action TEXT NOT NULL,
            before_hash TEXT,
            after_hash TEXT,
            before_snapshot TEXT NOT NULL,
            reversible INTEGER NOT NULL DEFAULT 1,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX IF NOT EXISTS idx_journal_session ON journal_entries(session_id);

        CREATE TABLE IF NOT EXISTS plans (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS todos (
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

        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'running',
            result TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            completed_at DATETIME
        );
        CREATE INDEX IF NOT EXISTS idx_tasks_session ON tasks(session_id);

        -- Phase 5: 异步任务按 session 持久化的新 schema（详细字段）
        -- 用 task_id 作为 PK；与上面旧的 id 字段兼容（用 task_id 即可）
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

        -- Phase 2: per-session loop_state / stop_reason（设计文档 §3.5/§3.6 统一 SessionSnapshot）
        -- 仅 1 行 per session（PRIMARY KEY = session_id）
        CREATE TABLE IF NOT EXISTS session_state (
            session_id TEXT PRIMARY KEY,
            loop_state TEXT NOT NULL DEFAULT 'idle', -- idle | running | stopped | waiting_for_user
            stop_reason TEXT,                         -- last stop reason（completed / cancelled / failed / max_iters_reached ...）
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        -- Phase 4: per-session pending Ask（ask_user 服务重启可恢复）
        CREATE TABLE IF NOT EXISTS pending_ask (
            session_id TEXT PRIMARY KEY,
            ask_id TEXT NOT NULL,
            tool_call_id TEXT NOT NULL,
            request_json TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'pending', -- pending | answered | cancelled
            submission_json TEXT,
            result_json TEXT,
            created_at_ms INTEGER NOT NULL,
            answered_at_ms INTEGER,
            cancelled_at_ms INTEGER,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        -- Phase 4: per-session pending Plan（plan approval 服务重启可恢复）
        CREATE TABLE IF NOT EXISTS pending_plan (
            session_id TEXT PRIMARY KEY,
            plan_id TEXT NOT NULL,
            content_json TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'pending', -- pending | approved | rejected | edited
            created_at_ms INTEGER NOT NULL,
            decided_at_ms INTEGER,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        -- Phase 5b / 终审修复 #15: per-session key-value attrs
        -- 用于持久化 role 等需要 snapshot 复原的轻量配置
        CREATE TABLE IF NOT EXISTS session_attrs (
            session_id TEXT NOT NULL,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (session_id, key)
        );
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}
