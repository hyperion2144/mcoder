// 共享纯逻辑：Ask 工具入参校验 + 答案校验
// 服务端 Rust 实现与本文件必须保持行为一致（见 mcoder/src/ask_user.rs::validate_request/validate_submission）

import {
  AskQuestion,
  AskQuestionAnswer,
  AskRequest,
  AskSubmission,
  AskValidationResult,
  ASK_MAX_QUESTIONS,
  ASK_MAX_OPTIONS,
  ASK_MIN_QUESTIONS,
  ASK_MIN_OPTIONS,
} from './types.js';

/** 校验 LLM 传入的原始 args（来自 tool_call.args） */
export function validateAskRequest(raw: unknown): AskValidationResult {
  const errors: string[] = [];
  if (raw === null || typeof raw !== 'object') {
    return { valid: false, errors: ['ask_user args must be an object'] };
  }
  const obj = raw as Record<string, unknown>;
  const questionsRaw = obj.questions;
  if (!Array.isArray(questionsRaw)) {
    return { valid: false, errors: ['ask_user.questions must be an array'] };
  }
  if (questionsRaw.length < ASK_MIN_QUESTIONS || questionsRaw.length > ASK_MAX_QUESTIONS) {
    errors.push(
      `ask_user.questions length must be ${ASK_MIN_QUESTIONS}-${ASK_MAX_QUESTIONS}, got ${questionsRaw.length}`,
    );
  }
  const questions: AskQuestion[] = [];
  for (let i = 0; i < questionsRaw.length; i++) {
    const q = questionsRaw[i] as Record<string, unknown> | null;
    if (q === null || typeof q !== 'object') {
      errors.push(`questions[${i}] must be an object`);
      continue;
    }
    const questionText = q.question;
    if (typeof questionText !== 'string' || questionText.trim().length === 0) {
      errors.push(`questions[${i}].question must be a non-empty string`);
    }
    const optionsRaw = q.options;
    if (!Array.isArray(optionsRaw)) {
      errors.push(`questions[${i}].options must be an array`);
      continue;
    }
    if (optionsRaw.length < ASK_MIN_OPTIONS || optionsRaw.length > ASK_MAX_OPTIONS) {
      errors.push(
        `questions[${i}].options length must be ${ASK_MIN_OPTIONS}-${ASK_MAX_OPTIONS}, got ${optionsRaw.length}`,
      );
    }
    const options: { label: string; description?: string }[] = [];
    const seenLabels = new Set<string>();
    for (let j = 0; j < optionsRaw.length; j++) {
      const opt = optionsRaw[j] as Record<string, unknown> | null;
      if (opt === null || typeof opt !== 'object') {
        errors.push(`questions[${i}].options[${j}] must be an object`);
        continue;
      }
      const label = opt.label;
      if (typeof label !== 'string' || label.trim().length === 0) {
        errors.push(`questions[${i}].options[${j}].label must be a non-empty string`);
        continue;
      }
      if (seenLabels.has(label)) {
        errors.push(`questions[${i}].options[${j}].label duplicate: "${label}"`);
      }
      seenLabels.add(label);
      const desc = opt.description;
      const normalized: { label: string; description?: string } = { label };
      if (typeof desc === 'string' && desc.length > 0) {
        normalized.description = desc;
      }
      options.push(normalized);
    }
    const multi = q.multi_select;
    const question: AskQuestion = {
      question: typeof questionText === 'string' ? questionText : '',
      options,
    };
    if (typeof multi === 'boolean') question.multi_select = multi;
    const header = q.header;
    if (typeof header === 'string' && header.length > 0) question.header = header;
    questions.push(question);
  }

  if (errors.length > 0) return { valid: false, errors };
  return { valid: true, errors: [], value: { questions } };
}

