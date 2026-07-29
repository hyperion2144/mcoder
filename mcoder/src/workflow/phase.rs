// 5 步状态机 + Profile 差异化
use super::types::{WorkflowPhase, WorkflowProfile};

impl WorkflowPhase {
    /// 5 步循环的合法顺序，返回下一个阶段
    /// Archive 是终态，返回 None
    pub fn next(self) -> Option<WorkflowPhase> {
        match self {
            WorkflowPhase::Propose => Some(WorkflowPhase::Plan),
            WorkflowPhase::Plan => Some(WorkflowPhase::Apply),
            WorkflowPhase::Apply => Some(WorkflowPhase::Review),
            WorkflowPhase::Review => Some(WorkflowPhase::Archive),
            WorkflowPhase::Archive => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowPhase::Propose => "propose",
            WorkflowPhase::Plan => "plan",
            WorkflowPhase::Apply => "apply",
            WorkflowPhase::Review => "review",
            WorkflowPhase::Archive => "archive",
        }
    }

    pub fn from_str(s: &str) -> Option<WorkflowPhase> {
        match s {
            "propose" => Some(WorkflowPhase::Propose),
            "plan" => Some(WorkflowPhase::Plan),
            "apply" => Some(WorkflowPhase::Apply),
            "review" => Some(WorkflowPhase::Review),
            "archive" => Some(WorkflowPhase::Archive),
            _ => None,
        }
    }
}

impl WorkflowProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowProfile::Lite => "lite",
            WorkflowProfile::Standard => "standard",
        }
    }

    pub fn from_str(s: &str) -> Option<WorkflowProfile> {
        match s {
            "lite" => Some(WorkflowProfile::Lite),
            "standard" => Some(WorkflowProfile::Standard),
            _ => None,
        }
    }

    /// standard 强制 TDD，lite 可选
    pub fn is_tdd_mandatory(self) -> bool {
        matches!(self, WorkflowProfile::Standard)
    }

    /// standard 要求所有 task 通过，lite 不要求
    pub fn require_all_tasks_pass(self) -> bool {
        matches!(self, WorkflowProfile::Standard)
    }

    /// standard 支持并行执行，lite 顺序执行
    pub fn parallel_execution(self) -> bool {
        matches!(self, WorkflowProfile::Standard)
    }
}
