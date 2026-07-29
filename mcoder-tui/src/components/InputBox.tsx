// 设计文档 §6.3: components/InputBox.tsx - 输入框
// 支持：多行输入、@文件补全、历史记录导航

import { Box, Text, useInput } from 'ink';
import TextInput from 'ink-text-input';
import { useState, useEffect } from 'react';
import { useMessagesStore, useUiStore } from '../store/index.js';

interface Props {
  value: string;
  onChange: (v: string) => void;
  onSubmit: (v: string) => void;
}

export function InputBox({ value, onChange, onSubmit }: Props) {
  const { navigateHistory, addInputHistory, resetHistory } = useMessagesStore();
  const { setFileCompletions, navigateFileCompletion, fileCompletions, fileCompletionIndex } = useUiStore();
  const [showFileCompletions, setShowFileCompletions] = useState(false);

  // 设计文档 §6.8: @ 触发文件路径补全
  useEffect(() => {
    const lastAt = value.lastIndexOf('@');
    if (lastAt >= 0 && lastAt === value.length - 1) {
      // 用户刚输入 @，触发补全
      // 简化实现：通过 bash 工具获取文件列表
      // 实际由 App 层处理 RPC 调用
      setShowFileCompletions(true);
    } else {
      setShowFileCompletions(false);
    }
  }, [value]);

  useInput((input: string, key: any) => {
    // 设计文档 §6.8: 上下箭头切换历史输入
    if (key.upArrow) {
      const histVal = navigateHistory('up');
      if (histVal !== null) onChange(histVal);
    } else if (key.downArrow) {
      const histVal = navigateHistory('down');
      if (histVal !== null) onChange(histVal);
    } else if (key.return) {
      if (value.trim()) {
        addInputHistory(value);
      }
    }
  });

  return (
    <Box flexDirection="column">
      {/* 文件补全列表 */}
      {showFileCompletions && fileCompletions && fileCompletions.length > 0 && (
        <Box flexDirection="column" paddingX={1}>
          <Text color="gray" italic>File completions:</Text>
          {fileCompletions.slice(0, 5).map((f, i) => (
            <Text key={i} color={i === fileCompletionIndex ? 'cyan' : 'gray'}>
              {i === fileCompletionIndex ? '▸ ' : '  '}{f}
            </Text>
          ))}
        </Box>
      )}
      <Box paddingX={1}>
        <Text color="cyan">{'>'} </Text>
        <TextInput
          value={value}
          onChange={onChange}
          onSubmit={onSubmit}
          placeholder="type a message or /help for commands"
        />
      </Box>
    </Box>
  );
}
