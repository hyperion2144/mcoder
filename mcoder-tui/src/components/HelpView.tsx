// DESIGN.md §4 / §10: Help 视图（面板）
// - single border + textMuted
// - 移除：italic、press ESC to close

import { useState, useEffect } from 'react';
import { Box, Text } from 'ink';
import type { WsClient } from '../rpc/client.js';
import { TUI_COLORS } from '../theme.js';
import { t } from '../i18n.js';

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
    <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted}>
      <Text bold color={TUI_COLORS.accent}>{t('ui.commands')}</Text>
      {cmds.map(c => (
        <Text key={c.name + c.type} color={TUI_COLORS.textPrimary}>
          {'/' + c.name.padEnd(30)} {c.description}
        </Text>
      ))}
      <Text color={TUI_COLORS.textMuted}> </Text>
      <Text color={TUI_COLORS.textMuted} bold>{t('ui.shortcuts')}</Text>
      <Text color={TUI_COLORS.textPrimary}>Ctrl+S                    sessions list</Text>
      <Text color={TUI_COLORS.textPrimary}>Ctrl+T                    todo view</Text>
      <Text color={TUI_COLORS.textPrimary}>Ctrl+K                    task monitor</Text>
      <Text color={TUI_COLORS.textPrimary}>Ctrl+,                    settings</Text>
      <Text color={TUI_COLORS.textPrimary}>/provider                 manage LLM providers (/providers also works)</Text>
      <Text color={TUI_COLORS.textPrimary}>/thinking (Ctrl+T)         toggle thinking depth</Text>
      <Text color={TUI_COLORS.textPrimary}>PgUp/PgDn                 scroll messages</Text>
      <Text color={TUI_COLORS.textPrimary}>↑/↓                       input history</Text>
      <Text color={TUI_COLORS.textPrimary}>ESC                       close overlay</Text>
    </Box>
  );
}