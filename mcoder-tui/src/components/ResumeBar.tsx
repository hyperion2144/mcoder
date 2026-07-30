// Phase 3: TUI Resume 入口（消息区下方、输入框上方的固定状态提示附近；非模态）
//
// 设计：
// - 仅在 can_resume=true 且 loop_state != running 时显示
// - 通过 snapshot.can_resume / loop_state / stop_reason / 未完成 todo / interrupted
//   tasks 决定是否显示
// - 触发键：Ctrl+R（由 App.tsx 注册；ResumeBar 暴露 onResume handler）
// - 触发后调用 `session.resume` RPC；调用后更新 loop state
// - 三端共用纯逻辑：mcoder-tui/src/resume/state.ts
// - Phase 5c: 传入 has_interrupted_tasks；与 Rust decide_resume 5 参数完全一致

import React from 'react';
import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';
import {
  computeResumeEntry,
  hasResumeEntry,
  type ResumeEntry,
} from '../resume/state.js';

interface Props {
  sessionId: string | null;
}

export function ResumeBar({ sessionId }: Props) {
  const sessionStore = useSessionStore();

  if (!sessionId) return null;

  const entry: ResumeEntry = computeResumeEntry({
    loop_state: sessionStore.loopState,
    stop_reason: sessionStore.stopReason,
    has_unfinished_todo: (sessionStore.pendingTodos?.filter(
      (t: any) => t.status === 'pending' || t.status === 'in_progress',
    ) ?? []).length > 0,
    loop_running: !sessionStore.canResume,
    // Phase 5c: 5 参数与 Rust 同步
    has_interrupted_tasks: (sessionStore.backgroundTasks ?? []).some(
      (t: any) => t.status === 'Interrupted' || t.status === 'interrupted',
    ),
  });

  if (!hasResumeEntry(entry)) return null;

  const label = entry.kind === 'auto_resume'
    ? '▶ Resume (auto)'
    : entry.kind === 'requires_input'
      ? '⏸ Resume (waiting for input)'
      : '⏸ Resume (waiting for ask)';

  const color = entry.kind === 'auto_resume'
    ? 'cyan'
    : entry.kind === 'waiting_user'
      ? 'yellow'
      : 'gray';

  return (
    <Box paddingX={1} borderStyle="single" borderColor="gray" flexDirection="column">
      <Box>
        <Text color={color} bold>{label}</Text>
        <Text color="gray"> · {entry.reason}</Text>
      </Box>
      <Text color="gray">press Ctrl+R to resume (non-modal, near fixed status line)</Text>
    </Box>
  );
}