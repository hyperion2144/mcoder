// 设计文档 §6.2: components/PlanApproval.tsx - Plan 审批 UI
// [y] approve  [e] edit  [n] reject

import { Box, Text, useInput } from 'ink';
import { useSessionStore } from '../store/index.js';
import type { WsClient } from '../rpc/client.js';

interface Props {
  client: WsClient;
}

export function PlanApproval({ client }: Props) {
  const { pendingPlan, currentSessionId, setPendingPlan } = useSessionStore();

  useInput((input: string, key: any) => {
    if (!pendingPlan) return;
    const sid = currentSessionId;
    if (!sid) return;

    // 设计文档 §6.2: [y] approve [e] edit [n] reject
    if (input === 'y') {
      // approve
      client.request('session.approve', { session_id: sid, plan_id: pendingPlan.id || '' })
        .then(() => setPendingPlan(null))
        .catch(() => {});
    } else if (input === 'n') {
      // 终审修复 #12：reject 通过 session.approve action=reject 提交（与 server 字段统一）
      client.request('session.approve', { session_id: sid, plan_id: pendingPlan.id || '', action: 'reject' })
        .then(() => setPendingPlan(null))
        .catch(() => {});
    } else if (input === 'e') {
      // edit - 切换到输入框让用户输入修改意见
      // 简化实现：reject 并提示用户重新描述
      setPendingPlan(null);
    }
  });

  if (!pendingPlan) return null;

  return (
    <Box paddingX={1} borderStyle="single" flexDirection="column">
      <Text color="yellow" bold>Plan pending approval</Text>
      <Box flexDirection="column" marginY={0}>
        {Array.isArray(pendingPlan.steps) ? (
          pendingPlan.steps.map((step: any, i: number) => (
            <Text key={i} color="white">
              {i + 1}. {step.description || step.text || JSON.stringify(step)}
            </Text>
          ))
        ) : (
          <Text color="white">{JSON.stringify(pendingPlan, null, 2)}</Text>
        )}
      </Box>
      <Text color="cyan">[y] approve</Text>
      <Text color="yellow">[e] edit</Text>
      <Text color="red">[n] reject</Text>
    </Box>
  );
}
