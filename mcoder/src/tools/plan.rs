use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

// ==================== Plan 工具（结构化 steps）====================

/// plan - 统一计划工具
/// action=create: 创建结构化计划（原 plan_create）
/// action=update: 更新计划步骤状态（原 plan_update）
/// action=query: 读取当前 session 的计划状态（原 plan_query）
pub struct PlanTool;

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
impl Tool for PlanTool {
    fn name(&self) -> &str { "plan" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plan".into(),
            description: "Manage structured plans. action=create: create a plan with steps. action=update: update a step's status/note. action=query: read current session's plan state.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["create", "update", "query"], "description": "Operation to perform" },
                    "steps": {
                        "type": "array",
                        "description": "create: array of step objects",
                        "items": {
                            "type": "object",
                            "properties": {
                                "description": { "type": "string" },
                                "files_affected": { "type": "array", "items": { "type": "string" } },
                                "depends_on": { "type": "array", "items": { "type": "integer" } }
                            },
                            "required": ["description"]
                        }
                    },
                    "step_id": { "type": "integer", "description": "update: step id (1-indexed)" },
                    "status": { "type": "string", "enum": ["in_progress", "done", "skipped", "failed"], "description": "update: new status" },
                    "note": { "type": "string", "description": "update: optional note" }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let action: String = serde_json::from_value(args["action"].clone())?;

        match action.as_str() {
            // 原 plan_create: 创建结构化计划
            "create" => {
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
                // Phase 4: 写到 per-session SQLite pending_plan（不再写项目级 plan.json）
                let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
                let content_json = serde_json::to_value(&plan)
                    .context("serialize plan for pending_plan table")?;
                ctx.session_state
                    .create_pending_plan(&ctx.session_id, &plan_id, content_json, chrono::Utc::now().timestamp_millis())
                    .await
                    .map_err(|e| anyhow::anyhow!("plan_create: persist pending_plan failed: {}", e))?;
                // loop_state=waiting_for_user
                ctx.session_state
                    .set_session_state(&ctx.session_id, "waiting_for_user", Some("plan_pending"))
                    .await
                    .map_err(|e| anyhow::anyhow!("plan_create: set_session_state failed: {}", e))?;

                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "created": true,
                    "step_count": steps.len(),
                    "steps": steps,
                    "plan_id": plan_id,
                }) })
            }
            // 原 plan_update: 更新计划步骤状态
            "update" => {
                let step_id: u32 = serde_json::from_value(args["step_id"].clone())?;
                let status: String = serde_json::from_value(args["status"].clone())?;
                let note: Option<String> = args["note"].as_str().map(|s| s.to_string());

                // Phase 4: 从 per-session SQLite pending_plan 读取并回写
                let rec = ctx
                    .session_state
                    .get_pending_plan(&ctx.session_id)
                    .await
                    .context("no plan exists. Use plan create first.")?;

                // 终审修复 #7: 仅 pending plan 可改；approved/rejected/edited 后 step 状态应通过
                // 完成事件流同步，绝不允许在 plan 已决议后篡改 step。
                use crate::persistence::session_state::PendingPlanState;
                if rec.state != PendingPlanState::Pending {
                    anyhow::bail!(
                        "plan_update rejected: plan is in terminal state {:?}; \
                         use the live execution flow instead of mutating past plans",
                        rec.state
                    );
                }

                let mut plan: Plan = serde_json::from_value(rec.content.clone())
                    .context("plan_update: pending_plan.content parse failed")?;

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
                let content_json = serde_json::to_value(&plan)
                    .context("plan_update: serialize plan")?;
                // 仅更新 content，state 保持原样（plan_update 是执行期改 step 状态，不是用户决议）
                ctx.session_state
                    .update_pending_plan_content(&ctx.session_id, content_json)
                    .await
                    .map_err(|e| anyhow::anyhow!("plan_update: persist failed: {}", e))?;

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
            // 原 plan_query: 读取当前 session 的 plan 状态
            "query" => {
                let rec = match ctx.session_state.get_pending_plan(&ctx.session_id).await {
                    Some(r) => r,
                    None => {
                        return Ok(ToolOutput::Sync {
                            result: serde_json::json!({
                                "plan": null,
                                "exists": false,
                            }),
                        });
                    }
                };
                let state_str = match rec.state {
                    crate::persistence::session_state::PendingPlanState::Pending => "pending",
                    crate::persistence::session_state::PendingPlanState::Approved => "approved",
                    crate::persistence::session_state::PendingPlanState::Edited => "edited",
                    crate::persistence::session_state::PendingPlanState::Rejected => "rejected",
                };
                Ok(ToolOutput::Sync {
                    result: serde_json::json!({
                        "plan": {
                            "plan_id": rec.plan_id,
                            "state": state_str,
                            "content": rec.content,
                            "created_at_ms": rec.created_at_ms,
                            "decided_at_ms": rec.decided_at_ms,
                        },
                        "exists": true,
                    }),
                })
            }
            other => anyhow::bail!("unknown action: {} (use create|update|query)", other),
        }
    }
}

