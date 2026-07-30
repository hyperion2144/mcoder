// 设计文档 §6.12: store/messages.ts - 消息状态管理

import { create } from 'zustand';
import type { Message } from '../rpc/types.js';

interface MessagesState {
  messages: Message[];
  streaming: boolean;
  error: string | null;
  expandedToolCalls: Set<string>;
  // 设计文档 §6.8: 历史记录
  inputHistory: string[];
  historyIndex: number;

  setMessages: (m: Message[]) => void;
  addMessage: (m: Message) => void;
  /// Phase 5c: 增量追加 messages（不替换；用于重连/增量 hydrate）
  appendMessages: (msgs: Message[]) => void;
  setStreaming: (v: boolean) => void;
  setError: (e: string | null) => void;
  toggleToolCallExpand: (id: string) => void;
  addInputHistory: (input: string) => void;
  navigateHistory: (direction: 'up' | 'down') => string | null;
  resetHistory: () => void;
  clear: () => void;
}

export const useMessagesStore = create<MessagesState>((set, get) => ({
  messages: [],
  streaming: false,
  error: null,
  expandedToolCalls: new Set(),
  inputHistory: [],
  historyIndex: -1,

  setMessages: (m) => set({ messages: m }),
  addMessage: (m) => set((st) => ({ messages: [...st.messages, m] })),
  appendMessages: (msgs) =>
    set((st) => {
      if (!msgs || msgs.length === 0) return st;
      // Phase 5c: 用简单 fingerprint 去重，防止重连把同一 ToolResult 推两遍
      const fp = (m: Message) => {
        if (!m || !m.content) return `${m?.role || ''}#empty`;
        const parts: string[] = [];
        for (const b of m.content) {
          if ((b as any).type === 'text') {
            parts.push(`t:${(b as any).text}`);
          } else if ((b as any).type === 'tool_use') {
            const t = b as any;
            parts.push(`u:${t.id || ''}:${t.name || ''}:${JSON.stringify(t.args || {})}`);
          } else if ((b as any).type === 'tool_result') {
            const t = b as any;
            parts.push(`r:${t.id || ''}:${JSON.stringify(t.output || {})}`);
          } else {
            parts.push(`x:${JSON.stringify(b)}`);
          }
        }
        return `${m.role || ''}|${parts.join('|')}`;
      };
      const existing = new Set(st.messages.map(fp));
      const toAdd: Message[] = [];
      for (const m of msgs) {
        if (!existing.has(fp(m))) {
          toAdd.push(m);
        }
      }
      if (toAdd.length === 0) return st;
      return { messages: [...st.messages, ...toAdd] };
    }),
  setStreaming: (v) => set({ streaming: v }),
  setError: (e) => set({ error: e }),
  toggleToolCallExpand: (id) => set((st) => {
    const next = new Set(st.expandedToolCalls);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    return { expandedToolCalls: next };
  }),

  // 设计文档 §6.8: 上下箭头切换历史输入
  addInputHistory: (input: string) => set((st) => ({
    inputHistory: [...st.inputHistory, input].slice(-100), // 最多保留 100 条
    historyIndex: -1,
  })),

  navigateHistory: (direction: 'up' | 'down') => {
    const { inputHistory, historyIndex } = get();
    if (inputHistory.length === 0) return null;

    let newIndex: number;
    if (direction === 'up') {
      // 向上 = 更早的历史
      newIndex = historyIndex === -1
        ? inputHistory.length - 1
        : Math.max(0, historyIndex - 1);
    } else {
      // 向下 = 更近的历史
      if (historyIndex === -1) return null;
      newIndex = historyIndex + 1;
      if (newIndex >= inputHistory.length) {
        set({ historyIndex: -1 });
        return ''; // 超出范围返回空字符串（清空输入框）
      }
    }
    set({ historyIndex: newIndex });
    return inputHistory[newIndex] || null;
  },

  resetHistory: () => set({ historyIndex: -1 }),
  clear: () => set({ messages: [], streaming: false, error: null }),
}));
