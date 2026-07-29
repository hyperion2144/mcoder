// 设计文档 §6.7: components/TaskMonitor.tsx - 任务监控视图

import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';

export function TaskMonitor() {
  const { backgroundTasks } = useSessionStore();
  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single">
      <Text bold color="cyan">Background Tasks</Text>
      {!backgroundTasks || backgroundTasks.length === 0 ? (
        <Text color="gray">No background tasks running.</Text>
      ) : (
        backgroundTasks.map((task: any, i: number) => (
          <Box key={i} justifyContent="space-between">
            <Text color={task.status === 'running' ? 'yellow' : task.status === 'done' ? 'green' : 'red'}>
              {task.id || task.task_id || `task-${i}`}
            </Text>
            <Text color="white">{task.name || task.description || ''}</Text>
            <Text color="gray">{task.status || ''}</Text>
          </Box>
        ))
      )}
      <Text color="gray" italic>press ESC to close</Text>
    </Box>
  );
}
