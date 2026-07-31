// DESIGN.md §4 / §10: TaskMonitor（面板）
// - single border + textMuted
// - 状态色统一：running=accent / done=success / failed=error / interrupted=warning
// - 移除：press ESC to close、italic

import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';
import { TUI_COLORS, PREFIX } from '../theme.js';

export function TaskMonitor() {
  const { backgroundTasks } = useSessionStore();
  const tasks = backgroundTasks || [];
  const interruptedCount = tasks.filter((t: any) => t.status === 'Interrupted').length;
  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted}>
      <Box>
        <Text bold color={TUI_COLORS.accent}>Background Tasks</Text>
        <Text color={TUI_COLORS.textMuted}> · {tasks.length}{interruptedCount > 0 ? ` · ${interruptedCount} interrupted` : ''}</Text>
      </Box>
      {tasks.length === 0 ? (
        <Text color={TUI_COLORS.textMuted}>empty</Text>
      ) : (
        tasks.map((task: any, i: number) => {
          const status = (task.status || '').toString();
          const isInterrupted = status === 'Interrupted';
          const isRunning = status === 'Running' || status === 'Pending';
          const isDone = status === 'Completed';
          const statusColor = isInterrupted
            ? TUI_COLORS.warning
            : isRunning
              ? TUI_COLORS.accent
              : isDone
                ? TUI_COLORS.success
                : TUI_COLORS.error;
          const prefix = isRunning ? PREFIX.running : isDone ? PREFIX.done : isInterrupted ? PREFIX.pending : PREFIX.failed;
          const id = task.task_id || task.id || `task-${i}`;
          const name = task.tool_name || task.name || task.description || '';
          return (
            <Box key={i} flexDirection="column" marginY={0}>
              <Box justifyContent="space-between">
                <Text color={statusColor}>{prefix} {id}</Text>
                <Text color={TUI_COLORS.textPrimary}>{name}</Text>
                <Text color={statusColor}>{status}</Text>
              </Box>
              {isInterrupted && task.args_json != null && (
                <Text color={TUI_COLORS.textMuted} wrap="truncate-end">
                  {'  '}args: {JSON.stringify(task.args_json).slice(0, 80)}
                  {task.error ? ` · error: ${task.error}` : ''}
                </Text>
              )}
            </Box>
          );
        })
      )}
    </Box>
  );
}