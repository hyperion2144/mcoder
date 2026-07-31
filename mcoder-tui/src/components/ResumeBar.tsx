// DESIGN.md §4 / §10: ResumeBar（状态提示条）
// - single border + textMuted
// - 移除：⏸ emoji、press Ctrl+R... 提示
// - 状态用 PREFIX 符号

import React from 'react';
import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';
import {
  computeResumeEntry,
  hasResumeEntry,
  type ResumeEntry,
} from '../resume/state.js';
import { TUI_COLORS, PREFIX } from '../theme.js';

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
    has_interrupted_tasks: (sessionStore.backgroundTasks ?? []).some(
      (t: any) => t.status === 'Interrupted' || t.status === 'interrupted',
    ),
  });

  if (!hasResumeEntry(entry)) return null;

  const prefix = entry.kind === 'auto_resume' ? PREFIX.running : PREFIX.pending;
  const color = entry.kind === 'auto_resume'
    ? TUI_COLORS.accent
    : entry.kind === 'waiting_user'
      ? TUI_COLORS.warning
      : TUI_COLORS.textMuted;
  const label = entry.kind === 'auto_resume'
    ? 'Resume (auto)'
    : entry.kind === 'requires_input'
      ? 'Resume (waiting for input)'
      : 'Resume (waiting for ask)';

  return (
    <Box paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted} flexDirection="column">
      <Box>
        <Text color={color} bold>{prefix} {label}</Text>
        <Text color={TUI_COLORS.textMuted}>{` ${PREFIX.sep} ${entry.reason}`}</Text>
      </Box>
    </Box>
  );
}