// DESIGN.md §4 / §10: Session 列表（面板）
// - single border + textMuted
// - 移除：press ESC to close、italic

import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';
import { TUI_COLORS } from '../theme.js';

export function SessionList() {
  const { sessions, currentSessionId } = useSessionStore();
  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted}>
      <Box>
        <Text bold color={TUI_COLORS.accent}>Sessions</Text>
        <Text color={TUI_COLORS.textMuted}> · {sessions.length}</Text>
      </Box>
      {sessions.length === 0 ? (
        <Text color={TUI_COLORS.textMuted}>empty</Text>
      ) : (
        sessions.map((s) => (
          <Box key={s.session_id} justifyContent="space-between">
            <Text color={s.session_id === currentSessionId ? TUI_COLORS.success : TUI_COLORS.textPrimary}>
              {s.session_id.slice(0, 20)}
            </Text>
            <Text color={TUI_COLORS.textMuted}>{s.title}</Text>
          </Box>
        ))
      )}
    </Box>
  );
}