// ==================== Todo 工具（per-session, SQLite-backed）====================
//
// 取代旧版：项目级 .mcoder/plans/todo.json（不兼容旧数据）
// 数据存放在 per-project 的 todos.db (todos 表)，按 session_id 严格隔离
// ToolContext.session_state 由 SessionManager 自动注入（绑 session_id）
// 模型不可指定其他 session；任何操作都只影响当前 ctx.session_id

use crate::persistence::session_state::{
    TodoInput, TodoSummary, PRIORITY_MEDIUM,
    STATUS_PENDING,
    VALID_PRIORITIES, VALID_STATUSES,
};

pub struct TodoTool;

#[derive(Debug, Deserialize)]
struct TodoAddArgs {
    content: String,
    #[serde(default = "default_medium_priority")]
    priority: String,
    #[serde(default = "default_pending_status")]
    status: String,
}

#[derive(Debug, Deserialize)]
struct TodoUpdateArgs {
    id: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
}

fn default_medium_priority() -> String { PRIORITY_MEDIUM.into() }
fn default_pending_status() -> String { STATUS_PENDING.into() }

/// 把底层 sqlx::Error 转为 anyhow::Error 并附上当前 session_id 上下文
fn wrap_db_err(ctx: &ToolContext, op: &str, e: sqlx::Error) -> anyhow::Error {
    anyhow::anyhow!("todo.{} failed for session {}: {}", op, ctx.session_id, e)
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str { "todo" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "todo".into(),
            description: "Manage todo list scoped to the current session. \
                         action=list|replace|add|update|remove|clear_completed. \
                         status=pending|in_progress|completed|cancelled. \
                         priority=high|medium|low. \
                         Only one todo may be in_progress at a time; \
                         a snapshot is broadcast to clients after every change.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "replace", "add", "update", "remove", "clear_completed"],
                        "description": "Required. list=read; replace=set whole list; add/update/remove mutate single items; clear_completed removes completed."
                    },
                    "id": { "type": "string", "description": "update/remove: item id" },
                    "content": { "type": "string", "description": "add/update: todo text (replace: per-item content)" },
                    "status": { "type": "string", "enum": VALID_STATUSES, "description": "add/update: status; replace: per-item status" },
                    "priority": { "type": "string", "enum": VALID_PRIORITIES, "default": "medium" },
                    "items": {
                        "type": "array",
                        "description": "replace: full todo list [{content,status,priority}]; order in array defines stable sort order",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": { "type": "string" },
                                "status": { "type": "string", "enum": VALID_STATUSES },
                                "priority": { "type": "string", "enum": VALID_PRIORITIES, "default": "medium" }
                            },
                            "required": ["content"]
                        }
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let action: String = serde_json::from_value(args["action"].clone())
            .or_else(|_| serde_json::from_value(args["op"].clone()))?;

        let store = &ctx.session_state;
        let session_id = &ctx.session_id;

        match action.as_str() {
            "list" => {
                let items = store.list_todos(session_id).await.map_err(|e| wrap_db_err(ctx, "list", e))?;
                let summary = TodoSummary::from_items(&items);
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "items": items,
                    "summary": summary,
                }) })
            }
            "add" => {
                let parsed: TodoAddArgs = serde_json::from_value(args.clone())
                    .context("todo.add: invalid args, need {content, priority?, status?}")?;
                let input = TodoInput::new(parsed.content, parsed.status, parsed.priority);
                let record = store.add_todo(session_id, input).await.map_err(|e| wrap_db_err(ctx, "add", e))?;
                let _ = self.broadcast(ctx, &record.id).await;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "added": record,
                }) })
            }
            "update" => {
                let parsed: TodoUpdateArgs = serde_json::from_value(args.clone())
                    .context("todo.update: invalid args, need {id, content?, status?, priority?}")?;
                let record = store.update_todo(
                    session_id,
                    &parsed.id,
                    parsed.content.as_deref(),
                    parsed.status.as_deref(),
                    parsed.priority.as_deref(),
                ).await.map_err(|e| wrap_db_err(ctx, "update", e))?;
                let _ = self.broadcast(ctx, &record.id).await;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "updated": record,
                }) })
            }
            "remove" | "delete" => {
                let id: String = serde_json::from_value(args["id"].clone())
                    .context("todo.remove: missing id")?;
                let removed = store.remove_todo(session_id, &id).await.map_err(|e| wrap_db_err(ctx, "remove", e))?;
                if removed {
                    let _ = self.broadcast(ctx, &id).await;
                }
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "removed": id,
                    "existed": removed,
                }) })
            }
            "clear_completed" => {
                let n = store.clear_completed_todos(session_id).await.map_err(|e| wrap_db_err(ctx, "clear_completed", e))?;
                let _ = self.broadcast(ctx, "").await;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "cleared_completed": n,
                }) })
            }
            "replace" => {
                // items: array of {content, status?, priority?}
                let raw = args.get("items").cloned()
                    .ok_or_else(|| anyhow::anyhow!("todo.replace: missing 'items'"))?;
                let inputs_raw: Vec<serde_json::Value> = serde_json::from_value(raw)
                    .context("todo.replace: items must be an array")?;
                let mut inputs: Vec<TodoInput> = Vec::with_capacity(inputs_raw.len());
                for (i, v) in inputs_raw.iter().enumerate() {
                    let content = v.get("content").and_then(|x| x.as_str())
                        .ok_or_else(|| anyhow::anyhow!("todo.replace: items[{}].content required", i))?;
                    let status = v.get("status").and_then(|x| x.as_str()).unwrap_or(STATUS_PENDING);
                    let priority = v.get("priority").and_then(|x| x.as_str()).unwrap_or(PRIORITY_MEDIUM);
                    inputs.push(TodoInput::new(content.to_string(), status.to_string(), priority.to_string()));
                }
                let records = store.replace_todos(session_id, inputs).await.map_err(|e| wrap_db_err(ctx, "replace", e))?;
                let _ = self.broadcast(ctx, "").await;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "replaced": true,
                    "count": records.len(),
                    "items": records,
                }) })
            }
            other => anyhow::bail!("unknown action: {} (use list|replace|add|update|remove|clear_completed)", other),
        }
    }
}

impl TodoTool {
    /// 广播 TodoUpdated；广播失败仅 warn，不影响工具调用本身
    async fn broadcast(&self, ctx: &ToolContext, _changed_id: &str) -> Result<()> {
        let items = ctx.session_state.list_todos(&ctx.session_id).await
            .map_err(|e| wrap_db_err(ctx, "broadcast.list", e))?;
        let summary = TodoSummary::from_items(&items);
        let _ = ctx.event_tx.send(crate::session_manager::ServerEvent::TodoUpdated {
            session_id: ctx.session_id.clone(),
            todos: items,
            summary,
        });
        Ok(())
    }
}
