use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::types::{ToolOutput, ToolSchema};
use crate::workflow::ArtifactType;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;

/// workflow - 统一工作流工具
/// action=create: 创建工作流实体（原 workflow_create）
/// action=query: 查询工作流实体（原 workflow_query）
/// action=update: 更新工作流实体（原 workflow_update）
pub struct WorkflowTool;

#[async_trait]
impl Tool for WorkflowTool {
    fn name(&self) -> &str { "workflow" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "workflow".into(),
            description: "Manage workflow entities and operations. Actions: create/query/update (entity CRUD), init (initialize .mcoder/workflow/), finalize (archive a reviewed change), continue (detect next step + return full instructions), step (get full instructions for a specific step), state (read workflow state), list (list changes and spec domains), template (get document template), context (build step context), stats (execution statistics). Blueprint-style project management with 5-phase cycle (propose->plan->apply->review->archive).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["create", "query", "update", "init", "finalize", "continue", "step", "state", "list", "template", "context", "stats"], "description": "Workflow action" },
                    "op": { "type": "string", "enum": ["init", "roadmap", "milestone", "change", "spec", "task", "implementation", "proposal", "design", "review", "roadmaps", "milestones", "changes", "tasks", "tasks_full", "proposals", "designs", "specs", "reviews", "list", "task_status", "phase_next", "phase_set", "roadmap_status", "milestone_status", "change_status", "impl_status", "spec_content"], "description": "Sub-operation (varies by action)" },
                    "profile": { "type": "string", "enum": ["lite", "standard"], "description": "create init/roadmap: workflow profile, default standard" },
                    "roadmap_id": { "type": "string", "description": "create milestone / query milestones / update roadmap_status: parent roadmap id" },
                    "milestone_id": { "type": "string", "description": "create change / query changes / update milestone_status: parent milestone id" },
                    "change_id": { "type": "string", "description": "create spec/task/proposal/design/review / query tasks etc / update phase_next/phase_set/change_status: parent change id" },
                    "task_id": { "type": "string", "description": "create implementation / update task_status/impl_status: target task id" },
                    "spec_id": { "type": "string", "description": "update spec_content: target spec id" },
                    "title": { "type": "string", "description": "create: entity title" },
                    "description": { "type": "string", "description": "create: entity description" },
                    "milestone_title": { "type": "string", "description": "create init: first milestone title" },
                    "change_title": { "type": "string", "description": "create init: first change title" },
                    "content": { "type": "string", "description": "create spec/proposal/design/review / update spec_content: full content" },
                    "tdd": { "type": "boolean", "description": "create spec: enable TDD mode, default false" },
                    "order": { "type": "integer", "description": "create milestone/task: sort order, default 0" },
                    "verdict": { "type": "string", "enum": ["pass", "fail", "needs_work"], "description": "create review: verdict, default needs_work" },
                    "status": { "type": "string", "enum": ["todo", "in_progress", "done", "blocked", "draft", "active", "completed", "archived", "cancelled"], "description": "update task_status/impl_status/roadmap_status/milestone_status/change_status: new status" },
                    "phase": { "type": "string", "enum": ["propose", "plan", "apply", "review", "archive"], "description": "update phase_set: target phase" },
                    "type": { "type": "string", "description": "[template] Template type: proposal|design|tasks|spec|review|roadmap|config|global-spec" },
                    "step": { "type": "string", "description": "[context] Workflow step: plan|apply|review|archive" },
                    "change": { "type": "string", "description": "[context/finalize/continue/step] Change name" },
                    "name": { "type": "string", "description": "[finalize] Change name to archive. [step] Step name: init|propose|plan|apply|review|archive" },
                    "fix": { "type": "boolean", "description": "[step] Fix mode (read review.md issues), default false" }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let action: String = serde_json::from_value(args["action"].clone())?;
        match action.as_str() {
            "create" => Self::create(&args, ctx).await,
            "query" => Self::query(&args, ctx).await,
            "update" => Self::update(&args, ctx).await,
            "init" => Self::init_workflow_dir(&args, ctx).await,
            "finalize" => Self::finalize_change(&args, ctx).await,
            "continue" => Self::continue_workflow(&args, ctx).await,
            "step" => Self::step_workflow(&args, ctx).await,
            "state" => Self::state_workflow(&args, ctx).await,
            "list" => Self::list_workflow(&args, ctx).await,
            "template" => Self::template_workflow(&args, ctx).await,
            "context" => Self::context_workflow(&args, ctx).await,
            "stats" => Self::stats_workflow(&args, ctx).await,
            other => anyhow::bail!(
                "unknown action: {} (use create|query|update|init|finalize|continue|state|list|template|context|stats)",
                other
            ),
        }
    }
}

