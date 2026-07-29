// 设计文档 §2.1: sqlx-based DbPool 为 forward-looking alternative
// 当前实现使用 rusqlite 直接连接；sqlx 池化实现保留供未来切换
#![allow(dead_code)]

pub mod jsonl;
pub mod sandbox;

use anyhow::{Context, Result};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::path::Path;

pub type DbPool = SqlitePool;

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
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}
