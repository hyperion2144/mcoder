use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;

/// 工作流创建工具：roadmap / milestone / change / spec / task / implementation
pub struct WorkflowCreateTool;

#[async_trait]
impl Tool for WorkflowCreateTool {
    fn name(&self) -> &str { "workflow_create" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "workflow_create".into(),
            description: "Create workflow entities. op=init|roadmap|milestone|change|spec|task|implementation|proposal|design|review. init=one-shot create roadmap+milestone+change. Blueprint-style project management with 5-phase cycle (propose->plan->apply->review->archive).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["init", "roadmap", "milestone", "change", "spec", "task", "implementation", "proposal", "design", "review"] },
                    "profile": { "type": "string", "enum": ["lite", "standard"], "description": "init/roadmap: workflow profile, default standard" },
                    "roadmap_id": { "type": "string", "description": "milestone: parent roadmap id" },
                    "milestone_id": { "type": "string", "description": "change: parent milestone id" },
                    "change_id": { "type": "string", "description": "spec/task/proposal/design/review: parent change id" },
                    "task_id": { "type": "string", "description": "implementation: parent task id" },
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "milestone_title": { "type": "string", "description": "init: first milestone title" },
                    "change_title": { "type": "string", "description": "init: first change title" },
                    "content": { "type": "string", "description": "spec/proposal/design/review: full content" },
                    "tdd": { "type": "boolean", "description": "spec: enable TDD mode, default false" },
                    "order": { "type": "integer", "description": "milestone/task: sort order, default 0" },
                    "verdict": { "type": "string", "enum": ["pass", "fail", "needs_work"], "description": "review: verdict, default needs_work" }
                },
                "required": ["op", "title"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let op: String = serde_json::from_value(args["op"].clone())?;
        let title: String = serde_json::from_value(args["title"].clone())
            .context("title required")?;

        let id = match op.as_str() {
            // 设计文档 §8.5: /workflow init 一次性创建初始 workflow
            "init" => {
                let desc = args["description"].as_str().unwrap_or("").to_string();
                let profile_str = args["profile"].as_str().unwrap_or("standard");
                let profile = match profile_str {
                    "lite" => crate::workflow::WorkflowProfile::Lite,
                    _ => crate::workflow::WorkflowProfile::Standard,
                };
                let milestone_title = args["milestone_title"].as_str().unwrap_or("Milestone 1").to_string();
                let change_title = args["change_title"].as_str().unwrap_or(&title).to_string();
                let (roadmap_id, milestone_id, change_id) = ctx.workflow.init_workflow(
                    &title, &desc, profile, &milestone_title, &change_title,
                )?;
                return Ok(ToolOutput::Sync { result: serde_json::json!({
                    "created": true,
                    "op": "init",
                    "roadmap_id": roadmap_id,
                    "milestone_id": milestone_id,
                    "change_id": change_id,
                    "phase": "propose",
                    "profile": profile_str,
                    "hint": "Use workflow_update op=phase_next to advance to plan phase"
                }) });
            }
            "roadmap" => {
                let desc = args["description"].as_str().unwrap_or("").to_string();
                let profile_str = args["profile"].as_str().unwrap_or("standard");
                let profile = match profile_str {
                    "lite" => crate::workflow::WorkflowProfile::Lite,
                    _ => crate::workflow::WorkflowProfile::Standard,
                };
                ctx.workflow.create_roadmap_with_profile(&title, &desc, profile)?
            }
            "milestone" => {
                let roadmap_id: String = serde_json::from_value(args["roadmap_id"].clone())
                    .context("roadmap_id required for milestone")?;
                let desc = args["description"].as_str().unwrap_or("").to_string();
                let order = args["order"].as_u64().unwrap_or(0) as u32;
                ctx.workflow.create_milestone(&roadmap_id, &title, &desc, order)?
            }
            "change" => {
                let milestone_id: String = serde_json::from_value(args["milestone_id"].clone())
                    .context("milestone_id required for change")?;
                let desc = args["description"].as_str().unwrap_or("").to_string();
                ctx.workflow.create_change(&milestone_id, &title, &desc)?
            }
            "spec" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for spec")?;
                let content: String = serde_json::from_value(args["content"].clone())
                    .context("content required for spec")?;
                let tdd = args["tdd"].as_bool().unwrap_or(false);
                ctx.workflow.create_spec(&change_id, &title, &content, tdd)?
            }
            "task" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for task")?;
                let desc = args["description"].as_str().unwrap_or("").to_string();
                let order = args["order"].as_u64().unwrap_or(0) as u32;
                ctx.workflow.create_task(&change_id, &title, &desc, order)?
            }
            "implementation" => {
                let task_id: String = serde_json::from_value(args["task_id"].clone())
                    .context("task_id required for implementation")?;
                let desc = args["description"].as_str().unwrap_or("").to_string();
                ctx.workflow.create_implementation(&task_id, &title, &desc)?
            }
            "proposal" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for proposal")?;
                let content: String = serde_json::from_value(args["content"].clone())
                    .context("content required for proposal")?;
                ctx.workflow.create_proposal(&change_id, &title, &content)?
            }
            "design" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for design")?;
                let content: String = serde_json::from_value(args["content"].clone())
                    .context("content required for design")?;
                ctx.workflow.create_design(&change_id, &title, &content)?
            }
            "review" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for review")?;
                let content: String = serde_json::from_value(args["content"].clone())
                    .context("content required for review")?;
                let verdict_str = args["verdict"].as_str().unwrap_or("needs_work");
                let verdict = crate::workflow::ReviewVerdict::from_str(verdict_str)
                    .context("invalid verdict (use pass|fail|needs_work)")?;
                ctx.workflow.create_review(&change_id, &title, &content, verdict)?
            }
            other => anyhow::bail!("unknown op: {}", other),
        };

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "created": true,
            "op": op,
            "id": id,
            "title": title
        }) })
    }
}

