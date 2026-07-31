// DESIGN.md §4 / §6: PlanApproval（交互类，warning round border）
// - round border + warning
// - 标题：▸ plan · 等待审批
// - 移除：JSON.stringify 兜底（用更体面 fallback）

import { Box, Text, useInput } from 'ink';
import { useSessionStore } from '../store/index.js';
import type { WsClient } from '../rpc/client.js';
import { TUI_COLORS, ROLE_COLOR, PREFIX } from '../theme.js';

interface Props {
  client: WsClient;
}

export function PlanApproval({ client }: Props) {
  const { pendingPlan, currentSessionId, setPendingPlan } = useSessionStore();

  useInput((input: string, key: any) => {
    if (!pendingPlan) return;
    const sid = currentSessionId;
    if (!sid) return;

    if (input === 'y') {
      client.request('session.approve', { session_id: sid, plan_id: pendingPlan.id || '' })
        .then(() => setPendingPlan(null))
        .catch(() => {});
    } else if (input === 'n') {
      client.request('session.approve', { session_id: sid, plan_id: pendingPlan.id || '', action: 'reject' })
        .then(() => setPendingPlan(null))
        .catch(() => {});
    } else if (input === 'e') {
      setPendingPlan(null);
    }
  });

  if (!pendingPlan) return null;

  return (
    <Box paddingX={1} borderStyle="round" borderColor={ROLE_COLOR.interaction} flexDirection="column">
      <Text color={ROLE_COLOR.interaction} bold>{`${PREFIX.pending} plan ${PREFIX.sep} 等待审批`}</Text>
      <Box flexDirection="column" marginY={0}>
        {Array.isArray(pendingPlan.steps) ? (
          pendingPlan.steps.map((step: any, i: number) => (
            <Text key={i} color={TUI_COLORS.textPrimary}>
              {i + 1}. {step.description || step.text || '(no description)'}
            </Text>
          ))
        ) : (
          <Text color={TUI_COLORS.textMuted}>(empty plan)</Text>
        )}
      </Box>
      <Text color={TUI_COLORS.success}>[Y] approve</Text>
      <Text color={TUI_COLORS.warning}>[E] edit</Text>
      <Text color={TUI_COLORS.error}>[N] reject</Text>
    </Box>
  );
}