// mcoder UI Redesign v2 - TodoSummaryBar (inline dock section)
// Shows pending todos as an inline section above the input box

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
    <Box flexDirection="column" paddingX={1}>
      <Text color={TUI_COLORS.textMuted}>todos ({view.totalUnfinished})</Text>
      {view.visible.map((t) => {
        const color = t.status === 'in_progress' ? TUI_COLORS.accent : TUI_COLORS.textMuted;
        const prefix = t.status === 'in_progress' ? PREFIX.dot : PREFIX.open;
        return (
          <Text key={t.id} color={color}>
            {prefix} {t.content}
          </Text>
        );
      })}
      {view.remaining > 0 && (
        <Text color={TUI_COLORS.textMuted}>... ({view.remaining} more)</Text>
      )}
    </Box>
  );
}
