// 设计文档 §6.3: components/ProjectLine.tsx - 项目上下文条
// ~/projects/myapp · main · 3 files changed

import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';
import { shortenPath, formatContext, formatCost } from '../utils/format.js';

export function ProjectLine() {
  const { projectPath, gitBranch, filesChanged } = useSessionStore();
  if (!projectPath && !gitBranch && filesChanged === 0) return null;
  const shortPath = shortenPath(projectPath);
  return (
    <Box paddingX={1}>
      <Text color="gray">
        {shortPath && <Text color="white">{shortPath}</Text>}
        {gitBranch && <> · <Text color="green">{gitBranch}</Text></>}
        {filesChanged > 0 && <> · <Text color="yellow">{filesChanged} files changed</Text></>}
      </Text>
    </Box>
  );
}

/// 设计文档 §6.5: 紧凑模式 - 合并 ContextLine + ProjectLine 为一行
/// mcoder · 重构登录模块 · ~/projects/myapp · main · plan · gpt-4o · 12.4k/128k · $0.03
export function CompactLine() {
  const {
    currentSessionTitle, currentRole, currentModel,
    contextUsed, contextWindow, sessionCost,
    projectPath, gitBranch, filesChanged,
  } = useSessionStore();
  const shortPath = shortenPath(projectPath);
  const contextStr = formatContext(contextUsed, contextWindow);
  const costStr = formatCost(sessionCost);
  return (
    <Box paddingX={1}>
      <Text color="gray">
        <Text color="cyan" bold>mcoder</Text>
        {currentSessionTitle && ` · ${currentSessionTitle}`}
        {shortPath && <> · <Text color="white">{shortPath}</Text></>}
        {gitBranch && <> · <Text color="green">{gitBranch}</Text></>}
        {filesChanged > 0 && <> · <Text color="yellow">{filesChanged} changed</Text></>}
        {' · '}
        <Text color="magenta">{currentRole}</Text>
        {currentModel && <> · <Text color="blue">{currentModel}</Text></>}
        {` · ${contextStr}`}
        {costStr && ` · ${costStr}`}
      </Text>
    </Box>
  );
}

