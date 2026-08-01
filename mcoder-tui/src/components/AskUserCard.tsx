// DESIGN.md §3 / §6: ask_user 卡片（交互类，warning round border）
// 标题：▸ ask_user · 等待输入
// 移除：↑ 当前问题（...）、(多选，可选多个)、输入 1-4 选择 · ...

import { Box, Text } from 'ink';
import { TUI_COLORS, ROLE_COLOR, PREFIX } from '../theme.js';
import { formatAskFullSummary } from '../ask/summary.js';
import type { AskRequest } from '../ask/types.js';
import { t } from '../i18n.js';

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

/** pending 状态：交互卡片 */
export function AskUserCard({ request, selections, focusIndex }: Props) {
  return (
    <Box flexDirection="column" borderStyle="round" borderColor={ROLE_COLOR.interaction} paddingX={1} marginY={1}>
      <Text color={ROLE_COLOR.interaction} bold>
        {`${PREFIX.pending} ask_user ${PREFIX.sep} ${t('ui.waiting_input')}`}
      </Text>
      {request.questions.map((q, i) => {
        const sel = selections?.[i] || [];
        const isMulti = !!q.multi_select;
        const focused = focusIndex === i;
        return (
          <Box key={i} flexDirection="column" marginY={0}>
            <Text color={focused ? TUI_COLORS.accent : TUI_COLORS.textPrimary} bold={focused}>
              {'  '}Q{i + 1}. {q.question}
            </Text>
            {q.options.map((opt, j) => {
              const checked = sel.includes(opt.label);
              return (
                <Text key={j} color={checked ? TUI_COLORS.success : TUI_COLORS.textSecondary}>
                  {'     '}[{j + 1}] {checked ? '●' : '○'} {opt.label}
                  {opt.description ? ` ${PREFIX.sep} ${opt.description}` : ''}
                </Text>
              );
            })}
            {isMulti && (
              <Text color={TUI_COLORS.textMuted}>{'     '}multi-select</Text>
            )}
          </Box>
        );
      })}
    </Box>
  );
}

/** 已回答的只读摘要 */
export function AskUserSummary({
  request,
  submission,
}: {
  request: AskRequest;
  submission: { cancelled: boolean; answers: Record<number, any> };
}) {
  const text = formatAskFullSummary(request, submission as any);
  return (
    <Box flexDirection="column" borderStyle="single" borderColor={ROLE_COLOR.done} paddingX={1} marginY={1}>
      <Text color={TUI_COLORS.textMuted} bold>{`ask_user ${PREFIX.sep} ${t('ui.answered')}`}</Text>
      {text.split('\n').map((line, k) => (
        <Text key={k} color={TUI_COLORS.textPrimary}>{'  '}{line}</Text>
      ))}
    </Box>
  );
}

/** Helper: 从 store 读出当前 session 的 pending + 摘要（消息流渲染用） */
export function useAskForSession(session_id: string | null) {
  // 注：原文件保留 useAskStore 的 hook，迁到 MessageList 内使用更合适
  // 这里仅导出 helper 以兼容旧引用
  return { pending: null, last: null };
}