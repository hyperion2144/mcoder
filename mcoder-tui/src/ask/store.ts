// 共享 store: per-session 的 pending Ask 状态
// 三端各自实例化（zustand）；保持 store API 兼容
// 服务端永远是 source of truth（pending Ask 由 server 广播 session.ask_pending）
//
// 二次 review（issue 7）：把 lastSubmission 从"每 session 单个"
// 改为"按 session + tool_call_id 的 map"——使多个历史 ask 都能显示摘要。
// 同时保留 lastSubmission 单值，作为"最近一次"的兼容查询，供 AskCardSummary
// 等旧消费者使用（不影响 TUI MessageList 中按 tool_call_id 渲染的逻辑）。

import { create } from 'zustand';
import type { AskRequest, AskSubmission } from './types.js';

export interface PendingAsk {
  ask_id: string;        // 服务端 ask_id（与 tool_use.id 不同，独立生成）
  tool_call_id: string;  // 对应 tool_use.id，用于在 messages 流中锚定卡片位置
  session_id: string;
  request: AskRequest;
  created_at: number;
}

/** 单个 ask 的终态记录（按 tool_call_id 索引） */
export interface SubmissionRecord {
  ask_id: string;
  tool_call_id: string;
  submission: AskSubmission;
}

interface AskState {
  /** session_id → 当前 pending Ask；null 表示该 session 无 pending */
  pending: Record<string, PendingAsk | null>;
  /** session_id → 已完成的提交（含 cancelled），用于"原位置显示只读摘要"。
   *  二次 review：按 tool_call_id 索引，使多个历史 ask 都能显示摘要。 */
  submissions: Record<string, Record<string, SubmissionRecord>>;
  /** session_id → 最近一次终态（兼容 AskCardSummary 等旧消费者） */
  lastSubmission: Record<string, SubmissionRecord | null>;
  /** session_id → 当前用户在客户端的选择/输入暂存（提交/取消时清空） */
  draftSelections: Record<string, Record<number, string[]>>;
  draftNotes: Record<string, Record<number, string>>;
  draftFocus: Record<string, number>;
  /** session_id → 输入框是否处于 ask 模式（true 时 InputBox 接受数字键并切换为 ask 提示） */
  askInputMode: Record<string, boolean>;

  setPending: (p: PendingAsk | null) => void;
  /** 设置当前 pending；幂等：同 ask_id + tool_call_id 不重复插入；
   *  若新 tool_call_id 与旧的 pending 不同（覆盖场景），广播 cancelled 并替换 */
  setPendingIdempotent: (p: PendingAsk) => boolean;
  /** Phase 2: hydrateSnapshot 入口 —— 一次性写入 snapshot.pending_ask
   *  取代 attach 后单独 ask.pending 调用 */
  setPendingAskFromSnapshot: (ask: any | null) => void;
  setSubmission: (
    session_id: string,
    ask_id: string,
    tool_call_id: string,
    sub: AskSubmission,
  ) => void;
  /** 同 setSubmission，但工具_call_id 不匹配时不写（防止后写覆盖） */
  setSubmissionIfMatch: (
    session_id: string,
    ask_id: string,
    tool_call_id: string,
    sub: AskSubmission,
  ) => boolean;
  /** 按 tool_call_id 查询单个历史终态（issue 7） */
  getSubmissionByToolCallId: (
    session_id: string,
    tool_call_id: string,
  ) => SubmissionRecord | null;
  /** 校验 ask_id + tool_call_id 后清空 pending（issue 8：防误清） */
  clearPendingByIds: (
    session_id: string,
    ask_id: string,
    tool_call_id: string,
  ) => boolean;
  clearSession: (session_id: string) => void;
  /** 清空 draft + pending + askInputMode（issue 9: 取消/关闭 session 后不残留） */
  resetSession: (session_id: string) => void;
  /** 终审修复 #17：清空所有 session 的 ask store（mobile disconnect / desktop 全退） */
  resetAll: () => void;
  getPending: (session_id: string) => PendingAsk | null;

