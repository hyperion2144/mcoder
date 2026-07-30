// Phase 2: 统一 SessionSnapshot 类型 + hydrateSnapshot 纯函数
//
// 设计（与 Rust SessionSnapshot 字段一一对应）：
// - session { session_id, title, project_path, role, model, loop_state, stop_reason }
// - messages（offset-aware）
// - todos
// - plan（项目级）
// - pending_ask（in-memory）
// - tasks（best-effort 快照）
// - context { tokens, cost }
// - can_resume
//
// hydrateSnapshot 是**纯函数**（不调用任何 RPC / 模型）：
// 1) 清空当前 session 的旧状态（避免残留旧 session 的消息 / ask / plan / todo）
// 2) 用 snapshot 全量替换（offset>0 时仅替换 messages 为增量，其它字段仍是全量）
//
// 三端（TUI / Desktop / Mobile）attach 路径都通过 hydrateSnapshot 完成状态初始化，
// 不再单独调 ask.pending / task.list / todo.list。

import type { Message } from './types.js';

export interface SessionSnapshotSession {
  session_id: string;
  title: string;
  project_path: string;
  role: string;
  model: string;
  loop_state: 'idle' | 'running' | 'stopped' | string;
  stop_reason: string | null;
}

export interface SessionSnapshotContext {
  tokens: number;
  cost: number;
}

export interface SessionSnapshotPendingAsk {
  ask_id: string;
  tool_call_id: string;
  session_id: string;
  request: any;
  created_at_ms: number;
}

export interface SessionSnapshotTask {
  task_id: string;
  tool_name: string;
  /// Running | Pending | Completed | Failed | Cancelled | Interrupted
  status: string;
  args_json?: any;
  output_json?: any;
  error?: string | null;
  created_at_ms?: number;
  updated_at_ms?: number;
}

export interface SessionSnapshotTodo {
  id: string;
  session_id: string;
  content: string;
  status: string;
  priority: string;
  order: number;
  created_at: string;
  updated_at: string;
}

export interface SessionSnapshot {
  session: SessionSnapshotSession;
  messages: Message[];
  todos: SessionSnapshotTodo[];
  plan: any | null;
  pending_ask: SessionSnapshotPendingAsk | null;
  tasks: SessionSnapshotTask[];
  context: SessionSnapshotContext;
  can_resume: boolean;
}

// ==================== hydrateSnapshot 纯函数 ====================

export interface HydrateInput {
  /** 当前 session 的 id（用于 setCurrentSessionId） */
  sessionId: string;
  /** 服务端返回的 snapshot */
  snapshot: SessionSnapshot;
  /**
   * 可选：当前 store 已有的消息数（用于判断 offset 增量 append 还是全量替换）
   * - 未传或与 snapshot.messages.length 不一致 → 走"全量替换"路径
   * - 传了且 snapshot.messages 较少 → 走"增量 append + 去重"路径
   */
  currentMessageCount?: number;
  /** 三端各自的 store actions（platform-agnostic 注入） */
  store: {
    /** 把 sessionId 写入 store */
    setCurrentSessionId: (id: string) => void;
    /** 清空当前 session 的消息（每次 attach 都先清，避免残留旧 session 的 messages） */
    setMessages: (msgs: Message[]) => void;
    /** 增量追加 messages（不替换；用于重连/增量 hydrate） */
    appendMessages?: (msgs: Message[]) => void;
    /** 读取当前消息（用于 offset-aware dedup） */
    getMessages?: () => Message[];
    /** 设置当前 role（覆盖之前的猜测） */
    setRole: (role: string) => void;
    /** 设置当前 model */
    setModel: (model: string) => void;
    /** 设置当前 project_path */
    setProjectPath: (p: string) => void;
    /** 设置 context used / window */
    setContextUsage: (used: number, window: number) => void;
    /** 设置 plan */
    setPendingPlan: (plan: any | null) => void;
    /** 设置 todos */
    setPendingTodos: (todos: SessionSnapshotTodo[]) => void;
    /** 设置 background tasks（best-effort；当前 TaskManager 是全局的） */
    setBackgroundTasks: (tasks: SessionSnapshotTask[]) => void;
    /** ask store action：把 snapshot.pending_ask 一次性写入（取代 attach 后单独 ask.pending） */
    setPendingAskFromSnapshot: (ask: SessionSnapshotPendingAsk | null) => void;
    /** 清空 ask store 上一次 session 的终态记录（防止换 session 时旧 summary 残留） */
    clearAskSession: (sessionId: string) => void;
    /** todo store action：把 snapshot.todos 替换（清后写） */
    replaceTodosFromSnapshot: (todos: SessionSnapshotTodo[]) => void;
  };
}

