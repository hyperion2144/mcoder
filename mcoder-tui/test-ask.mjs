// 共享纯逻辑单元测试（不依赖 ink/react 等渲染层）
// 运行：pnpm test:ask  或  node --import tsx test-ask.mjs
// 验证 schema 校验 / answer 校验 / summary 格式化
// 必须与服务端 mcoder/src/ask_user.rs 行为保持一致
//
// 覆盖 review：tool_call_id/custom_response/首答竞态/客户端 store 渲染

import { validateAskRequest, validateAskSubmission, serializeSubmission } from './src/ask/validation.ts';
import { formatAskSummary, formatAskFullSummary } from './src/ask/summary.ts';
import { hasToolUse } from './src/ask/messages.ts';
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
    fail++;
  }
}

console.log('=== validateAskRequest ===');

test('rejects non-object', () => {
  const r = validateAskRequest('nope');
  assert.equal(r.valid, false);
  assert.ok(r.errors.some((e) => e.includes('must be an object')));
});

test('rejects missing questions', () => {
  const r = validateAskRequest({});
  assert.equal(r.valid, false);
  assert.ok(r.errors.some((e) => e.includes('questions must be an array')));
});

test('rejects too few questions (0)', () => {
  const r = validateAskRequest({ questions: [] });
  assert.equal(r.valid, false);
  assert.ok(r.errors[0].includes('1-4'));
});

test('rejects too many questions (5)', () => {
  const qs = Array.from({ length: 5 }, (_, i) => ({
    question: `Q${i}`,
    options: [{ label: 'A' }, { label: 'B' }],
  }));
  const r = validateAskRequest({ questions: qs });
  assert.equal(r.valid, false);
});

test('rejects too few options (1)', () => {
  const r = validateAskRequest({ questions: [{ question: 'q', options: [{ label: 'A' }] }] });
  assert.equal(r.valid, false);
  assert.ok(r.errors[0].includes('2-4'));
});

test('rejects too many options (5)', () => {
  const opts = Array.from({ length: 5 }, (_, i) => ({ label: `O${i}` }));
  const r = validateAskRequest({ questions: [{ question: 'q', options: opts }] });
  assert.equal(r.valid, false);
});

test('rejects empty question text', () => {
  const r = validateAskRequest({ questions: [{ question: '   ', options: [{ label: 'A' }, { label: 'B' }] }] });
  assert.equal(r.valid, false);
});

test('rejects empty label', () => {
  const r = validateAskRequest({ questions: [{ question: 'q', options: [{ label: 'A' }, { label: ' ' }] }] });
  assert.equal(r.valid, false);
});

test('rejects duplicate labels', () => {
  const r = validateAskRequest({ questions: [{ question: 'q', options: [{ label: 'A' }, { label: 'A' }] }] });
  assert.equal(r.valid, false);
  assert.ok(r.errors[0].includes('duplicate'));
});

test('accepts valid 4q 4o', () => {
  const qs = Array.from({ length: 4 }, (_, i) => ({
    question: `Q${i}`,
    header: `H${i}`,
    options: Array.from({ length: 4 }, (_, j) => ({ label: `O${i}-${j}` })),
    multi_select: i % 2 === 0,
  }));
  const r = validateAskRequest({ questions: qs });
  assert.equal(r.valid, true);
  assert.equal(r.value.questions.length, 4);
  for (let i = 0; i < 4; i++) {
    assert.equal(r.value.questions[i].options.length, 4);
    assert.equal(r.value.questions[i].multi_select, i % 2 === 0);
  }
});

console.log('\n=== validateAskSubmission ===');

const req1 = { questions: [{ question: 'q1', options: [{ label: 'A' }, { label: 'B' }] }] };
const req2 = {
  questions: [
    { question: 'q1', options: [{ label: 'A' }, { label: 'B' }] },
    { question: 'q2', options: [{ label: 'X' }, { label: 'Y' }], multi_select: true },
  ],
};

