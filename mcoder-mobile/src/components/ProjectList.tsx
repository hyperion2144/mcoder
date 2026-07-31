// 设计文档 §8.6.2: 项目列表（入口页）
// 移动端以项目为入口，按 project_path 分组展示会话
// 顶部"新建会话"输入工作目录，卡片式触摸友好，显示最后活动时间

import React, { useState, useMemo } from 'react';
import type { SessionMeta } from '@mcoder/shared/rpc/types.js';
import { X } from './icons.js';

interface Props {
  sessions: SessionMeta[];
  onSelectProject: (projectPath: string) => void;
  onNewSession: (projectPath: string) => void;
  onDisconnect: () => void;
}

function basename(path: string): string {
  const parts = path.split(/[/\\]/).filter(Boolean);
  return parts[parts.length - 1] || path;
}

// 相对时间格式化
function timeAgo(iso: string): string {
  const t = new Date(iso).getTime();
  if (isNaN(t)) return '';
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
  path: string;
  sessions: SessionMeta[];
  count: number;
  lastIso: string;
}

export function ProjectList({ sessions, onSelectProject, onNewSession, onDisconnect }: Props) {
  const [newPath, setNewPath] = useState('');
  const [showNew, setShowNew] = useState(false);

  const projects = useMemo<ProjectGroup[]>(() => {
    const map = new Map<string, ProjectGroup>();
    for (const s of sessions) {
      const key = s.project_path || '(unknown)';
      const existing = map.get(key);
      if (!existing) {
        map.set(key, { path: key, sessions: [s], count: 1, lastIso: s.created_at });
      } else {
        existing.sessions.push(s);
        existing.count += 1;
        if (new Date(s.created_at).getTime() > new Date(existing.lastIso).getTime()) {
          existing.lastIso = s.created_at;
        }
      }
    }
    return Array.from(map.values()).sort((a, b) =>
      new Date(b.lastIso).getTime() - new Date(a.lastIso).getTime()
    );
  }, [sessions]);

  const handleCreate = () => {
    const trimmed = newPath.trim();
    if (!trimmed) return;
    onNewSession(trimmed);
    setNewPath('');
    setShowNew(false);
  };

  return (
    <div className="project-list-page">
      <div className="project-list-header">
        <span className="project-list-title">Projects</span>
        <button className="project-list-disconnect" onClick={onDisconnect}>
          Disconnect
        </button>
      </div>

      <div className="project-new-bar">
        {!showNew ? (
          <button className="project-new-toggle" onClick={() => setShowNew(true)}>
            + New Session
          </button>
        ) : (
          <div className="project-new-form">
            <input
              type="text"
              className="project-new-input"
              value={newPath}
              onChange={(e) => setNewPath(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') handleCreate(); }}
              placeholder="/path/to/project"
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
              autoFocus
            />
            <button className="project-new-confirm" onClick={handleCreate}>Create</button>
            <button
              className="project-new-cancel"
              onClick={() => { setShowNew(false); setNewPath(''); }}
              aria-label="cancel"
            >
              <X size={16} />
            </button>
          </div>
        )}
      </div>

      <div className="project-cards">
        {projects.length === 0 && (
          <div className="project-empty">
            No projects yet.{'\n'}Create a new session to get started.
          </div>
        )}
        {projects.map((p) => (
          <button
            key={p.path}
            className="project-card"
            onClick={() => onSelectProject(p.path)}
          >
            <div className="project-card-name">{basename(p.path)}</div>
            <div className="project-card-path">{p.path}</div>
            <div className="project-card-meta">
              <span className="project-card-count">
                {p.count} session{p.count !== 1 ? 's' : ''}
              </span>
              <span className="project-card-last">{timeAgo(p.lastIso)}</span>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}
