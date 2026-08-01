// 子代理实时 chip 栏（Desktop 端）
//
// 设计：水平 chip 栏，在输入框上方显示当前会话的子代理
//   - Bot 图标表示子代理，Brain 图标表示 handoff
//   - running 状态有脉冲动画点
//   - 点击 chip 切换到该子代理 session
//   - 无子代理时隐藏
//
// 后端 RPC：
//   - session.list_children { parent_session_id } -> ChildSession[]
//   - session.state_changed 通知 { session_id, loop_state, message_count }

import { useState, useEffect } from 'react';
import { Bot, Brain } from './icons';
import type { WsClient } from '@mcoder/shared/rpc/client.js';

interface ChildSession {
  session_id: string;
  title: string;
  model: string;
  source: 'subagent' | 'handoff' | 'normal';
  subagent_role: string | null;
  loop_state: string;
  message_count: number;
}

interface Props {
  client: WsClient;
  currentSessionId: string | null;
  onSwitchSession: (sessionId: string) => void;
}

export function SubagentBar({ client, currentSessionId, onSwitchSession }: Props) {
  const [children, setChildren] = useState<ChildSession[]>([]);

  const refresh = async () => {
    if (!client || !currentSessionId) { setChildren([]); return; }
    try {
      const result = await client.request('session.list_children', { parent_session_id: currentSessionId });
      setChildren(result || []);
    } catch { /* 静默 */ }
  };

  useEffect(() => { refresh(); }, [currentSessionId]);

  useEffect(() => {
    if (!client) return;
    const handler = (n: any) => {
      if (n.method === 'session.state_changed') {
        const p = n.params;
        setChildren(prev => {
          const exists = prev.some(c => c.session_id === p.session_id);
          if (exists) {
            return prev.map(c => c.session_id === p.session_id
              ? { ...c, loop_state: p.loop_state, message_count: p.message_count ?? c.message_count }
              : c);
          } else {
            refresh(); // 新子代理
            return prev;
          }
        });
      }
      if (n.method === 'session_created' || n.method === 'session.created') {
        refresh();
      }
    };
    client.onNotification(handler);
    return () => client.offNotification(handler);
  }, [client, currentSessionId]);

  if (children.length === 0) return null;

  return (
    <div className="subagent-bar">
      {children.map(c => (
        <div
          key={c.session_id}
          className={`subagent-chip ${c.loop_state === 'running' ? 'running' : ''} ${c.source === 'handoff' ? 'handoff' : ''}`}
          onClick={() => onSwitchSession(c.session_id)}
          title={c.title}
        >
          {c.source === 'handoff' ? <Brain size={12} /> : <Bot size={12} />}
          <span className="subagent-chip-name">{c.title.length > 30 ? c.title.slice(0, 27) + '...' : c.title}</span>
          {c.loop_state === 'running' && <span className="subagent-chip-dot" />}
          <span className="subagent-chip-count">{c.message_count}</span>
        </div>
      ))}
    </div>
  );
}
