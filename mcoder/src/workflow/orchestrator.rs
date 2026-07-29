// phase 推进时自动调度子代理的逻辑
// 先实现返回调度提示的逻辑，实际的子代理 spawn 由调用方（agent loop）执行
use serde::{Deserialize, Serialize};

use super::types::WorkflowPhase;

/// 子代理调度提示
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnSubagentHint {
    /// 子代理角色：planner / executor / reviewer
    pub role: String,
    /// 关联的 change ID
    pub change_id: String,
    /// 当前 phase
    pub phase: String,
    /// 调度提示文本
    pub prompt: String,
}

/// 工作流编排器：phase 推进时返回子代理调度提示
pub struct WorkflowOrchestrator;

impl WorkflowOrchestrator {
    /// 根据 phase 返回子代理调度提示
    /// plan -> planner, apply -> executor, review -> reviewer
    /// propose/archive 无需调度子代理，返回 None
    pub fn schedule_for_phase(phase: WorkflowPhase, change_id: &str) -> Option<SpawnSubagentHint> {
        match phase {
            WorkflowPhase::Plan => Some(SpawnSubagentHint {
                role: "planner".into(),
                change_id: change_id.into(),
                phase: phase.as_str().into(),
                prompt: format!(
                    "planner: analyze change {} and generate spec/tasks",
                    change_id
                ),
            }),
            WorkflowPhase::Apply => Some(SpawnSubagentHint {
                role: "executor".into(),
                change_id: change_id.into(),
                phase: phase.as_str().into(),
                prompt: format!(
                    "executor: implement change {} according to spec",
                    change_id
                ),
            }),
            WorkflowPhase::Review => Some(SpawnSubagentHint {
                role: "reviewer".into(),
                change_id: change_id.into(),
                phase: phase.as_str().into(),
                prompt: format!("reviewer: review implementation of change {}", change_id),
            }),
            _ => None,
        }
    }
}
