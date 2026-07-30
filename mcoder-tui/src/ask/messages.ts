// 共享纯逻辑：从消息流中检测是否已存在某个 tool_use 块。
// 用于三端 attach / ask_pending 实时通知时，决定是否需要追加占位 tool_use，
// 避免重复制造已经存在的 block（issue 6 / review 第二轮反馈）。
//
// 纯函数：便于在 store / AskCard / App 入口都引用同一份判定逻辑。

import type { Message } from '../rpc/types.js';

/**
 * 判断 messages 中是否已经存在 tool_call_id 对应的 tool_use block。
 *
 * 作用：
 * - attach 后 peek ask.pending 时，避免在消息流中重复插入 tool_use 占位
 * - session.ask_pending 实时通知到达时，避免重复追加已存在的 tool_use
 * - 三端共用同一个判定函数，行为保持一致
 */
export function hasToolUse(
  messages: ReadonlyArray<Message> | undefined | null,
  tool_call_id: string | null | undefined,
): boolean {
  if (!messages || !tool_call_id) return false;
  for (const msg of messages) {
    for (const block of msg.content) {
      if (block.type === 'tool_use' && block.id === tool_call_id) {
        return true;
      }
    }
  }
  return false;
}