// Todo 摘要条（Desktop 端）
//
// 设计：固定放消息区下方、输入框上方
//   - Desktop: 最多 3 条未完成 + "+N more"
//   - 全部完成 → 隐藏
//   - 保留原有完整 Todo 视图（TodoPanel）
//
// 共用逻辑：mcoder-tui/src/todo/summary.ts（selectTodoSummary 等）

import React from 'react';
import { useSessionStore } from '@mcoder/shared/store/index.js';
import {
  selectTodoSummary,
  PLATFORM_DESKTOP,
  type TodoItem,
} from '@mcoder/shared/todo/summary.js';

export function TodoSummaryBar() {
  const todos = useSessionStore((s) => s.pendingTodos);
  const view = selectTodoSummary((todos ?? []) as TodoItem[], PLATFORM_DESKTOP, false);
  if (!view) return null;

  return (
    <div className="todo-summary-bar">
      <div className="todo-summary-header">
        <span className="todo-summary-title">Todos</span>
        <span className="todo-summary-count">{view.totalUnfinished} unfinished</span>
      </div>
      <ul className="todo-summary-list">
        {view.visible.map((t) => (
          <li key={t.id} className={`todo-summary-item todo-status-${t.status}`}>
            <span className="todo-summary-icon">{t.status === 'in_progress' ? '▶' : '☐'}</span>
            <span className="todo-summary-text">{t.content}</span>
          </li>
        ))}
      </ul>
      {view.remaining > 0 && (
        <div className="todo-summary-more">+{view.remaining} more</div>
      )}
    </div>
  );
}