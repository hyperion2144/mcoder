#![allow(dead_code)]
use std::path::Path;
use serde::{Serialize, Deserialize};
use crate::workflow::continue_::{count_tasks, has_design_issues};

/// Workflow state derived from disk (no state machine, pure file-based detection)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    pub has_config: bool,
    pub has_roadmap: bool,
    pub milestone: Option<(String, String)>,  // (id, name)
    pub phase: Option<(String, String)>,      // (name, status)
    pub active_change: Option<(String, String)>,  // (name, stage)
    pub pending_changes: Vec<(String, String)>,   // (name, stage)
    pub spec_domains: Vec<String>,
    pub next_action: Option<String>,
}

/// Read workflow state from disk by checking file existence in .mcoder/workflow/
pub fn read_workflow_state(project_dir: &Path) -> Option<WorkflowState> {
    let workflow_dir = project_dir.join(".mcoder").join("workflow");

    // 1. Check config.yaml exists -> if not, return None
    let config_path = workflow_dir.join("config.yaml");
    if !config_path.exists() {
        return None;
    }

    // 2. Read roadmap.md -> parse for ACTIVE milestone/phase
    let roadmap_path = workflow_dir.join("roadmap.md");
    let has_roadmap = roadmap_path.exists();
    let (milestone, phase) = if has_roadmap {
        match std::fs::read_to_string(&roadmap_path) {
            Ok(content) => (parse_active_milestone(&content), parse_active_phase(&content)),
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };

    // 3. Scan changes/ (exclude archive/) -> detect stage for each
    let changes_dir = workflow_dir.join("changes");
    let mut changes_with_stages: Vec<(String, String, std::time::SystemTime)> = Vec::new();

    if changes_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&changes_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name == "archive" {
                            continue;
                        }
                        let change_dir = entry.path();
                        let stage = detect_stage_string(&change_dir);
                        let modified = entry
                            .metadata()
                            .and_then(|m| m.modified())
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                        changes_with_stages.push((name, stage, modified));
                    }
                }
            }
        }
    }

    // Sort by modification time (most recent first)
    changes_with_stages.sort_by(|a, b| b.2.cmp(&a.2));

    // Most recently modified is active, rest are pending
    let (active_change, pending_changes) = if changes_with_stages.is_empty() {
        (None, Vec::new())
    } else {
        let active = changes_with_stages[0].clone();
        let pending = changes_with_stages[1..]
            .iter()
            .map(|(n, s, _)| (n.clone(), s.clone()))
            .collect();
        (Some((active.0, active.1)), pending)
    };

    // 4. List spec domains: scan .mcoder/workflow/specs/ for subdirectories
    let specs_dir = workflow_dir.join("specs");
    let spec_domains = list_spec_domains(&specs_dir);

    // 5. Determine next_action based on state
    let next_action = determine_next_action(has_roadmap, &active_change, &workflow_dir);

    Some(WorkflowState {
        has_config: true,
        has_roadmap,
        milestone,
        phase,
        active_change,
        pending_changes,
        spec_domains,
        next_action,
    })
}

/// Generate compact context block (<workflow-context> XML, <=4KB)
pub fn build_compact_context(project_dir: &Path) -> Option<String> {
    let state = read_workflow_state(project_dir)?;
    let workflow_dir = project_dir.join(".mcoder").join("workflow");

    let mut output = String::from("<workflow-context>\n");

    // Specs with requirement counts
    output.push_str("<specs>\n");
    let specs_dir = workflow_dir.join("specs");
    if specs_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&specs_dir) {
            let mut domains: Vec<_> = entries.flatten().collect();
            domains.sort_by_key(|e| e.file_name());
            for entry in domains {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        let domain = entry.file_name().to_string_lossy().to_string();
                        let spec_path = specs_dir.join(&domain).join("spec.md");
                        if spec_path.exists() {
                            let req_count = count_requirements(&spec_path);
                            let rel_path = format!(".mcoder/workflow/specs/{}/spec.md", domain);
                            output.push_str(&format!("- {} ({} requirements)\n", rel_path, req_count));
                        }
                    }
                }
            }
        }
    }
    output.push_str("</specs>\n");

    // Conventions
    output.push_str("<conventions>\n");
    let conv_dir = workflow_dir.join("conventions");
    if conv_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&conv_dir) {
            let mut files: Vec<_> = entries.flatten().collect();
            files.sort_by_key(|e| e.file_name());
            for entry in files {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".md") {
                    output.push_str(&format!("- .mcoder/workflow/conventions/{}\n", name));
                }
            }
        }
    }
    output.push_str("</conventions>\n");

    // Active change
    if let Some((name, stage)) = &state.active_change {
        output.push_str("<active-change>\n");
        output.push_str(&format!("- name: {}\n", name));
        output.push_str(&format!("- stage: {}\n", stage));

        // Task progress
        let tasks_path = workflow_dir.join("changes").join(name).join("tasks.md");
        if let Some((completed, total)) = count_tasks(&tasks_path) {
            output.push_str(&format!("- tasks: {}/{} completed\n", completed, total));
        }
        output.push_str("</active-change>\n");
    }

    // Next action
    if let Some(action) = &state.next_action {
        output.push_str(&format!("<next-action>{}</next-action>\n", action));
    }

    output.push_str("</workflow-context>");

    // Truncate to 4096 bytes if needed (UTF-8 safe)
    if output.len() > 4096 {
        let mut end = 4096;
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
    }

    Some(output)
}

