// 设计文档 §6.9: components/HelpView.tsx - 帮助视图
// 命令列表从服务端获取（command.list RPC），客户端不再硬编码

import { useState, useEffect } from 'react';
import { Box, Text } from 'ink';
import type { WsClient } from '../rpc/client.js';

interface CommandEntry {
  name: string;
  type: 'meta' | 'command' | 'skill';
  description: string;
  argument_hint?: string;
}

interface Props {
  client: WsClient;
}

export function HelpView({ client }: Props) {
  const [cmds, setCmds] = useState<CommandEntry[]>([]);

  useEffect(() => {
    client
      .request('command.list')
      .then((result: CommandEntry[]) => setCmds(result))
      .catch(() => setCmds([]));
  }, [client]);

  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single">
      <Text bold color="cyan">
        Commands
      </Text>
      {cmds.map(c => (
        <Text key={c.name + c.type} color="white">
          {'/' + c.name.padEnd(30)} {c.description}
        </Text>
      ))}
      <Text color="gray"> </Text>
      <Text color="gray" bold>
        Shortcuts:
      </Text>
      <Text color="white">Ctrl+S                    sessions list</Text>
      <Text color="white">Ctrl+T                    todo view</Text>
      <Text color="white">Ctrl+K                    task monitor</Text>
      <Text color="white">Ctrl+,                    settings</Text>
      <Text color="white">PgUp/PgDn                 scroll messages</Text>
      <Text color="white">↑/↓                       input history</Text>
      <Text color="white">ESC                       close overlay</Text>
      <Text color="gray" italic>
        press ESC to close
      </Text>
    </Box>
  );
}
