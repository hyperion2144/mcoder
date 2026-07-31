// 设计文档 §8.6.2: 会话 tab 栏
// 水平滚动 tab，每个 tab 显示会话标题（简短）+ 关闭按钮
// 末尾 "+" 用于新建会话，当前激活 tab 高亮

import React, { useRef, useEffect } from 'react';
import type { SessionMeta } from '@mcoder/shared/rpc/types.js';
import { X } from './icons.js';

interface Props {
  sessions: SessionMeta[];
  currentSessionId: string | null;
  onSelectSession: (id: string) => void;
  onCloseSession: (id: string) => void;
  onNewSession: () => void;
}

function shortTitle(s: SessionMeta): string {
  if (s.title) {
    return s.title.length > 12 ? s.title.slice(0, 12) + '…' : s.title;
  }
  return s.session_id.slice(0, 8);
}

export function SessionTabs({
  sessions,
  currentSessionId,
  onSelectSession,
  onCloseSession,
  onNewSession,
}: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);

  // 当前激活 tab 滚入视野
  useEffect(() => {
    const container = scrollRef.current;
    if (!container) return;
    const active = container.querySelector('.session-tab-active') as HTMLElement | null;
    if (active) {
      active.scrollIntoView({ behavior: 'smooth', inline: 'center', block: 'nearest' });
    }
  }, [currentSessionId]);

  return (
    <div className="session-tabs" ref={scrollRef}>
      {sessions.map((s) => {
        const active = s.session_id === currentSessionId;
        return (
          <div
            key={s.session_id}
            className={`session-tab ${active ? 'session-tab-active' : ''}`}
            onClick={() => onSelectSession(s.session_id)}
          >
            <span className="session-tab-title">{shortTitle(s)}</span>
            <button
              className="session-tab-close"
              onClick={(e) => {
                e.stopPropagation();
                onCloseSession(s.session_id);
              }}
              aria-label="close tab"
            >
              <X size={14} />
            </button>
          </div>
        );
      })}
      <button
        className="session-tab session-tab-new"
        onClick={onNewSession}
        aria-label="new session"
      >
        +
      </button>
    </div>
  );
}