  // draft 操作
  toggleSelection: (session_id: string, qIndex: number, optionLabel: string) => void;
  setNote: (session_id: string, qIndex: number, note: string) => void;
  setFocus: (session_id: string, qIndex: number) => void;
  setAskInputMode: (session_id: string, on: boolean) => void;
  clearDraft: (session_id: string) => void;
}

export const useAskStore = create<AskState>((set, get) => ({
  pending: {},
  submissions: {},
  lastSubmission: {},
  draftSelections: {},
  draftNotes: {},
  draftFocus: {},
  askInputMode: {},

  setPending: (p) => {
    if (p === null) return;
    set((st) => ({
      pending: { ...st.pending, [p.session_id]: p },
      // 新的 pending：重置 draft
      draftSelections: { ...st.draftSelections, [p.session_id]: {} },
      draftNotes: { ...st.draftNotes, [p.session_id]: {} },
      draftFocus: { ...st.draftFocus, [p.session_id]: 0 },
      askInputMode: { ...st.askInputMode, [p.session_id]: true },
    }));
  },

  setPendingIdempotent: (p) => {
    const cur = get().pending[p.session_id];
    // 已有 pending 且 ask_id 一致 → 不重复插入（issue 7：防止 attach 后重复消息）
    if (cur && cur.ask_id === p.ask_id) {
      return false;
    }
    set((st) => ({
      pending: { ...st.pending, [p.session_id]: p },
      draftSelections: { ...st.draftSelections, [p.session_id]: {} },
      draftNotes: { ...st.draftNotes, [p.session_id]: {} },
      draftFocus: { ...st.draftFocus, [p.session_id]: 0 },
      askInputMode: { ...st.askInputMode, [p.session_id]: true },
    }));
    return true;
  },

  /** Phase 2: hydrateSnapshot 入口 —— 一次性写入 snapshot.pending_ask
   *  null 表示该 session 当前无 pending Ask
   */
  setPendingAskFromSnapshot: (ask) => {
    if (ask === null) return;
    if (!ask.ask_id || !ask.tool_call_id || !ask.session_id || !ask.request) return;
    set((st) => ({
      pending: { ...st.pending, [ask.session_id]: ask },
      draftSelections: { ...st.draftSelections, [ask.session_id]: {} },
      draftNotes: { ...st.draftNotes, [ask.session_id]: {} },
      draftFocus: { ...st.draftFocus, [ask.session_id]: 0 },
      askInputMode: { ...st.askInputMode, [ask.session_id]: true },
    }));
  },

  setSubmission: (session_id, ask_id, tool_call_id, sub) => {
    const record: SubmissionRecord = { ask_id, tool_call_id, submission: sub };
    set((st) => ({
      submissions: {
        ...st.submissions,
        [session_id]: {
          ...(st.submissions[session_id] || {}),
          [tool_call_id]: record,
        },
      },
      lastSubmission: { ...st.lastSubmission, [session_id]: record },
      pending: { ...st.pending, [session_id]: null },
      askInputMode: { ...st.askInputMode, [session_id]: false },
    }));
  },

  setSubmissionIfMatch: (session_id, ask_id, tool_call_id, sub) => {
    const cur = get().pending[session_id];
    const record: SubmissionRecord = { ask_id, tool_call_id, submission: sub };
    // 当前 pending 不匹配 → 不写（首答生效，issue 4 客户端侧防护）
    if (!cur || cur.ask_id !== ask_id || cur.tool_call_id !== tool_call_id) {
      // 但若已 submissions 同 ask_id（仅换 tool_call_id 但 ask_id 相同），覆盖写
      const sub_map = get().submissions[session_id] || {};
      if (sub_map[tool_call_id] && sub_map[tool_call_id].ask_id === ask_id) {
        set((st) => ({
          submissions: {
            ...st.submissions,
            [session_id]: {
              ...sub_map,
              [tool_call_id]: record,
            },
          },
          lastSubmission: { ...st.lastSubmission, [session_id]: record },
        }));
        return true;
      }
      return false;
    }
    set((st) => ({
      submissions: {
        ...st.submissions,
        [session_id]: {
          ...(st.submissions[session_id] || {}),
          [tool_call_id]: record,
        },
      },
      lastSubmission: { ...st.lastSubmission, [session_id]: record },
      pending: { ...st.pending, [session_id]: null },
      askInputMode: { ...st.askInputMode, [session_id]: false },
    }));
    return true;
  },

  getSubmissionByToolCallId: (session_id, tool_call_id) => {
    const map = get().submissions[session_id];
    if (!map) return null;
    return map[tool_call_id] || null;
  },

  // 校验 ask_id + tool_call_id 后清空 pending（issue 8：防止误清其他 ask）
  clearPendingByIds: (session_id, ask_id, tool_call_id) => {
    const cur = get().pending[session_id];
    if (!cur) return false;
    if (cur.ask_id !== ask_id || cur.tool_call_id !== tool_call_id) {
      return false;
    }
    set((st) => ({
      pending: { ...st.pending, [session_id]: null },
      askInputMode: { ...st.askInputMode, [session_id]: false },
    }));
    return true;
  },

  clearSession: (session_id) => {
    set((st) => ({
      pending: { ...st.pending, [session_id]: null },
      askInputMode: { ...st.askInputMode, [session_id]: false },
    }));
  },

  resetSession: (session_id) => {
    set((st) => ({
      pending: { ...st.pending, [session_id]: null },
      draftSelections: { ...st.draftSelections, [session_id]: {} },
      draftNotes: { ...st.draftNotes, [session_id]: {} },
      draftFocus: { ...st.draftFocus, [session_id]: 0 },
      askInputMode: { ...st.askInputMode, [session_id]: false },
    }));
  },

  resetAll: () => {
    set(() => ({
      pending: {},
      submissions: {},
      lastSubmission: {},
      draftSelections: {},
      draftNotes: {},
      draftFocus: {},
      askInputMode: {},
    }));
  },

  getPending: (session_id) => {
    return get().pending[session_id] || null;
  },

  toggleSelection: (session_id, qIndex, optionLabel) => {
    const pending = get().pending[session_id];
    if (!pending) return;
    const q = pending.request.questions[qIndex];
    if (!q) return;
    const isMulti = !!q.multi_select;
    const cur = get().draftSelections[session_id] || {};
    const curForQ = cur[qIndex] || [];
    let next: string[];
    if (isMulti) {
      next = curForQ.includes(optionLabel)
        ? curForQ.filter((s) => s !== optionLabel)
        : [...curForQ, optionLabel];
    } else {
      next = [optionLabel];
    }
    set((st) => ({
      draftSelections: {
        ...st.draftSelections,
        [session_id]: { ...cur, [qIndex]: next },
      },
    }));
  },
  setNote: (session_id, qIndex, note) => {
    set((st) => ({
      draftNotes: {
        ...st.draftNotes,
        [session_id]: { ...(st.draftNotes[session_id] || {}), [qIndex]: note },
      },
    }));
  },
  setFocus: (session_id, qIndex) => {
    set((st) => ({ draftFocus: { ...st.draftFocus, [session_id]: qIndex } }));
  },
  setAskInputMode: (session_id, on) => {
    set((st) => ({ askInputMode: { ...st.askInputMode, [session_id]: on } }));
  },
  clearDraft: (session_id) => {
    set((st) => ({
      draftSelections: { ...st.draftSelections, [session_id]: {} },
      draftNotes: { ...st.draftNotes, [session_id]: {} },
    }));
  },
}));

export function clearAskPendingOnServerEvent(state: AskState, session_id: string, ask_id: string): void {
  const cur = state.pending[session_id];
  if (cur && cur.ask_id === ask_id) {
    state.clearSession(session_id);
  }
}