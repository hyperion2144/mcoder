// DESIGN.md §7.3: TUI 流动光效（逐字符扫描）
// 三档亮度模拟 RGB 渐变：gray（暗）→ white（亮）→ white bold（最亮）
//
// 帧间隔：80ms（12.5fps）
// 波形：sin(i / N * π * 2 + phase)，phase 每帧 +0.4
// 亮度区间：0.35 ~ 1.0
//
// 用法：所有正在执行的工具卡片标题、thinking 卡片首行

import React, { useState, useEffect } from 'react';
import { Text } from 'ink';
import { TUI_COLORS } from '../theme.js';

interface Props {
  text: string;
  /** 是否启动流光（false 时退化为静态渲染） */
  active?: boolean;
  /** 颜色调色板（默认 textPrimary → textMuted 三档） */
  highlight?: string;
  lowlight?: string;
}

export function ShimmerText({
  text,
  active = true,
  highlight = TUI_COLORS.textPrimary,
  lowlight = TUI_COLORS.textMuted,
}: Props) {
  const [phase, setPhase] = useState(0);

  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => setPhase((p) => p + 0.4), 80);
    return () => clearInterval(id);
  }, [active]);

  if (!text) return null;
  if (!active) return <Text>{text}</Text>;

  return (
    <Text>
      {text.split('').map((ch, i) => {
        const wave = Math.sin((i / Math.max(text.length, 1)) * Math.PI * 2 + phase);
        const brightness = 0.35 + 0.65 * Math.max(0, wave);
        // 三档亮度：gray（暗） / white（亮） / white bold（最亮）
        if (brightness < 0.5) {
          return <Text key={i} color={lowlight}>{ch}</Text>;
        }
        if (brightness < 0.85) {
          return <Text key={i} color={highlight}>{ch}</Text>;
        }
        return <Text key={i} color={highlight} bold>{ch}</Text>;
      })}
    </Text>
  );
}