impl WorkflowTool {
    /// 原 workflow_create: 创建工作流实体
    async fn create(args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
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
                let id = ctx.workflow.create_spec(&change_id, &title, &content, tdd)?;
                ctx.workflow.write_artifact(
                    &change_id,
                    &ArtifactType::DeltaSpec(title.clone()),
                    &content,
                )?;
                id
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
                let id = ctx.workflow.create_proposal(&change_id, &title, &content)?;
                ctx.workflow.write_artifact(&change_id, &ArtifactType::Proposal, &content)?;
                id
            }
            "design" => {
                let change_id: String = serde_json::from_value(args["change_id"].clone())
                    .context("change_id required for design")?;
                let content: String = serde_json::from_value(args["content"].clone())
                    .context("content required for design")?;
                let id = ctx.workflow.create_design(&change_id, &title, &content)?;
                ctx.workflow.write_artifact(&change_id, &ArtifactType::Design, &content)?;
                id
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

    /// 原 workflow_query: 查询工作流实体
    async fn query(args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
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

    /// 原 workflow_update: 更新工作流实体
    async fn update(args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
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

    // ===== New workflow actions =====

    /// action=init: Initialize .mcoder/workflow/ directory structure
    async fn init_workflow_dir(_args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let workflow_dir = ctx.project_dir.join("workflow");

        // Create directories
        std::fs::create_dir_all(workflow_dir.join("specs"))?;
        std::fs::create_dir_all(workflow_dir.join("changes"))?;
        std::fs::create_dir_all(workflow_dir.join("changes").join("archive"))?;
        std::fs::create_dir_all(workflow_dir.join("conventions"))?;

        // Write config.yaml (don't overwrite if exists)
        let config_path = workflow_dir.join("config.yaml");
        if !config_path.exists() {
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let project_name = ctx
                .project_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "project".to_string());
            let config_content = crate::workflow::templates::CONFIG_TEMPLATE
                .replace("{{date}}", &today)
                .replace("{{project-name}}", &project_name);
            std::fs::write(&config_path, &config_content)?;
        }

        // Write empty conventions/coding.md (don't overwrite if exists)
        let coding_path = workflow_dir.join("conventions").join("coding.md");
        if !coding_path.exists() {
            std::fs::write(
                &coding_path,
                "# Coding Conventions\n\n<!-- Add project coding conventions here -->\n",
            )?;
        }

        // Write empty roadmap.md (don't overwrite if exists)
        let roadmap_path = workflow_dir.join("roadmap.md");
        if !roadmap_path.exists() {
            std::fs::write(&roadmap_path, "# Roadmap\n\n<!-- Define project milestones here -->\n")?;
        }

        Ok(ToolOutput::Sync {
            result: serde_json::json!({
                "initialized": true,
                "workflow_dir": workflow_dir.to_string_lossy(),
                "created": [
                    "specs/", "changes/", "changes/archive/", "conventions/",
                    "config.yaml", "conventions/coding.md", "roadmap.md"
                ]
            }),
        })
    }

    /// action=finalize: Archive a reviewed change (merge delta specs, move to archive)
    async fn finalize_change(args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let name: String = serde_json::from_value(args["name"].clone())
            .context("name required for finalize")?;
        let workflow_dir = ctx.project_dir.join("workflow");
        let change_dir = workflow_dir.join("changes").join(&name);

        if !change_dir.exists() {
            anyhow::bail!("change directory not found: {}", change_dir.display());
        }

        // Verify review.md exists and verdict is PASS
        let review_path = change_dir.join("review.md");
        if !review_path.exists() {
            anyhow::bail!(
                "review.md not found for change '{}', cannot finalize without PASS review",
                name
            );
        }
        let review_content = std::fs::read_to_string(&review_path)?;
        let mut has_pass = false;
        for line in review_content.lines() {
            let trimmed = line.trim();
            if let Some(after) = trimmed.strip_prefix("## Overall Verdict:") {
                if after.trim().starts_with("PASS") {
                    has_pass = true;
                    break;
                }
            }
        }
        if !has_pass {
            anyhow::bail!(
                "review verdict is not PASS for change '{}', cannot finalize",
                name
            );
        }

        // Merge delta specs into global specs
        let change_specs_dir = change_dir.join("specs");
        let mut merged_domains = Vec::new();
        if change_specs_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&change_specs_dir) {
                for entry in entries.flatten() {
                    if let Ok(ft) = entry.file_type() {
                        if ft.is_dir() {
                            let domain = entry.file_name().to_string_lossy().to_string();
                            let delta_path = change_specs_dir.join(&domain).join("spec.md");
                            if delta_path.exists() {
                                let delta_content = std::fs::read_to_string(&delta_path)?;
                                let global_spec_dir = workflow_dir.join("specs").join(&domain);
                                std::fs::create_dir_all(&global_spec_dir)?;
                                let global_spec_path = global_spec_dir.join("spec.md");
                                let global_content = if global_spec_path.exists() {
                                    std::fs::read_to_string(&global_spec_path)?
                                } else {
                                    delta_content.clone()
                                };
                                let merged = crate::workflow::merge_delta_spec(
                                    &delta_content,
                                    &global_content,
                                )?;
                                std::fs::write(&global_spec_path, &merged)?;
                                merged_domains.push(domain);
                            }
                        }
                    }
                }
            }
        }

        // Move changes/<name>/ to changes/archive/<date>-<name>/
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let archive_name = format!("{}-{}", today, name);
        let archive_dir = workflow_dir
            .join("changes")
            .join("archive")
            .join(&archive_name);
        std::fs::rename(&change_dir, &archive_dir)?;

        // Update roadmap.md if proposal has a Roadmap Reference section
        let mut roadmap_updated = false;
        let archived_proposal = archive_dir.join("proposal.md");
        if archived_proposal.exists() {
            if let Ok(proposal_content) = std::fs::read_to_string(&archived_proposal) {
                if proposal_content.contains("## Roadmap Reference") {
                    let roadmap_path = workflow_dir.join("roadmap.md");
                    if roadmap_path.exists() {
                        if let Ok(roadmap_content) = std::fs::read_to_string(&roadmap_path) {
                            // Find the change name in the roadmap and mark it as [x]
                            let unchecked = format!("- [ ] {}", name);
                            let checked = format!("- [x] {}", name);
                            if roadmap_content.contains(&unchecked) {
                                let updated_roadmap = roadmap_content.replace(&unchecked, &checked);
                                let _ = std::fs::write(&roadmap_path, &updated_roadmap);
                                roadmap_updated = true;
                            }
                        }
                    }
                }
            }
        }

        Ok(ToolOutput::Sync {
            result: serde_json::json!({
                "finalized": true,
                "change": name,
                "archived_to": format!("changes/archive/{}", archive_name),
                "merged_domains": merged_domains,
                "roadmap_updated": roadmap_updated,
                "hint": "consider running graph_index(path=\".\") to refresh the code graph"
            }),
        })
    }

