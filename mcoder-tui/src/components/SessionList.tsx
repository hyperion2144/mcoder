// 设计文档 §6.7: components/SessionList.tsx - 会话列表视图

import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';

export function SessionList() {
  const { sessions, currentSessionId } = useSessionStore();
  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single">
      <Text bold color="cyan">Sessions</Text>
      {sessions.length === 0 ? (
        <Text color="gray">No sessions. Use /sessions new to create one.</Text>
      ) : (
        sessions.map((s) => (
          <Box key={s.session_id} justifyContent="space-between">
            <Text color={s.session_id === currentSessionId ? 'green' : 'white'}>
              {s.session_id.slice(0, 20)}
            </Text>
            <Text color="gray">{s.title}</Text>
          </Box>
        ))
      )}
      <Text color="gray" italic>press ESC to close</Text>
    </Box>
  );
}
