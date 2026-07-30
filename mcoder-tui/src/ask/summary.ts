// 共享纯逻辑：Ask 答案的只读摘要渲染（用于回答后原位显示）
// 三端共用；纯函数，便于在 TUI/Desktop/Mobile 中复用

import { AskRequest, AskSubmission, AskQuestion, AskQuestionAnswer } from './types.js';

function isMulti(q: AskQuestion): boolean {
  return !!q.multi_select;
}

function formatOneAnswer(q: AskQuestion, a: AskQuestionAnswer): string {
  const kind = (a as { kind?: string }).kind;
  if (kind === 'skipped') return '(skipped)';
  if (kind === 'custom') return `note: ${(a as { note: string }).note}`;
  // 兼容旧版 untagged
  if (isMulti(q) || kind === 'multi') {
    const opts = (a as { options?: string[] }).options || [];
    return opts.join(', ');
  }
  const opt = (a as { option?: string }).option || '';
  return opt;
}

/** 拼接所有问答的"已回答"摘要。
 *  cancelled=true → 返回 "cancelled"
 *  否则按 question 顺序逐行渲染 */
export function formatAskSummary(req: AskRequest, sub: AskSubmission): string {
  if (sub.cancelled) return 'cancelled';
  const lines: string[] = [];
  for (let i = 0; i < req.questions.length; i++) {
    const q = req.questions[i];
    const a = sub.answers[i];
    if (!a) {
      lines.push(`Q${i + 1}. (no answer)`);
      continue;
    }
    const head = q.header || `Q${i + 1}`;
    lines.push(`${head}: ${formatOneAnswer(q, a)}`);
    const kind = (a as { kind?: string }).kind;
    if (kind !== 'custom') {
      const note = (a as { note?: string }).note;
      if (note && note.trim().length > 0) {
        lines.push(`  note: ${note.trim()}`);
      }
    }
  }
  // 顶层 custom_response（多题整段答复）
  if (sub.custom_response && sub.custom_response.trim().length > 0) {
    lines.push(`custom: ${sub.custom_response.trim()}`);
  }
  return lines.join('\n');
}

/** 完整摘要：问题文本 + 答案，客户端卡片用 */
export function formatAskFullSummary(req: AskRequest, sub: AskSubmission): string {
  if (sub.cancelled) {
    return 'Ask cancelled by user';
  }
  const lines: string[] = [];
  for (let i = 0; i < req.questions.length; i++) {
    const q = req.questions[i];
    const a = sub.answers[i];
    lines.push(`Q${i + 1}. ${q.question}`);
    if (!a) {
      lines.push('   → (no answer)');
      continue;
    }
    lines.push(`   → ${formatOneAnswer(q, a)}`);
    const kind = (a as { kind?: string }).kind;
    if (kind !== 'custom') {
      const note = (a as { note?: string }).note;
      if (note && note.trim().length > 0) {
        lines.push(`   note: ${note.trim()}`);
      }
    }
  }
  if (sub.custom_response && sub.custom_response.trim().length > 0) {
    lines.push(`custom: ${sub.custom_response.trim()}`);
  }
  return lines.join('\n');
}