// 设计文档 §8.6.2: 抽屉菜单
// 会话切换、新建会话、断开连接、命令列表

import React from 'react';
import type { SessionMeta } from '@mcoder/shared/rpc/types.js';

interface CommandInfo {
  name: string;
  description: string;
  usage: string;
}

interface Props {
  open: boolean;
  onClose: () => void;
  sessions: SessionMeta[];
  currentSessionId: string | null;
  onSelectSession: (id: string) => void;
  onNewSession: () => void;
  onDisconnect: () => void;
  commands: CommandInfo[];
}

export function Drawer({
  open,
  onClose,
  sessions,
  currentSessionId,
  onSelectSession,
  onNewSession,
  onDisconnect,
  commands,
}: Props) {
  return (
    <>
      {open && <div className="drawer-overlay" onClick={onClose} />}
      <div className={`drawer ${open ? 'drawer-open' : ''}`}>
        <div className="drawer-header">
          <span className="drawer-title">Sessions</span>
          <button className="drawer-close" onClick={onClose}>Close</button>
        </div>

        <div className="drawer-section">
          <button className="drawer-item drawer-new" onClick={onNewSession}>
            + New Session
          </button>
          {sessions.map((s) => (
            <button
              key={s.session_id}
              className={`drawer-item ${s.session_id === currentSessionId ? 'drawer-item-active' : ''}`}
              onClick={() => onSelectSession(s.session_id)}
            >
              <div className="session-title">{s.title || s.session_id.slice(0, 8)}</div>
              <div className="session-meta">
                {s.model} · {new Date(s.created_at).toLocaleDateString()}
              </div>
            </button>
          ))}
        </div>

        <div className="drawer-section">
          <div className="drawer-section-title">Commands</div>
          {commands.map((cmd) => (
            <div key={cmd.name} className="drawer-cmd">
              <span className="cmd-name">/{cmd.name}</span>
              <span className="cmd-desc">{cmd.description}</span>
            </div>
          ))}
        </div>

        <div className="drawer-footer">
          <button className="drawer-disconnect" onClick={onDisconnect}>
            Disconnect
          </button>
        </div>
      </div>
    </>
  );
}
