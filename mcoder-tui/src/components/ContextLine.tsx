// 设计文档 §6.3: components/ContextLine.tsx - 会话上下文条
// mcoder · 重构登录模块 · plan · gpt-4o · 12.4k/128k · $0.03 · 2 tasks

import { Box, Text } from 'ink';
import { useSessionStore, useMessagesStore } from '../store/index.js';
import { formatContext, formatCost } from '../utils/format.js';

export function ContextLine() {
  const {
    currentSessionTitle, currentRole, currentModel,
    contextUsed, contextWindow, sessionCost, taskCount,
  } = useSessionStore();
  const { streaming } = useMessagesStore();

  const contextPct = contextWindow > 0 ? (contextUsed / contextWindow * 100).toFixed(1) : '0';
  const contextStr = formatContext(contextUsed, contextWindow);
  const costStr = formatCost(sessionCost);
  const taskStr = taskCount > 0 ? `${taskCount} task${taskCount > 1 ? 's' : ''}` : '';
  const runningStr = streaming ? ' · running' : '';

  return (
    <Box paddingX={1}>
      <Text color="gray">
        <Text color="cyan" bold>mcoder</Text>
        {currentSessionTitle && ` · ${currentSessionTitle}`}
        {' · '}
        <Text color="magenta">{currentRole}</Text>
        {currentModel && <> · <Text color="blue">{currentModel}</Text></>}
        {` · ${contextStr} (${contextPct}%)`}
        {costStr && ` · ${costStr}`}
        {taskStr && ` · ${taskStr}`}
        {runningStr && <Text color="yellow">{runningStr}</Text>}
      </Text>
    </Box>
  );
}
