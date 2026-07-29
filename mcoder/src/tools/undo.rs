use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;

/// Undo 工具：暴露 FileJournal 的撤销能力给 agent
/// 设计文档 §4.9: /undo 撤销最后一次文件变更 / /undo <id> 撤销指定变更 / /undo --list
pub struct UndoTool;

#[async_trait]
impl Tool for UndoTool {
    fn name(&self) -> &str { "undo" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "undo".into(),
            description: "Undo file changes recorded by FileJournal. op=last|entry|batch|list. last=undo most recent change; entry=undo by journal_id; batch=undo by batch_id; list=list recent entries.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["last", "entry", "batch", "list"] },
                    "id": { "type": "string", "description": "entry: journal_id; batch: batch_id" },
                    "limit": { "type": "integer", "description": "list: max entries to return, default 20" }
                },
                "required": ["op"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let op: String = serde_json::from_value(args["op"].clone())?;

        match op.as_str() {
            "last" => {
                ctx.journal.undo_last()
                    .context("undo last failed")?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "op": "last",
                    "undone": true,
                    "hint": "If batch, all files in batch were reverted."
                }) })
            }
            "entry" => {
                let id: String = serde_json::from_value(args["id"].clone())
                    .context("id required for entry")?;
                let entry = ctx.journal.get_entry(&id)
                    .context("journal entry not found")?;
                let path = entry.path.clone();
                ctx.journal.undo(&id)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "op": "entry",
                    "id": id,
                    "file": path.display().to_string(),
                    "undone": true
                }) })
            }
            "batch" => {
                let batch_id: String = serde_json::from_value(args["id"].clone())
                    .context("id (batch_id) required for batch")?;
                ctx.journal.undo_batch(&batch_id)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "op": "batch",
                    "batch_id": batch_id,
                    "undone": true
                }) })
            }
            "list" => {
                let limit = args["limit"].as_u64().unwrap_or(20) as usize;
                let entries = ctx.journal.list(limit);
                let list: Vec<_> = entries.iter().map(|e| serde_json::json!({
                    "id": e.id,
                    "timestamp": e.timestamp,
                    "path": e.path.display().to_string(),
                    "operation": e.operation,
                    "batch_id": e.batch_id
                })).collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "entries": list,
                    "count": list.len()
                }) })
            }
            other => anyhow::bail!("unknown op: {} (use last|entry|batch|list)", other),
        }
    }
}
