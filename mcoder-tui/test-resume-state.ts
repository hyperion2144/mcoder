// Phase 3: 共享 Resume 决策纯函数单元测试
// 运行：node --import tsx test-resume-state.ts
//
// 覆盖：
// - 矩阵：loop_state / stop_reason / has_unfinished / loop_running 组合
// - 至少覆盖任务要求的：
//   · 未完成 todo 启动（auto_resume）
//   · 无工作不启动（requires_input）
//   · running 状态不显示（none）
//   · waiting_for_user 显示（waiting_user，不抢答）
//   · blocked / cancelled / failed 也触发 auto_resume（即使无 todo）
// - 边界：completed + has_unfinished → auto_resume（防止遗漏）

import {
  computeResumeEntry,
  hasResumeEntry,
  type ResumeKind,
} from './src/resume/state.ts';
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

const eq = (actual: ResumeKind, expected: ResumeKind, msg: string) => {
  assert.equal(actual, expected, msg);
};

console.log('=== computeResumeEntry ===');

test('unfinished todos trigger auto_resume (stopped + cancelled)', () => {
  eq(
    computeResumeEntry({
      loop_state: 'stopped',
      stop_reason: 'cancelled',
      has_unfinished_todo: true,
    }).kind,
    'auto_resume',
    'must auto_resume when there are unfinished todos',
  );
});

test('unfinished todos trigger auto_resume (completed + no reason)', () => {
  // 即便 loop_state=completed，has_unfinished 必须触发 auto_resume
  eq(
    computeResumeEntry({
      loop_state: 'completed',
      stop_reason: null,
      has_unfinished_todo: true,
    }).kind,
    'auto_resume',
    'completed + unfinished must still auto_resume (avoid missing work)',
  );
});

test('cancelled stop_reason with no unfinished still auto_resume', () => {
  eq(
    computeResumeEntry({
      loop_state: 'stopped',
      stop_reason: 'cancelled',
      has_unfinished_todo: false,
    }).kind,
    'auto_resume',
    'cancelled without unfinished still allows resume',
  );
});

test('failed stop_reason with no unfinished still auto_resume', () => {
  eq(
    computeResumeEntry({
      loop_state: 'stopped',
      stop_reason: 'failed',
      has_unfinished_todo: false,
    }).kind,
    'auto_resume',
    'failed without unfinished still allows resume',
  );
});

test('blocked stop_reason with no unfinished still auto_resume', () => {
  eq(
    computeResumeEntry({
      loop_state: 'stopped',
      stop_reason: 'blocked',
      has_unfinished_todo: false,
    }).kind,
    'auto_resume',
    'blocked without unfinished still allows resume',
  );
});

test('completed with no unfinished → requires_input (no work)', () => {
  eq(
    computeResumeEntry({
      loop_state: 'completed',
      stop_reason: null,
      has_unfinished_todo: false,
    }).kind,
    'requires_input',
    'clean completed → requires user input',
  );
});

test('idle with no unfinished → requires_input', () => {
  eq(
    computeResumeEntry({
      loop_state: 'idle',
      has_unfinished_todo: false,
    }).kind,
    'requires_input',
  );
});

test('stopped without reason + no todo → requires_input', () => {
  eq(
    computeResumeEntry({
      loop_state: 'stopped',
      stop_reason: null,
      has_unfinished_todo: false,
    }).kind,
    'requires_input',
  );
});

test('running loop_state → none (no resume entry)', () => {
  eq(
    computeResumeEntry({
      loop_state: 'running',
      has_unfinished_todo: true,
    }).kind,
    'none',
    'running must not show resume',
  );
});

test('running loopRunning flag → none', () => {
  eq(
    computeResumeEntry({
      loop_state: 'stopped',
      stop_reason: 'cancelled',
      has_unfinished_todo: true,
      loop_running: true,
    }).kind,
    'none',
    'in-memory loop_running overrides snapshot',
  );
});

test('waiting_for_user → waiting_user (do NOT auto-answer)', () => {
  eq(
    computeResumeEntry({
      loop_state: 'waiting_for_user',
      has_unfinished_todo: true,
    }).kind,
    'waiting_user',
    'waiting_for_user must surface as waiting_user, not auto_resume',
  );
});

test('waiting_for_user + no unfinished → waiting_user', () => {
  eq(
    computeResumeEntry({
      loop_state: 'waiting_for_user',
      has_unfinished_todo: false,
    }).kind,
    'waiting_user',
  );
});

test('max_iters_reached + no unfinished → requires_input (not auto-resume)', () => {
  eq(
    computeResumeEntry({
      loop_state: 'stopped',
      stop_reason: 'max_iters_reached',
      has_unfinished_todo: false,
    }).kind,
    'requires_input',
    'max_iters_reached is terminal — not in RESUME_REASONS',
  );
});

test('loop_condition_met + no unfinished → requires_input', () => {
  eq(
    computeResumeEntry({
      loop_state: 'stopped',
      stop_reason: 'loop_condition_met',
      has_unfinished_todo: false,
    }).kind,
    'requires_input',
  );
});

console.log('=== hasResumeEntry ===');

test('hasResumeEntry false for none', () => {
  assert.equal(
    hasResumeEntry({ kind: 'none', reason: 'x' }),
    false,
  );
});

test('hasResumeEntry true for requires_input', () => {
  assert.equal(
    hasResumeEntry({ kind: 'requires_input', reason: 'x' }),
    true,
  );
});

test('hasResumeEntry true for auto_resume', () => {
  assert.equal(
    hasResumeEntry({ kind: 'auto_resume', reason: 'x' }),
    true,
  );
});

test('hasResumeEntry true for waiting_user', () => {
  assert.equal(
    hasResumeEntry({ kind: 'waiting_user', reason: 'x' }),
    true,
  );
});

console.log(`  ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);