    /// action=continue: Detect workflow state and return next step + full instructions
    async fn continue_workflow(args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let change_name: Option<String> = args["change"].as_str().map(|s| s.to_string());
        let next_step = crate::workflow::continue_::determine_next_step(
            &ctx.project_path,
            change_name.as_deref(),
        );

        match next_step {
            Some(step) => {
                let change = step.change_name.as_deref().unwrap_or("");
                let fix = step.action.contains("fix");
                let action_clean = if step.action.ends_with(" --fix") {
                    &step.action[..step.action.len() - 6]
                } else {
                    &step.action
                };

                let instructions = Self::get_step_prompt(&action_clean, change, fix);

                Ok(ToolOutput::Sync {
                    result: serde_json::json!({
                        "action": step.action,
                        "change": step.change_name,
                        "reason": step.reason,
                        "instructions": instructions,
                    }),
                })
            }
            None => Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "action": "unknown",
                    "reason": "Could not determine next step. Use workflow(action=list) to see active changes, or workflow(action=step, name=<step>, change=<name>) to get instructions for a specific step."
                }),
            }),
        }
    }

    /// action=step: Get full instructions for a specific step
    async fn step_workflow(args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let step_name = args.get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'name' for action=step (use propose|plan|apply|review|archive|init)"))?;
        let change = args.get("change")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let fix = args.get("fix")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let instructions = Self::get_step_prompt(step_name, change, fix);

        Ok(ToolOutput::Sync {
            result: serde_json::json!({
                "step": step_name,
                "change": change,
                "fix": fix,
                "instructions": instructions,
            }),
        })
    }

    /// 根据 action 名获取对应的编排步骤提示词
    fn get_step_prompt(action: &str, change: &str, fix: bool) -> String {
        match action {
            "init" => crate::commands::workflow_prompts::init_prompt(),
            "roadmap" => crate::commands::workflow_prompts::roadmap_prompt(),
            "propose" => crate::commands::workflow_prompts::propose_prompt(change),
            "plan" => crate::commands::workflow_prompts::plan_prompt(change, fix),
            "apply" => crate::commands::workflow_prompts::apply_prompt(change, fix),
            "review" => crate::commands::workflow_prompts::review_prompt(change, fix),
            "archive" => crate::commands::workflow_prompts::archive_prompt(change),
            "continue" => crate::commands::workflow_prompts::continue_prompt(),
            "ff" => crate::commands::workflow_prompts::ff_prompt(),
            "loop" => crate::commands::workflow_prompts::loop_prompt(),
            other => format!("[workflow] unknown step '{}': use init|roadmap|propose|plan|apply|review|archive", other),
        }
    }

    /// action=state: Return workflow state as JSON
    async fn state_workflow(_args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let state = crate::workflow::context::read_workflow_state(&ctx.project_path);
        match state {
            Some(s) => {
                let result = serde_json::to_value(&s)?;
                Ok(ToolOutput::Sync { result })
            }
            None => Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "initialized": false,
                    "hint": "Run workflow(action=init) to initialize"
                }),
            }),
        }
    }

    /// action=list: List active changes and spec domains
    async fn list_workflow(_args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let workflow_dir = ctx.project_dir.join("workflow");

        // Scan changes/ (exclude archive/)
        let mut changes = Vec::new();
        let changes_dir = workflow_dir.join("changes");
        if changes_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&changes_dir) {
                for entry in entries.flatten() {
                    if let Ok(ft) = entry.file_type() {
                        if ft.is_dir() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if name == "archive" {
                                continue;
                            }
                            let change_dir = entry.path();
                            let stage =
                                crate::workflow::continue_::detect_change_stage(&change_dir);
                            changes.push(serde_json::json!({
                                "name": name,
                                "stage": stage.as_str()
                            }));
                        }
                    }
                }
            }
        }
        changes.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });

        // Scan specs/
        let mut spec_domains = Vec::new();
        let specs_dir = workflow_dir.join("specs");
        if specs_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&specs_dir) {
                for entry in entries.flatten() {
                    if let Ok(ft) = entry.file_type() {
                        if ft.is_dir() {
                            spec_domains.push(entry.file_name().to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
        spec_domains.sort();

        Ok(ToolOutput::Sync {
            result: serde_json::json!({
                "changes": changes,
                "spec_domains": spec_domains
            }),
        })
    }

    /// action=template: Return a document template
    async fn template_workflow(args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let type_name: String = serde_json::from_value(args["type"].clone()).context(
            "type required for template (proposal|design|tasks|spec|review|roadmap|config|global-spec)",
        )?;

        match crate::workflow::templates::get_template(&type_name) {
            Some(template) => Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "type": type_name,
                    "template": template
                }),
            }),
            None => anyhow::bail!(
                "unknown template type: {} (use proposal|design|tasks|spec|review|roadmap|config|global-spec)",
                type_name
            ),
        }
    }

    /// action=context: Return context for a workflow step
    async fn context_workflow(args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let step: String = serde_json::from_value(args["step"].clone())
            .context("step required for context (plan|apply|review|archive)")?;
        let change_name: Option<String> = args["change"].as_str().map(|s| s.to_string());

        let context_str = crate::workflow::context::build_full_context(
            &ctx.project_path,
            &step,
            change_name.as_deref(),
        );

        match context_str {
            Some(c) => Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "step": step,
                    "change": change_name,
                    "context": c
                }),
            }),
            None => Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "step": step,
                    "change": change_name,
                    "context": "",
                    "hint": "No context available. Initialize workflow first."
                }),
            }),
        }
    }

    /// action=stats: Return execution statistics
    async fn stats_workflow(_args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let meta_dir = ctx.project_path.join(".meta");
        let mut stats = serde_json::json!({
            "meta_dir_exists": meta_dir.exists()
        });

        if meta_dir.exists() {
            let mut file_count = 0u32;
            if let Ok(entries) = std::fs::read_dir(&meta_dir) {
                for entry in entries.flatten() {
                    if entry.path().is_file() {
                        file_count += 1;
                    }
                }
            }
            stats["meta_file_count"] = serde_json::json!(file_count);

            let reviewer_history = meta_dir.join("reviewer-history.json");
            if reviewer_history.exists() {
                stats["has_reviewer_history"] = serde_json::json!(true);
            }
        }

        // Include workflow state summary
        if let Some(state) = crate::workflow::context::read_workflow_state(&ctx.project_path) {
            stats["active_change"] = serde_json::json!(state.active_change);
            stats["pending_changes_count"] = serde_json::json!(state.pending_changes.len());
            stats["spec_domains"] = serde_json::json!(state.spec_domains);
        }

        Ok(ToolOutput::Sync { result: stats })
    }
}
