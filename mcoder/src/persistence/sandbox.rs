// 设计文档 §2.1: sandbox 输出持久化为 forward-looking scaffolding
// 当前 sandbox 输出走内存 handle；DB 持久化保留供未来大输出场景
#![allow(dead_code)]

use crate::persistence::DbPool;
use anyhow::Result;
use sqlx::Row;

pub struct SandboxOutput {
    pub handle: String,
    pub session_id: String,
    pub tool_name: String,
    pub content: String,
    pub content_type: String,
}

pub async fn store_output(
    pool: &DbPool,
    session_id: &str,
    tool_name: &str,
    content: &str,
    content_type: &str,
) -> Result<String> {
    let handle = format!(
        "out_{}",
        uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect::<String>()
    );

    sqlx::query(
        "INSERT INTO sandbox_outputs (handle, session_id, tool_name, content, content_type)
         VALUES (?, ?, ?, ?, ?)"
    )
    .bind(&handle)
    .bind(session_id)
    .bind(tool_name)
    .bind(content)
    .bind(content_type)
    .execute(pool)
    .await?;

    Ok(handle)
}

pub async fn get_output(pool: &DbPool, handle: &str) -> Result<Option<SandboxOutput>> {
    let row = sqlx::query(
        "SELECT handle, session_id, tool_name, content, content_type
         FROM sandbox_outputs WHERE handle = ?"
    )
    .bind(handle)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| SandboxOutput {
        handle: r.get(0),
        session_id: r.get(1),
        tool_name: r.get(2),
        content: r.get(3),
        content_type: r.get(4),
    }))
}

pub async fn read_range(
    pool: &DbPool,
    handle: &str,
    offset: usize,
    limit: usize,
) -> Result<Option<Vec<String>>> {
    let output = get_output(pool, handle).await?;
    Ok(output.map(|o| {
        o.content.lines()
            .skip(offset)
            .take(limit)
            .map(|s| s.to_string())
            .collect()
    }))
}