test('cancelled is always valid', () => {
  const r = validateAskSubmission(req1, { cancelled: true, answers: {} });
  assert.equal(r.valid, true);
});

test('rejects unknown option', () => {
  const r = validateAskSubmission(req1, { cancelled: false, answers: { 0: { option: 'C' } } });
  assert.equal(r.valid, false);
  assert.ok(r.errors[0].includes('unknown option'));
});

test('rejects missing answer (no custom_response)', () => {
  const r = validateAskSubmission(req2, { cancelled: false, answers: { 0: { option: 'A' } } });
  assert.equal(r.valid, false);
  assert.ok(r.errors[0].includes('missing answer'));
});

test('rejects empty multi', () => {
  const r = validateAskSubmission(req2, {
    cancelled: false,
    answers: { 0: { option: 'A' }, 1: { options: [] } },
  });
  assert.equal(r.valid, false);
  assert.ok(r.errors[0].includes('multi-select requires non-empty'));
});

test('accepts full answers (single + multi)', () => {
  const r = validateAskSubmission(req2, {
    cancelled: false,
    answers: {
      0: { option: 'B', note: 'ok' },
      1: { options: ['X'] },
    },
  });
  assert.equal(r.valid, true);
});

// ==================== review 新增测试 ====================

test('issue 3: single Custom(note) + custom_response is accepted', () => {
  const r = validateAskSubmission(req1, {
    cancelled: false,
    answers: { 0: { kind: 'custom', note: '整段答复' } },
    custom_response: '整段答复',
  });
  assert.equal(r.valid, true, JSON.stringify(r.errors));
});

test('issue 3: multi-question all Custom + custom_response is accepted', () => {
  const r = validateAskSubmission(req2, {
    cancelled: false,
    answers: {
      0: { kind: 'custom', note: 'free' },
      1: { kind: 'custom', note: 'free' },
    },
    custom_response: 'free',
  });
  assert.equal(r.valid, true, JSON.stringify(r.errors));
});

test('issue 3: only custom_response (empty answers) is accepted', () => {
  const r = validateAskSubmission(req2, {
    cancelled: false,
    answers: {},
    custom_response: '整段答复',
  });
  assert.equal(r.valid, true, JSON.stringify(r.errors));
});

test('issue 3: completely empty submission is rejected', () => {
  const r = validateAskSubmission(req1, {
    cancelled: false,
    answers: {},
  });
  assert.equal(r.valid, false);
});

test('issue 3: Skipped + custom_response is accepted', () => {
  const r = validateAskSubmission(req2, {
    cancelled: false,
    answers: {
      0: { kind: 'skipped' },
      1: { kind: 'custom', note: 'only second' },
    },
    custom_response: 'only second',
  });
  assert.equal(r.valid, true, JSON.stringify(r.errors));
});

test('issue 3: Custom answer without note is rejected', () => {
  const r = validateAskSubmission(req1, {
    cancelled: false,
    answers: { 0: { kind: 'custom' } },
  });
  assert.equal(r.valid, false);
  assert.ok(r.errors[0].includes('note'));
});

console.log('\n=== mode mismatch (issue 4) ===');

test('single question with multi answer is rejected (mode mismatch)', () => {
  const r = validateAskSubmission(req1, {
    cancelled: false,
    answers: { 0: { kind: 'multi', options: ['A'] } },
  });
  assert.equal(r.valid, false);
  assert.ok(
    r.errors.some((e) => e.includes('mode')),
    `errors must mention mode, got: ${JSON.stringify(r.errors)}`,
  );
});

test('multi question with single answer is rejected (mode mismatch)', () => {
  const multiReq = {
    questions: [
      { question: 'q', options: [{ label: 'A' }, { label: 'B' }], multi_select: true },
    ],
  };
  const r = validateAskSubmission(multiReq, {
    cancelled: false,
    answers: { 0: { kind: 'single', option: 'A' } },
  });
  assert.equal(r.valid, false);
  assert.ok(
    r.errors.some((e) => e.includes('mode')),
    `errors must mention mode, got: ${JSON.stringify(r.errors)}`,
  );
});

