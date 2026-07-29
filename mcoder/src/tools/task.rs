use crate::agent::async_tasks::TaskStatus;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;

/// Task 管理工具：暴露 AsyncTaskManager 给 agent
/// 设计文档 §3.7: task_status / task_wait / task_list / task_cancel
pub struct TaskTool;

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str { "task" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "task".into(),
            description: "Manage async background tasks. op=status|wait|list|cancel. Use to check on bash/code_exec/subagent tasks running in background.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["status", "wait", "list", "cancel"] },
                    "id": { "type": "string", "description": "status/wait/cancel: task id" },
                    "timeout_ms": { "type": "integer", "description": "wait: max wait in milliseconds, default 10000" },
                    "filter": { "type": "string", "description": "list: filter by status (running|completed|failed|cancelled)" }
                },
                "required": ["op"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let op: String = serde_json::from_value(args["op"].clone())?;

        match op.as_str() {
            "status" => {
                let id: String = serde_json::from_value(args["id"].clone())
                    .context("id required for status")?;
                let task = ctx.task_manager.get_task(&id).await
                    .context("task not found")?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "id": task.id,
                    "name": task.name,
                    "status": format!("{:?}", task.status),
                    "result": task.result,
                    "error": task.error
                }) })
            }
            "wait" => {
                let id: String = serde_json::from_value(args["id"].clone())
                    .context("id required for wait")?;
                let timeout_ms = args["timeout_ms"].as_u64().unwrap_or(10000);
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);

                loop {
                    let task = ctx.task_manager.get_task(&id).await
                        .context("task not found")?;
                    if task.status != TaskStatus::Pending && task.status != TaskStatus::Running {
                        return Ok(ToolOutput::Sync { result: serde_json::json!({
                            "id": task.id,
                            "status": format!("{:?}", task.status),
                            "result": task.result,
                            "error": task.error,
                            "waited": true
                        }) });
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Ok(ToolOutput::Sync { result: serde_json::json!({
                            "id": id,
                            "status": "running",
                            "timed_out": true,
                            "hint": "Task still running. Use op=status to check later."
                        }) });
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
            "list" => {
                let filter = args["filter"].as_str();
                let tasks = ctx.task_manager.list().await;
                let filtered: Vec<_> = tasks.into_iter()
                    .filter(|t| {
                        if let Some(f) = filter {
                            format!("{:?}", t.status).to_lowercase() == f.to_lowercase()
                        } else {
                            true
                        }
                    })
                    .map(|t| serde_json::json!({
                        "id": t.id,
                        "name": t.name,
                        "status": format!("{:?}", t.status),
                    }))
                    .collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "tasks": filtered,
                    "total": filtered.len()
                }) })
            }
            "cancel" => {
                let id: String = serde_json::from_value(args["id"].clone())
                    .context("id required for cancel")?;
                let ok = ctx.task_manager.cancel(&id).await;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "cancelled": ok,
                    "id": id
                }) })
            }
            other => anyhow::bail!("unknown op: {} (use status|wait|list|cancel)", other),
        }
    }
}
