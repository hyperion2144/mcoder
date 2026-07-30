// AskUserCard - 消息流中的 ask_user 交互卡片
// 设计：非模态/非 Sheet/非居中面板，仅作为消息流中 tool_use 卡片位置的内联展示
// 交互通过 InputBox（底部输入框）完成：数字键 1-4 选择、Enter 提交、Esc 取消、文本作为 note
// 回答后原位置显示只读摘要

import { Box, Text } from 'ink';
import { useAskStore } from '../ask/store.js';
import { formatAskFullSummary } from '../ask/summary.js';
import type { AskRequest } from '../ask/types.js';

interface Props {
  ask_id: string;
  tool_call_id: string;
  request: AskRequest;
  /** 当前选择状态（来自 useAskStore.localSelections） */
  selections?: Record<number, string[]>;
  /** 当前各 question 的 note 状态 */
  notes?: Record<number, string>;
  /** 当前聚焦的 question 索引（0..N-1），用于文本输入归属 */
  focusIndex?: number;
}

/** 纯展示：pending 状态下显示问题 + 选项（带数字标号）+ 已选标记 */
export function AskUserCard({ request, selections, focusIndex }: Props) {
  return (
    <Box flexDirection="column" borderStyle="round" borderColor="yellow" paddingX={1} marginY={1}>
      <Text color="yellow" bold>
        ▸ ask_user (等待你的回答)
      </Text>
      {request.questions.map((q, i) => {
        const sel = selections?.[i] || [];
        const isMulti = !!q.multi_select;
        const focused = focusIndex === i;
        return (
          <Box key={i} flexDirection="column" marginY={0}>
            <Text color={focused ? 'cyan' : 'white'} bold={focused}>
              {'  '}Q{i + 1}. {q.question}
            </Text>
            {q.options.map((opt, j) => {
              const checked = sel.includes(opt.label);
              return (
                <Text key={j} color={checked ? 'green' : 'gray'}>
                  {'     '}[{j + 1}] {checked ? '●' : '○'} {opt.label}
                  {opt.description ? ` — ${opt.description}` : ''}
                </Text>
              );
            })}
            {isMulti && (
              <Text color="gray">{'     '}(多选，可选多个)</Text>
            )}
            {focused && (
              <Text color="cyan">{'     '}↑ 当前问题（直接输入文字作为 note）</Text>
            )}
          </Box>
        );
      })}
      <Text color="gray">{'  '}输入 1-4 选择 · 文字作为 note · Enter 提交 · Esc 取消</Text>
    </Box>
  );
}

/** 已回答的只读摘要：在原 AskUserCard 位置显示 */
export function AskUserSummary({
  request,
  submission,
}: {
  request: AskRequest;
  submission: { cancelled: boolean; answers: Record<number, any> };
}) {
  const text = formatAskFullSummary(request, submission as any);
  return (
    <Box flexDirection="column" borderStyle="round" borderColor="gray" paddingX={1} marginY={1}>
      <Text color="gray" bold>▸ ask_user (已回答)</Text>
      {text.split('\n').map((line, k) => (
        <Text key={k} color="white">{'  '}{line}</Text>
      ))}
    </Box>
  );
}

/** Helper: 从 store 读出当前 session 的 pending + 摘要（消息流渲染用） */
export function useAskForSession(session_id: string | null) {
  const pending = useAskStore((s) => (session_id ? s.pending[session_id] : null));
  const last = useAskStore((s) => (session_id ? s.lastSubmission[session_id] : null));
  return { pending, last };
}
