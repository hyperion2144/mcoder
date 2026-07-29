// 设计文档 §8.6.1: 桌面端 Diff Viewer
// 显示 git diff，支持语法高亮

import React, { useState, useEffect } from 'react';
import type { WsClient } from '@mcoder/shared/rpc/client.js';

export function DiffViewer({ client }: { client: WsClient }) {
  const [diff, setDiff] = useState<string>('');
  const [loading, setLoading] = useState(false);

  const loadDiff = async () => {
    setLoading(true);
    try {
      const result = await client.request('tool.call', {
        name: 'bash',
        args: { cmd: 'git diff', timeout: 10 },
      });
      setDiff(result.stdout || result.output || '');
    } catch (e: any) {
      setDiff(`Error: ${e.message}`);
    }
    setLoading(false);
  };

  useEffect(() => {
    loadDiff();
  }, []);

  const renderLine = (line: string, i: number) => {
    let className = 'diff-line';
    if (line.startsWith('+')) className += ' diff-added';
    else if (line.startsWith('-')) className += ' diff-removed';
    else if (line.startsWith('@@')) className += ' diff-hunk';
    return (
      <div key={i} className={className}>
        <span className="diff-line-content">{line || ' '}</span>
      </div>
    );
  };

  return (
    <div className="diff-viewer">
      <div className="diff-viewer-header">
        <span>Git Diff</span>
        <button onClick={loadDiff} disabled={loading}>
          {loading ? 'Loading...' : 'Refresh'}
        </button>
      </div>
      <div className="diff-content">
        {diff ? diff.split('\n').map(renderLine) : (
          <div className="diff-empty">No changes</div>
        )}
      </div>
    </div>
  );
}
