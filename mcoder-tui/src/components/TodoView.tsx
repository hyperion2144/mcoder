// DESIGN.md §4 / §10: Todo 视图（面板）
// - single border + textMuted
// - 移除：press ESC to close、italic、(多选...)、✓/☐ emoji
// - 标题：Todos · N unfinished（用 · 分隔）

import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';
import { TUI_COLORS, PREFIX } from '../theme.js';

export function TodoView() {
  const { pendingTodos } = useSessionStore();
  const unfinished = (pendingTodos || []).filter((t: any) => !t.done);
  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted}>
      <Box>
        <Text bold color={TUI_COLORS.accent}>Todos</Text>
        <Text color={TUI_COLORS.textMuted}>{` ${PREFIX.sep} ${unfinished.length} unfinished`}</Text>
      </Box>
      {!pendingTodos || pendingTodos.length === 0 ? (
        <Text color={TUI_COLORS.textMuted}>empty</Text>
      ) : (
        pendingTodos.map((todo: any, i: number) => (
          <Text key={i} color={todo.done ? TUI_COLORS.textMuted : TUI_COLORS.textPrimary}>
            {todo.done ? PREFIX.done : PREFIX.pending} {todo.text || todo.description || JSON.stringify(todo)}
          </Text>
        ))
      )}
    </Box>
  );
}