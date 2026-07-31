// DESIGN.md §4 / §10: Config 视图（面板）
// - single border + textMuted
// - 移除：press ESC to close、italic、blue/magenta/green 装饰色
// - 统一用 accent/textPrimary/textMuted

import { Box, Text } from 'ink';
import { TUI_COLORS, PREFIX } from '../theme.js';

interface Props {
  currentModel?: string;
  currentRole?: string;
  projectPath?: string;
  gitBranch?: string;
  contextUsed?: number;
  contextWindow?: number;
}

export function ConfigView({
  currentModel, currentRole, projectPath, gitBranch, contextUsed, contextWindow,
}: Props) {
  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted}>
      <Text bold color={TUI_COLORS.accent}>Config</Text>
      <Text color={TUI_COLORS.textPrimary}>{`model ${PREFIX.sep} `}<Text color={TUI_COLORS.accent}>{currentModel || '(not set)'}</Text></Text>
      <Text color={TUI_COLORS.textPrimary}>{`role ${PREFIX.sep} `}<Text color={TUI_COLORS.accent}>{currentRole}</Text></Text>
      <Text color={TUI_COLORS.textPrimary}>{`project ${PREFIX.sep} `}<Text color={TUI_COLORS.textPrimary}>{projectPath || '(unknown)'}</Text></Text>
      <Text color={TUI_COLORS.textPrimary}>{`branch ${PREFIX.sep} `}<Text color={TUI_COLORS.success}>{gitBranch || '(unknown)'}</Text></Text>
      <Text color={TUI_COLORS.textPrimary}>{`context ${PREFIX.sep} ${contextUsed}/${contextWindow}`}</Text>
    </Box>
  );
}