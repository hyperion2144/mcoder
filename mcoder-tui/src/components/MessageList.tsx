// 设计文档 §6.2: components/MessageList.tsx - 消息列表（支持滚动）
// ask_user 卡片：当 tool_use.name === 'ask_user' 时，渲染交互式 AskUserCard（pending 状态）
//                或 AskUserSummary（submitted 状态）。完全内联在消息流中。
// 工具调用：统一渲染为 ToolCard（三态折叠 + 流光 loading）

import { Box, Text } from 'ink';
import { useMessagesStore, useUiStore, useSessionStore } from '../store/index.js';
import { ToolCard } from './ToolCard.js';
import { ShimmerText } from './ShimmerText.js';
import { AskUserCard, AskUserSummary } from './AskUserCard.js';
import { PermissionCard, PermissionSummary } from '../permission/PermissionCard.js';
import { usePermissionStore } from '../permission/store.js';
import type { Message, ContentBlock } from '../rpc/types.js';
import { ASK_USER_TOOL } from '../ask/types.js';
import { TUI_COLORS, PREFIX } from '../theme.js';
/// 设计文档 §8.8: 权限审批占位 tool name（与 App.tsx 同步）
const PERMISSION_TOOL_NAME = '__permission_pending__';
import { useAskStore } from '../ask/store.js';
import { formatUsageDelta, shortenPath } from '../utils/format.js';

function MessageView({
  msg,
  askRenderState,
  sessionId,
  /** 按 tool_use id 索引的 result（来自消息流全局配对） */
  resultsById,
}: {
  msg: Message;
  askRenderState?: AskRenderState | null;
  sessionId?: string | null;
  resultsById?: Map<string, ContentBlock>;
}) {
  // DESIGN.md: 角色色统一：user=success / assistant=accent / system=muted / tool=warning
  const colors: Record<string, string> = {
    user: TUI_COLORS.success,
    assistant: TUI_COLORS.accent,
    system: TUI_COLORS.textMuted,
    tool: TUI_COLORS.warning,
  };
  const color = colors[msg.role] || TUI_COLORS.textPrimary;
  const labels: Record<string, string> = {
    user: 'You',
    assistant: 'Assistant',
    system: 'System',
    tool: 'Tool',
  };

  // 二次 review（issue 7）：按 tool_call_id 查询历史终态，让多个 ask 都能显示摘要
  const historicalSub = useAskStore((s) => {
    if (!sessionId || !askRenderState || askRenderState.kind !== 'submitted') {
      return null;
    }
    const sid = sessionId;
    const map = s.submissions[sid];
    if (!map) return null;
    return (askRenderState as any).tool_call_id ? map[(askRenderState as any).tool_call_id] || null : null;
  });

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
          // ask_user 工具：渲染为 AskUserCard（pending）/ AskUserSummary（已提交）
          if (block.name === ASK_USER_TOOL) {
            const isPending =
              askRenderState && askRenderState.kind === 'pending' &&
              block.id === (askRenderState as any).tool_call_id;
            const isSubmitted =
              askRenderState && askRenderState.kind === 'submitted' &&
              block.id === (askRenderState as any).tool_call_id;
            if (isPending) {
              const p = askRenderState as Extract<AskRenderState, { kind: 'pending' }>;
              return (
                <Box key={i} flexDirection="column">
                  <Text color={TUI_COLORS.textMuted}>{`  ${PREFIX.selected} ask_user ${PREFIX.sep} 等待输入`}</Text>
                  <AskUserCard
                    ask_id={p.ask_id}
                    tool_call_id={block.id || ''}
                    request={p.request}
                    selections={p.selections}
                    focusIndex={p.focusIndex}
                  />
                </Box>
              );
            }
            if (isSubmitted) {
              const s = askRenderState as Extract<AskRenderState, { kind: 'submitted' }>;
              // 二次 review（issue 7）：优先用按 tool_call_id 索引的终态，让多个 ask 都能显示
              const submittedSub = historicalSub?.submission || s.submission;
              return (
                <Box key={i} flexDirection="column">
                  <Text color={TUI_COLORS.textMuted}>{`  ${PREFIX.selected} ask_user ${PREFIX.sep} 已回答`}</Text>
                  <AskUserSummary request={(block as any).args || (s as any).request || { questions: [] }} submission={submittedSub as any} />
                </Box>
              );
            }
            // 没有匹配状态：保守渲染为普通 tool_call（避免显示空白）
            return <ToolCard key={i} block={block} resultBlock={null} />;
          }
          // 设计文档 §8.8: 权限审批卡片（与 ask_user 同模式；渲染分支）
          if (block.name === PERMISSION_TOOL_NAME) {
            return <PermissionToolBlock key={i} tool_call_id={block.id || ''} args={block.args} sessionId={sessionId || ''} />;
          }
          // 普通工具调用：统一用 ToolCard（tool_result 不再单独渲染）
          const result = block.id && resultsById ? resultsById.get(block.id) || null : null;
          return (
            <ToolCard
              key={i}
              block={block}
              resultBlock={result}
            />
          );
        }
        if (block.type === 'tool_result') {
          // tool_result 由 ToolCard 内联显示，这里不再单独渲染
          return null;
        }
        if (block.type === 'image' && block.path) {
          const filename = block.path.split('/').pop() || block.path;
          return (
            <Box key={i} flexDirection="column" paddingLeft={1}>
              <Text color={TUI_COLORS.textMuted}>{`[image ${PREFIX.sep} ${filename}]`}</Text>
            </Box>
          );
        }
        return null;
      })}
      {msg.role === 'assistant' && msg.usage && formatUsageDelta(msg.usage) && (
        <Text color={TUI_COLORS.textMuted}>  ↳ {formatUsageDelta(msg.usage)}</Text>
      )}
    </Box>
  );
}

