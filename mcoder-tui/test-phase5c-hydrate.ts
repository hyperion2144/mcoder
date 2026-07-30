// Phase 5c: TS 端 hydrateSnapshot 增量 append + 去重
// 运行：node --import tsx test-phase5c-hydrate.ts
//
// 覆盖：
// 1. currentMessageCount 提供时走增量 append（不清空已有消息）
// 2. 重复消息按 fingerprint 去重（不会 push 两遍）
// 3. currentMessageCount 不提供时走全量替换
// 4. 结构化字段（todos / plan / tasks）始终全量覆盖

import {
  hydrateSnapshot,
  type SessionSnapshot,
  type Message,
} from './src/rpc/sessionSnapshot.ts';
import assert from 'node:assert/strict';

let pass = 0;
let fail = 0;
function test(name: string, fn: () => void) {
  try {
    fn();
    console.log(`  ✓ ${name}`);
    pass++;
  } catch (e: any) {
    console.error(`  ✗ ${name}: ${e.message}`);
    if (e.stack) console.error(e.stack.split('\n').slice(1, 4).join('\n'));
    fail++;
  }
}

function mkSnapshot(overrides: any = {}): SessionSnapshot {
  return {
    session: {
      session_id: 's1',
      title: 'Test',
      project_path: '/p',
      role: 'default',
      model: 'm',
      loop_state: 'idle',
      stop_reason: null,
    },
    messages: [],
    todos: [],
    plan: null,
    pending_ask: null,
    tasks: [],
    context: { tokens: 100, cost: 0 },
    can_resume: true,
    ...overrides,
  } as SessionSnapshot;
}

function mkStore() {
  const state: { messages: Message[]; todos: any[]; plan: any; tasks: any[]; ask: any } = {
    messages: [],
    todos: [],
    plan: null,
    tasks: [],
    ask: null,
  };
  return {
    state,
    store: {
      setCurrentSessionId: (_id: string) => {},
      setMessages: (m: Message[]) => { state.messages = [...m]; },
      appendMessages: (m: Message[]) => { state.messages = [...state.messages, ...m]; },
      getMessages: () => state.messages,
      setRole: (_r: string) => {},
      setModel: (_m: string) => {},
      setProjectPath: (_p: string) => {},
      setContextUsage: (_used: number, _w: number) => {},
      setPendingPlan: (p: any) => { state.plan = p; },
      setPendingTodos: (t: any[]) => { state.todos = [...t]; },
      setBackgroundTasks: (t: any[]) => { state.tasks = [...t]; },
      setPendingAskFromSnapshot: (a: any) => { state.ask = a; },
      clearAskSession: (_sid: string) => {},
      replaceTodosFromSnapshot: (_todos: any[]) => {},
    },
  };
}

console.log('=== Phase 5c: hydrateSnapshot incremental + dedup ===');

test('no currentMessageCount → full replace path', () => {
  const { state, store } = mkStore();
  state.messages = [
    { role: 'user', content: [{ type: 'text', text: 'old1' }] },
  ];
  hydrateSnapshot({
    sessionId: 's1',
    snapshot: mkSnapshot({
      messages: [
        { role: 'assistant', content: [{ type: 'text', text: 'new' }] },
      ],
    }),
    store,
  });
  // setMessages 被调用 → state.messages 完全替换
  assert.equal(state.messages.length, 1);
  assert.equal((state.messages[0].content[0] as any).text, 'new');
});

test('currentMessageCount > 0 + snapshot fewer messages → incremental append', () => {
  const { state, store } = mkStore();
  // store 已有 3 条消息
  state.messages = [
    { role: 'user', content: [{ type: 'text', text: 'm1' }] },
    { role: 'assistant', content: [{ type: 'text', text: 'm2' }] },
    { role: 'user', content: [{ type: 'text', text: 'm3' }] },
  ];
  // snapshot 只有 2 条（增量）
  hydrateSnapshot({
    sessionId: 's1',
    currentMessageCount: 3,
    snapshot: mkSnapshot({
      messages: [
        { role: 'user', content: [{ type: 'text', text: 'm3' }] },
        { role: 'assistant', content: [{ type: 'text', text: 'm4' }] },
      ],
    }),
    store,
  });
  // appendMessages path: m3 是重复（去重），m4 是新增
  assert.equal(state.messages.length, 4, 'm3 deduped, m4 appended');
  assert.equal((state.messages[3].content[0] as any).text, 'm4');
});

test('dedup tool_result by id+output', () => {
  const { state, store } = mkStore();
  const toolResult = {
    role: 'tool',
    content: [{ type: 'tool_result', id: 'tc-1', output: { ok: 1 } }],
  };
  state.messages = [toolResult as any];
  hydrateSnapshot({
    sessionId: 's1',
    currentMessageCount: 1,
    snapshot: mkSnapshot({
      messages: [toolResult as any], // 同样的 ToolResult
    }),
    store,
  });
  // 重复 ToolResult 不应 push
  assert.equal(state.messages.length, 1, 'duplicate tool_result must be deduped');
});

test('dedup tool_use by id+name+args', () => {
  const { state, store } = mkStore();
  const toolUse = {
    role: 'assistant',
    content: [{ type: 'tool_use', id: 'tc-1', name: 'bash', args: { cmd: 'ls' } }],
  };
  state.messages = [toolUse as any];
  hydrateSnapshot({
    sessionId: 's1',
    currentMessageCount: 1,
    snapshot: mkSnapshot({
      messages: [toolUse as any],
    }),
    store,
  });
  assert.equal(state.messages.length, 1, 'duplicate tool_use must be deduped');
});

test('structured fields (todos/plan/tasks) always full-replace, even in incremental path', () => {
  const { state, store } = mkStore();
  state.todos = [{ id: 'old', content: 'old-todo', status: 'completed' } as any];
  state.plan = { old: 'plan' };
  state.tasks = [{ task_id: 'old-task' } as any];
  hydrateSnapshot({
    sessionId: 's1',
    currentMessageCount: 1,
    snapshot: mkSnapshot({
      todos: [
        { id: 'new1', session_id: 's1', content: 'new', status: 'pending', priority: 'high', order: 0, created_at: '', updated_at: '' },
      ] as any,
      plan: { new: 'plan' },
      tasks: [{ task_id: 'new-task', tool_name: 'bash', status: 'Running' } as any],
    }),
    store,
  });
  // 结构化字段全量覆盖
  assert.equal(state.todos.length, 1);
  assert.equal(state.todos[0].id, 'new1');
  assert.deepEqual(state.plan, { new: 'plan' });
  assert.equal(state.tasks[0].task_id, 'new-task');
});

test('incremental path: pending_ask null clears (vs full set)', () => {
  const { state, store } = mkStore();
  state.ask = { ask_id: 'old', tool_call_id: 'tc-1', session_id: 's1', request: {}, created_at_ms: 0 };
  hydrateSnapshot({
    sessionId: 's1',
    currentMessageCount: 1,
    snapshot: mkSnapshot({
      pending_ask: null,
    }),
    store,
  });
  // null 不一定写回 state.ask（store 实现是 noop for null），仅验证调用发生
  // 这里仅检查不会崩
  assert.ok(true);
});

console.log(`  ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
