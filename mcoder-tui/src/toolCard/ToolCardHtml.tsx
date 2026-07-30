// Desktop/Mobile 共享工具卡片组件（HTML）
// 三态折叠 + CSS 波浪流光 loading
// loading 由 resultBlock 缺失自动推导（不接收外部 loading prop）

import React, { useRef, useState } from 'react';
import {
  extractToolMeta, summarizeResult, formatToolResult,
  type ToolCategory, type FoldState,
} from './meta.js';
import type { ContentBlock } from '../rpc/types.js';

/** 类别 → CSS 变量名（边框色） */
const CATEGORY_BORDER: Record<ToolCategory, string> = {
  thinking: 'var(--mauve)',
  file: 'var(--blue)',
  command: 'var(--peach)',
  code: 'var(--peach)',
  graph: 'var(--green)',
  subagent: 'var(--teal)',
  plan: 'var(--yellow)',
  workflow: 'var(--mauve)',
  other: 'var(--overlay0)',
};

/** 三态循环 */
function nextFold(f: FoldState): FoldState {
  if (f === 'collapsed') return 'semi';
  if (f === 'semi') return 'expanded';
  return 'collapsed';
}

/** 单击/双击阈值（ms） */
const DOUBLE_CLICK_THRESHOLD = 220;

interface ToolCardProps {
  block: ContentBlock;
  resultBlock?: ContentBlock | null;
}

export function ToolCard({ block, resultBlock }: ToolCardProps) {
  const meta = extractToolMeta(block);
  const [fold, setFold] = useState<FoldState>(meta.defaultFold);
  const borderColor = CATEGORY_BORDER[meta.category];

  // loading 由 resultBlock 推导
  const loading = !resultBlock;
  const status: 'loading' | 'done' | 'failed' = loading
    ? 'loading'
    : (isError(resultBlock!.output) ? 'failed' : 'done');

  // 单击/双击区分：单击延迟触发，期间若发生双击则取消单击
  const clickTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const cancelPendingClick = () => {
    if (clickTimerRef.current !== null) {
      clearTimeout(clickTimerRef.current);
      clickTimerRef.current = null;
    }
  };

  const handleClick = () => {
    // 已有待执行的单击 → 视为双击的一部分，交给 handleDoubleClick 处理
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

  // 卸载时清理定时器，避免 setState on unmounted
  React.useEffect(() => () => cancelPendingClick(), []);

  const inputJson = JSON.stringify(block.args, null, 2);
  const resultFull = resultBlock ? formatToolResult(resultBlock.output) : '';
  const resultSummary = resultBlock ? summarizeResult(resultBlock.output, 3) : null;

  return (
    <div
      className={`tool-card tool-card-${meta.category} tool-card-${status} tool-card-${fold}`}
      style={{ borderLeftColor: borderColor }}
    >
      {/* 标题栏 */}
      <div
        className={`tool-card-title ${status === 'loading' ? 'loading' : ''}`}
        onClick={handleClick}
        onDoubleClick={handleDoubleClick}
        style={{ color: status === 'loading' ? undefined : borderColor }}
      >
        <span className="tool-card-fold-icon">
          {fold === 'collapsed' ? '▸' : '▾'}
        </span>
        <span className="tool-card-title-text">{meta.title}</span>
        <span className={`tool-card-status tool-card-status-${status}`}>
          {status === 'done' && 'done'}
          {status === 'failed' && 'failed'}
        </span>
      </div>

      {/* 内容区 */}
      {fold !== 'collapsed' && (
        <div className="tool-card-body">
          {/* 输入 */}
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

          {/* 结果 */}
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
                <div className="tool-card-truncated">... ({resultSummary.totalLines} lines total)</div>
              )}
            </div>
          )}
          {loading && (
            <div className="tool-card-running">running...</div>
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
