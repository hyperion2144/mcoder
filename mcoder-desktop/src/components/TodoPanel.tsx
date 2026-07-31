// 设计文档 §6.7: Todo 面板（goal mode）
// 显示当前 todo 列表，已完成/未完成状态

import React from 'react';
import { Check, Square } from './icons.js';

interface Props {
  todos: any[] | null;
}

export function TodoPanel({ todos }: Props) {
  if (!todos || todos.length === 0) return null;

  const done = todos.filter((t: any) => t.done || t.status === 'done').length;
  const total = todos.length;
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;

  return (
    <div className="todo-panel">
      <div className="todo-panel-header">
        <span className="todo-panel-title">Todos</span>
        <span className="todo-panel-progress">
          {done}/{total} · {pct}%
        </span>
      </div>
      <div className="todo-progress-bar">
        <div className="todo-progress-fill" style={{ width: `${pct}%` }} />
      </div>
      <ul className="todo-list">
        {todos.map((todo: any, i: number) => {
          const isDone = todo.done || todo.status === 'done';
          return (
            <li key={i} className={`todo-item ${isDone ? 'todo-done' : ''}`}>
              <span className="todo-check">{isDone ? <Check size={14} /> : <Square size={14} />}</span>
              <span className="todo-text">
                {todo.text || todo.description || JSON.stringify(todo)}
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
