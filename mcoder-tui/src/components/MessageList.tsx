// 设计文档 §6.2: components/MessageList.tsx - 消息列表（支持滚动）
// ask_user 卡片：当 tool_use.name === 'ask_user' 时，渲染交互式 AskUserCard（pending 状态）
//                或 AskUserSummary（submitted 状态）。完全内联在消息流中。
// 工具调用：统一渲染为 ToolCard（三态折叠 + 流光 loading）

import { Box, Text } from 'ink';
import Image from 'ink-picture';
import { useMessagesStore, useUiStore, useSessionStore } from '../store/index.js';
import { ToolCard } from './ToolCard.js';
import { AskUserCard, AskUserSummary } from './AskUserCard.js';
import type { Message, ContentBlock } from '../rpc/types.js';
import { ASK_USER_TOOL } from '../ask/types.js';
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
                  <Text color="yellow">  ▸ ask_user (等待你的回答)</Text>
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
                  <Text color="gray">  ▸ ask_user (已回答)</Text>
                  <AskUserSummary request={(block as any).args || (s as any).request || { questions: [] }} submission={submittedSub as any} />
                </Box>
              );
            }
            // 没有匹配状态：保守渲染为普通 tool_call（避免显示空白）
            return <ToolCard key={i} block={block} resultBlock={null} />;
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
              <Image src={block.path} width={40} height={20} alt={`image: ${filename}`} />
              <Text color="gray" dimColor>  {filename}</Text>
            </Box>
          );
        }
        return null;
      })}
      {msg.role === 'assistant' && msg.usage && formatUsageDelta(msg.usage) && (
        <Text color="gray" dimColor>  ↳ {formatUsageDelta(msg.usage)}</Text>
      )}
    </Box>
  );
}

// ask 渲染状态：pending = 显示 AskUserCard；submitted = 显示 AskUserSummary
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
          <Text bold color="cyan">mcoder v{version}</Text>
          <Text color="gray">model: {currentModel || '-'}  project: {shortenPath(projectPath) || '-'}</Text>
          {lspServers.length > 0 && (
            <Text color="gray">lsp: {lspServers.join(', ')}</Text>
          )}
        </Box>
      )}
      {visibleMessages.map((msg, i) => (
        <MessageView key={i} msg={msg} askRenderState={askRenderState} sessionId={sessionId} resultsById={resultsById} />
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
