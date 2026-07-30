// Todo 摘要条（三端共用逻辑：mcoder-tui/src/todo/summary.ts）
//
// 设计：固定放消息区下方、输入框上方
//   - TUI/Desktop: 最多 3 条未完成 + "+N more"
//   - Mobile: 默认 1 条，点击展开最多 3 条
//   - 全部完成 → 隐藏
//   - 保留原有完整 Todo 视图（TodoView + TodoPanel + MobileTodoPanel）
//
// 这里只渲染"摘要条"。完整 Todo 视图保留 TodoView 等已有组件。

import React from 'react';
import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';
import {
  selectTodoSummary,
  type TodoSummaryPlatform,
  PLATFORM_TUI,
} from '../todo/summary.js';

interface Props {
  /// 平台配置：TUI 默认 PLATFORM_TUI（与 Desktop 相同），Mobile 单独组件传 PLATFORM_MOBILE
  platform?: TodoSummaryPlatform;
}

export function TodoSummaryBar({ platform = PLATFORM_TUI }: Props) {
  const todos = useSessionStore((s) => s.pendingTodos);
  const view = selectTodoSummary(todos ?? [], platform, false);
  if (!view) return null;

  const labels: Record<string, string> = {
    in_progress: '▶',
    pending: '☐',
  };
  const colors: Record<string, string> = {
    in_progress: 'cyan',
    pending: 'yellow',
  };

  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor="gray">
      <Box>
        <Text bold color="cyan">Todos</Text>
        <Text color="gray"> · {view.totalUnfinished} unfinished</Text>
      </Box>
      {view.visible.map((t) => (
        <Text key={t.id} color={colors[t.status] || 'white'}>
          {labels[t.status] || '☐'} {t.content}
        </Text>
      ))}
      {view.remaining > 0 && (
        <Text color="gray" italic>+{view.remaining} more</Text>
      )}
    </Box>
  );
}