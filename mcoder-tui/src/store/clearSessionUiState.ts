// Phase 5c: clearSessionUiState — 共享的 session-scoped UI 清理 helper
//
// 触发点（三端共用）：
// 1. 切换 session：close / open 新 session 前清理旧 session 的 todos / plan /
//    pending ask / tasks / messages / ask draft，避免闪旧 session 的 UI
// 2. session.close：清空该 session 的 ask / todo / messages
// 3. session.create：新 session 创建后清空（新建前可能残留旧 session UI）
// 4. disconnect：所有 session 全清（终审修复 #17）
//
// 清理粒度：仅清 store 上"per-session"的状态；保留 inputHistory / streaming /
// 当前路由等 session-agnostic 状态。

import { useSessionStore } from './session.js';
import { useMessagesStore } from './messages.js';
import { useAskStore } from '../ask/store.js';

export interface ClearSessionUiOptions {
  /// 明确指定要清空的 session id；省略则清空所有 session scoped state
  sessionId?: string;
  /// 是否清空全局 per-session map（用于 disconnect / 全退场景）
  clearAll?: boolean;
}

/**
 * 清理 session-scoped UI 状态。
 *
 * 始终清：
 * - useMessagesStore.messages（最直接：防止旧 session 消息闪新 session）
 * - useAskStore.draftXxx（按 session 清；clearAll=true 则 resetAll）
 * - useSessionStore 的 pendingPlan / pendingTodos / backgroundTasks / stopReason
 *   （结构化字段：全清，避免闪旧 session 的 plan/todo/task）
 *
 * 不清：
 * - useSessionStore.connected / currentModel / contextWindow（全局）
 * - useMessagesStore.inputHistory（用户输入历史，跨 session 保留）
 */
export function clearSessionUiState(opts: ClearSessionUiOptions = {}): void {
  const { sessionId, clearAll } = opts;
  const sessionStore = useSessionStore.getState();
  const msgStore = useMessagesStore.getState();
  const askStore = useAskStore.getState();

  // 1. messages: 清空（最关键）
  msgStore.setMessages([]);

  // 2. ask store: per-session 清；clearAll 走 resetAll
  if (clearAll) {
    try {
      askStore.resetAll();
    } catch {
      /* swallow */
    }
  } else if (sessionId) {
    try {
      askStore.resetSession(sessionId);
      // 额外清 ask store 的 lastSubmission（防止换 session 后还显示旧 summary）
      askStore.clearSession(sessionId);
    } catch {
      /* swallow */
    }
  }

  // 3. session store 结构化字段：清 plan / todo / tasks
  sessionStore.setPendingPlan(null);
  sessionStore.setPendingTodos(null);
  sessionStore.setBackgroundTasks(null);
  // loop state：清回 idle（除非有显式传入）
  sessionStore.setLoopState('idle', null);
  sessionStore.setCanResume(true);
}
