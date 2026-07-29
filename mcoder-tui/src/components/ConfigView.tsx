// 设计文档 §6.7: components/ConfigView.tsx - 配置视图

import { Box, Text } from 'ink';
import { useSessionStore } from '../store/index.js';

export function ConfigView() {
  const { currentModel, currentRole, projectPath, gitBranch, contextUsed, contextWindow } = useSessionStore();
  return (
    <Box flexDirection="column" paddingX={1} borderStyle="single">
      <Text bold color="cyan">Configuration</Text>
      <Text color="white">Model: <Text color="blue">{currentModel || '(not set)'}</Text></Text>
      <Text color="white">Role: <Text color="magenta">{currentRole}</Text></Text>
      <Text color="white">Project: <Text color="green">{projectPath || '(unknown)'}</Text></Text>
      <Text color="white">Branch: <Text color="green">{gitBranch || '(unknown)'}</Text></Text>
      <Text color="white">Context: {contextUsed}/{contextWindow}</Text>
      <Text color="gray" italic>press ESC to close</Text>
    </Box>
  );
}
