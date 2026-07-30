// 设计文档 §6.12: store/session.ts - 会话状态管理

import { create } from 'zustand';
import type { SessionMeta, Message } from '../rpc/types.js';
import type { SessionSnapshotTask } from '../rpc/sessionSnapshot.js';

interface SessionState {
  connected: boolean;
  sessions: SessionMeta[];
  currentSessionId: string | null;
  currentSessionTitle: string;
  currentRole: string;
  currentModel: string;
  contextUsed: number;
  contextWindow: number;
  sessionCost: number;
  taskCount: number;
  projectPath: string;
  gitBranch: string;
  filesChanged: number;
  pendingPlan: any | null;
  pendingTodos: any[] | null;
  backgroundTasks: SessionSnapshotTask[] | null;
  // Phase 2: 统一 SessionSnapshot 用
  loopState: string;
  stopReason: string | null;
  canResume: boolean;

  setConnected: (v: boolean) => void;
  setSessions: (s: SessionMeta[]) => void;
  setCurrentSession: (id: string | null) => void;
  setRole: (r: string) => void;
  setModel: (m: string) => void;
  setContextUsage: (used: number, window: number) => void;
  addCost: (c: number) => void;
  setTaskCount: (n: number) => void;
  setProjectPath: (p: string) => void;
  setGitBranch: (b: string) => void;
  setFilesChanged: (n: number) => void;
  setPendingPlan: (p: any | null) => void;
  setPendingTodos: (t: any[] | null) => void;
  setBackgroundTasks: (t: SessionSnapshotTask[] | null) => void;
  setLoopState: (state: string, reason: string | null) => void;
  setCanResume: (v: boolean) => void;
  reset: () => void;
}

export const useSessionStore = create<SessionState>((set) => ({
  connected: false,
  sessions: [],
  currentSessionId: null,
  currentSessionTitle: '',
  currentRole: 'default',
  currentModel: '',
  contextUsed: 0,
  contextWindow: 128000,
  sessionCost: 0,
  taskCount: 0,
  projectPath: '',
  gitBranch: '',
  filesChanged: 0,
  pendingPlan: null,
  pendingTodos: null,
  backgroundTasks: null,
  loopState: 'idle',
  stopReason: null,
  canResume: true,

  setConnected: (v) => set({ connected: v }),
  setSessions: (s) => set({ sessions: s }),
  setCurrentSession: (id) => set({ currentSessionId: id }),
  setRole: (r) => set({ currentRole: r }),
  setModel: (m) => set({ currentModel: m }),
  setContextUsage: (used, window) => set({ contextUsed: used, contextWindow: window }),
  addCost: (c) => set((st) => ({ sessionCost: st.sessionCost + c })),
  setTaskCount: (n) => set({ taskCount: n }),
  setProjectPath: (p) => set({ projectPath: p }),
  setGitBranch: (b) => set({ gitBranch: b }),
  setFilesChanged: (n) => set({ filesChanged: n }),
  setPendingPlan: (p) => set({ pendingPlan: p }),
  setPendingTodos: (t) => set({ pendingTodos: t }),
  setBackgroundTasks: (t) => set({ backgroundTasks: t }),
  setLoopState: (state, reason) => set({ loopState: state, stopReason: reason }),
  setCanResume: (v) => set({ canResume: v }),
  reset: () => set({
    currentSessionId: null,
    currentSessionTitle: '',
    currentRole: 'default',
    contextUsed: 0,
    sessionCost: 0,
    taskCount: 0,
    filesChanged: 0,
    pendingPlan: null,
    pendingTodos: null,
    backgroundTasks: null,
  }),
}));
