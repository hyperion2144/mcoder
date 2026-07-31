// DESIGN.md §3 / §7: 工具卡片（执行类）
// - 单一颜色：execution（accent/cyan）；thinking（mauve）单独角色
// - 标题：loading 时 ShimmerText 流光；完成后静态
// - 移除：6 种分类色、── Input ── 分隔符、running... 文本

import React from 'react';
import { Box, Text } from 'ink';
import {
  extractToolMeta, summarizeResult, formatToolResult,
  type FoldState,
} from '../toolCard/meta.js';
import type { ContentBlock } from '../rpc/types.js';
import { TUI_COLORS, ROLE_COLOR, PREFIX } from '../theme.js';
import { ShimmerText } from './ShimmerText.js';

interface ToolCardProps {
  block: ContentBlock;
  /** 工具结果（同 id 的 tool_result block）；null 表示仍在执行 */
  resultBlock?: ContentBlock | null;
}

/** 折叠指示符 */
function foldIcon(f: FoldState): string {
  return f === 'collapsed' ? PREFIX.pending : PREFIX.expanded;
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

export function ToolCard({ block, resultBlock }: ToolCardProps) {
  const meta = extractToolMeta(block);
  const fold = meta.defaultFold;
  const loading = !resultBlock;
  const status = loading ? 'running' : (isError(resultBlock!.output) ? 'failed' : 'done');

  // DESIGN.md §3: 角色色
  // - thinking 类别：mauve
  // - 执行类工具（默认）：accent
  // - 错误状态：error
  // - 已完成：textMuted
  const role = status === 'running' ? ROLE_COLOR.execution
    : status === 'failed' ? ROLE_COLOR.error
    : ROLE_COLOR.done;

  // DESIGN.md §6.1: 状态前缀
  const prefix = status === 'running' ? PREFIX.running
    : status === 'failed' ? PREFIX.failed
    : PREFIX.done;

  const title = `${prefix} ${meta.title}`;

  return (
    <Box flexDirection="column" marginLeft={2} marginY={0}>
      <Box>
        <Text color={role}>│ </Text>
        {status === 'running'
          ? <ShimmerText text={title} />
          : <Text color={role}>{title}</Text>}
      </Box>

      {fold !== 'collapsed' && (
        <Box flexDirection="column" marginLeft={4}>
          {/* DESIGN.md §6 / §8.1: Input 区块（小字标题 + 1px 分割线，不要 ──）*/}
          {fold === 'expanded' ? (
            <Box flexDirection="column">
              <Text color={TUI_COLORS.textMuted}>Input</Text>
              <Text color={TUI_COLORS.textPrimary}>{JSON.stringify(block.args, null, 2)}</Text>
            </Box>
          ) : (
            meta.inputSummary && (
              <Text color={TUI_COLORS.textMuted}>{meta.inputSummary}</Text>
            )
          )}

          {/* Result 区块 */}
          {resultBlock && fold === 'expanded' && (
            <Box flexDirection="column" marginTop={1}>
              <Text color={TUI_COLORS.textMuted}>Result</Text>
              <Text color={TUI_COLORS.textPrimary}>{formatToolResult(resultBlock.output)}</Text>
            </Box>
          )}
          {resultBlock && fold === 'semi' && (
            <Box flexDirection="column">
              {(() => {
                const summary = summarizeResult(resultBlock.output, 3);
                return (
                  <>
                    <Text color={TUI_COLORS.textMuted}>{summary.text}</Text>
                    {summary.truncated && (
                      <Text color={TUI_COLORS.textMuted}>+{summary.totalLines - 3} more</Text>
                    )}
                  </>
                );
              })()}
            </Box>
          )}
        </Box>
      )}
    </Box>
  );
}