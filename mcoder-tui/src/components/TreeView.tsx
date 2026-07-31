// DESIGN.md §4 / §10: TreeView（面板）
// - single border + textMuted
// - 角色色：user/assistant/tool 用统一色（不混搭 blue/green/gray）
// - 移除：italic、press ESC to close、← head emoji

import { Box, Text, useInput } from 'ink';
import { useState, useEffect } from 'react';
import type { WsClient } from '../rpc/client.js';
import type { MessageTree, MessageTreeNode } from '../rpc/types.js';
import { hydrateSnapshot, type SessionSnapshot } from '../rpc/sessionSnapshot.js';
import { useSessionStore, useMessagesStore, useUiStore } from '../store/index.js';
import { useAskStore } from '../ask/store.js';
import { TUI_COLORS, PREFIX } from '../theme.js';

interface Props {
  client: WsClient;
}

/// 计算每个节点的缩进深度
function buildIndented(nodes: MessageTreeNode[]): Array<{ node: MessageTreeNode; depth: number }> {
  const byId = new Map<string, MessageTreeNode>();
  for (const n of nodes) byId.set(n.id, n);
  const depthCache = new Map<string, number>();
  const depthOf = (id: string): number => {
    if (depthCache.has(id)) return depthCache.get(id)!;
    const n = byId.get(id);
    if (!n || !n.parent_id) {
      depthCache.set(id, 0);
      return 0;
    }
    const d = depthOf(n.parent_id) + 1;
    depthCache.set(id, d);
    return d;
  };
  return nodes.map((node) => ({ node, depth: depthOf(node.id) }));
}

export function TreeView({ client }: Props) {
  const sid = useSessionStore((s) => s.currentSessionId);
  const [tree, setTree] = useState<MessageTree | null>(null);
  const [selected, setSelected] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadTree = async () => {
    if (!sid) return;
    setLoading(true);
    setError(null);
    try {
      const result = await client.request('session.tree', { session_id: sid });
      setTree(result as MessageTree);
      const headIdx = (result as MessageTree).nodes.findIndex((n) => n.is_head);
      setSelected(headIdx >= 0 ? headIdx : 0);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadTree();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sid]);

  useInput((_input: string, key: any) => {
    if (!tree || tree.nodes.length === 0) return;
    if (key.upArrow) {
      setSelected((i) => Math.max(0, i - 1));
    } else if (key.downArrow) {
      setSelected((i) => Math.min(tree.nodes.length - 1, i + 1));
    } else if (key.return) {
      const target = tree.nodes[selected];
      if (!target || target.is_head) return;
      doCheckout(target.id);
    }
  });

  const doCheckout = async (messageId: string) => {
    if (!sid) return;
    setLoading(true);
    setError(null);
    try {
      const snapshot = await client.request('session.checkout', {
        session_id: sid,
        message_id: messageId,
      }) as SessionSnapshot;
      hydrateSnapshot({
        sessionId: sid,
        snapshot,
        store: {
          setCurrentSessionId: (id) => useSessionStore.getState().setCurrentSession(id),
          setMessages: (m) => useMessagesStore.getState().setMessages(m),
          setRole: (r) => useSessionStore.getState().setRole(r),
          setModel: (m) => useSessionStore.getState().setModel(m),
          setProjectPath: (p) => useSessionStore.getState().setProjectPath(p),
          setContextUsage: (used, w) => useSessionStore.getState().setContextUsage(used, w),
          setPendingPlan: (p) => useSessionStore.getState().setPendingPlan(p),
          setPendingTodos: (t) => useSessionStore.getState().setPendingTodos(t),
          setBackgroundTasks: (t) => useSessionStore.getState().setBackgroundTasks(t),
          setPendingAskFromSnapshot: (ask) => {
            const askStore = useAskStore.getState();
            if (ask === null) {
              askStore.clearSession(sid);
              return;
            }
            askStore.setPendingAskFromSnapshot(ask);
          },
          clearAskSession: (s) => useAskStore.getState().clearSession(s),
          replaceTodosFromSnapshot: () => {},
        },
      });
      useUiStore.getState().setView('chat');
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  if (!sid) {
    return (
      <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted}>
        <Text bold color={TUI_COLORS.accent}>Message Tree</Text>
        <Text color={TUI_COLORS.textMuted}>no active session</Text>
      </Box>
    );
  }

  if (loading && !tree) {
    return (
      <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted}>
        <Text bold color={TUI_COLORS.accent}>Message Tree</Text>
        <Text color={TUI_COLORS.textMuted}>loading</Text>
      </Box>
    );
  }

  if (error) {
    return (
      <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted}>
        <Text bold color={TUI_COLORS.accent}>Message Tree</Text>
        <Text color={TUI_COLORS.error}>{error}</Text>
      </Box>
    );
  }

  if (!tree || tree.nodes.length === 0) {
    return (
      <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted}>
        <Text bold color={TUI_COLORS.accent}>Message Tree</Text>
        <Text color={TUI_COLORS.textMuted}>no messages</Text>
      </Box>
    );
  }

  const indented = buildIndented(tree.nodes);

  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single" borderColor={TUI_COLORS.textMuted}>
      <Text bold color={TUI_COLORS.accent}>Message Tree</Text>
      {indented.map(({ node, depth }, i) => {
        const isSel = i === selected;
        const prefix = isSel ? PREFIX.running : '  ';
        const indent = '  '.repeat(depth);
        const roleColor = node.role === 'user' ? TUI_COLORS.success : node.role === 'assistant' ? TUI_COLORS.accent : TUI_COLORS.textMuted;
        const preview = node.preview.length > 50 ? node.preview.slice(0, 50) + '...' : node.preview;
        return (
          <Box key={node.id}>
            <Text color={isSel ? TUI_COLORS.accent : TUI_COLORS.textMuted}>{prefix}{indent}</Text>
            <Text color={roleColor}>[{node.role}]</Text>
            <Text color={node.is_head ? TUI_COLORS.warning : isSel ? TUI_COLORS.accent : TUI_COLORS.textPrimary}>
              {' '}{preview}
            </Text>
            {node.is_head && <Text color={TUI_COLORS.warning}>{` ${PREFIX.sep} head`}</Text>}
          </Box>
        );
      })}
      <Text color={TUI_COLORS.textMuted}>{`↑↓ navigate ${PREFIX.sep} Enter checkout ${PREFIX.sep} ESC close`}</Text>
    </Box>
  );
}