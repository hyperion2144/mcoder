// 设计文档 §6.12: store/ui.ts - UI 状态管理

import { create } from 'zustand';

// 设计文档 §6.7: 视图类型
export type ViewType = 'chat' | 'sessions' | 'todos' | 'tasks' | 'config' | 'help' | 'diff' | 'tree' | 'model' | 'setting';

interface UiState {
  currentView: ViewType;
  // 设计文档 §6.2: 消息滚动偏移
  scrollOffset: number;
  // 设计文档 §6.8: 文件路径补全
  fileCompletions: string[] | null;
  fileCompletionIndex: number;

  setView: (v: ViewType) => void;
  setScrollOffset: (n: number) => void;
  scrollUp: (lines: number) => void;
  scrollDown: (lines: number) => void;
  resetScroll: () => void;
  setFileCompletions: (c: string[] | null) => void;
  navigateFileCompletion: (direction: 'up' | 'down') => string | null;
}

export const useUiStore = create<UiState>((set, get) => ({
  currentView: 'chat',
  scrollOffset: 0,
  fileCompletions: null,
  fileCompletionIndex: 0,

  setView: (v) => set({ currentView: v }),
  setScrollOffset: (n) => set({ scrollOffset: Math.max(0, n) }),
  scrollUp: (lines) => set((st) => ({ scrollOffset: st.scrollOffset + lines })),
  scrollDown: (lines) => set((st) => ({ scrollOffset: Math.max(0, st.scrollOffset - lines) })),
  resetScroll: () => set({ scrollOffset: 0 }),
  setFileCompletions: (c) => set({ fileCompletions: c, fileCompletionIndex: 0 }),

  navigateFileCompletion: (direction) => {
    const { fileCompletions, fileCompletionIndex } = get();
    if (!fileCompletions || fileCompletions.length === 0) return null;
    let newIndex: number;
    if (direction === 'up') {
      newIndex = fileCompletionIndex <= 0
        ? fileCompletions.length - 1
        : fileCompletionIndex - 1;
    } else {
      newIndex = (fileCompletionIndex + 1) % fileCompletions.length;
    }
    set({ fileCompletionIndex: newIndex });
    return fileCompletions[newIndex];
  },
}));
