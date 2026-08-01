// 命令选择面板：用户输入 / 时弹出，显示命令名 + 描述
// 交互模式类似 ThinkingPicker：↑↓ 导航 + Enter 选中 + Esc 关闭
// 调用 command.list RPC 获取命令列表（description 已按当前语言翻译）

import { useState, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';
import { TUI_COLORS, PREFIX } from '../theme.js';
import { t } from '../i18n.js';
import type { WsClient } from '../rpc/client.js';

interface CommandEntry {
  name: string;
  type: string;      // "meta" | "command" | "skill"
  description: string;
  argument_hint?: string;
}

interface Props {
  client: WsClient;
  /** 用户已输入的部分（如 "/ha"），用于过滤 */
  filter: string;
  /** 选中命令后回调 */
  onSelect: (command: string) => void;
  /** 关闭面板 */
  onClose: () => void;
  pendingPermission?: boolean;
}

export function CommandPicker({ client, filter, onSelect, onClose, pendingPermission }: Props) {
  const [commands, setCommands] = useState<CommandEntry[]>([]);
  const [cursor, setCursor] = useState(0);

  useEffect(() => {
    (async () => {
      try {
        const result = await client.request('command.list');
        setCommands(result || []);
      } catch { /* 静默 */ }
    })();
  }, []);

  // 根据 filter 过滤命令（filter 去掉前导 /）
  const filtered = commands.filter(c =>
    c.name.startsWith(filter.replace(/^\//, ''))
  );

  const safeCursor = Math.min(cursor, Math.max(0, filtered.length - 1));

  useInput((input, key) => {
    if (key.escape) { onClose(); return; }
    if (key.upArrow) setCursor(c => Math.max(0, c - 1));
    else if (key.downArrow) setCursor(c => Math.min(filtered.length - 1, c + 1));
    else if (key.return) {
      const cmd = filtered[safeCursor];
      if (cmd) onSelect('/' + cmd.name);
    }
  }, { isActive: !pendingPermission });

  if (filtered.length === 0) {
    return (
      <Box borderStyle="single" borderColor={TUI_COLORS.textMuted} paddingX={1}>
        <Text color={TUI_COLORS.textMuted}>{t('ui.no_commands')}</Text>
      </Box>
    );
  }

  return (
    <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.accent} paddingX={1}>
      <Text color={TUI_COLORS.accent} bold>{PREFIX.setting} {t('ui.commands')}</Text>
      <Text color={TUI_COLORS.textMuted}>↑↓ {t('ui.navigate')} {PREFIX.sep} Enter {t('ui.select')} {PREFIX.sep} Esc {t('ui.close')}</Text>
      <Text color={TUI_COLORS.textMuted}>{'─'.repeat(60)}</Text>
      {filtered.map((c, i) => (
        <Box key={c.name}>
          <Text color={i === safeCursor ? TUI_COLORS.accent : TUI_COLORS.textMuted}>
            {i === safeCursor ? `${PREFIX.running} ` : '  '}
          </Text>
          <Text color={TUI_COLORS.textPrimary} bold={i === safeCursor}>
            /{c.name.padEnd(20)}
          </Text>
          <Text color={TUI_COLORS.textMuted}>
            {' '}{c.description}
          </Text>
        </Box>
      ))}
    </Box>
  );
}
