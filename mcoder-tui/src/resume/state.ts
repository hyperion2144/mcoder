// Phase 3: 共享 Resume 决策纯函数（三端共用）
//
// 单一职责：根据 loop_state / stop_reason / unfinished todo 数 / loop_running
// / has_interrupted_tasks 计算客户端 UI 应当显示的 Resume 入口类型。
// 三端（TUI / Desktop / Mobile）均通过 `computeResumeEntry` 判定是否渲染
// "Resume"按钮/链接。
//
// 与服务端 `decide_resume`（mcoder/src/resume_policy.rs）语义一致；
// 客户端纯函数用于：UI gating；服务端用于：实际决策。
//
// 输入语义：
// - loop_state: 'idle' | 'running' | 'stopped' | 'waiting_for_user'
// - stop_reason: undefined | 'completed' | 'cancelled' | 'failed' | 'blocked' |
//                'unfinished_todos' | 'max_iters_reached' | 'empty_response' |
//                'loop_condition_met' | 'hook_blocked' | 'interrupted_tasks'
// - hasUnfinished: 是否有未完成 todo（pending + in_progress）
// - loopRunning: 服务端 in-memory flag（snapshot.can_resume 已蕴含；这里冗余保留）
// - hasInterruptedTasks: Phase 5b: session 是否有 interrupted tasks（服务重启时
//   标记为 interrupted 的后台任务；agent inspect 后决定是否重跑）
//
// 输出：
// - 'none'            — 不显示 Resume（如 running / waiting_for_user / 已 completed 无 todo）
// - 'requires_input'  — 显示"Resume"按钮，点击不启动模型，等用户输入
// - 'auto_resume'     — 显示"Resume"按钮，点击直接启动 loop（恢复未完成工作）
// - 'waiting_user'    — 显示"Resume"（语义：继续 ask 流程，不抢答）

export type ResumeKind = 'none' | 'requires_input' | 'auto_resume' | 'waiting_user';

export interface ResumeEntryInput {
  loop_state: string;
  stop_reason?: string | null;
  has_unfinished_todo: boolean;
  loop_running?: boolean;
  /// Phase 5b: 当前 session 是否存在 interrupted tasks
  has_interrupted_tasks?: boolean;
}

export interface ResumeEntry {
  kind: ResumeKind;
  /// 给前端文案 / tooltip 用的提示信息
  reason: string;
}

/// 触发自动 Resume 的 stop_reason 集合：
/// - blocked：被 hook 拦截，需要重试
/// - cancelled：被用户取消，可能需要续上
/// - failed：失败，重试
/// - unfinished_todos：未完成 todo，需要续上
/// - interrupted_tasks：Phase 5b: 服务重启打断的 task，agent inspect 后决定重跑
const RESUME_REASONS = new Set([
  'blocked',
  'cancelled',
  'failed',
  'unfinished_todos',
  'interrupted_tasks',
]);

/// 纯函数：根据 snapshot 计算 Resume 入口
export function computeResumeEntry(input: ResumeEntryInput): ResumeEntry {
  const { loop_state, stop_reason, has_unfinished_todo, has_interrupted_tasks } = input;
  const loopRunning = !!input.loop_running;

  // 1. running 状态：绝对不显示
  if (loopRunning || loop_state === 'running') {
    return { kind: 'none', reason: 'loop is running' };
  }

  // 2. waiting_for_user：显示 Resume（=继续 ask 流程，不抢答）
  if (loop_state === 'waiting_for_user') {
    return { kind: 'waiting_user', reason: 'waiting for user answer' };
  }

  // 3. 有未完成 todo：永远可以 auto_resume
  if (has_unfinished_todo) {
    return {
      kind: 'auto_resume',
      reason: stop_reason ? `unfinished after ${stop_reason}` : 'unfinished todos',
    };
  }

  // 4. stop_reason ∈ RESUME_REASONS 且无未完成：也允许 resume（保守）
  if (stop_reason && RESUME_REASONS.has(stop_reason)) {
    return {
      kind: 'auto_resume',
      reason: `previous stop_reason=${stop_reason}`,
    };
  }

  // 5. Phase 5b: 仅有 interrupted tasks（无 unfinished + 无 stop_reason）也允许 resume
  //    让 agent inspect 后决定是否重跑
  if (has_interrupted_tasks) {
    return {
      kind: 'auto_resume',
      reason: 'interrupted async tasks waiting to be inspected',
    };
  }

  // 6. completed / idle / stopped + 无 stop_reason / 无未完成：等用户输入
  return { kind: 'requires_input', reason: 'no pending work' };
}

/// 是否允许渲染"Resume"按钮入口（kind !== 'none'）
export function hasResumeEntry(entry: ResumeEntry): boolean {
  return entry.kind !== 'none';
}