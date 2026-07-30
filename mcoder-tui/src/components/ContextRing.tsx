// 共享 ContextRing 组件（SVG 圆环，仅 Web 端使用：Desktop / Mobile）
// 显示上下文使用率，按阈值变色：
//   <70%  green  (#a6e3a1)
//   70-90 yellow (#f9e2af)
//   >90%  red    (#f38ba8)
//
// 用法：<ContextRing used={12345} window={128000} size={28} />
// 默认 size=24，strokeWidth=3

import React from 'react';

export interface ContextRingProps {
  used: number;
  window: number;
  size?: number;
  strokeWidth?: number;
  /** 可选：自定义阈值（百分比 0-100），默认 70/90 */
  thresholds?: { green: number; yellow: number };
  /** 可选：显示在圆环中心的文本（默认百分比数字） */
  label?: (pct: number) => string;
}

export function ContextRing({
  used,
  window: ctxWindow,
  size = 24,
  strokeWidth = 3,
  thresholds = { green: 70, yellow: 90 },
  label,
}: ContextRingProps) {
  const pctNum = ctxWindow > 0 ? Math.min(100, (used / ctxWindow) * 100) : 0;
  const pct = Math.round(pctNum);

  const color = pctNum < thresholds.green
    ? '#a6e3a1' // green
    : pctNum <= thresholds.yellow
      ? '#f9e2af' // yellow
      : '#f38ba8'; // red

  const r = (size - strokeWidth) / 2;
  const c = 2 * Math.PI * r;
  const dash = (pctNum / 100) * c;

  const centerText = label ? label(pct) : `${pct}`;

  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${size} ${size}`}
      style={{ display: 'inline-block', verticalAlign: 'middle' }}
      role="img"
      aria-label={`context ${pct}%`}
    >
      <circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill="none"
        stroke="rgba(255,255,255,0.12)"
        strokeWidth={strokeWidth}
      />
      <circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill="none"
        stroke={color}
        strokeWidth={strokeWidth}
        strokeDasharray={`${dash} ${c - dash}`}
        strokeLinecap="round"
        transform={`rotate(-90 ${size / 2} ${size / 2})`}
        style={{ transition: 'stroke-dasharray 0.3s ease, stroke 0.3s ease' }}
      />
      {size >= 28 && (
        <text
          x="50%"
          y="50%"
          textAnchor="middle"
          dominantBaseline="central"
          fontSize={size * 0.32}
          fill={color}
          style={{ fontWeight: 600, userSelect: 'none' }}
        >
          {centerText}
        </text>
      )}
    </svg>
  );
}

export default ContextRing;
