// 设计文档 §8.6.2: 状态栏
// 显示连接状态、网络状态、模型、上下文用量、成本

import React from 'react';

interface Props {
  connected: boolean;
  networkStatus: 'online' | 'offline';
  role: string;
  model: string;
  contextUsed: number;
  contextWindow: number;
  cost: number;
  onMenuClick: () => void;
}

export function StatusBar({
  connected,
  networkStatus,
  role,
  model,
  contextUsed,
  contextWindow,
  cost,
  onMenuClick,
}: Props) {
  const ctxPct = contextWindow > 0 ? Math.round((contextUsed / contextWindow) * 100) : 0;
  const ctxStr = contextUsed > 1000 ? `${(contextUsed / 1000).toFixed(1)}k` : `${contextUsed}`;
  const winStr = contextWindow > 1000 ? `${(contextWindow / 1000).toFixed(0)}k` : `${contextWindow}`;

  return (
    <div className="status-bar">
      <button className="menu-button" onClick={onMenuClick} aria-label="menu">
        ☰
      </button>
      <div className="status-indicators">
        <span className={`status-dot ${connected ? 'online' : 'offline'}`}>
          {connected ? '●' : '○'}
        </span>
        {networkStatus === 'offline' && (
          <span className="status-net status-net-offline">offline</span>
        )}
        <span className="status-role">{role}</span>
        {model && <span className="status-model">{model}</span>}
      </div>
      <div className="status-context">
        <span className="ctx-text">{ctxStr}/{winStr}</span>
        <div className="ctx-bar">
          <div className="ctx-bar-fill" style={{ width: `${Math.min(ctxPct, 100)}%` }} />
        </div>
      </div>
      {cost > 0 && <span className="status-cost">${cost.toFixed(3)}</span>}
    </div>
  );
}