/** 计算单条消息的"内容指纹"：用于 hydrate 时去重（避免重连后重复 push 同一 ToolResult 等） */
function messageFingerprint(m: Message): string {
  if (!m || !m.content) return `role:${m?.role || ''}|empty`;
  const parts: string[] = [];
  for (const b of m.content) {
    if (b.type === 'text') {
      parts.push(`text:${(b as any).text}`);
    } else if (b.type === 'tool_use') {
      const t = b as any;
      parts.push(`tool_use:${t.id || ''}:${t.name || ''}:${JSON.stringify(t.args || {})}`);
    } else if (b.type === 'tool_result') {
      const t = b as any;
      parts.push(`tool_result:${t.id || ''}:${JSON.stringify(t.output || {})}`);
    } else {
      parts.push(`unknown:${JSON.stringify(b)}`);
    }
  }
  return `role:${m.role || ''}|${parts.join('|')}`;
}

/** offset>0 增量 hydrate：仅追加 snapshot.messages 中 store 还没有的（按 fingerprint 去重） */
function appendUniqueMessages(
  storeMessages: Message[] | undefined,
  newMessages: Message[],
  appendFn: (msgs: Message[]) => void,
): void {
  if (!storeMessages) {
    // 没有 getter：直接追加（最坏去重失败，但不会丢数据）
    appendFn(newMessages);
    return;
  }
  const existing = new Set(storeMessages.map(messageFingerprint));
  const toAdd: Message[] = [];
  for (const m of newMessages) {
    if (!existing.has(messageFingerprint(m))) {
      toAdd.push(m);
    }
  }
  if (toAdd.length > 0) appendFn(toAdd);
}

/**
 * hydrateSnapshot：把 snapshot 的全量状态一次性写入客户端 store
 *
 * 不变量：
 * 1. 先清空旧 session 的 ask / todo / messages（避免残留）
 * 2. 用 snapshot 全量替换（offset>0 时 messages 仍是增量；其它字段仍为全量最新值）
 * 3. 不调用任何 RPC / 模型 —— 纯客户端 hydration
 * 4. pending_ask 通过 store.setPendingAskFromSnapshot 一次性写入，
 *    三端不再单独调 ask.pending / task.list / todo.list
 * 5. Phase 5c：当传入 `currentMessageCount` 且 store 支持 `appendMessages` /
 *    `getMessages` 时，hydrate 会把新 messages 仅 append（去重），
 *    而不是 setMessages 覆盖，避免重连瞬间闪掉已有消息。
 *    其它结构化字段（session / todos / plan / tasks / pending_ask）仍为
 *    全量最新值覆盖。
 *
 * 该函数是 hydrateSnapshot 实现的唯一入口，三端共用。
 */
export function hydrateSnapshot(input: HydrateInput): void {
  const { snapshot, store } = input;

  // 1. 清空上一 session 的 ask 终态（防止 AskCardSummary 显示旧 session 的历史）
  store.clearAskSession(input.sessionId);

  // 2. setCurrentSessionId（同时清掉旧 session 的 "current" 标记）
  store.setCurrentSessionId(input.sessionId);

  // 3. session meta block（结构化字段：全量覆盖）
  store.setRole(snapshot.session.role);
  store.setModel(snapshot.session.model);
  store.setProjectPath(snapshot.session.project_path);

  // 4. context（Phase 4 再接 pricing；Phase 2 window 从 model_config 取不到，传 0 由 store 兜底）
  store.setContextUsage(snapshot.context.tokens, 0);

  // 5. messages：
  //   - 如果 caller 提供了 currentMessageCount 且 store 支持 append / get，
  //     走"增量 append + 去重"路径（offset>0 重连专用，避免闪消息）
  //   - 否则走"全量替换"路径（首次 attach 或无 store 能力时）
  const useIncremental =
    typeof input.currentMessageCount === 'number' &&
    !!store.appendMessages &&
    !!store.getMessages &&
    input.currentMessageCount > 0 &&
    snapshot.messages.length < input.currentMessageCount + 1000; // sanity check
  if (useIncremental && store.appendMessages && store.getMessages) {
    appendUniqueMessages(store.getMessages(), snapshot.messages, store.appendMessages);
  } else {
    store.setMessages(snapshot.messages);
  }

  // 6. plan（结构化字段：全量覆盖）
  store.setPendingPlan(snapshot.plan ?? null);

  // 7. todos（结构化字段：全量覆盖）
  store.setPendingTodos(snapshot.todos);
  store.replaceTodosFromSnapshot(snapshot.todos);

  // 8. tasks（结构化字段：全量覆盖）
  store.setBackgroundTasks(snapshot.tasks);

  // 9. pending ask（一次性写入；取代 attach 后单独 ask.pending）
  store.setPendingAskFromSnapshot(snapshot.pending_ask);
}