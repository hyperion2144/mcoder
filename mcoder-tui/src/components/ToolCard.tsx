// mcoder UI Redesign v2 - ToolCard
// Layout: tool-head (name | file | fill | status) + tool-body (content) + usage-line
// Status: running (shimmer), done (✓), failed (✗)
// Bash results get colored borders: success (green), warn (yellow), fail (red)

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
  resultBlock?: ContentBlock | null;
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

function isBashSuccess(output: any): boolean {
  if (output == null || typeof output !== 'object') return false;
  return output.exit_code === 0 && !output.error;
}

function isBashFail(output: any): boolean {
  if (output == null || typeof output !== 'object') return false;
  return output.exit_code !== undefined && output.exit_code !== 0;
}

export function ToolCard({ block, resultBlock }: ToolCardProps) {
  const meta = extractToolMeta(block);
  const fold = meta.defaultFold;
  const loading = !resultBlock;
  const status = loading ? 'running' : (isError(resultBlock!.output) ? 'failed' : 'done');

  const role = status === 'running' ? ROLE_COLOR.execution
    : status === 'failed' ? ROLE_COLOR.error
    : ROLE_COLOR.done;

  const prefix = status === 'running' ? PREFIX.running
    : status === 'failed' ? PREFIX.failed
    : PREFIX.done;

  const title = `${prefix} ${meta.title}`;

  // Bash result coloring (for visual status, not used as border in TUI)
  const bashStatus = !loading && meta.title.startsWith('bash')
    ? (isBashFail(resultBlock!.output) ? 'fail'
       : isBashSuccess(resultBlock!.output) ? 'success'
       : 'neutral')
    : 'neutral';

  return (
    <Box flexDirection="column" marginLeft={2} marginY={0}>
      {/* Tool head: name | file | fill | status */}
      <Box>
        <Text color={role}>│ </Text>
        {status === 'running'
          ? <ShimmerText text={title} />
          : <Text color={role}>{title}</Text>}
      </Box>

      {fold !== 'collapsed' && (
        <Box flexDirection="column" marginLeft={4}>
          {/* Input summary */}
          {fold !== 'expanded' && meta.inputSummary && (
            <Text color={TUI_COLORS.textMuted}>{meta.inputSummary}</Text>
          )}
          {fold === 'expanded' && (
            <Box flexDirection="column">
              <Text color={TUI_COLORS.textMuted}>Input</Text>
              <Text color={TUI_COLORS.textPrimary}>{JSON.stringify(block.args, null, 2)}</Text>
            </Box>
          )}

          {/* Result */}
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
