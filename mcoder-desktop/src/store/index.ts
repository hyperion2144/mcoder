// 桌面端导航状态：项目入口 → 会话 tab
// 共享的 sessions / currentSessionId 仍来自 @mcoder/shared/store，
// 这里只存放桌面端专属的视图/项目/已打开 tab 状态。

import { create } from 'zustand';

export type DesktopView = 'projects' | 'sessions';

interface DesktopNavState {
  view: DesktopView;
  currentProject: string | null;
  // 已打开的会话 tab（session_id 列表）
  openTabs: string[];
  setView: (v: DesktopView) => void;
  setCurrentProject: (p: string | null) => void;
  setOpenTabs: (ids: string[]) => void;
  openTab: (sessionId: string) => void;
  closeTab: (sessionId: string) => void;
  reset: () => void;
}

export const useDesktopStore = create<DesktopNavState>((set) => ({
  view: 'projects',
  currentProject: null,
  openTabs: [],

  setView: (v) => set({ view: v }),
  setCurrentProject: (p) => set({ currentProject: p }),
  setOpenTabs: (ids) => set({ openTabs: ids }),
  openTab: (id) =>
    set((st) =>
      st.openTabs.includes(id) ? st : { openTabs: [...st.openTabs, id] },
    ),
  closeTab: (id) =>
    set((st) => ({ openTabs: st.openTabs.filter((t) => t !== id) })),
  reset: () => set({ view: 'projects', currentProject: null, openTabs: [] }),
}));