/** 校验用户提交的答案：必须对应 req 中的每个问题 */
export function validateAskSubmission(
  req: AskRequest,
  submission: AskSubmission,
): { valid: boolean; errors: string[] } {
  const errors: string[] = [];
  if (submission.cancelled) return { valid: true, errors: [] };
  // 顶层 custom_response 一旦非空，所有题均可"未填"（整段答复覆盖整个 Ask）
  const hasCustomResp =
    typeof submission.custom_response === 'string' &&
    submission.custom_response.trim().length > 0;

  let structuralPresent = 0;
  for (let i = 0; i < req.questions.length; i++) {
    const q = req.questions[i];
    const a = submission.answers[i];
    if (!a) {
      if (!hasCustomResp) errors.push(`missing answer for question ${i}`);
      continue;
    }
    const labels = new Set(q.options.map((o) => o.label));
    const kind = (a as { kind?: string }).kind;
    // 兼容：旧版 untagged 数据没有 kind 字段，靠 options/option 字段识别
    const isMulti = !!q.multi_select;
    if (kind === 'custom') {
      const note = (a as { note?: string }).note;
      if (typeof note !== 'string') {
        errors.push(`question ${i}: custom answer requires note`);
      }
      continue;
    }
    if (kind === 'skipped') continue;
    if (kind === 'multi' || (!kind && isMulti)) {
      // 模式校验（issue 4）：single 题收到 multi 答案 → 拒绝
      if (!isMulti) {
        errors.push(`question ${i}: mode mismatch — question is single-select but answer is multi-select`);
        continue;
      }
      const arr = (a as { options?: string[] }).options;
      if (!Array.isArray(arr) || arr.length === 0) {
        errors.push(`question ${i}: multi-select requires non-empty options[]`);
        continue;
      }
      for (const opt of arr) {
        if (!labels.has(opt)) errors.push(`question ${i}: unknown option "${opt}"`);
      }
      structuralPresent += 1;
      continue;
    }
    // single（默认）
    // 模式校验（issue 4）：multi 题收到 single 答案 → 拒绝
    if (isMulti) {
      errors.push(`question ${i}: mode mismatch — question is multi-select but answer is single-select`);
      continue;
    }
    const opt = (a as { option?: string }).option;
    if (typeof opt !== 'string' || opt.length === 0) {
      errors.push(`question ${i}: single-select requires non-empty option`);
      continue;
    }
    if (!labels.has(opt)) errors.push(`question ${i}: unknown option "${opt}"`);
    structuralPresent += 1;
  }
  // 兜底：所有题既无结构化答复也无 Custom/Skipped → 必须有 custom_response
  const allSkippedOrEmpty =
    structuralPresent === 0 &&
    Object.values(submission.answers).every(
      (a) => (a as { kind?: string }).kind === 'skipped' || (a as { kind?: string }).kind === 'custom',
    );
  if (
    req.questions.length > 0 &&
    allSkippedOrEmpty &&
    !hasCustomResp &&
    Object.keys(submission.answers).length === 0
  ) {
    errors.push(
      'submission must contain at least one structural answer, a per-question Custom, or a top-level custom_response',
    );
  }
  return { valid: errors.length === 0, errors };
}

/** 判断用户的自由文本是否构成"提交 Ask 答案"
 *  当一个 session 存在 pending Ask 时，文本输入应优先作为该 Ask 的 note 提交。
 *  此函数用于客户端判定：non-empty → 视为 note；empty / 取消 / slash → 不算 */
export function isNonEmptyText(s: string): boolean {
  return typeof s === 'string' && s.trim().length > 0;
}

/** 把 submission 序列化为发给服务端的 JSON。
 *  - kind: 'custom' / 'skipped' 必须显式写出，否则 untagged 反序列化可能误识别
 *  - kind: 'single' / 'multi' 也写出来，便于跨版本兼容 */
export function serializeSubmission(sub: AskSubmission): AskSubmission {
  const answers: Record<number, AskQuestionAnswer> = {};
  for (const [k, v] of Object.entries(sub.answers)) {
    const idx = Number(k);
    const kind = (v as { kind?: string }).kind;
    if (kind === 'custom') {
      answers[idx] = { kind: 'custom', note: (v as { note: string }).note };
    } else if (kind === 'skipped') {
      answers[idx] = { kind: 'skipped' };
    } else if (kind === 'multi' || (!kind && Array.isArray((v as { options?: unknown }).options))) {
      const opts = (v as { options: string[] }).options || [];
      const note = (v as { note?: string }).note;
      answers[idx] = { kind: 'multi', options: opts, ...(note ? { note } : {}) };
    } else {
      const option = (v as { option: string }).option;
      const note = (v as { note?: string }).note;
      answers[idx] = { kind: 'single', option, ...(note ? { note } : {}) };
    }
  }
  return {
    cancelled: sub.cancelled,
    answers,
    ...(sub.custom_response !== undefined ? { custom_response: sub.custom_response } : {}),
  };
}