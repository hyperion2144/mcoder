// components/TreeView.tsx - 消息树视图（桌面端右栏面板）
//
// 展示会话消息树，点击消息可切换到该分支（checkout）。
// 通过 /tree 命令或右栏 Tree 标签打开。

import { useState, useEffect, useCallback } from 'react';
import type { WsClient } from '@mcoder/shared/rpc/client.js';
import type { MessageTree, MessageTreeNode } from '@mcoder/shared/rpc/types.js';
import { hydrateSnapshot, type SessionSnapshot } from '@mcoder/shared/rpc/sessionSnapshot.js';
import { useSessionStore, useMessagesStore } from '@mcoder/shared/store/index.js';
import { useAskStore } from '@mcoder/shared/ask/index.js';
import { t } from '../i18n.js';

interface Props {
  client: WsClient;
}

/// 计算每个节点的深度
function withDepth(nodes: MessageTreeNode[]): Array<{ node: MessageTreeNode; depth: number }> {
  const byId = new Map<string, MessageTreeNode>();
  for (const n of nodes) byId.set(n.id, n);
  const cache = new Map<string, number>();
  const depthOf = (id: string): number => {
    if (cache.has(id)) return cache.get(id)!;
    const n = byId.get(id);
    if (!n || !n.parent_id) {
      cache.set(id, 0);
      return 0;
    }
    const d = depthOf(n.parent_id) + 1;
    cache.set(id, d);
    return d;
  };
  return nodes.map((node) => ({ node, depth: depthOf(node.id) }));
}

export function TreeView({ client }: Props) {
  const currentSessionId = useSessionStore((s) => s.currentSessionId);
  const [tree, setTree] = useState<MessageTree | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadTree = useCallback(async () => {
    if (!currentSessionId) return;
    setLoading(true);
    setError(null);
    try {
      const result = await client.request('session.tree', { session_id: currentSessionId });
      setTree(result as MessageTree);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  }, [client, currentSessionId]);

  useEffect(() => {
    loadTree();
  }, [loadTree]);

  const doCheckout = async (messageId: string) => {
    if (!currentSessionId) return;
    setLoading(true);
    setError(null);
    try {
      const snapshot = await client.request('session.checkout', {
        session_id: currentSessionId,
        message_id: messageId,
      }) as SessionSnapshot;
      hydrateSnapshot({
        sessionId: currentSessionId,
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
              askStore.clearSession(currentSessionId);
              return;
            }
            askStore.setPendingAskFromSnapshot(ask);
          },
          clearAskSession: (sid) => useAskStore.getState().clearSession(sid),
          replaceTodosFromSnapshot: () => {},
        },
      });
      await loadTree();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  if (!currentSessionId) {
    return <div className="tree-view-empty">{t('ui.no_active_session')}</div>;
  }
  if (loading && !tree) {
    return <div className="tree-view-empty">{t('ui.loading')}</div>;
  }
  if (error) {
    return <div className="tree-view-error">{error}</div>;
  }
  if (!tree || tree.nodes.length === 0) {
    return <div className="tree-view-empty">{t('ui.no_messages_session')}</div>;
  }

  const indented = withDepth(tree.nodes);

  return (
    <div className="tree-view">
      <div className="tree-view-header">
        <span>{t('ui.message_tree')}</span>
        <button className="tree-view-refresh" onClick={loadTree} title={t('ui.refresh')}>↻</button>
      </div>
      <div className="tree-view-list">
        {indented.map(({ node, depth }) => (
          <div
            key={node.id}
            className={`tree-node ${node.is_head ? 'is-head' : ''}`}
            style={{ paddingLeft: `${8 + depth * 16}px` }}
          >
            <span className={`tree-node-role role-${node.role}`}>{node.role}</span>
            <span className="tree-node-preview" title={node.preview}>
              {node.preview.length > 60 ? node.preview.slice(0, 60) + '...' : node.preview}
            </span>
            {node.is_head && <span className="tree-node-head-badge">head</span>}
            {!node.is_head && (
              <button
                className="tree-node-checkout"
                onClick={() => doCheckout(node.id)}
                title="Switch to this branch"
              >
                checkout
              </button>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
