// Phase 5c: TS 端 resume state 5 参数 + interrupted policy
// 运行：node --import tsx test-phase5c-resume.ts
//
// 覆盖：
// - has_interrupted_tasks=true → auto_resume
// - 既有 stop_reason ∈ RESUME_REASONS → auto_resume
// - has_interrupted_tasks + stop_reason 共存 → auto_resume
// - 与 running / waiting_for_user 优先级一致

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

console.log('=== Phase 5c: has_interrupted_tasks ===');

test('interrupted tasks alone triggers auto_resume (no unfinished, no stop_reason)', () => {
  eq(
    computeResumeEntry({
      loop_state: 'stopped',
      stop_reason: null,
      has_unfinished_todo: false,
      has_interrupted_tasks: true,
    }).kind,
    'auto_resume',
    'has_interrupted_tasks alone must trigger auto_resume',
  );
});

test('interrupted tasks + stop_reason=interrupted_tasks → auto_resume', () => {
  eq(
    computeResumeEntry({
      loop_state: 'stopped',
      stop_reason: 'interrupted_tasks',
      has_unfinished_todo: false,
      has_interrupted_tasks: true,
    }).kind,
    'auto_resume',
  );
});

test('interrupted tasks + completed (no work) → auto_resume (Phase 5b)', () => {
  // 即使 loop_state=completed, 只要有 interrupted 也应 auto_resume
  eq(
    computeResumeEntry({
      loop_state: 'completed',
      stop_reason: 'completed',
      has_unfinished_todo: false,
      has_interrupted_tasks: true,
    }).kind,
    'auto_resume',
  );
});

test('no interrupted + no unfinished + no reason → requires_input', () => {
  eq(
    computeResumeEntry({
      loop_state: 'completed',
      has_unfinished_todo: false,
      has_interrupted_tasks: false,
    }).kind,
    'requires_input',
  );
});

test('running always wins over interrupted (no entry)', () => {
  eq(
    computeResumeEntry({
      loop_state: 'running',
      has_unfinished_todo: true,
      has_interrupted_tasks: true,
    }).kind,
    'none',
    'running must suppress resume even with interrupted',
  );
});

test('waiting_for_user with interrupted → waiting_user (not auto_resume)', () => {
  eq(
    computeResumeEntry({
      loop_state: 'waiting_for_user',
      has_unfinished_todo: true,
      has_interrupted_tasks: true,
    }).kind,
    'waiting_user',
    'waiting_for_user must not auto-resume even with interrupted',
  );
});

test('default has_interrupted_tasks (undefined) treated as false', () => {
  // 兼容性：旧调用方不传 has_interrupted_tasks → 行为同 false
  eq(
    computeResumeEntry({
      loop_state: 'stopped',
      stop_reason: 'cancelled',
      has_unfinished_todo: false,
    }).kind,
    'auto_resume',
    'cancelled without interrupted still triggers auto_resume',
  );
});

test('cancelled + interrupted tasks → auto_resume with interrupted reason', () => {
  const e = computeResumeEntry({
    loop_state: 'stopped',
    stop_reason: 'cancelled',
    has_unfinished_todo: false,
    has_interrupted_tasks: true,
  });
  eq(e.kind, 'auto_resume', 'must be auto_resume');
  assert.ok(
    e.reason.includes('interrupted') || e.reason.includes('cancelled'),
    `reason must mention interrupted or cancelled, got: ${e.reason}`,
  );
});

console.log(`  ${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
