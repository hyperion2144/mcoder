use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::types::{ToolOutput, ToolSchema};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;

/// 大输出存储：当工具输出超过阈值时，存到 sandbox 文件返回 handle
/// 后续用 sandbox_read 工具按 handle + offset/limit 分页读取
pub struct SandboxStore;

impl SandboxStore {
    pub fn store(project_dir: &PathBuf, content: &str) -> Result<String> {
        let sandbox_dir = project_dir.join("sandbox");
        std::fs::create_dir_all(&sandbox_dir)?;
        let handle = format!(
            "out_{}",
            uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect::<String>()
        );
        let path = sandbox_dir.join(format!("{}.txt", handle));
        std::fs::write(&path, content)?;
        Ok(handle)
    }

    pub fn read(project_dir: &PathBuf, handle: &str) -> Result<Option<String>> {
        let path = project_dir.join("sandbox").join(format!("{}.txt", handle));
        if !path.exists() { return Ok(None); }
        Ok(Some(std::fs::read_to_string(&path)?))
    }

    pub fn read_range(project_dir: &PathBuf, handle: &str, offset: usize, limit: usize) -> Result<Option<Vec<String>>> {
        let content = Self::read(project_dir, handle)?;
        Ok(content.map(|c| {
            c.lines().skip(offset).take(limit).map(|s| s.to_string()).collect()
        }))
    }
}

pub struct SandboxReadTool;

#[async_trait]
impl Tool for SandboxReadTool {
    fn name(&self) -> &str { "sandbox_read" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "sandbox_read".into(),
            description: "Read large output by handle. Use when previous tool output was truncated (handle returned in result). op=read|range|list.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["read", "range", "list"] },
                    "handle": { "type": "string", "description": "read/range: handle from previous truncated output" },
                    "offset": { "type": "integer", "description": "range: line offset (0-indexed), default 0" },
                    "limit": { "type": "integer", "description": "range: max lines to read, default 200" }
                },
                "required": ["op"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let op: String = serde_json::from_value(args["op"].clone())?;

        match op.as_str() {
            "read" => {
                let handle: String = serde_json::from_value(args["handle"].clone())?;
                let content = SandboxStore::read(&ctx.project_dir, &handle)?
                    .unwrap_or_default();
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "handle": handle,
                    "content": content,
                    "bytes": content.len()
                }) })
            }
            "range" => {
                let handle: String = serde_json::from_value(args["handle"].clone())?;
                let offset = args["offset"].as_u64().unwrap_or(0) as usize;
                let limit = args["limit"].as_u64().unwrap_or(200) as usize;
                let lines = SandboxStore::read_range(&ctx.project_dir, &handle, offset, limit)?
                    .unwrap_or_default();
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "handle": handle,
                    "offset": offset,
                    "returned": lines.len(),
                    "lines": lines
                }) })
            }
            "list" => {
                let sandbox_dir = ctx.project_dir.join("sandbox");
                let mut handles = Vec::new();
                if sandbox_dir.exists() {
                    for e in std::fs::read_dir(&sandbox_dir)? {
                        let e = e?;
                        let name = e.file_name().to_string_lossy().to_string();
                        if let Some(h) = name.strip_suffix(".txt") {
                            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                            handles.push(serde_json::json!({ "handle": h, "bytes": size }));
                        }
                    }
                }
                Ok(ToolOutput::Sync { result: serde_json::json!({ "handles": handles }) })
            }
            other => anyhow::bail!("unknown op: {}", other),
        }
    }
}
