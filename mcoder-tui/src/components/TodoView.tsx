// 设计文档 §6.7: components/TodoView.tsx - Todo 视图（goal mode）

import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';

export function TodoView() {
  const { pendingTodos } = useSessionStore();
  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single">
      <Text bold color="cyan">Todos</Text>
      {!pendingTodos || pendingTodos.length === 0 ? (
        <Text color="gray">No todos. Switch to goal mode (/mode goal) to create todos.</Text>
      ) : (
        pendingTodos.map((todo: any, i: number) => (
          <Text key={i} color={todo.done ? 'gray' : 'white'}>
            {todo.done ? '✓' : '☐'} {todo.text || todo.description || JSON.stringify(todo)}
          </Text>
        ))
      )}
      <Text color="gray" italic>press ESC to close</Text>
    </Box>
  );
}
