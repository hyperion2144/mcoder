// spec-driven workflow 的数据结构定义
// 7+1 类 artifact：RM/MS/CH/PR/DS/SP/T/RV
use serde::{Deserialize, Serialize};

// ============ 状态枚举 ============

/// 工作流实体状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Draft,
    Active,
    InProgress,
    Completed,
    Archived,
    Cancelled,
}

/// 任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Todo,
    InProgress,
    Done,
    Blocked,
}

/// 实现状态（替代原 Implementation 独立表，内联到 Task）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplStatus {
    Draft,
    InProgress,
    Done,
    Blocked,
}

impl ImplStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ImplStatus::Draft => "draft",
            ImplStatus::InProgress => "in_progress",
            ImplStatus::Done => "done",
            ImplStatus::Blocked => "blocked",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "draft" => Some(ImplStatus::Draft),
            "in_progress" => Some(ImplStatus::InProgress),
            "done" => Some(ImplStatus::Done),
            "blocked" => Some(ImplStatus::Blocked),
            _ => None,
        }
    }
}

/// 审查结论
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Pass,
    Fail,
    NeedsWork,
}

impl ReviewVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewVerdict::Pass => "pass",
            ReviewVerdict::Fail => "fail",
            ReviewVerdict::NeedsWork => "needs_work",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pass" => Some(ReviewVerdict::Pass),
            "fail" => Some(ReviewVerdict::Fail),
            "needs_work" => Some(ReviewVerdict::NeedsWork),
            _ => None,
        }
    }
}

// ============ Phase / Profile ============

/// 5 步循环阶段：propose -> plan -> apply -> review -> archive
/// 状态机逻辑见 phase.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    /// 提案阶段：创建 Change，描述要做什么、为什么
    Propose,
    /// 规划阶段：planner 子代理生成 spec/tasks
    Plan,
    /// 执行阶段：executor 子代理按 spec 实现
    Apply,
    /// 审查阶段：reviewer 子代理检查实现是否符合 spec
    Review,
    /// 归档阶段：已审查通过，归档变更
    Archive,
}

/// 工作流 profile
/// lite: 顺序执行、TDD 可选、review 任意通过
/// standard: 并行、TDD 强制、review 全通过
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowProfile {
    Lite,
    Standard,
}

impl Default for WorkflowProfile {
    fn default() -> Self {
        WorkflowProfile::Standard
    }
}

// ============ 7+1 类 Artifact ============

/// Roadmap (RM-N) - 路线图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Roadmap {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: WorkflowStatus,
    pub created_at: String,
    pub milestones: Vec<Milestone>,
}

/// Milestone (MS-N) - 里程碑
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    pub id: String,
    pub roadmap_id: String,
    pub title: String,
    pub description: String,
    pub status: WorkflowStatus,
    pub order: u32,
}

/// Change (CH-N) - 变更，phase 状态机挂在这里
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    pub milestone_id: String,
    pub title: String,
    pub description: String,
    pub status: WorkflowStatus,
    pub phase: WorkflowPhase,
    pub spec_id: Option<String>,
    pub created_at: String,
}

/// Proposal (PR-N) - 提案文档（propose 阶段产出）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub change_id: String,
    pub title: String,
    pub content: String,
    pub created_at: String,
}

/// Design (DS-N) - 设计文档（plan 阶段产出）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Design {
    pub id: String,
    pub change_id: String,
    pub title: String,
    pub content: String,
    pub created_at: String,
}

/// Spec (SP-N) - 规格，含 TDD 标志
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    pub id: String,
    pub change_id: String,
    pub title: String,
    pub content: String,
    pub tdd: bool,
    pub created_at: String,
}

/// Task (T-N) - 任务，含 impl_status 字段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub change_id: String,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub order: u32,
    pub impl_id: Option<String>,
    pub impl_status: Option<ImplStatus>,
}

/// Review (RV-N) - 审查记录（review 阶段产出）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    pub change_id: String,
    pub title: String,
    pub content: String,
    pub verdict: ReviewVerdict,
    pub created_at: String,
}
