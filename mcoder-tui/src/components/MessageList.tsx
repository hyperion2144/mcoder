// mcoder UI Redesign v2 - MessageList
// Layout: header-card (logo + tips + LSP + recent sessions) + messages stream
// Messages: role label + body; thinking block (mauve italic); tool cards inline

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
const PERMISSION_TOOL_NAME = '__permission_pending__';
import { useAskStore } from '../ask/store.js';
import { formatUsageDelta, shortenPath } from '../utils/format.js';
import { t } from '../i18n.js';

function MessageView({
  msg,
  askRenderState,
  sessionId,
  resultsById,
}: {
  msg: Message;
  askRenderState?: AskRenderState | null;
  sessionId?: string | null;
  resultsById?: Map<string, ContentBlock>;
}) {
  const colors: Record<string, string> = {
    user: TUI_COLORS.success,
    assistant: TUI_COLORS.accent,
    system: TUI_COLORS.textMuted,
    tool: TUI_COLORS.warning,
  };
  const color = colors[msg.role] || TUI_COLORS.textPrimary;
  const labels: Record<string, string> = {
    user: 'user',
    assistant: 'mcoder',
    system: 'system',
    tool: 'tool',
  };

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
          return <Text key={i} color={color}>  {block.text}</Text>;
        }
        if (block.type === 'tool_use') {
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
                  <Text color={TUI_COLORS.warning}>{`  ${PREFIX.pending} ask_user ${PREFIX.sep} waiting for input`}</Text>
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
              const submittedSub = historicalSub?.submission || s.submission;
              return (
                <Box key={i} flexDirection="column">
                  <Text color={TUI_COLORS.textMuted}>{`  ask_user ${PREFIX.sep} answered`}</Text>
                  <AskUserSummary request={(block as any).args || (s as any).request || { questions: [] }} submission={submittedSub as any} />
                </Box>
              );
            }
            return <ToolCard key={i} block={block} resultBlock={null} />;
          }
          if (block.name === PERMISSION_TOOL_NAME) {
            return <PermissionToolBlock key={i} tool_call_id={block.id || ''} args={block.args} sessionId={sessionId || ''} />;
          }
          const result = block.id && resultsById ? resultsById.get(block.id) || null : null;
          return <ToolCard key={i} block={block} resultBlock={result} />;
        }
        if (block.type === 'tool_result') {
          return null;
        }
        if (block.type === 'image' && block.path) {
          const filename = block.path.split('/').pop() || block.path;
          return (
            <Box key={i} flexDirection="column" paddingLeft={2}>
              <Text color={TUI_COLORS.textMuted}>[image {PREFIX.sep} {filename}]</Text>
            </Box>
          );
        }
        return null;
      })}
      {msg.role === 'assistant' && msg.usage && formatUsageDelta(msg.usage) && (
        <Text color={TUI_COLORS.textMuted}>  {formatUsageDelta(msg.usage)}</Text>
      )}
    </Box>
  );
}

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
  const { currentModel, projectPath, sessions, currentSessionTitle } = useSessionStore();

  const visibleMessages = scrollOffset > 0
    ? messages.slice(0, Math.max(0, messages.length - scrollOffset))
    : messages;

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

  // Header card: only show when at bottom (scrollOffset=0) and no messages yet (welcome screen)
  const showHeader = scrollOffset === 0 && visibleMessages.length === 0;

  return (
    <Box flexDirection="column" paddingX={1} flexGrow={1} overflow="hidden">
      {/* Header card: logo + tips + LSP + recent sessions */}
      {showHeader && (
        <Box flexDirection="column" borderStyle="round" borderColor={TUI_COLORS.textMuted} flexShrink={0} marginBottom={1}>
          {/* Welcome section */}
          <Box paddingX={2} paddingY={1}>
            <Text color={TUI_COLORS.cyan} bold>{'mcoder'}</Text>
            <Text color={TUI_COLORS.textMuted}> v{version}</Text>
          </Box>
          {/* Tips */}
          <Box paddingX={2} flexDirection="column">
            <Text color={TUI_COLORS.textPrimary} bold>Tips</Text>
            <Text color={TUI_COLORS.textMuted}>  # prompt actions  / commands  ! bash  $ python</Text>
          </Box>
          {/* LSP + Recent sessions */}
          <Box paddingX={2} flexDirection="column">
            {lspServers.length > 0 && (
              <>
                <Text color={TUI_COLORS.textPrimary} bold>LSP Servers</Text>
                {lspServers.map((s) => (
                  <Text key={s} color={TUI_COLORS.textMuted}>  {PREFIX.dot} {s}</Text>
                ))}
              </>
            )}
            {sessions.length > 0 && (
              <>
                <Text color={TUI_COLORS.textPrimary} bold>Recent sessions</Text>
                {sessions.slice(0, 4).map((s) => (
                  <Text key={s.session_id} color={TUI_COLORS.textMuted}>  {PREFIX.dot} {s.title}</Text>
                ))}
              </>
            )}
          </Box>
        </Box>
      )}

      {/* Top info bar (when at bottom but has messages) */}
      {scrollOffset === 0 && !showHeader && (
        <Box flexDirection="column" flexShrink={0} marginBottom={1}>
          <Text color={TUI_COLORS.textMuted}>
            {currentModel || '-'} {PREFIX.sep} {shortenPath(projectPath) || '-'}
          </Text>
        </Box>
      )}

      {visibleMessages.map((msg, i) => (
        <MessageView key={i} msg={msg} askRenderState={askRenderState} sessionId={sessionId} resultsById={resultsById} />
      ))}
      {streaming && (
        <Box paddingLeft={2}>
          <ShimmerText text={`${PREFIX.running} thinking`} />
        </Box>
      )}
      {error && (
        <Text color={TUI_COLORS.error}>  {error}</Text>
      )}
      {scrollOffset > 0 && (
        <Text color={TUI_COLORS.textMuted}>{`${PREFIX.sep} ${scrollOffset} lines scrolled ${PREFIX.sep} PgDn to bottom`}</Text>
      )}
    </Box>
  );
}
