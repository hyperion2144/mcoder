// 设计文档 §6.9: components/HelpView.tsx - 帮助视图

import { Box, Text } from 'ink';
import { listCommands } from '../commands/index.js';

export function HelpView() {
  const cmds = listCommands();
  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single">
      <Text bold color="cyan">Slash Commands</Text>
      {cmds.map(c => (
        <Text key={c.name} color="white">
          {c.usage.padEnd(40)} {c.description}
        </Text>
      ))}
      <Text color="gray"> </Text>
      <Text color="gray" bold>Shortcuts:</Text>
      <Text color="white">Ctrl+S                    sessions list</Text>
      <Text color="white">Ctrl+T                    todo view</Text>
      <Text color="white">Ctrl+K                    task monitor</Text>
      <Text color="white">Ctrl+,                    config view</Text>
      <Text color="white">PgUp/PgDn                 scroll messages</Text>
      <Text color="white">↑/↓                       input history</Text>
      <Text color="white">ESC                       close overlay</Text>
      <Text color="gray" italic>press ESC to close</Text>
    </Box>
  );
}
