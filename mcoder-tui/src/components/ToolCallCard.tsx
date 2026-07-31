// DESIGN.md §3: 工具调用卡片（折叠/展开）
// - 角色色：execution = accent；done = textMuted
// - 标题：▸ tool_name(args) 或 ✓ 已完成

import { Box, Text } from 'ink';
import { useMessagesStore } from '../store/index.js';
import { TUI_COLORS, PREFIX } from '../theme.js';
import { formatToolOutput } from '../utils/format.js';
import type { ContentBlock } from '../rpc/types.js';

export function ToolCallCard({ block }: { block: ContentBlock }) {
  const { expandedToolCalls, toggleToolCallExpand } = useMessagesStore();
  if (block.type !== 'tool_use') return null;
  const id = block.id || block.name || '';
  const expanded = expandedToolCalls.has(id);
  const argsStr = JSON.stringify(block.args || {});
  const argsPreview = argsStr.length > 60 ? argsStr.slice(0, 60) + '...' : argsStr;

  return (
    <Box flexDirection="column">
      <Text color={TUI_COLORS.accent}>
        {'  '}{PREFIX.pending} {block.name}({argsPreview})
      </Text>
      {expanded && (
        <Box flexDirection="column" marginLeft={4}>
          <Text color={TUI_COLORS.textMuted}>args: {argsStr}</Text>
        </Box>
      )}
    </Box>
  );
}

/// 工具结果视图
export function ToolResultView({ block }: { block: ContentBlock }) {
  const output = formatToolOutput(block.output, 200);
  return (
    <Text color={TUI_COLORS.textMuted}>
      {'  '}{PREFIX.done} {output}
    </Text>
  );
}