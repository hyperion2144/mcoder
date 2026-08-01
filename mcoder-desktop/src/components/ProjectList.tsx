// 项目选择页：展示所有有会话的项目，按 project_path 分组
// 顶部有"新建会话"入口（输入工作目录）

import React, { useState, useMemo } from 'react';
import type { SessionMeta } from '@mcoder/shared/rpc/types.js';
import { t } from '../i18n.js';

interface ProjectListProps {
  sessions: SessionMeta[];
  onSelectProject: (projectPath: string) => void;
  onCreateSession: (projectPath: string) => void;
}

function basename(p: string): string {
  if (!p) return p;
  const norm = p.replace(/\/+$/, '');
  const idx = norm.lastIndexOf('/');
  return idx >= 0 ? norm.slice(idx + 1) : norm;
}

// 相对时间格式化：2h ago / yesterday / 3d ago / 2026-01-01
function timeAgo(iso: string): string {
  const t = new Date(iso).getTime();
  if (isNaN(t)) return iso;
  const diff = Date.now() - t;
  const min = Math.floor(diff / 60_000);
  if (min < 1) return 'just now';
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day === 1) return 'yesterday';
  if (day < 30) return `${day}d ago`;
  return new Date(iso).toISOString().slice(0, 10);
}

interface ProjectGroup {
  projectPath: string;
  count: number;
  lastCreatedAt: number;
  lastIso: string;
}

export function ProjectList({ sessions, onSelectProject, onCreateSession }: ProjectListProps) {
  const [newProject, setNewProject] = useState('');

  const groups = useMemo<ProjectGroup[]>(() => {
    const map = new Map<string, ProjectGroup>();
    for (const s of sessions) {
      const p = s.project_path || '(unknown)';
      const existing = map.get(p);
      const created = new Date(s.created_at).getTime() || 0;
      if (!existing) {
        map.set(p, {
          projectPath: p,
          count: 1,
          lastCreatedAt: created,
          lastIso: s.created_at,
        });
      } else {
        existing.count += 1;
        if (created > existing.lastCreatedAt) {
          existing.lastCreatedAt = created;
          existing.lastIso = s.created_at;
        }
      }
    }
    return Array.from(map.values()).sort((a, b) => b.lastCreatedAt - a.lastCreatedAt);
  }, [sessions]);

  const handleCreate = () => {
    const p = newProject.trim();
    if (!p) return;
    onCreateSession(p);
    setNewProject('');
  };

  return (
    <div className="project-list">
      <div className="project-list-header">
        <span className="project-list-title">{t('ui.projects')}</span>
        <span className="project-list-sub">{groups.length} project(s)</span>
      </div>

      <div className="project-new">
        <input
          className="project-new-input"
          type="text"
          placeholder="/path/to/project  (new session)"
          value={newProject}
          onChange={(e) => setNewProject(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') handleCreate();
          }}
        />
        <button
          className="project-new-btn"
          onClick={handleCreate}
          disabled={!newProject.trim()}
        >
          {t('ui.new_session_btn')}
        </button>
      </div>

      <div className="project-cards">
        {groups.length === 0 && (
          <div className="project-empty">{t('ui.no_sessions_yet')}</div>
        )}
        {groups.map((g) => (
          <div
            key={g.projectPath}
            className="project-card"
            onClick={() => onSelectProject(g.projectPath)}
            title={g.projectPath}
          >
            <div className="project-card-name">{basename(g.projectPath)}</div>
            <div className="project-card-path">{g.projectPath}</div>
            <div className="project-card-meta">
              <span>{g.count} session{g.count === 1 ? '' : 's'}</span>
              <span className="project-card-last">last: {timeAgo(g.lastIso)}</span>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
