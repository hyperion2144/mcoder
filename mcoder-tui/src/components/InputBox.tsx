// DESIGN.md §4 / §10: InputBox（输入栏）
// - 移除：italic File completions、>' prefix
// - 状态栏：单一 prompt 前缀 `▸`

import { Box, Text, useInput } from 'ink';
import TextInput from 'ink-text-input';
import { useState, useEffect } from 'react';
import { useMessagesStore, useUiStore } from '../store/index.js';
import { useAskStore } from '../ask/store.js';
import { TUI_COLORS, PREFIX } from '../theme.js';

interface Props {
  value: string;
  onChange: (v: string) => void;
  onSubmit: (v: string) => void;
  placeholder?: string;
}

export function InputBox({ value, onChange, onSubmit, placeholder }: Props) {
  const { navigateHistory, addInputHistory } = useMessagesStore();
  const { setFileCompletions, fileCompletions, fileCompletionIndex } = useUiStore();
  const askStore = useAskStore();
  const [showFileCompletions, setShowFileCompletions] = useState(false);

  useEffect(() => {
    const lastAt = value.lastIndexOf('@');
    if (lastAt >= 0 && lastAt === value.length - 1) {
      setShowFileCompletions(true);
    } else {
      setShowFileCompletions(false);
    }
  }, [value]);

  useInput((input: string, key: any) => {
    if (key.upArrow) {
      const histVal = navigateHistory('up');
      if (histVal !== null) onChange(histVal);
    } else if (key.downArrow) {
      const histVal = navigateHistory('down');
      if (histVal !== null) onChange(histVal);
    } else if (key.return) {
      const sid = (askStore as any);
      const askMode = Object.values(sid.askInputMode || {}).some(Boolean);
      if (askMode) return;
      if (value.trim()) {
        addInputHistory(value);
      }
    }
  });

  return (
    <Box flexDirection="column">
      {showFileCompletions && fileCompletions && fileCompletions.length > 0 && (
        <Box flexDirection="column" paddingX={1}>
          <Text color={TUI_COLORS.textMuted}>File completions</Text>
          {fileCompletions.slice(0, 5).map((f, i) => (
            <Text key={i} color={i === fileCompletionIndex ? TUI_COLORS.accent : TUI_COLORS.textMuted}>
              {i === fileCompletionIndex ? PREFIX.running + ' ' : '  '}{f}
            </Text>
          ))}
        </Box>
      )}
      <Box paddingX={1}>
        <Text color={TUI_COLORS.accent}>{PREFIX.pending} </Text>
        <TextInput
          value={value}
          onChange={onChange}
          onSubmit={onSubmit}
          placeholder={placeholder || 'type a message · /help for commands'}
        />
      </Box>
    </Box>
  );
}