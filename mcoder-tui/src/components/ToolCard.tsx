// TUI 工具卡片组件（纯渲染，无交互）
// 三态折叠：collapsed / semi / expanded（按工具默认 fold 显示）
// loading 时标题逐字符波浪流光（银白色）
// 背景透明（继承终端）
// TUI 无鼠标，不做交互折叠（设计文档约定）

import React, { useState, useEffect } from 'react';
import { Box, Text } from 'ink';
import {
  extractToolMeta, summarizeResult, formatToolResult,
  type ToolCategory, type FoldState,
} from '../toolCard/meta.js';
import type { ContentBlock } from '../rpc/types.js';

/** 类别 → 边框色（TUI 前景色） */
const CATEGORY_COLOR: Record<ToolCategory, string> = {
  thinking: 'magenta',
  file: 'blue',
  command: 'yellow',
  code: 'yellow',
  graph: 'green',
  subagent: 'cyan',
  plan: 'yellow',
  workflow: 'magenta',
  other: 'gray',
};

/** 折叠指示符 */
function foldIcon(f: FoldState): string {
  if (f === 'collapsed') return '▸';
  return '▾';
}

/** 逐字符波浪流光文字 */
function ShimmerText({ text }: { text: string }) {
  const [phase, setPhase] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setPhase(p => p + 0.4), 80);
    return () => clearInterval(id);
  }, []);

  if (!text) return null;
  return (
    <Text>
      {text.split('').map((ch, i) => {
        const wave = Math.sin((i / Math.max(text.length, 1)) * Math.PI * 2 + phase);
        const brightness = 0.35 + 0.65 * Math.max(0, wave);
        const r = Math.round(160 * brightness + 40);
        const g = Math.round(160 * brightness + 40);
        const b = Math.round(190 * brightness + 50);
        return (
          <Text key={i} color={`rgb(${r},${g},${b})`}>{ch}</Text>
        );
      })}
    </Text>
  );
}

interface ToolCardProps {
  block: ContentBlock;
  /** 工具结果（同 id 的 tool_result block）；null 表示仍在执行 */
  resultBlock?: ContentBlock | null;
}

export function ToolCard({ block, resultBlock }: ToolCardProps) {
  const meta = extractToolMeta(block);
  const fold = meta.defaultFold;
  const loading = !resultBlock;
  const borderColor = CATEGORY_COLOR[meta.category];
  const status = loading ? 'loading' : (isError(resultBlock!.output) ? 'failed' : 'done');

  // 标题行
  const titleNode = status === 'loading' ? (
    <ShimmerText text={`${foldIcon(fold)} ${meta.title}`} />
  ) : (
    <Text color={borderColor}>
      {foldIcon(fold)} {meta.title}
      <Text color={status === 'done' ? 'green' : 'red'}>
        {' '}{status === 'done' ? 'done' : 'failed'}
      </Text>
    </Text>
  );

  return (
    <Box flexDirection="column" marginLeft={2} marginY={0}>
      <Box>
        <Text color={borderColor}>│ </Text>
        {titleNode}
      </Box>

      {fold !== 'collapsed' && (
        <Box flexDirection="column" marginLeft={4}>
          {/* 输入区 */}
          {fold === 'expanded' ? (
            <Box flexDirection="column">
              <Text color="gray" dimColor>── Input ──</Text>
              <Text color="white">{JSON.stringify(block.args, null, 2)}</Text>
            </Box>
          ) : (
            meta.inputSummary && (
              <Text color="gray" dimColor>{meta.inputSummary}</Text>
            )
          )}

          {/* 结果区 */}
          {resultBlock && fold === 'expanded' && (
            <Box flexDirection="column" marginTop={1}>
              <Text color="gray" dimColor>── Result ──</Text>
              <Text color="white">{formatToolResult(resultBlock.output)}</Text>
            </Box>
          )}
          {resultBlock && fold === 'semi' && (
            <Box flexDirection="column">
              {(() => {
                const summary = summarizeResult(resultBlock.output, 3);
                return (
                  <>
                    <Text color="gray" dimColor>{summary.text}</Text>
                    {summary.truncated && (
                      <Text color="gray" dimColor>... ({summary.totalLines} lines total)</Text>
                    )}
                  </>
                );
              })()}
            </Box>
          )}
          {loading && (
            <Text color="gray" dimColor italic>running...</Text>
          )}
        </Box>
      )}
    </Box>
  );
}

/** 判断结果是否为错误 */
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
