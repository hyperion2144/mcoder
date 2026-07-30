use crate::memory::{MemoryEntry, MemoryScope};
use crate::tools::{Tool, ToolContext};
use crate::types::{ToolOutput, ToolSchema};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

/// memory - 统一记忆工具
/// action=store: 存储记忆（原 memory_store）
/// action=search: 搜索记忆（原 memory_search）
/// action=list: 列出记忆（原 memory_list）
pub struct MemoryTool;

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str { "memory" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "memory".into(),
            description: "Manage memories. action=store: store a memory entry (scope=project|experience). action=search: search memories by keyword (searches project + global experiences). action=list: list project memories or global experiences.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["store", "search", "list"], "description": "Operation to perform" },
                    "scope": { "type": "string", "enum": ["project", "experience"], "description": "store: memory scope (default project). search: optional filter. list: which scope to list (default project)." },
                    "key": { "type": "string", "description": "store: short identifier" },
                    "content": { "type": "string", "description": "store: full memory content" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "store: optional tags" },
                    "query": { "type": "string", "description": "search: search keyword" },
                    "limit": { "type": "integer", "default": 10, "description": "search: max results" }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let action: String = serde_json::from_value(args["action"].clone())?;

        match action.as_str() {
            // 原 memory_store: 存储记忆
            "store" => {
                let scope_str: String = serde_json::from_value(args["scope"].clone())
                    .unwrap_or_else(|_| "project".to_string());
                let scope = if scope_str == "experience" {
                    MemoryScope::Experience
                } else {
                    MemoryScope::Project
                };
                let key: String = serde_json::from_value(args["key"].clone())?;
                let content: String = serde_json::from_value(args["content"].clone())?;
                let tags: Vec<String> = args["tags"].as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                let project_hash = if scope == MemoryScope::Project {
                    Some(ctx.project_hash.clone())
                } else {
                    None
                };

                let entry = MemoryEntry {
                    id: None,
                    scope,
                    key,
                    content,
                    tags,
                    created_at: String::new(),
                    updated_at: String::new(),
                    project_hash,
                };

                // 设计文档 §8.3: experience 写入全局库，project 写入项目库
                let store = if scope == MemoryScope::Experience {
                    &ctx.experience_store
                } else {
                    &ctx.memory_store
                };
                let id = store.store(&entry)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({ "stored": true, "id": id, "scope": scope_str }) })
            }
            // 原 memory_search: 搜索项目记忆 + 全局经验，合并结果返回
            "search" => {
                let query: String = serde_json::from_value(args["query"].clone())?;
                let limit = args["limit"].as_u64().unwrap_or(10) as u32;
                let scope_filter: Option<String> = args["scope"].as_str().map(|s| s.to_string());

                // 设计文档 §8.3: 同时搜索项目记忆和全局经验（scope 可选过滤）
                let mut entries = if scope_filter.as_deref() != Some("experience") {
                    ctx.memory_store.search(&query, Some(MemoryScope::Project), Some(&ctx.project_hash), limit)?
                } else {
                    Vec::new()
                };
                if scope_filter.as_deref() != Some("project") {
                    let experiences = ctx.experience_store.search(&query, Some(MemoryScope::Experience), None, limit)?;
                    entries.extend(experiences);
                }

                let result: Vec<Value> = entries.iter().map(|e| serde_json::json!({
                    "id": e.id,
                    "scope": e.scope.as_str(),
                    "key": e.key,
                    "content": e.content,
                    "tags": e.tags,
                })).collect();

                Ok(ToolOutput::Sync { result: serde_json::Value::Array(result) })
            }
            // 原 memory_list: 列出项目记忆或全局经验
            "list" => {
                let scope = args["scope"].as_str().unwrap_or("project");
                let entries = if scope == "experience" {
                    ctx.experience_store.list_experiences(50)?
                } else {
                    ctx.memory_store.list_project(&ctx.project_hash)?
                };

                let result: Vec<Value> = entries.iter().map(|e| serde_json::json!({
                    "id": e.id,
                    "key": e.key,
                    "content": &e.content[..e.content.len().min(100)],
                    "tags": e.tags,
                })).collect();

                Ok(ToolOutput::Sync { result: serde_json::Value::Array(result) })
            }
            other => anyhow::bail!("unknown action: {} (use store|search|list)", other),
        }
    }
}
