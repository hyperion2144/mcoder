// 设计文档 §6.7: components/TaskMonitor.tsx - 任务监控视图
// Phase 5: 显示 per-session task 元数据（task_id / tool_name / status / args / output）
// 包含 interrupted 状态（service restart 时被打断）

import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';

export function TaskMonitor() {
  const { backgroundTasks } = useSessionStore();
  const tasks = backgroundTasks || [];
  const interruptedCount = tasks.filter((t: any) => t.status === 'Interrupted').length;
  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single">
      <Text bold color="cyan">
        Background Tasks ({tasks.length}
        {interruptedCount > 0 ? `, ${interruptedCount} interrupted` : ''})
      </Text>
      {tasks.length === 0 ? (
        <Text color="gray">No background tasks running.</Text>
      ) : (
        tasks.map((task: any, i: number) => {
          const status = (task.status || '').toString();
          const isInterrupted = status === 'Interrupted';
          const isRunning = status === 'Running' || status === 'Pending';
          const isDone = status === 'Completed';
          const color = isInterrupted
            ? 'yellow'
            : isRunning
              ? 'cyan'
              : isDone
                ? 'green'
                : 'red';
          const id = task.task_id || task.id || `task-${i}`;
          const name = task.tool_name || task.name || task.description || '';
          return (
            <Box key={i} flexDirection="column" marginY={0}>
              <Box justifyContent="space-between">
                <Text color={color}>{id}</Text>
                <Text color="white">{name}</Text>
                <Text color={color}>{status}</Text>
              </Box>
              {isInterrupted && task.args_json != null && (
                <Text color="gray" wrap="truncate-end">
                  {'  '}args: {JSON.stringify(task.args_json).slice(0, 80)}
                  {task.error ? ` · error: ${task.error}` : ''}
                </Text>
              )}
            </Box>
          );
        })
      )}
      <Text color="gray" italic>press ESC to close</Text>
    </Box>
  );
}
