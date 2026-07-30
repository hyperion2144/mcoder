#![allow(dead_code)]
use std::path::Path;
use crate::workflow::context::{self, WorkflowState};

/// Change stage derived from artifact files on disk
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeStage {
    /// No proposal.md exists - next step is propose
    NotProposed,
    /// proposal.md exists, but no design.md/tasks.md yet - next step is plan
    Proposed,
    /// Design and tasks exist, but no tasks started (0 checked)
    Planned,
    /// Some tasks checked but not all
    InProgress,
    /// All tasks checked, no review.md yet
    Implemented,
    /// review.md exists with "Overall Verdict: PASS"
    Reviewed,
    /// review.md exists but verdict is not PASS
    NeedsRevision,
}

impl ChangeStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChangeStage::NotProposed => "not-proposed",
            ChangeStage::Proposed => "proposed",
            ChangeStage::Planned => "planned",
            ChangeStage::InProgress => "in-progress",
            ChangeStage::Implemented => "implemented",
            ChangeStage::Reviewed => "reviewed",
            ChangeStage::NeedsRevision => "needs-revision",
        }
    }
}

/// The next workflow step to execute
#[derive(Debug, Clone)]
pub struct NextStep {
    /// "propose" | "plan" | "apply" | "review" | "archive" | "init" | "continue"
    pub action: String,
    pub change_name: Option<String>,
    pub reason: String,
}

/// Detect a change's stage from its artifact files
///
/// Stage detection order:
/// 1. no proposal.md -> NotProposed
/// 2. no design.md or tasks.md -> Proposed
/// 3. tasks.md has 0 checked tasks -> Planned
/// 4. tasks.md has some but not all checked -> InProgress
/// 5. no review.md -> Implemented
/// 6. review.md "Overall Verdict: PASS" -> Reviewed
/// 7. else -> NeedsRevision
pub fn detect_change_stage(change_dir: &Path) -> ChangeStage {
    let proposal = change_dir.join("proposal.md");
    let design = change_dir.join("design.md");
    let tasks = change_dir.join("tasks.md");
    let review = change_dir.join("review.md");

    // no proposal.md -> NotProposed
    if !proposal.exists() {
        return ChangeStage::NotProposed;
    }
    // no design.md or tasks.md -> Proposed
    if !design.exists() || !tasks.exists() {
        return ChangeStage::Proposed;
    }

    // Check task completion
    if let Some((completed, total)) = count_tasks(&tasks) {
        if total == 0 || completed == 0 {
            // Empty tasks or nothing started yet -> Planned
            return ChangeStage::Planned;
        }
        if completed < total {
            return ChangeStage::InProgress;
        }
        // completed == total: all tasks done, fall through to review check
    } else {
        // Can't read tasks.md
        return ChangeStage::Proposed;
    }

    // no review.md -> Implemented
    if !review.exists() {
        return ChangeStage::Implemented;
    }
    // review.md "Overall Verdict: PASS" -> Reviewed
    if let Some(verdict) = check_review_verdict(&review) {
        if verdict == "PASS" {
            return ChangeStage::Reviewed;
        }
    }
    // else -> NeedsRevision
    ChangeStage::NeedsRevision
}