/// 工作流查询工具：列出 roadmaps / milestones / changes / tasks
pub struct WorkflowQueryTool;

#[async_trait]
impl Tool for WorkflowQueryTool {
    fn name(&self) -> &str { "workflow_query" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "workflow_query".into(),
            description: "Query workflow entities. op=roadmaps|milestones|changes|tasks|tasks_full|proposals|designs|specs|reviews|list. Returns lists of entities.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["roadmaps", "milestones", "changes", "tasks", "tasks_full", "proposals", "designs", "specs", "reviews", "list"] },
                    "roadmap_id": { "type": "string", "description": "milestones: parent roadmap id" },
                    "milestone_id": { "type": "string", "description": "changes: parent milestone id" },
                    "change_id": { "type": "string", "description": "tasks/tasks_full/proposals/designs/specs/reviews: parent change id" }
                },
                "required": ["op"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let op: String = serde_json::from_value(args["op"].clone())?;

        match op.as_str() {
            "roadmaps" => {
                let list = ctx.workflow.list_roadmaps()?;
                let arr: Vec<_> = list.into_iter().map(|(id, title, status)| {
                    serde_json::json!({ "id": id, "title": title, "status": status })
                }).collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({ "roadmaps": arr }) })
            }
            "milestones" => {
                let roadmap_id: String = serde_json::from_value(args["roadmap_id"].clone())
                    .context("roadmap_id required for milestones")?;
                let list = ctx.workflow.get_milestones(&roadmap_id)?;
                let arr: Vec<_> = list.into_iter().map(|(id, title, status, order)| {
                    serde_json::json!({ "id": id, "title": title, "status": status, "order": order })
                }).collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({ "milestones": arr }) })
            }
            "changes" => {
                let milestone_id: String = serde_json::from_value(args["milestone_id"].clone())
                    .context("milestone_id required for changes")?;
                let list = ctx.workflow.get_changes(&milestone_id)?;
                let arr: Vec<_> = list.into_iter().map(|(id, title, status, phase)| {
                    serde_json::json!({ "id": id, "title": title, "status": status, "phase": phase })
                }).collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({ "changes": arr }) })
            }
            "tasks" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for tasks")?;
                let list = ctx.workflow.get_tasks(&change_id)?;
                let arr: Vec<_> = list.into_iter().map(|(id, title, status, order)| {
                    serde_json::json!({ "id": id, "title": title, "status": status, "order": order })
                }).collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({ "tasks": arr }) })
            }
            "tasks_full" => {
                // 含 impl_status 字段的完整 task 查询
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for tasks_full")?;
                let list = ctx.workflow.get_tasks_with_impl(&change_id)?;
                let arr: Vec<_> = list.into_iter().map(|(id, title, status, order, impl_status)| {
                    serde_json::json!({ "id": id, "title": title, "status": status, "order": order, "impl_status": impl_status })
                }).collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({ "tasks": arr }) })
            }
            "proposals" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for proposals")?;
                let list = ctx.workflow.get_proposals(&change_id)?;
                let arr: Vec<_> = list.into_iter().map(|(id, title, content)| {
                    serde_json::json!({ "id": id, "title": title, "content": content })
                }).collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({ "proposals": arr }) })
            }
            "designs" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for designs")?;
                let list = ctx.workflow.get_designs(&change_id)?;
                let arr: Vec<_> = list.into_iter().map(|(id, title, content)| {
                    serde_json::json!({ "id": id, "title": title, "content": content })
                }).collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({ "designs": arr }) })
            }
            "specs" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for specs")?;
                let list = ctx.workflow.get_specs(&change_id)?;
                let arr: Vec<_> = list.into_iter().map(|(id, title, content, tdd)| {
                    serde_json::json!({ "id": id, "title": title, "content": content, "tdd": tdd })
                }).collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({ "specs": arr }) })
            }
            "reviews" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for reviews")?;
                let list = ctx.workflow.get_reviews(&change_id)?;
                let arr: Vec<_> = list.into_iter().map(|(id, title, content, verdict)| {
                    serde_json::json!({ "id": id, "title": title, "content": content, "verdict": verdict })
                }).collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({ "reviews": arr }) })
            }
            "list" => {
                let roadmaps = ctx.workflow.list_roadmaps()?;
                let mut arr = Vec::new();
                for (id, title, status) in &roadmaps {
                    let milestones = ctx.workflow.get_milestones(id)?;
                    let milestone_count = milestones.len() as u32;
                    let mut change_count = 0u32;
                    for (ms_id, _, _, _) in &milestones {
                        let changes = ctx.workflow.get_changes(ms_id)?;
                        change_count += changes.len() as u32;
                    }
                    arr.push(serde_json::json!({
                        "id": id,
                        "title": title,
                        "status": status,
                        "milestone_count": milestone_count,
                        "change_count": change_count
                    }));
                }
                Ok(ToolOutput::Sync { result: serde_json::json!({ "roadmaps": arr }) })
            }
            other => anyhow::bail!("unknown op: {} (use roadmaps|milestones|changes|tasks|tasks_full|proposals|designs|specs|reviews|list)", other),
        }
    }
}

