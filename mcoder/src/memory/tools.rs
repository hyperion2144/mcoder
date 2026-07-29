use crate::memory::{MemoryEntry, MemoryScope};
use crate::tools::{Tool, ToolContext};
use crate::types::{ToolOutput, ToolSchema};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

/// 设计文档 §2.1 / §8.3: memory_store 工具
/// scope=project → 写入项目级 memory.db（仅当前项目可见）
/// scope=experience → 写入全局 experiences/sqlite.db（跨项目共享）
pub struct MemoryStoreTool;

#[async_trait]
impl Tool for MemoryStoreTool {
    fn name(&self) -> &str { "memory_store" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "memory_store".into(),
            description: "Store a memory entry. scope='project' for project-specific decisions (isolated to current project), scope='experience' for cross-project lessons (shared globally).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "enum": ["project", "experience"], "default": "project" },
                    "key": { "type": "string", "description": "Short identifier" },
                    "content": { "type": "string", "description": "Full memory content" },
                    "tags": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["key", "content"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
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
}

/// 设计文档 §8.3: memory_search 工具
/// 搜索项目记忆 + 全局经验，合并结果返回
pub struct MemorySearchTool;

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str { "memory_search" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "memory_search".into(),
            description: "Search memories by keyword. Searches current project's memories AND global experiences (cross-project lessons). Returns merged results.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "default": 10 }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let query: String = serde_json::from_value(args["query"].clone())?;
        let limit = args["limit"].as_u64().unwrap_or(10) as u32;

        // 设计文档 §8.3: 同时搜索项目记忆和全局经验
        let mut entries = ctx.memory_store.search(&query, Some(MemoryScope::Project), Some(&ctx.project_hash), limit)?;
        let experiences = ctx.experience_store.search(&query, Some(MemoryScope::Experience), None, limit)?;
        entries.extend(experiences);

        let result: Vec<Value> = entries.iter().map(|e| serde_json::json!({
            "id": e.id,
            "scope": e.scope.as_str(),
            "key": e.key,
            "content": e.content,
            "tags": e.tags,
        })).collect();

        Ok(ToolOutput::Sync { result: serde_json::Value::Array(result) })
    }
}

/// 设计文档 §8.3: memory_list 工具
/// scope=project → 当前项目记忆；scope=experience → 全局经验
pub struct MemoryListTool;

#[async_trait]
impl Tool for MemoryListTool {
    fn name(&self) -> &str { "memory_list" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "memory_list".into(),
            description: "List project memories (current project only) or global experiences (cross-project).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": { "type": "string", "enum": ["project", "experience"], "default": "project" }
                },
                "required": []
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
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
}