/// Generate full context for a workflow step (includes file paths and content snippets)
pub fn build_full_context(project_dir: &Path, step: &str, change_name: Option<&str>) -> Option<String> {
    let workflow_dir = project_dir.join(".mcoder").join("workflow");

    let mut output = String::new();

    // 1. Read config.yaml for rules
    let config_path = workflow_dir.join("config.yaml");
    if config_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            output.push_str("=== config.yaml ===\n");
            output.push_str(&truncate_content(&content, 8192));
            output.push_str("\n\n");
        }
    }

    // 2. List relevant specs (filter by change's context.jsonl if change_name given)
    output.push_str("=== Specs ===\n");
    let specs_dir = workflow_dir.join("specs");
    if specs_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&specs_dir) {
            let mut domains: Vec<_> = entries.flatten().collect();
            domains.sort_by_key(|e| e.file_name());
            for entry in domains {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        let domain = entry.file_name().to_string_lossy().to_string();
                        let spec_path = specs_dir.join(&domain).join("spec.md");
                        if spec_path.exists() {
                            // If change_name given, filter by context.jsonl references
                            if let Some(cn) = change_name {
                                let context_jsonl = workflow_dir
                                    .join("changes")
                                    .join(cn)
                                    .join("context.jsonl");
                                if context_jsonl.exists() {
                                    if let Ok(ctx_content) = std::fs::read_to_string(&context_jsonl) {
                                        let spec_ref = format!("specs/{}/spec.md", domain);
                                        if !ctx_content.contains(&spec_ref) {
                                            continue;
                                        }
                                    }
                                }
                            }
                            output.push_str(&format!(
                                "--- .mcoder/workflow/specs/{}/spec.md ---\n",
                                domain
                            ));
                            if let Ok(content) = std::fs::read_to_string(&spec_path) {
                                output.push_str(&truncate_content(&content, 8192));
                            }
                            output.push_str("\n");
                        }
                    }
                }
            }
        }
    }

    // 3. List conventions
    output.push_str("=== Conventions ===\n");
    let conv_dir = workflow_dir.join("conventions");
    if conv_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&conv_dir) {
            let mut files: Vec<_> = entries.flatten().collect();
            files.sort_by_key(|e| e.file_name());
            for entry in files {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".md") {
                    let conv_path = conv_dir.join(&name);
                    output.push_str(&format!("--- .mcoder/workflow/conventions/{} ---\n", name));
                    if let Ok(content) = std::fs::read_to_string(&conv_path) {
                        output.push_str(&truncate_content(&content, 8192));
                    }
                    output.push_str("\n");
                }
            }
        }
    }

    // 4. If change_name: list change artifacts (proposal.md, design.md, tasks.md, review.md)
    if let Some(name) = change_name {
        let change_dir = workflow_dir.join("changes").join(name);
        output.push_str(&format!("=== Change: {} ===\n", name));

        for artifact in &["proposal.md", "design.md", "tasks.md", "review.md"] {
            let artifact_path = change_dir.join(artifact);
            if artifact_path.exists() {
                output.push_str(&format!(
                    "--- .mcoder/workflow/changes/{}/{} ---\n",
                    name, artifact
                ));
                if let Ok(content) = std::fs::read_to_string(&artifact_path) {
                    output.push_str(&truncate_content(&content, 8192));
                }
                output.push_str("\n");
            }
        }
    }

    // 5. Step annotation
    output.push_str(&format!("=== Step: {} ===\n", step));

    Some(output)
}

// ============ Helper functions ============

/// Parse roadmap.md for active milestone
/// Format: ## Milestone: M1 - Name [ACTIVE]
fn parse_active_milestone(content: &str) -> Option<(String, String)> {
    for line in content.lines() {
        let line = line.trim();
        if !line.starts_with("## Milestone:") || !line.contains("[ACTIVE]") {
            continue;
        }
        let after_prefix = line.strip_prefix("## Milestone:")?.trim();
        let active_idx = after_prefix.find("[ACTIVE]")?;
        let before_active = after_prefix[..active_idx].trim();
        // Format: "M1 - Name"
        let dash_idx = before_active.find(" - ")?;
        let id = before_active[..dash_idx].trim().to_string();
        let name = before_active[dash_idx + 3..].trim().to_string();
        return Some((id, name));
    }
    None
}