/// 工作流更新工具：更新任务状态、推进 5 步循环阶段等
/// 设计文档 §8.5: 支持 /workflow slash command 的 propose/plan/apply/review/archive/continue 操作
pub struct WorkflowUpdateTool;

#[async_trait]
impl Tool for WorkflowUpdateTool {
    fn name(&self) -> &str { "workflow_update" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "workflow_update".into(),
            description: "Update workflow entities. op=task_status|phase_next|phase_set|roadmap_status|milestone_status|change_status|impl_status|spec_content. task_status updates task (todo|in_progress|done|blocked). phase_next advances change to next phase (propose->plan->apply->review->archive). phase_set explicitly sets phase (for rollback). impl_status updates task impl status (draft|in_progress|done|blocked).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["task_status", "phase_next", "phase_set", "roadmap_status", "milestone_status", "change_status", "impl_status", "spec_content"] },
                    "task_id": { "type": "string", "description": "task_status/impl_status: target task id" },
                    "status": { "type": "string", "enum": ["todo", "in_progress", "done", "blocked", "draft", "active", "completed", "archived", "cancelled"], "description": "task_status/impl_status/roadmap_status/milestone_status/change_status: new status" },
                    "change_id": { "type": "string", "description": "phase_next/phase_set/change_status: target change id" },
                    "phase": { "type": "string", "enum": ["propose", "plan", "apply", "review", "archive"], "description": "phase_set: target phase" },
                    "roadmap_id": { "type": "string", "description": "roadmap_status: target roadmap id" },
                    "milestone_id": { "type": "string", "description": "milestone_status: target milestone id" },
                    "spec_id": { "type": "string", "description": "spec_content: target spec id" },
                    "content": { "type": "string", "description": "spec_content: new spec content" }
                },
                "required": ["op"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let op: String = serde_json::from_value(args["op"].clone())?;

        match op.as_str() {
            "task_status" => {
                let task_id: String = serde_json::from_value(args["task_id"].clone())?;
                let status: String = serde_json::from_value(args["status"].clone())?;
                ctx.workflow.update_task_status(&task_id, &status)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "updated": true,
                    "task_id": task_id,
                    "status": status
                }) })
            }
            // 设计文档 §8.5: 5 步循环推进（/workflow continue）
            "phase_next" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for phase_next")?;
                let current = ctx.workflow.get_change_phase(&change_id)?;
                let next = ctx.workflow.transition_phase(&change_id)?;

                let hint = match next {
                    crate::workflow::WorkflowPhase::Plan => "planner sub-agent should now generate spec/tasks",
                    crate::workflow::WorkflowPhase::Apply => "executor sub-agent should now implement the spec",
                    crate::workflow::WorkflowPhase::Review => "reviewer sub-agent should now review the implementation",
                    crate::workflow::WorkflowPhase::Archive => "change archived (completed)",
                    _ => "",
                };

                let mut result = serde_json::json!({
                    "updated": true,
                    "change_id": change_id,
                    "previous_phase": current.as_str(),
                    "new_phase": next.as_str(),
                    "hint": hint
                });

                // 注入 spawn_subagent 字段：agent loop 检测到此字段会自动调度子代理
                if let Some(spawn_hint) = crate::workflow::WorkflowOrchestrator::schedule_for_phase(next, &change_id) {
                    result["spawn_subagent"] = serde_json::json!({
                        "role": spawn_hint.role,
                        "change_id": spawn_hint.change_id,
                        "phase": spawn_hint.phase,
                        "prompt": spawn_hint.prompt,
                    });
                }

                // P3: TDD 强制 - 进入 apply 阶段且 spec.tdd=true 时，注入 tdd_warning
                if next == crate::workflow::WorkflowPhase::Apply {
                    if let Ok(Some(spec)) = ctx.workflow.get_spec_for_change(&change_id) {
                        if spec.3 {
                            result["tdd_warning"] = serde_json::json!(
                                "This change has TDD enabled. Implementation MUST follow RED->GREEN->REFACTOR: write a failing test first, then minimal implementation, then refactor."
                            );
                            result["hint"] = serde_json::json!(
                                "executor sub-agent should now implement the spec (TDD mode: RED->GREEN->REFACTOR)"
                            );
                        }
                    }
                }

                Ok(ToolOutput::Sync { result })
            }
            // 设计文档 §8.5: 显式设置 phase（用于回退或跳转）
            "phase_set" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for phase_set")?;
                let phase_str: String = serde_json::from_value(args["phase"].clone())
                    .context("phase required for phase_set")?;
                let phase = crate::workflow::WorkflowPhase::from_str(&phase_str)
                    .context("invalid phase value (use propose|plan|apply|review|archive)")?;
                ctx.workflow.set_phase(&change_id, phase)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "updated": true,
                    "change_id": change_id,
                    "phase": phase_str
                }) })
            }
            "roadmap_status" => {
                let roadmap_id: String = serde_json::from_value(args["roadmap_id"].clone())
                    .context("roadmap_id required for roadmap_status")?;
                let status: String = serde_json::from_value(args["status"].clone())
                    .context("status required for roadmap_status")?;
                ctx.workflow.update_roadmap_status(&roadmap_id, &status)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "updated": true,
                    "roadmap_id": roadmap_id,
                    "status": status
                }) })
            }
            "milestone_status" => {
                let milestone_id: String = serde_json::from_value(args["milestone_id"].clone())
                    .context("milestone_id required for milestone_status")?;
                let status: String = serde_json::from_value(args["status"].clone())
                    .context("status required for milestone_status")?;
                ctx.workflow.update_milestone_status(&milestone_id, &status)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "updated": true,
                    "milestone_id": milestone_id,
                    "status": status
                }) })
            }
            "change_status" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for change_status")?;
                let status: String = serde_json::from_value(args["status"].clone())
                    .context("status required for change_status")?;
                ctx.workflow.update_change_status(&change_id, &status)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "updated": true,
                    "change_id": change_id,
                    "status": status
                }) })
            }
            "impl_status" => {
                let task_id: String = serde_json::from_value(args["task_id"].clone())
                    .context("task_id required for impl_status")?;
                let status_str: String = serde_json::from_value(args["status"].clone())
                    .context("status required for impl_status")?;
                let status = crate::workflow::ImplStatus::from_str(&status_str)
                    .context("invalid impl status (use draft|in_progress|done|blocked)")?;
                ctx.workflow.update_task_impl_status(&task_id, status)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "updated": true,
                    "task_id": task_id,
                    "impl_status": status_str
                }) })
            }
            "spec_content" => {
                let spec_id: String = serde_json::from_value(args["spec_id"].clone())
                    .context("spec_id required for spec_content")?;
                let content: String = serde_json::from_value(args["content"].clone())
                    .context("content required for spec_content")?;
                ctx.workflow.update_spec_content(&spec_id, &content)?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "updated": true,
                    "spec_id": spec_id
                }) })
            }
            other => anyhow::bail!("unknown op: {} (use task_status|phase_next|phase_set|roadmap_status|milestone_status|change_status|impl_status|spec_content)", other),
        }
    }
}
