// Desktop/Mobile 共享工具卡片组件（HTML）
// DESIGN.md §3 / §7: 工具卡片
// - 统一角色色：execution=accent；thinking=mauve；done=textMuted；failed=error
// - loading 时 ShimmerText 流光（CSS gradient + animation）
// - 状态前缀：▶ loading / ✓ done / ✗ failed
// - 移除：-- Input --、running...、... (lines total)

import React, { useRef, useState } from 'react';
import {
  extractToolMeta, summarizeResult, formatToolResult,
  type FoldState,
} from './meta.js';
import type { ContentBlock } from '../rpc/types.js';
import { PREFIX } from '../theme.js';

/** DESIGN.md §3: 角色色（5 类统一为 4 类） */
const ROLE_BORDER: Record<'execution' | 'thinking' | 'done' | 'error', string> = {
  execution: 'var(--accent)',
  thinking: 'var(--mauve)',
  done: 'var(--border-subtle)',
  error: 'var(--error)',
};

/** 状态前缀 */
const STATUS_PREFIX = {
  loading: '▶',
  done: '✓',
  failed: '✗',
} as const;

/** 三态循环 */
function nextFold(f: FoldState): FoldState {
  if (f === 'collapsed') return 'semi';
  if (f === 'semi') return 'expanded';
  return 'collapsed';
}

const DOUBLE_CLICK_THRESHOLD = 220;

interface ToolCardProps {
  block: ContentBlock;
  resultBlock?: ContentBlock | null;
}

export function ToolCard({ block, resultBlock }: ToolCardProps) {
  const meta = extractToolMeta(block);
  const [fold, setFold] = useState<FoldState>(meta.defaultFold);
  const loading = !resultBlock;
  const status: 'loading' | 'done' | 'failed' = loading
    ? 'loading'
    : (isError(resultBlock!.output) ? 'failed' : 'done');

  // DESIGN.md §3: 角色色
  // - thinking 类别单独一种颜色
  // - 执行类（默认） execution
  // - 失败 failed
  // - 完成 done（默认 border-subtle，淡化）
  const role: 'execution' | 'thinking' | 'done' | 'error' =
    meta.category === 'thinking'
      ? (status === 'failed' ? 'error' : 'thinking')
      : status === 'failed' ? 'error'
        : status === 'loading' ? 'execution'
        : 'done';

  const borderColor = ROLE_BORDER[role];
  const titleColor = status === 'failed' ? 'var(--error)'
    : status === 'loading' ? 'var(--accent)'
    : role === 'thinking' ? 'var(--mauve)'
    : 'var(--border-subtle)';

  const clickTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const cancelPendingClick = () => {
    if (clickTimerRef.current !== null) {
      clearTimeout(clickTimerRef.current);
      clickTimerRef.current = null;
    }
  };

  const handleClick = () => {
    if (clickTimerRef.current !== null) return;
    clickTimerRef.current = setTimeout(() => {
      clickTimerRef.current = null;
      setFold(f => nextFold(f));
    }, DOUBLE_CLICK_THRESHOLD);
  };

  const handleDoubleClick = (e: React.MouseEvent) => {
    e.preventDefault();
    cancelPendingClick();
    setFold('expanded');
  };

  React.useEffect(() => () => cancelPendingClick(), []);

  const inputJson = JSON.stringify(block.args, null, 2);
  const resultFull = resultBlock ? formatToolResult(resultBlock.output) : '';
  const resultSummary = resultBlock ? summarizeResult(resultBlock.output, 3) : null;

  return (
    <div
      className={`tool-card tool-card-${role} tool-card-${status} tool-card-${fold}`}
      data-loading={status === 'loading'}
      style={{ borderLeftColor: borderColor }}
    >
      <div
        className={`tool-card-title ${status === 'loading' ? 'loading' : ''}`}
        onClick={handleClick}
        onDoubleClick={handleDoubleClick}
        style={{ color: status === 'loading' ? undefined : titleColor }}
      >
        <span className="tool-card-fold-icon">
          {fold === 'collapsed' ? PREFIX.selected : PREFIX.expanded}
        </span>
        <span className="tool-card-title-text">
          {STATUS_PREFIX[status]} {meta.title}
        </span>
        <span className={`tool-card-status tool-card-status-${status}`}>
          {status === 'done' && 'done'}
          {status === 'failed' && 'failed'}
        </span>
      </div>

      {fold !== 'collapsed' && (
        <div className="tool-card-body">
          {fold === 'expanded' ? (
            <div className="tool-card-section">
              <div className="tool-card-section-label">Input</div>
              <pre className="tool-card-pre">{inputJson}</pre>
            </div>
          ) : (
            meta.inputSummary && (
              <div className="tool-card-section">
                <div className="tool-card-section-label">Input</div>
                <pre className="tool-card-pre tool-card-pre-dim">{meta.inputSummary}</pre>
              </div>
            )
          )}

          {resultBlock && fold === 'expanded' && (
            <div className="tool-card-section">
              <div className="tool-card-section-label">Result</div>
              <pre className="tool-card-pre">{resultFull}</pre>
            </div>
          )}
          {resultBlock && fold === 'semi' && resultSummary && (
            <div className="tool-card-section">
              <div className="tool-card-section-label">Result</div>
              <pre className="tool-card-pre tool-card-pre-dim">{resultSummary.text}</pre>
              {resultSummary.truncated && (
                <div className="tool-card-truncated">+{resultSummary.totalLines - 3} more</div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function isError(output: any): boolean {
  if (output == null) return false;
  if (typeof output === 'object') {
    return !!output.error || (typeof output.ok === 'boolean' && output.ok === false);
  }
  if (typeof output === 'string') {
    return output.toLowerCase().startsWith('error') || output.toLowerCase().startsWith('failed');
  }
  return false;
}