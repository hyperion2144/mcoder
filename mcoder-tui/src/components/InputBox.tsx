// mcoder UI Redesign v2 - InputBox (bottom dock input area)
// Layout: session-label (floating) > session-info (model|thinking|mode|path|branch + usage) > input-area (> prompt + hints)

import { Box, Text, useInput } from 'ink';
import TextInput from 'ink-text-input';
import { useState, useEffect } from 'react';
import { useMessagesStore, useUiStore, useSessionStore } from '../store/index.js';
import { useAskStore } from '../ask/store.js';
import { TUI_COLORS, PREFIX } from '../theme.js';
import { t } from '../i18n.js';
import { formatContext, formatCost } from '../utils/format.js';

interface Props {
  value: string;
  onChange: (v: string) => void;
  onSubmit: (v: string) => void;
  placeholder?: string;
  isActive?: boolean;
}

export function InputBox({ value, onChange, onSubmit, placeholder, isActive = true }: Props) {
  const { navigateHistory, addInputHistory } = useMessagesStore();
  const { setFileCompletions, fileCompletions, fileCompletionIndex } = useUiStore();
  const askStore = useAskStore();
  const sessionStore = useSessionStore();
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
  }, { isActive });

  const ctxStr = formatContext(sessionStore.contextUsed, sessionStore.contextWindow);
  const costStr = formatCost(sessionStore.sessionCost);
  const thinkingDepth = ''; // placeholder; thinking is session-level

  return (
    <Box flexDirection="column" borderStyle="round" borderColor={TUI_COLORS.textMuted} marginTop={1}>
      {/* Session info row */}
      <Box paddingX={1} flexDirection="column">
        {/* Primary: model | thinking | mode | path | branch */}
        <Box>
          {sessionStore.currentModel && (
            <Text color={TUI_COLORS.accent}>{sessionStore.currentModel}</Text>
          )}
          <Text color={TUI_COLORS.textMuted}> {PREFIX.sep} </Text>
          <Text color={sessionStore.currentRole !== 'default' ? TUI_COLORS.mauve : TUI_COLORS.textMuted}>
            {sessionStore.currentRole}
          </Text>
          {sessionStore.projectPath && (
            <>
              <Text color={TUI_COLORS.textMuted}> {PREFIX.sep} </Text>
              <Text color={TUI_COLORS.cyan}>{sessionStore.projectPath}</Text>
            </>
          )}
          {sessionStore.gitBranch && (
            <>
              <Text color={TUI_COLORS.textMuted}> {PREFIX.sep} </Text>
              <Text color={TUI_COLORS.textMuted}>{sessionStore.gitBranch}</Text>
            </>
          )}
        </Box>
        {/* Secondary: usage | cost | streaming */}
        <Box>
          <Text color={TUI_COLORS.textMuted}>{ctxStr}</Text>
          {costStr && <Text color={TUI_COLORS.textMuted}> {PREFIX.sep} {costStr}</Text>}
          {sessionStore.loopState === 'running' && (
            <Text color={TUI_COLORS.success}> {PREFIX.sep} streaming</Text>
          )}
        </Box>
      </Box>

      {/* File completions */}
      {showFileCompletions && fileCompletions && fileCompletions.length > 0 && (
        <Box flexDirection="column" paddingX={1}>
          <Text color={TUI_COLORS.textMuted}>File completions</Text>
          {fileCompletions.slice(0, 5).map((f, i) => (
            <Text key={i} color={i === fileCompletionIndex ? TUI_COLORS.accent : TUI_COLORS.textMuted}>
              {i === fileCompletionIndex ? `${PREFIX.running} ` : '  '}{f}
            </Text>
          ))}
        </Box>
      )}

      {/* Input area */}
      <Box paddingX={1}>
        <Text color={TUI_COLORS.accent}>{'>'} </Text>
        <TextInput
          value={value}
          onChange={onChange}
          onSubmit={onSubmit}
          placeholder={placeholder || t('ui.send_message_shift')}
        />
      </Box>

      {/* Hints */}
      <Box paddingX={1}>
        <Text color={TUI_COLORS.textMuted}>
          [Enter] send  [Shift+Enter] newline  [/] commands  [@] files
        </Text>
      </Box>
    </Box>
  );
}