test('serializeSubmission tags single/multi/custom/skipped correctly', () => {
  const sub = serializeSubmission({
    cancelled: false,
    answers: {
      0: { option: 'A', note: 'note' },
      1: { options: ['X', 'Y'] },
      2: { kind: 'custom', note: 'free' },
      3: { kind: 'skipped' },
    },
    custom_response: 'multi-note',
  });
  assert.equal(sub.answers[0].kind, 'single');
  assert.equal(sub.answers[0].option, 'A');
  assert.equal(sub.answers[1].kind, 'multi');
  assert.deepEqual(sub.answers[1].options, ['X', 'Y']);
  assert.equal(sub.answers[2].kind, 'custom');
  assert.equal(sub.answers[2].note, 'free');
  assert.equal(sub.answers[3].kind, 'skipped');
  assert.equal(sub.custom_response, 'multi-note');
});

test('serializeSubmission drops undefined custom_response', () => {
  const sub = serializeSubmission({
    cancelled: false,
    answers: { 0: { option: 'A' } },
  });
  assert.equal(sub.custom_response, undefined);
});

console.log('\n=== formatAskSummary / formatAskFullSummary ===');

test('summary returns "cancelled" for cancelled submission', () => {
  const s = formatAskSummary(req1, { cancelled: true, answers: {} });
  assert.equal(s, 'cancelled');
});

test('summary shows option labels', () => {
  const s = formatAskSummary(req1, { cancelled: false, answers: { 0: { option: 'B' } } });
  assert.ok(s.includes('B'));
  assert.ok(!s.includes('A'));
});

test('summary shows multi options joined', () => {
  const r = { questions: [{ question: 'q', options: [{ label: 'A' }, { label: 'B' }], multi_select: true }] };
  const s = formatAskSummary(r, { cancelled: false, answers: { 0: { options: ['A', 'B'] } } });
  assert.ok(s.includes('A, B'));
});

test('summary shows note on its own line', () => {
  const s = formatAskSummary(req1, { cancelled: false, answers: { 0: { option: 'A', note: 'because' } } });
  assert.ok(s.includes('note: because'));
});

test('full summary shows question + answer', () => {
  const s = formatAskFullSummary(req1, { cancelled: false, answers: { 0: { option: 'B' } } });
  assert.ok(s.includes('Q1.'));
  assert.ok(s.includes('q1'));
  assert.ok(s.includes('B'));
});

test('full summary for cancelled', () => {
  const s = formatAskFullSummary(req1, { cancelled: true, answers: {} });
  assert.equal(s, 'Ask cancelled by user');
});

test('summary handles Custom(note) answer', () => {
  const s = formatAskSummary(req1, {
    cancelled: false,
    answers: { 0: { kind: 'custom', note: 'free' } },
  });
  assert.ok(s.includes('free'));
});

test('summary handles Skipped answer', () => {
  const s = formatAskSummary(req1, {
    cancelled: false,
    answers: { 0: { kind: 'skipped' } },
  });
  assert.ok(s.includes('(skipped)'));
});

test('summary includes top-level custom_response', () => {
  const s = formatAskSummary(req2, {
    cancelled: false,
    answers: {},
    custom_response: 'multi free',
  });
  assert.ok(s.includes('custom: multi free'));
});

console.log('\n=== hasToolUse (issue 6) ===');

test('hasToolUse returns false for null/empty messages', () => {
  assert.equal(hasToolUse(null, 'tc1'), false);
  assert.equal(hasToolUse(undefined, 'tc1'), false);
  assert.equal(hasToolUse([], 'tc1'), false);
});

test('hasToolUse returns false when tool_call_id is empty', () => {
  const messages = [
    {
      role: 'assistant',
      content: [{ type: 'tool_use', id: 'tc1', name: 'ask_user', args: {} }],
    },
  ];
  assert.equal(hasToolUse(messages, null), false);
  assert.equal(hasToolUse(messages, ''), false);
});

