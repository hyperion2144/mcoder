// components/TreeView.tsx - 消息树视图（移动端全屏模态）
//
// 展示会话消息树，点击 checkout 按钮切换到该分支。
// 通过 /tree 命令或 Drawer 入口打开。

import { useState, useEffect, useCallback } from 'react';
import type { WsClient } from '@mcoder/shared/rpc/client.js';
import type { MessageTree, MessageTreeNode } from '@mcoder/shared/rpc/types.js';
import { hydrateSnapshot, type SessionSnapshot } from '@mcoder/shared/rpc/sessionSnapshot.js';
import { useSessionStore, useMessagesStore } from '@mcoder/shared/store/index.js';
import { useAskStore } from '@mcoder/shared/ask/index.js';
import { X } from './icons.js';

interface Props {
  client: WsClient;
  onClose: () => void;
}

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

export function TreeView({ client, onClose }: Props) {
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
      onClose();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="tree-modal-overlay" onClick={onClose}>
      <div className="tree-modal" onClick={(e) => e.stopPropagation()}>
        <div className="tree-modal-header">
          <span className="tree-modal-title">Message Tree</span>
          <button className="tree-modal-close" onClick={onClose} aria-label="close"><X size={18} /></button>
        </div>
        <div className="tree-modal-body">
          {loading && !tree && <div className="tree-modal-empty">Loading...</div>}
          {error && <div className="tree-modal-error">{error}</div>}
          {tree && tree.nodes.length === 0 && <div className="tree-modal-empty">No messages</div>}
          {tree && tree.nodes.length > 0 && (
            <div className="tree-modal-list">
              {withDepth(tree.nodes).map(({ node, depth }) => (
                <div
                  key={node.id}
                  className={`tree-modal-node ${node.is_head ? 'is-head' : ''}`}
                  style={{ paddingLeft: `${12 + depth * 16}px` }}
                >
                  <span className={`tree-modal-role role-${node.role}`}>{node.role}</span>
                  <span className="tree-modal-preview">
                    {node.preview.length > 50 ? node.preview.slice(0, 50) + '...' : node.preview}
                  </span>
                  {node.is_head ? (
                    <span className="tree-modal-head-badge">head</span>
                  ) : (
                    <button
                      className="tree-modal-checkout"
                      onClick={() => doCheckout(node.id)}
                    >
                      checkout
                    </button>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
