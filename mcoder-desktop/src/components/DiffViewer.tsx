// 设计文档 §8.6.1: 桌面端 Diff Viewer
// 显示 git diff，支持语法高亮

import React, { useState, useEffect } from 'react';
import type { WsClient } from '@mcoder/shared/rpc/client.js';
import { t } from '../i18n.js';

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
        <span>{t('ui.git_diff')}</span>
        <button onClick={loadDiff} disabled={loading}>
          {loading ? t('ui.loading') : t('ui.refresh')}
        </button>
      </div>
      <div className="diff-content">
        {diff ? diff.split('\n').map(renderLine) : (
          <div className="diff-empty">{t('ui.no_changes')}</div>
        )}
      </div>
    </div>
  );
}
