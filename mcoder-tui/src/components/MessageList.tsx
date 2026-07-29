// 设计文档 §6.2: components/MessageList.tsx - 消息列表（支持滚动）

import { Box, Text } from 'ink';
import { useMessagesStore, useUiStore } from '../store/index.js';
import { ToolCallCard, ToolResultView } from './ToolCallCard.js';
import type { Message } from '../rpc/types.js';

function MessageView({ msg }: { msg: Message }) {
  const colors: Record<string, string> = {
    user: 'green',
    assistant: 'blue',
    system: 'gray',
    tool: 'yellow',
  };
  const color = colors[msg.role] || 'white';
  const labels: Record<string, string> = {
    user: 'You',
    assistant: 'Assistant',
    system: 'System',
    tool: 'Tool',
  };

  return (
    <Box flexDirection="column" marginY={0}>
      <Text color={color} bold>
        {labels[msg.role] || msg.role}
      </Text>
      {msg.content.map((block, i) => {
        if (block.type === 'text' && block.text) {
          return <Text key={i} color={color}>{block.text}</Text>;
        }
        if (block.type === 'tool_use') {
          return <ToolCallCard key={i} block={block} />;
        }
        if (block.type === 'tool_result') {
          return <ToolResultView key={i} block={block} />;
        }
        return null;
      })}
    </Box>
  );
}

export function MessageList() {
  const { messages, streaming, error } = useMessagesStore();
  const { scrollOffset } = useUiStore();

  // 设计文档 §6.2: 滚动偏移（scrollOffset > 0 表示查看历史）
  const visibleMessages = scrollOffset > 0
    ? messages.slice(0, Math.max(0, messages.length - scrollOffset))
    : messages;

  return (
    <Box flexDirection="column" paddingX={1} flexGrow={1} overflow="hidden">
      {visibleMessages.map((msg, i) => (
        <MessageView key={i} msg={msg} />
      ))}
      {streaming && (
        <Box>
          <Text color="yellow">⠋</Text>
          <Text color="gray"> thinking...</Text>
        </Box>
      )}
      {error && (
        <Text color="red">⚠ {error}</Text>
      )}
      {scrollOffset > 0 && (
        <Text color="gray" italic>↑ {scrollOffset} lines scrolled (PgDn to bottom)</Text>
      )}
    </Box>
  );
}
