// 设计文档 §8.8: 权限审批 store（TUI 端）
// 与 ask/store 同模式：服务端 PermissionPending 通知 → store 记录 → 渲染卡片
// 用户决议（Allow/Deny/AlwaysAllow）→ store 更新 + 调 client.request 通知服务端

import { create } from 'zustand';

export type PermissionLevel = 'yolo' | 'standard' | 'strict';

export interface PermissionRequest {
  request_id: string;
  session_id: string;
  tool_call_id: string;
  tool_name: string;
  tool_args: unknown;
  reason: string;
  level: PermissionLevel;
}

export type PermissionDecisionKind = 'allow' | 'deny' | 'always_allow';

export interface PermissionDecision {
  type: PermissionDecisionKind;
  reason?: string;
}

interface PermissionState {
  /// session_id → 当前 pending 审批请求
  pending: Record<string, PermissionRequest | null>;
  /// session_id → 已决议历史（last decision）
  history: Record<string, Array<{
    request: PermissionRequest;
    decision: PermissionDecision;
    ts: number;
  }>>;
  /// 当前 session 的权限级别（来自 server snapshot）
  currentLevel: Record<string, PermissionLevel>;

  /// 由 ws_server 推送的 PermissionPending 触发
  setPending: (sessionId: string, req: PermissionRequest) => void;
  /// 由 ws_server 推送的 PermissionResolved 触发（清除 pending + 记录历史）
  setResolved: (sessionId: string, requestId: string, decision: PermissionDecision) => void;
  /// 设置 session 的权限级别（来自 config 或 /permission 命令）
  setLevel: (sessionId: string, level: PermissionLevel) => void;
  /// 清空 session 的所有状态（session 切换时调用）
  clearSession: (sessionId: string) => void;
}

export const usePermissionStore = create<PermissionState>((set) => ({
  pending: {},
  history: {},
  currentLevel: {},

  setPending: (sessionId, req) =>
    set((s) => ({
      pending: { ...s.pending, [sessionId]: req },
    })),

  setResolved: (sessionId, requestId, decision) =>
    set((s) => {
      const current = s.pending[sessionId];
      if (!current || current.request_id !== requestId) {
        // 已不在 pending 中（其他 client 决议过）；只记录 history
        return s;
      }
      const hist = s.history[sessionId] || [];
      return {
        pending: { ...s.pending, [sessionId]: null },
        history: {
          ...s.history,
          [sessionId]: [...hist, { request: current, decision, ts: Date.now() }],
        },
      };
    }),

  setLevel: (sessionId, level) =>
    set((s) => ({
      currentLevel: { ...s.currentLevel, [sessionId]: level },
    })),

  clearSession: (sessionId) =>
    set((s) => {
      const { [sessionId]: _, ...pendingRest } = s.pending;
      const { [sessionId]: __, ...histRest } = s.history;
      const { [sessionId]: ___, ...levelRest } = s.currentLevel;
      return { pending: pendingRest, history: histRest, currentLevel: levelRest };
    }),
}));

/// 序列化 decision → server wire format
export function serializeDecision(d: PermissionDecision) {
  if (d.type === 'allow') return { type: 'allow' };
  if (d.type === 'deny') return { type: 'deny', reason: d.reason ?? null };
  return { type: 'always_allow' };
}