// DESIGN.md §4 / §10: TodoSummaryBar（状态条）
// - single border + textMuted
// - 移除：▶ ☐ emoji、italic

import React from 'react';
import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';
import {
  selectTodoSummary,
  type TodoSummaryPlatform,
  PLATFORM_TUI,
} from '../todo/summary.js';
import { TUI_COLORS, PREFIX } from '../theme.js';

interface Props {
  platform?: TodoSummaryPlatform;
}

export function TodoSummaryBar({ platform = PLATFORM_TUI }: Props) {
  const todos = useSessionStore((s) => s.pendingTodos);
  const view = selectTodoSummary(todos ?? [], platform, false);
  if (!view) return null;

  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted}>
      <Box>
        <Text bold color={TUI_COLORS.accent}>Todos</Text>
        <Text color={TUI_COLORS.textMuted}> · {view.totalUnfinished} unfinished</Text>
      </Box>
      {view.visible.map((t) => {
        const color = t.status === 'in_progress' ? TUI_COLORS.accent : TUI_COLORS.textPrimary;
        const prefix = t.status === 'in_progress' ? PREFIX.running : PREFIX.pending;
        return (
          <Text key={t.id} color={color}>
            {prefix} {t.content}
          </Text>
        );
      })}
      {view.remaining > 0 && (
        <Text color={TUI_COLORS.textMuted}>+{view.remaining} more</Text>
      )}
    </Box>
  );
}