/// Parse roadmap.md for active phase
/// Format: ### Phase: Name [ACTIVE] or ### Phase: Name [IN_PROGRESS]
fn parse_active_phase(content: &str) -> Option<(String, String)> {
    for line in content.lines() {
        let line = line.trim();
        if !line.starts_with("### Phase:") {
            continue;
        }
        let after_prefix = line.strip_prefix("### Phase:")?.trim();
        // Format: "Name [STATUS]"
        let bracket_idx = after_prefix.find('[')?;
        let name = after_prefix[..bracket_idx].trim().to_string();
        let status_part = after_prefix[bracket_idx..].trim();
        let status = status_part
            .trim_matches(|c| c == '[' || c == ']')
            .to_string();
        return Some((name, status));
    }
    None
}

/// Detect stage string from change directory
/// Stages: "not-proposed", "proposed", "planned", "in-progress", "implemented", "reviewed", "needs-revision"
fn detect_stage_string(change_dir: &Path) -> String {
    let proposal = change_dir.join("proposal.md");
    let design = change_dir.join("design.md");
    let tasks = change_dir.join("tasks.md");
    let review = change_dir.join("review.md");

    // no proposal.md -> "not-proposed"
    if !proposal.exists() {
        return "not-proposed".to_string();
    }
    // no design.md or tasks.md -> "proposed"
    if !design.exists() || !tasks.exists() {
        return "proposed".to_string();
    }

    // Check task completion
    if let Some((completed, total)) = count_tasks(&tasks) {
        if total == 0 || completed == 0 {
            // Empty tasks or nothing started yet -> planned
            return "planned".to_string();
        }
        if completed < total {
            return "in-progress".to_string();
        }
        // completed == total: all tasks done, fall through to review check
    } else {
        // Can't read tasks.md
        return "proposed".to_string();
    }

    // no review.md -> "implemented"
    if !review.exists() {
        return "implemented".to_string();
    }
    // review.md has "Overall Verdict: PASS" -> "reviewed"
    if let Ok(review_content) = std::fs::read_to_string(&review) {
        if review_has_pass_verdict(&review_content) {
            return "reviewed".to_string();
        }
    }
    // else -> "needs-revision"
    "needs-revision".to_string()
}

/// List spec domain directories
fn list_spec_domains(specs_dir: &Path) -> Vec<String> {
    let mut domains = Vec::new();
    if specs_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(specs_dir) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        domains.push(entry.file_name().to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    domains.sort();
    domains
}

/// Count "### Requirement:" in spec.md
fn count_requirements(spec_path: &Path) -> usize {
    if let Ok(content) = std::fs::read_to_string(spec_path) {
        content
            .lines()
            .filter(|line| line.trim_start().starts_with("### Requirement:"))
            .count()
    } else {
        0
    }
}

/// Check if review.md content has "Overall Verdict: PASS"
fn review_has_pass_verdict(content: &str) -> bool {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(after) = trimmed.strip_prefix("## Overall Verdict:") {
            let verdict = after.trim();
            if verdict.starts_with("PASS") {
                return true;
            }
        }
    }
    false
}

/// Determine next action based on workflow state
fn determine_next_action(
    has_roadmap: bool,
    active_change: &Option<(String, String)>,
    workflow_dir: &Path,
) -> Option<String> {
    if !has_roadmap {
        return Some("init".to_string());
    }
    match active_change {
        None => Some("propose".to_string()),
        Some((name, stage)) => match stage.as_str() {
            "not-proposed" => Some("propose".to_string()),
            "proposed" => Some("plan".to_string()),
            "planned" => Some("apply".to_string()),
            "in-progress" => Some("apply".to_string()),
            "implemented" => Some("review".to_string()),
            "reviewed" => Some("archive".to_string()),
            "needs-revision" => {
                // Check for D-prefixed issues -> "plan --fix", else "apply --fix"
                let review_path = workflow_dir.join("changes").join(name).join("review.md");
                if has_design_issues(&review_path) {
                    Some("plan --fix".to_string())
                } else {
                    Some("apply --fix".to_string())
                }
            }
            _ => Some("propose".to_string()),
        },
    }
}

/// Truncate content to max_chars, adding "..." indicator if truncated
fn truncate_content(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    // Find a safe UTF-8 boundary at or before max_chars
    let mut end = max_chars;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = content[..end].to_string();
    truncated.push_str("\n... (truncated)");
    truncated
}
