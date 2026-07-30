// Todo 摘要条（Mobile 端）
//
// 设计：固定放消息区下方、输入框上方
//   - Mobile: 默认 1 条（折叠），可点击展开最多 3 条
//   - 全部完成 → 隐藏
//   - 保留原有完整 Todo 视图（MobileTodoPanel）
//
// 共用逻辑：mcoder-tui/src/todo/summary.ts（selectTodoSummary 等）

import React, { useState } from 'react';
import { useSessionStore } from '@mcoder/shared/store/index.js';
import {
  selectTodoSummary,
  PLATFORM_MOBILE,
  type TodoItem,
} from '@mcoder/shared/todo/summary.js';

export function TodoSummaryBar() {
  const todos = useSessionStore((s) => s.pendingTodos);
  const [expanded, setExpanded] = useState(false);

  const view = selectTodoSummary((todos ?? []) as TodoItem[], PLATFORM_MOBILE, expanded);
  if (!view) return null;

  const canExpand = view.totalUnfinished > PLATFORM_MOBILE.maxVisibleCollapsed;

  return (
    <div className="todo-summary-bar">
      <div
        className="todo-summary-header"
        onClick={() => canExpand && setExpanded(!expanded)}
        role={canExpand ? 'button' : undefined}
      >
        <span className="todo-summary-title">Todos</span>
        <span className="todo-summary-count">
          {view.totalUnfinished} unfinished
          {canExpand && (
            <span className="todo-summary-toggle">{expanded ? ' ▲' : ' ▼'}</span>
          )}
        </span>
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