/// Determine the next workflow step
///
/// Logic:
/// 1. Check config.yaml exists -> if not, return init
/// 2. If change_name given: detect that change's stage and return next action
/// 3. If no change_name:
///    a. Scan changes/ for active changes
///    b. If none: check roadmap -> "propose" or "init"
///    c. If one: detect its stage -> next action
///    d. If multiple: return "continue" (let user pick)
/// 4. Fix loop detection for NeedsRevision stage
pub fn determine_next_step(project_dir: &Path, change_name: Option<&str>) -> Option<NextStep> {
    let workflow_dir = project_dir.join(".mcoder").join("workflow");

    // 1. Check config.yaml exists -> if not, return init
    let config_path = workflow_dir.join("config.yaml");
    if !config_path.exists() {
        return Some(NextStep {
            action: "init".to_string(),
            change_name: None,
            reason: "Workflow not initialized - config.yaml not found".to_string(),
        });
    }

    // Read workflow state for context (provides quick overview)
    let _state: Option<WorkflowState> = context::read_workflow_state(project_dir);

    // 2. If change_name given: detect that change's stage and return next action
    if let Some(name) = change_name {
        let change_dir = workflow_dir.join("changes").join(name);
        if !change_dir.exists() {
            return Some(NextStep {
                action: "propose".to_string(),
                change_name: Some(name.to_string()),
                reason: format!(
                    "Change directory '{}' not found, create proposal first",
                    name
                ),
            });
        }
        let stage = detect_change_stage(&change_dir);
        return Some(stage_to_next_step(&stage, name, &change_dir));
    }

    // 3. If no change_name: scan changes/ for active changes
    let changes_dir = workflow_dir.join("changes");
    let mut active_changes: Vec<(String, std::time::SystemTime)> = Vec::new();

    if changes_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&changes_dir) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name == "archive" {
                            continue;
                        }
                        let modified = entry
                            .metadata()
                            .and_then(|m| m.modified())
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                        active_changes.push((name, modified));
                    }
                }
            }
        }
    }

    // Sort by modification time (most recent first)
    active_changes.sort_by(|a, b| b.1.cmp(&a.1));

    // 3b. If none: check roadmap -> "propose" or "init"
    if active_changes.is_empty() {
        let roadmap_path = workflow_dir.join("roadmap.md");
        if !roadmap_path.exists() || is_roadmap_empty(&roadmap_path) {
            return Some(NextStep {
                action: "init".to_string(),
                change_name: None,
                reason: "Roadmap not defined - initialize workflow with roadmap".to_string(),
            });
        }
        return Some(NextStep {
            action: "propose".to_string(),
            change_name: None,
            reason: "No active changes - create a new proposal based on roadmap".to_string(),
        });
    }

    // 3c. If one: detect its stage -> next action
    if active_changes.len() == 1 {
        let name = &active_changes[0].0;
        let change_dir = workflow_dir.join("changes").join(name);
        let stage = detect_change_stage(&change_dir);
        return Some(stage_to_next_step(&stage, name, &change_dir));
    }

    // 3d. If multiple: return "continue" (let user pick)
    let names: Vec<String> = active_changes.iter().map(|(n, _)| n.clone()).collect();
    Some(NextStep {
        action: "continue".to_string(),
        change_name: None,
        reason: format!(
            "Multiple active changes found: {}. Specify which change to continue.",
            names.join(", ")
        ),
    })
}

/// Map a ChangeStage to the next NextStep
fn stage_to_next_step(stage: &ChangeStage, change_name: &str, change_dir: &Path) -> NextStep {
    let review_path = change_dir.join("review.md");

    match stage {
        ChangeStage::NotProposed => NextStep {
            action: "propose".to_string(),
            change_name: Some(change_name.to_string()),
            reason: "No proposal.md found, create proposal first".to_string(),
        },
        ChangeStage::Proposed => NextStep {
            action: "plan".to_string(),
            change_name: Some(change_name.to_string()),
            reason: "Proposal exists, design and tasks needed".to_string(),
        },
        ChangeStage::Planned => NextStep {
            action: "apply".to_string(),
            change_name: Some(change_name.to_string()),
            reason: "Design and tasks ready, start implementation".to_string(),
        },
        ChangeStage::InProgress => NextStep {
            action: "apply".to_string(),
            change_name: Some(change_name.to_string()),
            reason: "Implementation in progress, continue executing tasks".to_string(),
        },
        ChangeStage::Implemented => NextStep {
            action: "review".to_string(),
            change_name: Some(change_name.to_string()),
            reason: "All tasks completed, ready for review".to_string(),
        },
        ChangeStage::Reviewed => NextStep {
            action: "archive".to_string(),
            change_name: Some(change_name.to_string()),
            reason: "Review passed, ready to archive".to_string(),
        },
        ChangeStage::NeedsRevision => {
            // 4. Fix loop detection

            // Check for diminishing returns (fuse)
            if let Some(fuse_reason) = check_diminishing_returns(&review_path) {
                return NextStep {
                    action: "review".to_string(),
                    change_name: Some(change_name.to_string()),
                    reason: fuse_reason,
                };
            }

            // Check for D-prefixed issues -> plan --fix (replan needed)
            if has_design_issues(&review_path) {
                NextStep {
                    action: "plan --fix".to_string(),
                    change_name: Some(change_name.to_string()),
                    reason: "Review found design issues (D-prefixed), replan needed".to_string(),
                }
            } else {
                // R/Q/G issues -> apply --fix (fix code)
                NextStep {
                    action: "apply --fix".to_string(),
                    change_name: Some(change_name.to_string()),
                    reason: "Review found code issues, fix and re-apply".to_string(),
                }
            }
        }
    }
}

