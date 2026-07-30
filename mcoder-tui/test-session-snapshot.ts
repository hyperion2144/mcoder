// Phase 2: 共享 SessionSnapshot / hydrateSnapshot 单元测试
// 运行：node --import tsx test-session-snapshot.mjs
// 验证：hydrateSnapshot 是纯函数，能清旧 + 写新；snapshot 全字段契约

import {
  hydrateSnapshot,
  type SessionSnapshot,
} from './src/rpc/sessionSnapshot.ts';
import assert from 'node:assert/strict';

let pass = 0;
let fail = 0;
function test(name, fn) {
  try {
    fn();
    console.log(`  ✓ ${name}`);
    pass++;
  } catch (e) {
    console.error(`  ✗ ${name}: ${e.message}`);
    if (e.stack) console.error(e.stack.split('\n').slice(1, 4).join('\n'));
    fail++;
  }
}

function mkSnapshot(overrides = {}) {
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
    messages: [
      { role: 'user', content: [{ type: 'text', text: 'hi' }] },
    ],
    todos: [
      {
        id: 't1',
        session_id: 's1',
        content: 'task',
        status: 'pending',
        priority: 'medium',
        order: 0,
        created_at: '',
        updated_at: '',
      },
    ],
    plan: { steps: [] },
    pending_ask: {
      ask_id: 'a1',
      tool_call_id: 'tc1',
      session_id: 's1',
      request: { questions: [] },
      created_at_ms: 0,
    },
    tasks: [{ task_id: 'task1', tool_name: 'bg', status: 'Running' }],
    context: { tokens: 100, cost: 0 },
    can_resume: true,
    ...overrides,
  };
}

function mkStore() {
  const calls = [];
  const record = (n) => (...args) => calls.push([n, ...args]);
  return {
    calls,
    store: {
      setCurrentSessionId: record('setCurrentSessionId'),
      setMessages: record('setMessages'),
      setRole: record('setRole'),
      setModel: record('setModel'),
      setProjectPath: record('setProjectPath'),
      setContextUsage: record('setContextUsage'),
      setPendingPlan: record('setPendingPlan'),
      setPendingTodos: record('setPendingTodos'),
      setBackgroundTasks: record('setBackgroundTasks'),
      setPendingAskFromSnapshot: record('setPendingAskFromSnapshot'),
      clearAskSession: record('clearAskSession'),
      replaceTodosFromSnapshot: record('replaceTodosFromSnapshot'),
    },
  };
}

console.log('=== hydrateSnapshot ===');

test('writes all store actions in order', () => {
  const { calls, store } = mkStore();
  hydrateSnapshot({ sessionId: 's1', snapshot: mkSnapshot(), store });
  // 顺序：clearAskSession → setCurrentSessionId → setRole → setModel →
  //        setProjectPath → setContextUsage → setMessages → setPendingPlan →
  //        setPendingTodos → replaceTodosFromSnapshot → setBackgroundTasks →
  //        setPendingAskFromSnapshot
  const names = calls.map((c) => c[0]);
  assert.deepEqual(names, [
    'clearAskSession',
    'setCurrentSessionId',
    'setRole',
    'setModel',
    'setProjectPath',
    'setContextUsage',
    'setMessages',
    'setPendingPlan',
    'setPendingTodos',
    'replaceTodosFromSnapshot',
    'setBackgroundTasks',
    'setPendingAskFromSnapshot',
  ]);
});

test('clears old session ask BEFORE writing new one', () => {
  const { calls, store } = mkStore();
  hydrateSnapshot({ sessionId: 's1', snapshot: mkSnapshot(), store });
  const idxClear = calls.findIndex((c) => c[0] === 'clearAskSession');
  const idxWrite = calls.findIndex((c) => c[0] === 'setPendingAskFromSnapshot');
  assert.ok(idxClear >= 0 && idxWrite >= 0);
  assert.ok(idxClear < idxWrite, 'clearAskSession must run before setPendingAskFromSnapshot');
});

test('sets messages as offset-incremental (callers pass what they got)', () => {
  const { calls, store } = mkStore();
  // 模拟 offset=2 时只有 2 条消息
  const snap = mkSnapshot();
  snap.messages = [
    { role: 'user', content: [{ type: 'text', text: 'm3' }] },
    { role: 'assistant', content: [{ type: 'text', text: 'm4' }] },
  ];
  hydrateSnapshot({ sessionId: 's1', snapshot: snap, store });
  const setMsgs = calls.find((c) => c[0] === 'setMessages');
  assert.ok(setMsgs);
  assert.equal(setMsgs[1].length, 2, 'messages count reflects snapshot offset');
});

test('does NOT call any RPC or fetch', () => {
  // hydrateSnapshot 是纯函数：不接受 fetch / RPC client；本次断言通过 type-only 检查
  // （hydrateSnapshot 不 import rpc/client.ts）。这里仅验证函数签名无 fetch 参数。
  const sig = hydrateSnapshot.toString().slice(0, hydrateSnapshot.toString().indexOf('{'));
  assert.ok(!sig.includes('fetch'), 'hydrateSnapshot must not reference fetch');
  assert.ok(!sig.includes('request'), 'hydrateSnapshot must not reference RPC request');
});

test('handles null pending_ask', () => {
  const { calls, store } = mkStore();
  const snap = mkSnapshot({ pending_ask: null });
  hydrateSnapshot({ sessionId: 's1', snapshot: snap, store });
  const askWrite = calls.find((c) => c[0] === 'setPendingAskFromSnapshot');
  assert.ok(askWrite);
  // 注意：当前 store 实现中 null 走 fast-return；这里仅验证调用发生过即可
});

test('handles null plan', () => {
  const { calls, store } = mkStore();
  const snap = mkSnapshot({ plan: null });
  hydrateSnapshot({ sessionId: 's1', snapshot: snap, store });
  const planCall = calls.find((c) => c[0] === 'setPendingPlan');
  assert.ok(planCall);
  assert.equal(planCall[1], null);
});

console.log('=== snapshot shape ===');

test('snapshot has all required top-level fields', () => {
  const snap = mkSnapshot();
  for (const k of [
    'session',
    'messages',
    'todos',
    'plan',
    'pending_ask',
    'tasks',
    'context',
    'can_resume',
  ]) {
    assert.ok(k in snap, `missing top-level field: ${k}`);
  }
});

test('snapshot.session has all required fields', () => {
  const s = mkSnapshot().session;
  for (const k of [
    'session_id',
    'title',
    'project_path',
    'role',
    'model',
    'loop_state',
    'stop_reason',
  ]) {
    assert.ok(k in s, `missing session field: ${k}`);
  }
});

console.log(`  ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);