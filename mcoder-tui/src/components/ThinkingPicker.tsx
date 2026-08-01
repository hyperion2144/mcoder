// 设计文档 §thinking: TUI 思考深度选择器
// 复用 ModelView 的交互模式（全屏列表 + ↑↓ + Enter + Esc）
// 调用 config.quick_thinking RPC 切换 session 的思考深度

import { useState } from 'react';
import { Box, Text, useInput } from 'ink';
import { TUI_COLORS, PREFIX } from '../theme.js';
import { t } from '../i18n.js';
import type { WsClient } from '../rpc/client.js';

interface Props {
  client: WsClient;
  sessionId: string | null;
  currentDepth: string;  // "none" | "low" | "medium" | "high" | "max"
  onClose: () => void;
  /** 成功应用后回调，用于父组件同步 currentThinking 状态 */
  onApplied: (depth: string) => void;
  /** S1 修复: 有 pending permission 时由父组件控制是否激活 useInput，避免快捷键冲突 */
  pendingPermission?: boolean;
}

const DEPTHS = [
  { value: 'none', label: 'None', descKey: 'thinking.none.desc' },
  { value: 'low', label: 'Low', descKey: 'thinking.low.desc' },
  { value: 'medium', label: 'Medium', descKey: 'thinking.medium.desc' },
  { value: 'high', label: 'High', descKey: 'thinking.high.desc' },
  { value: 'max', label: 'Max', descKey: 'thinking.max.desc' },
];

export function ThinkingPicker({ client, sessionId, currentDepth, onClose, onApplied, pendingPermission }: Props) {
  // 初始选中当前深度
  const initialIdx = Math.max(0, DEPTHS.findIndex((d) => d.value === currentDepth));
  const [selected, setSelected] = useState(initialIdx);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useInput((input, key) => {
    if (key.escape) { onClose(); return; }
    if (busy) return;
    if (key.upArrow) setSelected((i) => Math.max(0, i - 1));
    else if (key.downArrow) setSelected((i) => Math.min(DEPTHS.length - 1, i + 1));
    else if (key.return) {
      const depth = DEPTHS[selected].value;
      if (!sessionId) {
        setError('no active session');
        return;
      }
      (async () => {
        setBusy(true); setError(null);
        try {
          await client.request('config.quick_thinking', { session_id: sessionId, depth });
          // quick_thinking 不触发 config_updated（不写盘），需手动同步父组件状态
          onApplied(depth);
          onClose();
        } catch (e: any) { setError(e.message); }
        finally { setBusy(false); }
      })();
    }
  }, { isActive: !pendingPermission }); // S1 修复: 有 pending permission 时停用

  return (
    <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.textMuted} paddingX={1}>
      <Text color={TUI_COLORS.accent} bold>{PREFIX.setting} {t('ui.thinking_depth')}</Text>
      <Text color={TUI_COLORS.textMuted}>↑↓ {t('ui.navigate')} {PREFIX.sep} Enter {t('ui.select')} {PREFIX.sep} Esc {t('ui.close')}</Text>
      <Text color={TUI_COLORS.textMuted}>{'─'.repeat(40)}</Text>
      {DEPTHS.map((d, i) => (
        <Box key={d.value}>
          <Text color={i === selected ? TUI_COLORS.accent : TUI_COLORS.textMuted} bold={i === selected}>
            {i === selected ? `${PREFIX.running} ` : '  '}
            {d.value === currentDepth ? `${PREFIX.done} ` : '  '}
            {d.label.padEnd(10)} {t(d.descKey)}
          </Text>
        </Box>
      ))}
      {error && <Text color={TUI_COLORS.error}>{PREFIX.error} {error}</Text>}
      {busy && <Text color={TUI_COLORS.accent}>{PREFIX.loading} {t('ui.switching')}</Text>}
    </Box>
  );
}
