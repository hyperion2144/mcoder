// 共享 Todo 摘要选择函数单元测试（不依赖 ink/react）
// 运行：node --import tsx test-todo-summary.mjs
// 验证 selectTodoSummary / filterUnfinished 在各平台下的行为

import {
  selectTodoSummary,
  filterUnfinished,
  formatRemaining,
  PLATFORM_DESKTOP,
  PLATFORM_MOBILE,
  PLATFORM_TUI,
} from './src/todo/summary.ts';
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

function mk(overrides) {
  return {
    id: 'x',
    session_id: 's',
    content: 'c',
    status: 'pending',
    priority: 'medium',
    order: 0,
    created_at: '',
    updated_at: '',
    ...overrides,
  };
}

console.log('=== filterUnfinished ===');

test('keeps only pending and in_progress', () => {
  const items = [
    mk({ id: '1', status: 'pending' }),
    mk({ id: '2', status: 'in_progress' }),
    mk({ id: '3', status: 'completed' }),
    mk({ id: '4', status: 'cancelled' }),
  ];
  const r = filterUnfinished(items);
  assert.equal(r.length, 2);
  assert.deepEqual(r.map((x) => x.id), ['1', '2']);
});

console.log('=== selectTodoSummary — empty / hidden ===');

test('returns null when no unfinished todos', () => {
  const items = [
    mk({ id: '1', status: 'completed' }),
    mk({ id: '2', status: 'cancelled' }),
  ];
  assert.equal(selectTodoSummary(items), null);
});

test('returns null on empty input', () => {
  assert.equal(selectTodoSummary([], PLATFORM_DESKTOP), null);
});

console.log('=== selectTodoSummary — TUI/Desktop (max 3) ===');

test('TUI/Desktop: shows all 3 when exactly 3 unfinished', () => {
  const items = [
    mk({ id: '1', content: 'a' }),
    mk({ id: '2', content: 'b' }),
    mk({ id: '3', content: 'c' }),
  ];
  const r = selectTodoSummary(items, PLATFORM_TUI);
  assert.equal(r.visible.length, 3);
  assert.equal(r.remaining, 0);
  assert.equal(r.totalUnfinished, 3);
});

test('TUI/Desktop: truncates to 3 with remaining when 5 unfinished', () => {
  const items = [
    mk({ id: '1', content: 'a' }),
    mk({ id: '2', content: 'b' }),
    mk({ id: '3', content: 'c' }),
    mk({ id: '4', content: 'd' }),
    mk({ id: '5', content: 'e' }),
  ];
  const r = selectTodoSummary(items, PLATFORM_TUI);
  assert.equal(r.visible.length, 3);
  assert.equal(r.remaining, 2);
  assert.equal(r.totalUnfinished, 5);
  assert.deepEqual(r.visible.map((x) => x.id), ['1', '2', '3']);
});

test('TUI/Desktop: shows 1 todo when only 1 unfinished', () => {
  const items = [mk({ id: '1', content: 'only' })];
  const r = selectTodoSummary(items, PLATFORM_DESKTOP);
  assert.equal(r.visible.length, 1);
  assert.equal(r.remaining, 0);
  assert.equal(r.totalUnfinished, 1);
});

console.log('=== selectTodoSummary — Mobile (collapsed/expanded) ===');

test('Mobile collapsed: shows 1 + remaining when 4 unfinished', () => {
  const items = [
    mk({ id: '1', content: 'a' }),
    mk({ id: '2', content: 'b' }),
    mk({ id: '3', content: 'c' }),
    mk({ id: '4', content: 'd' }),
  ];
  const r = selectTodoSummary(items, PLATFORM_MOBILE, false);
  assert.equal(r.visible.length, 1);
  assert.equal(r.remaining, 3);
});

test('Mobile expanded: shows up to 3', () => {
  const items = [
    mk({ id: '1', content: 'a' }),
    mk({ id: '2', content: 'b' }),
    mk({ id: '3', content: 'c' }),
    mk({ id: '4', content: 'd' }),
  ];
  const r = selectTodoSummary(items, PLATFORM_MOBILE, true);
  assert.equal(r.visible.length, 3);
  assert.equal(r.remaining, 1);
});

test('Mobile expanded: shows all when 2 unfinished', () => {
  const items = [
    mk({ id: '1', content: 'a' }),
    mk({ id: '2', content: 'b' }),
  ];
  const r = selectTodoSummary(items, PLATFORM_MOBILE, true);
  assert.equal(r.visible.length, 2);
  assert.equal(r.remaining, 0);
});

console.log('=== formatRemaining ===');

test('formats remaining count', () => {
  const items = Array.from({ length: 5 }, (_, i) => mk({ id: String(i), content: 'x' }));
  const r = selectTodoSummary(items, PLATFORM_TUI);
  assert.equal(formatRemaining(r), '+2 more');
});

test('returns empty when nothing remaining', () => {
  const items = [mk({ id: '1' })];
  const r = selectTodoSummary(items, PLATFORM_TUI);
  assert.equal(formatRemaining(r), '');
});

console.log('=== ordering preserved (server already sorts) ===');

test('preserves server-provided order (in_progress first, then pending)', () => {
  // 模拟服务端排序后的输入
  const items = [
    mk({ id: '1', status: 'in_progress', priority: 'high', content: 'working' }),
    mk({ id: '2', status: 'pending', priority: 'high', content: 'next' }),
    mk({ id: '3', status: 'pending', priority: 'low', content: 'later' }),
    mk({ id: '4', status: 'completed', content: 'done' }), // 应被过滤
  ];
  const r = selectTodoSummary(items, PLATFORM_TUI);
  assert.equal(r.visible.length, 3);
  assert.deepEqual(r.visible.map((x) => x.id), ['1', '2', '3']);
  assert.equal(r.totalUnfinished, 3);
});

console.log('=== result ===');
console.log(`  ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);