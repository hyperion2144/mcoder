// 设计文档（ask_user）：跨 TUI / Desktop / Mobile 共享的 AskUser 工具类型
// - 服务端：mcoder/src/ask_user.rs（schema/pending 池/answer 校验）
// - 客户端：mcoder-tui/src/ask/{types,validation,store,summary}.ts
//
// Ask 工具是结构化普通工具，在消息流中渲染为交互式卡片
// （非模态框 / 非底部 Sheet）。回答后，原位置显示只读摘要。
//
// 约束：
//   - 1-4 个问题
//   - 每个问题 2-4 个选项
//   - 单选 / 多选 / 其他自由文本 / 取消

export type AskMode = 'single' | 'multi';

export interface AskOption {
  label: string;
  description?: string;
}

export interface AskQuestion {
  question: string;
  header?: string;          // 短标题（可选）
  options: AskOption[];     // 2-4 项
  multi_select?: boolean;   // 默认 false
}

export interface AskRequest {
  questions: AskQuestion[]; // 1-4 项
}

/** 单题答案：结构化 single / multi，或纯自由文本 Custom / 显式跳过 */
export interface AskSingleAnswer {
  kind?: 'single';           // 显式 tag，便于 round-trip（与服务端 AskQuestionAnswer 对齐）
  option: string;            // 选项 label（必须是原始选项之一）
  note?: string;             // 自由文本（"其他" 补充），可空
}

export interface AskMultiAnswer {
  kind?: 'multi';
  options: string[];         // 选项 label 列表（必须是原始选项子集）
  note?: string;
}

/** 自由文本答复：用户没选任何 option，仅给一段话（issue 3） */
export interface AskCustomAnswer {
  kind: 'custom';
  note: string;
}

/** 显式跳过该题（用户未回答） */
export interface AskSkippedAnswer {
  kind: 'skipped';
}

export type AskQuestionAnswer = AskSingleAnswer | AskMultiAnswer | AskCustomAnswer | AskSkippedAnswer;

export interface AskSubmission {
  /** 是否取消整个 Ask */
  cancelled: boolean;
  /** question 索引 → 答案；cancelled=true 时为空对象 */
  answers: Record<number, AskQuestionAnswer>;
  /** 跨题整段自由文本（issue 3）。当 answers 为空 / 全 Skipped / 全 Custom 时设置；服务端校验接受 */
  custom_response?: string;
}

/** 解析 + 验证纯函数：输入原始 JSON（来自 LLM），输出 {valid, errors, value} */
export interface AskValidationResult {
  valid: boolean;
  errors: string[];
  value?: AskRequest;
}

/** 工具名常量（与服务端 ask_user.rs 保持一致） */
export const ASK_USER_TOOL = 'ask_user' as const;

/** WS 事件 / RPC 方法名常量 */
export const ASK_PENDING_EVENT = 'session.ask_pending' as const;
export const ASK_ANSWERED_EVENT = 'session.ask_answered' as const;
export const ASK_CANCELLED_EVENT = 'session.ask_cancelled' as const;
export const RPC_ASK_PENDING = 'ask.pending';
export const RPC_ASK_ANSWER = 'ask.answer';
export const RPC_ASK_CANCEL = 'ask.cancel';

export const ASK_MAX_QUESTIONS = 4;
export const ASK_MAX_OPTIONS = 4;
export const ASK_MIN_QUESTIONS = 1;
export const ASK_MIN_OPTIONS = 2;