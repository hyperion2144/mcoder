// 子代理实时面板：在消息区下方、输入框上方显示子代理列表
// - Ctrl+A 切换面板焦点
// - 焦点时 ↑↓ 导航 + Enter 切换 session + Esc 退出焦点
// - 无子代理时隐藏

import { useState, useEffect, useRef } from 'react';
import { Box, Text, useInput } from 'ink';
import { TUI_COLORS, PREFIX } from '../theme.js';
import { t } from '../i18n.js';
import type { WsClient } from '../rpc/client.js';

interface ChildSession {
  session_id: string;
  title: string;
  model: string;
  source: 'subagent' | 'handoff' | 'normal';
  subagent_role: string | null;
  task_description: string | null;
  loop_state: string;   // "running" | "idle"
  message_count: number;
}

interface Props {
  client: WsClient;
  currentSessionId: string | null;
  /** 切换到指定 session */
  onSwitchSession: (sessionId: string) => void;
  /** 命令选择面板等覆盖层打开时不激活 useInput */
  isActive?: boolean;
  /** 有 pending permission 时不激活 useInput */
  pendingPermission?: boolean;
}

export function SubagentBar({ client, currentSessionId, onSwitchSession, isActive = true, pendingPermission }: Props) {
  const [children, setChildren] = useState<ChildSession[]>([]);
  const [focused, setFocused] = useState(false);
  const [cursor, setCursor] = useState(0);
  // ref 镜像：避免 notification handler 闭包中 children 过期
  const childrenRef = useRef<ChildSession[]>([]);
  childrenRef.current = children;

  // 拉取子代理列表
  const refresh = async () => {
    if (!currentSessionId) { setChildren([]); return; }
    try {
      const result = await client.request('session.list_children', { parent_session_id: currentSessionId });
      setChildren(result || []);
    } catch { /* 静默失败 */ }
  };

  // 首次加载 + session 切换时拉取
  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentSessionId]);

  // 监听 state_changed 通知刷新
  useEffect(() => {
    const handler = (n: any) => {
      if (n.method === 'session.state_changed') {
        const p = n.params;
        // 更新对应子代理的状态
        setChildren(prev => prev.map(c =>
          c.session_id === p.session_id
            ? { ...c, loop_state: p.loop_state, message_count: p.message_count ?? c.message_count }
            : c
        ));
        // 如果是新 session（不在列表中），refresh
        if (!childrenRef.current.some(c => c.session_id === p.session_id)) {
          refresh();
        }
      }
    };
    client.onNotification(handler);
    return () => client.offNotification(handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, currentSessionId]);

  // 监听 session_created 通知刷新
  useEffect(() => {
    const handler = (n: any) => {
      if (n.method === 'session_created' || n.method === 'session.created') {
        refresh();
      }
    };
    client.onNotification(handler);
    return () => client.offNotification(handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client]);

  // useInput: Ctrl+A 切换焦点
  useInput((input, key) => {
    if (key.ctrl && (input === 'a' || input === 'A')) {
      setFocused(f => !f);
      return;
    }
    if (!focused || children.length === 0) return;
    if (key.escape) { setFocused(false); return; }
    if (key.upArrow) setCursor(c => Math.max(0, c - 1));
    else if (key.downArrow) setCursor(c => Math.min(children.length - 1, c + 1));
    else if (key.return) {
      const child = children[cursor];
      if (child) {
        onSwitchSession(child.session_id);
        setFocused(false);
      }
    }
  }, { isActive: isActive && !pendingPermission });

  if (children.length === 0) return null;

  // m10: cursor 越界保护——children 变少时 clamp 到有效范围
  const safeCursor = Math.min(cursor, Math.max(0, children.length - 1));

  return (
    <Box flexDirection="column" borderStyle="single" borderColor={focused ? TUI_COLORS.accent : TUI_COLORS.textMuted} paddingX={1} flexShrink={0}>
      <Box>
        <Text color={TUI_COLORS.accent} bold>{PREFIX.running} {t('ui.subagents')}</Text>
        <Text color={TUI_COLORS.textMuted}> {PREFIX.sep} {children.filter(c => c.loop_state === 'running').length} {t('ui.active')} {PREFIX.sep} {children.length} {t('ui.total')}</Text>
      </Box>
      {focused && <Text color={TUI_COLORS.textMuted}>↑↓ {t('ui.navigate')} {PREFIX.sep} Enter {t('ui.switch')} {PREFIX.sep} Esc {t('ui.back')}</Text>}
      {children.map((c, i) => (
        <Box key={c.session_id}>
          <Text color={i === safeCursor && focused ? TUI_COLORS.accent : TUI_COLORS.textMuted}>
            {i === safeCursor && focused ? `${PREFIX.running} ` : '  '}
          </Text>
          <Text color={c.source === 'handoff' ? TUI_COLORS.mauve : TUI_COLORS.textPrimary}>
            {c.source === 'handoff' ? `${PREFIX.thinking} ` : `${PREFIX.pending} `}
            {c.title.length > 40 ? c.title.slice(0, 37) + '...' : c.title}
          </Text>
          <Text color={c.loop_state === 'running' ? TUI_COLORS.accent : TUI_COLORS.textMuted}>
            {' '}{c.loop_state === 'running' ? t('ui.running') : t('ui.idle')}{' '}
          </Text>
          <Text color={TUI_COLORS.textMuted}>{c.message_count} {t('ui.msg')}</Text>
        </Box>
      ))}
    </Box>
  );
}