/// Count checked vs unchecked tasks in tasks.md
/// Returns (completed, total)
pub(crate) fn count_tasks(tasks_path: &Path) -> Option<(usize, usize)> {
    let content = std::fs::read_to_string(tasks_path).ok()?;
    let mut completed = 0;
    let mut total = 0;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("- [x]") || trimmed.starts_with("- [X]") {
            completed += 1;
            total += 1;
        } else if trimmed.starts_with("- [ ]") {
            total += 1;
        }
    }
    Some((completed, total))
}

/// Check review verdict
/// Returns "PASS", "FAIL", "NEEDS_REVISION", or None
fn check_review_verdict(review_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(review_path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        // Match "## Overall Verdict: PASS" (case insensitive on verdict value)
        if let Some(after) = trimmed.strip_prefix("## Overall Verdict:") {
            let verdict = after.trim();
            // Extract the first word (PASS, FAIL, NEEDS_REVISION)
            let verdict_word = verdict.split_whitespace().next()?;
            return Some(verdict_word.to_uppercase());
        }
    }
    None
}

/// Check for D-prefixed issues in review.md
/// Matches lines like "- [ ] D1", "- [ ] D12"
pub(crate) fn has_design_issues(review_path: &Path) -> bool {
    if let Ok(content) = std::fs::read_to_string(review_path) {
        for line in content.lines() {
            let trimmed = line.trim_start();
            // Match "- [ ] D" followed by a digit
            if let Some(after) = trimmed.strip_prefix("- [ ] D") {
                if after.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                    return true;
                }
            }
        }
    }
    false
}

/// Check for diminishing returns in review history
///
/// Parses the Review History table in review.md. If the last 2+ rounds
/// each had <= 2 new issues, returns a fuse reason suggesting manual review.
fn check_diminishing_returns(review_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(review_path).ok()?;

    // Parse Review History table rows: | <round> | <date> | <verdict> | <new_issues> | ...
    // The table format follows: | Round | Date | Verdict | New Issues | ...
    let mut history_new_issues: Vec<usize> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            continue;
        }
        // Skip separator rows (| --- | --- |)
        if trimmed.contains("---") {
            continue;
        }

        let cols: Vec<&str> = trimmed.split('|').collect();
        // cols[0] is empty (before first |)
        if cols.len() >= 5 {
            let round_str = cols[1].trim();
            // Skip header row
            if round_str.eq_ignore_ascii_case("round") {
                continue;
            }
            // Verify this is a data row (round is a number)
            if round_str.parse::<usize>().is_ok() {
                // Try cols[3] as new_issues count
                // Table: | Round | Date | Verdict | New Issues | ...
                if let Ok(new_issues) = cols[3].trim().parse::<usize>() {
                    history_new_issues.push(new_issues);
                }
            }
        }
    }

    // Check diminishing returns: last 2+ rounds with <= 2 new issues each
    let fuse_rounds = 2;
    if history_new_issues.len() >= fuse_rounds {
        let recent = &history_new_issues[history_new_issues.len() - fuse_rounds..];
        if recent.iter().all(|&n| n <= 2) {
            return Some(format!(
                "[FUSE] Diminishing returns detected: last {} review rounds added <= 2 issues each. \
                 Recommend human verification before another fix cycle.",
                fuse_rounds
            ));
        }
    }

    None
}

/// Check if roadmap.md is empty (has placeholders or no milestone sections)
fn is_roadmap_empty(roadmap_path: &Path) -> bool {
    let content = match std::fs::read_to_string(roadmap_path) {
        Ok(c) => c,
        Err(_) => return true,
    };
    // Check for template placeholders ({{...}} on same line)
    for line in content.lines() {
        if line.contains("{{") && line.contains("}}") {
            return true;
        }
    }
    // Check for at least one milestone section
    if !content.contains("## Milestone:") {
        return true;
    }
    false
}