test('hasToolUse returns true when tool_use block exists for tool_call_id', () => {
  const messages = [
    {
      role: 'assistant',
      content: [{ type: 'tool_use', id: 'tc1', name: 'ask_user', args: {} }],
    },
    { role: 'user', content: [{ type: 'text', text: 'reply' }] },
  ];
  assert.equal(hasToolUse(messages, 'tc1'), true);
});

test('hasToolUse returns false when only a different tool_call_id exists', () => {
  const messages = [
    {
      role: 'assistant',
      content: [{ type: 'tool_use', id: 'tc2', name: 'ask_user', args: {} }],
    },
  ];
  assert.equal(hasToolUse(messages, 'tc1'), false);
});

test('hasToolUse ignores non tool_use blocks (text/tool_result)', () => {
  const messages = [
    {
      role: 'tool',
      content: [{ type: 'tool_result', id: 'tc1', output: 'x' }],
    },
  ];
  assert.equal(hasToolUse(messages, 'tc1'), false);
});

console.log('\n=== ask store: multi-terminal by tool_call_id (issue 7) ===');

// 动态 import store 是因为 store 用了 zustand，需保持单例
const { useAskStore } = await import('./src/ask/store.ts');

function resetAskStore() {
  const st = useAskStore.getState();
  // 重置所有 sessions
  const sids = new Set([
    ...Object.keys(st.pending),
    ...Object.keys(st.lastSubmission),
    ...Object.keys(st.submissions),
    ...Object.keys(st.draftSelections),
    ...Object.keys(st.draftNotes),
    ...Object.keys(st.draftFocus),
    ...Object.keys(st.askInputMode),
  ]);
  for (const sid of sids) st.resetSession(sid);
}

test('store tracks multiple historical terminal asks by tool_call_id', () => {
  resetAskStore();
  const store = useAskStore.getState();
  const sid = 'sess-multi';
  // 第一个 ask 已答完
  store.setSubmission(sid, 'ask-1', 'tc-1', { cancelled: false, answers: { 0: { option: 'A' } } });
  // 第二个 ask 也答完
  store.setSubmission(sid, 'ask-2', 'tc-2', { cancelled: false, answers: { 0: { option: 'B' } } });
  const st = useAskStore.getState();
  assert.ok(st.submissions[sid], 'submissions map should exist for session');
  assert.ok(st.submissions[sid]['tc-1'], 'tc-1 entry should exist');
  assert.ok(st.submissions[sid]['tc-2'], 'tc-2 entry should exist');
  assert.equal(st.submissions[sid]['tc-1'].ask_id, 'ask-1');
  assert.equal(st.submissions[sid]['tc-2'].ask_id, 'ask-2');
  // getSubmissionByToolCallId 必须能各自查到
  const tc1 = store.getSubmissionByToolCallId(sid, 'tc-1');
  const tc2 = store.getSubmissionByToolCallId(sid, 'tc-2');
  assert.ok(tc1);
  assert.ok(tc2);
  assert.equal(tc1.ask_id, 'ask-1');
  assert.equal(tc2.ask_id, 'ask-2');
});

test('store lastSubmission stays compatible (most recent)', () => {
  resetAskStore();
  const store = useAskStore.getState();
  const sid = 'sess-last';
  store.setSubmission(sid, 'ask-1', 'tc-1', { cancelled: false, answers: { 0: { option: 'A' } } });
  store.setSubmission(sid, 'ask-2', 'tc-2', { cancelled: false, answers: { 0: { option: 'B' } } });
  const st = useAskStore.getState();
  // lastSubmission 仍是单值（兼容 AskCardSummary 等旧消费者）
  assert.ok(st.lastSubmission[sid]);
  assert.equal(st.lastSubmission[sid].ask_id, 'ask-2');
});

console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);