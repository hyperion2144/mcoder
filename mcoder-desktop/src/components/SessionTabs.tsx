// 会话 tab 栏：水平排列当前项目的会话，支持切换/关闭/新建
// 显示 title + model · date，当前激活 tab 高亮

import React from 'react';
import type { SessionMeta } from '@mcoder/shared/rpc/types.js';
import { X, Plus } from './icons.js';
import { t } from '../i18n.js';

interface SessionTabsProps {
  sessions: SessionMeta[];
  openTabs: string[];
  activeSessionId: string;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  onNew: () => void;
}

// tab 标签：title (model · date)
function tabLabel(s: SessionMeta): string {
  const date = s.created_at ? s.created_at.slice(0, 10) : '';
  const model = s.model || '';
  const title = s.title || s.session_id.slice(0, 8);
  const suffix = [model, date].filter(Boolean).join(' · ');
  return suffix ? `${title} (${suffix})` : title;
}

export function SessionTabs({
  sessions,
  openTabs,
  activeSessionId,
  onSelect,
  onClose,
  onNew,
}: SessionTabsProps) {
  const byId = new Map(sessions.map((s) => [s.session_id, s]));

  return (
    <div className="session-tabs">
      {openTabs.map((id) => {
        const s = byId.get(id);
        const label = s ? tabLabel(s) : id.slice(0, 8);
        const isActive = id === activeSessionId;
        return (
          <div
            key={id}
            className={`session-tab ${isActive ? 'active' : ''}`}
            onClick={() => onSelect(id)}
            title={s?.title || id}
          >
            <span className="session-tab-label">{label}</span>
            <button
              className="session-tab-close"
              onClick={(e) => {
                e.stopPropagation();
                onClose(id);
              }}
              title={t('ui.close_tab')}
            >
              <X size={12} />
            </button>
          </div>
        );
      })}
      <button className="session-tab-new" onClick={onNew} title={t('ui.new_session_project')}>
        <Plus size={14} />
      </button>
    </div>
  );
}
