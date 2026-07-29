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
