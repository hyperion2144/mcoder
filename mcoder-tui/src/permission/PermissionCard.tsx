// DESIGN.md §3 / §6: 权限审批卡片（交互类，warning round border）
// 标题：▸ permission · STD · 等待确认
// 移除：🔒、中文括注、emoji

import React from 'react';
import { Box, Text } from 'ink';
import type { PermissionRequest, PermissionLevel } from './store.js';
import { TUI_COLORS, ROLE_COLOR, PREFIX } from '../theme.js';

interface Props {
  request: PermissionRequest;
}

const LEVEL_BADGE: Record<PermissionLevel, { text: string; color: string }> = {
  yolo:     { text: 'YOLO',   color: TUI_COLORS.error },
  standard: { text: 'STD',    color: TUI_COLORS.warning },
  strict:   { text: 'STRICT', color: TUI_COLORS.accent },
};

/** 格式化 tool_args */
function formatToolArgs(args: unknown): string {
  if (!args || typeof args !== 'object') return JSON.stringify(args);
  const obj = args as Record<string, unknown>;
  const parts: string[] = [];
  if (typeof obj.command === 'string') parts.push(`cmd: ${obj.command}`);
  if (typeof obj.file === 'string') parts.push(`file: ${obj.file}`);
  if (typeof obj.path === 'string') parts.push(`path: ${obj.path}`);
  if (typeof obj.pattern === 'string') parts.push(`pattern: ${obj.pattern}`);
  if (typeof obj.query === 'string') parts.push(`query: ${obj.query}`);
  if (typeof obj.url === 'string') parts.push(`url: ${obj.url}`);
  if (typeof obj.action === 'string') parts.push(`action: ${obj.action}`);
  if (parts.length === 0) {
    const full = JSON.stringify(obj);
    return full.length > 200 ? full.slice(0, 200) + '...' : full;
  }
  return parts.join(' · ');
}

/** pending 状态：交互卡片 */
export function PermissionCard({ request }: Props) {
  const badge = LEVEL_BADGE[request.level];
  return (
    <Box flexDirection="column" borderStyle="round" borderColor={ROLE_COLOR.interaction} paddingX={1} marginY={1}>
      <Box>
        <Text color={ROLE_COLOR.interaction} bold>{PREFIX.approval} permission</Text>
        <Text color={badge.color} bold> · [{badge.text}] · 等待确认</Text>
      </Box>
      <Box marginTop={1} flexDirection="column">
        <Text color={TUI_COLORS.textPrimary}>{'  '}tool: <Text color={TUI_COLORS.accent} bold>{request.tool_name}</Text></Text>
        <Text color={TUI_COLORS.textSecondary}>{'     '}{formatToolArgs(request.tool_args)}</Text>
        <Text color={TUI_COLORS.warning}>{'  '}reason: {request.reason}</Text>
      </Box>
      <Box marginTop={1} flexDirection="column">
        <Text color={TUI_COLORS.textMuted}>{'  '}[A] allow · [D] deny · [Y] always allow · [Esc] deny</Text>
      </Box>
    </Box>
  );
}

/** 已决议摘要 */
export function PermissionSummary({
  request,
  decision,
}: {
  request: PermissionRequest;
  decision: 'allow' | 'deny' | 'always_allow';
}) {
  const label = decision === 'allow' ? '已通过'
    : decision === 'always_allow' ? '永久通过'
    : '已拒绝';
  const color = decision === 'deny' ? TUI_COLORS.error : TUI_COLORS.success;
  return (
    <Box flexDirection="column" borderStyle="single" borderColor={ROLE_COLOR.done} paddingX={1} marginY={1}>
      <Text color={TUI_COLORS.textMuted} bold>permission · {label}</Text>
      <Text color={TUI_COLORS.textSecondary}>{'  '}tool: {request.tool_name}</Text>
      <Text color={color}>{'  '}decision: {label}</Text>
    </Box>
  );
}

/** 权限级别徽章（常驻显示） */
export function PermissionLevelBadge({ level }: { level: PermissionLevel }) {
  const badge = LEVEL_BADGE[level];
  return <Text color={badge.color} bold>[{badge.text}]</Text>;
}