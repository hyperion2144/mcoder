use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

// ==================== Plan 工具（结构化 steps）====================

/// 设计文档 §4.7: plan_create({ steps: [{ description, files_affected?, depends_on? }] })
pub struct PlanCreateTool;

/// 设计文档 §4.7: plan_update({ step_id, status, note? })
pub struct PlanUpdateTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: u32,
    pub description: String,
    #[serde(default)]
    pub files_affected: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<u32>,
    pub status: String, // pending | in_progress | done | skipped | failed
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
    pub created_at: String,
    pub updated_at: String,
}

fn plan_path(project_dir: &PathBuf) -> PathBuf {
    project_dir.join("plans").join("plan.json")
}

fn load_plan(project_dir: &PathBuf) -> Result<Option<Plan>> {
    let path = plan_path(project_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&content).unwrap_or(Plan {
        steps: Vec::new(),
        created_at: String::new(),
        updated_at: String::new(),
    })))
}

fn save_plan(project_dir: &PathBuf, plan: &Plan) -> Result<()> {
    let path = plan_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(plan)?)?;
    Ok(())
}

#[async_trait]
impl Tool for PlanCreateTool {
    fn name(&self) -> &str { "plan_create" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plan_create".into(),
            description: "Create a structured plan with steps. Each step: {description, files_affected?, depends_on?}. Replaces existing plan.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "description": { "type": "string" },
                                "files_affected": { "type": "array", "items": { "type": "string" } },
                                "depends_on": { "type": "array", "items": { "type": "integer" } }
                            },
                            "required": ["description"]
                        }
                    }
                },
                "required": ["steps"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let steps_val: Vec<Value> = serde_json::from_value(args["steps"].clone())
            .context("steps array required")?;

        let now = chrono::Utc::now().to_rfc3339();
        let mut steps: Vec<PlanStep> = Vec::new();
        for (i, sv) in steps_val.iter().enumerate() {
            let description: String = serde_json::from_value(sv["description"].clone())
                .context(format!("step {} missing description", i))?;
            let files_affected: Vec<String> = sv["files_affected"].as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let depends_on: Vec<u32> = sv["depends_on"].as_array()
                .map(|a| a.iter().filter_map(|v| v.as_u64().map(|n| n as u32)).collect())
                .unwrap_or_default();
            steps.push(PlanStep {
                id: (i + 1) as u32,
                description,
                files_affected,
                depends_on,
                status: "pending".into(),
                note: None,
            });
        }

        let plan = Plan {
            steps: steps.clone(),
            created_at: now.clone(),
            updated_at: now,
        };
        save_plan(&ctx.project_dir, &plan)?;

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "created": true,
            "step_count": steps.len(),
            "steps": steps,
            "path": plan_path(&ctx.project_dir).display().to_string()
        }) })
    }
}

#[async_trait]
impl Tool for PlanUpdateTool {
    fn name(&self) -> &str { "plan_update" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plan_update".into(),
            description: "Update a plan step's status/note. status=in_progress|done|skipped|failed.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "step_id": { "type": "integer", "description": "Step id (1-indexed)" },
                    "status": { "type": "string", "enum": ["in_progress", "done", "skipped", "failed"] },
                    "note": { "type": "string", "description": "Optional note about the update" }
                },
                "required": ["step_id", "status"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let step_id: u32 = serde_json::from_value(args["step_id"].clone())?;
        let status: String = serde_json::from_value(args["status"].clone())?;
        let note: Option<String> = args["note"].as_str().map(|s| s.to_string());

        let mut plan = load_plan(&ctx.project_dir)?
            .context("no plan exists. Use plan_create first.")?;

        let mut found = false;
        for step in plan.steps.iter_mut() {
            if step.id == step_id {
                step.status = status.clone();
                if note.is_some() { step.note = note.clone(); }
                found = true;
                break;
            }
        }

        if !found {
            anyhow::bail!("step {} not found in plan", step_id);
        }

        plan.updated_at = chrono::Utc::now().to_rfc3339();
        save_plan(&ctx.project_dir, &plan)?;

        // 统计进度
        let total = plan.steps.len();
        let done = plan.steps.iter().filter(|s| s.status == "done").count();
        let in_progress = plan.steps.iter().filter(|s| s.status == "in_progress").count();
        let failed = plan.steps.iter().filter(|s| s.status == "failed").count();

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "updated": true,
            "step_id": step_id,
            "status": status,
            "progress": {
                "total": total,
                "done": done,
                "in_progress": in_progress,
                "failed": failed,
                "pending": total - done - in_progress - failed
            }
        }) })
    }
}

// ==================== Todo 工具 ====================

pub struct TodoTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TodoItem {
    id: String,
    content: String,
    status: String,
    priority: String,
}

impl TodoTool {
    fn todo_path(project_dir: &PathBuf) -> PathBuf {
        project_dir.join("plans").join("todo.json")
    }

    fn load(project_dir: &PathBuf) -> Result<Vec<TodoItem>> {
        let path = Self::todo_path(project_dir);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&content).unwrap_or_default())
    }

    fn save(project_dir: &PathBuf, items: &[TodoItem]) -> Result<()> {
        let path = Self::todo_path(project_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(items)?)?;
        Ok(())
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str { "todo" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "todo".into(),
            description: "Manage todo list. action=list|add|update|remove. status=pending|in_progress|done. priority=high|medium|low.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "add", "update", "remove"] },
                    "id": { "type": "string", "description": "update/remove: item id" },
                    "content": { "type": "string", "description": "add: todo text" },
                    "status": { "type": "string", "enum": ["pending", "in_progress", "done"] },
                    "priority": { "type": "string", "enum": ["high", "medium", "low"], "default": "medium" }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let action: String = serde_json::from_value(args["action"].clone())
            .or_else(|_| serde_json::from_value(args["op"].clone()))?;
        let mut items = Self::load(&ctx.project_dir)?;

        match action.as_str() {
            "add" => {
                let content: String = serde_json::from_value(args["content"].clone())
                    .context("content required for add")?;
                let priority = args["priority"].as_str().unwrap_or("medium").to_string();
                let id = format!("td-{}", chrono::Utc::now().timestamp_millis());
                let item = TodoItem {
                    id: id.clone(),
                    content,
                    status: "pending".into(),
                    priority,
                };
                items.push(item.clone());
                Self::save(&ctx.project_dir, &items)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({ "added": item }) })
            }
            "list" => {
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "items": items,
                    "total": items.len(),
                    "pending": items.iter().filter(|i| i.status == "pending").count(),
                    "in_progress": items.iter().filter(|i| i.status == "in_progress").count(),
                    "completed": items.iter().filter(|i| i.status == "done").count(),
                }) })
            }
            "update" => {
                let id: String = serde_json::from_value(args["id"].clone())?;
                for item in items.iter_mut() {
                    if item.id == id {
                        if let Some(s) = args["status"].as_str() { item.status = s.into(); }
                        if let Some(p) = args["priority"].as_str() { item.priority = p.into(); }
                        if let Some(c) = args["content"].as_str() { item.content = c.into(); }
                    }
                }
                Self::save(&ctx.project_dir, &items)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({ "updated": id }) })
            }
            "remove" | "delete" => {
                let id: String = serde_json::from_value(args["id"].clone())?;
                items.retain(|i| i.id != id);
                Self::save(&ctx.project_dir, &items)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({ "deleted": id }) })
            }
            other => anyhow::bail!("unknown action: {} (use list|add|update|remove)", other),
        }
    }
}
