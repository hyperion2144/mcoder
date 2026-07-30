// 消息流中 tool_use 和 tool_result 配对辅助函数（三端共享）
// tool_result 通过 id 关联到 tool_use

import type { Message, ContentBlock } from '../rpc/types.js';

export interface PairedToolCall {
  /** tool_use block */
  use: ContentBlock;
  /** 对应的 tool_result block（可能为 null = 仍在执行） */
  result: ContentBlock | null;
}

/**
 * 扫描消息流，把 tool_use 和同 id 的 tool_result 配对
 * 顺序保持 tool_use 在消息流中出现的顺序
 */
export function pairToolCalls(messages: Message[]): PairedToolCall[] {
  const resultsById = new Map<string, ContentBlock>();
  const uses: ContentBlock[] = [];

  // 先扫一遍收集所有 tool_result（按 id 索引）和 tool_use（按顺序）
  for (const msg of messages) {
    for (const block of msg.content) {
      if (block.type === 'tool_result' && block.id) {
        // 后出现的 result 覆盖前面的（避免重复）
        if (!resultsById.has(block.id)) {
          resultsById.set(block.id, block);
        }
      }
      if (block.type === 'tool_use' && block.id) {
        uses.push(block);
      }
    }
  }

  return uses.map(use => ({
    use,
    result: use.id ? resultsById.get(use.id) || null : null,
  }));
}

/**
 * 判断某条消息中是否有未配对的 tool_use（即仍在执行中的工具调用）
 */
export function hasPendingToolUse(messages: Message[]): boolean {
  const paired = new Set<string>();
  for (const msg of messages) {
    for (const block of msg.content) {
      if (block.type === 'tool_result' && block.id) {
        paired.add(block.id);
      }
    }
  }
  for (const msg of messages) {
    for (const block of msg.content) {
      if (block.type === 'tool_use' && block.id && !paired.has(block.id)) {
        return true;
      }
    }
  }
  return false;
}