// 设计文档 §8.8: 权限审批的 tool_use 块渲染（独立子组件以正确订阅 zustand）
function PermissionToolBlock({
  tool_call_id,
  sessionId,
}: {
  tool_call_id: string;
  args: unknown;
  sessionId: string;
}) {
  const pending = usePermissionStore((s) => s.pending[sessionId]);
  const history = usePermissionStore((s) => s.history[sessionId] || []);
  if (pending && pending.tool_call_id === tool_call_id) {
    return <PermissionCard request={pending} />;
  }
  const last = history.find((h) => h.request.tool_call_id === tool_call_id);
  if (last) {
    return <PermissionSummary request={last.request} decision={last.decision.type} />;
  }
  return null;
}

export type AskRenderState =
  | {
      kind: 'pending';
      ask_id: string;
      tool_call_id?: string;
      request: import('../ask/types.js').AskRequest;
      selections?: Record<number, string[]>;
      focusIndex?: number;
      notes?: Record<number, string>;
    }
  | {
      kind: 'submitted';
      ask_id: string;
      tool_call_id: string;
      request?: import('../ask/types.js').AskRequest;
      submission: { cancelled: boolean; answers: Record<number, any> };
    };

export function MessageList({
  askRenderState,
  sessionId,
  version = '0.1.0',
  lspServers = [],
}: {
  askRenderState?: AskRenderState | null;
  sessionId?: string | null;
  version?: string;
  lspServers?: string[];
}) {
  const { messages, streaming, error } = useMessagesStore();
  const { scrollOffset } = useUiStore();
  const { currentModel, projectPath } = useSessionStore();

  const visibleMessages = scrollOffset > 0
    ? messages.slice(0, Math.max(0, messages.length - scrollOffset))
    : messages;

  // 全局配对 tool_use → tool_result
  const resultsById = new Map<string, ContentBlock>();
  for (const msg of visibleMessages) {
    for (const block of msg.content) {
      if (block.type === 'tool_result' && block.id) {
        if (!resultsById.has(block.id)) {
          resultsById.set(block.id, block);
        }
      }
    }
  }

  return (
    <Box flexDirection="column" paddingX={1} flexGrow={1} overflow="hidden">
      {/* Top info - visible at bottom (scrollOffset=0), scrolls away when user scrolls up */}
      {scrollOffset === 0 && (
        <Box flexDirection="column" flexShrink={0} marginBottom={1}>
          <Text bold color={TUI_COLORS.accent}>mcoder v{version}</Text>
          <Text color={TUI_COLORS.textMuted}>{`${currentModel || '-'} ${PREFIX.sep} ${shortenPath(projectPath) || '-'}`}</Text>
        </Box>
      )}
      {visibleMessages.map((msg, i) => (
        <MessageView key={i} msg={msg} askRenderState={askRenderState} sessionId={sessionId} resultsById={resultsById} />
      ))}
      {streaming && (
        <Box>
          <ShimmerText text={PREFIX.running + ' Thinking'} />
        </Box>
      )}
      {error && (
        <Text color={TUI_COLORS.error}>{error}</Text>
      )}
      {scrollOffset > 0 && (
        <Text color={TUI_COLORS.textMuted}>{`↑ ${scrollOffset} lines scrolled ${PREFIX.sep} PgDn to bottom`}</Text>
      )}
    </Box>
  );
}
