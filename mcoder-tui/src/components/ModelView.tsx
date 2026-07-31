// 设计文档 §6.7: components/ModelView.tsx - 模型选择视图（交互式 picker）

import { useState, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';
import { useSessionStore } from '../store/session.js';
import { useUiStore } from '../store/ui.js';
import type { WsClient } from '../rpc/client.js';
import { TUI_COLORS, PREFIX } from '../theme.js';

interface ModelInfo {
  name: string;
  model?: string;
  context_window?: number;
  protocol?: string;
}

export function ModelView({ client }: { client: WsClient | null }) {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [filter, setFilter] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const sessionStore = useSessionStore();
  const uiStore = useUiStore();
  const currentModel = sessionStore.currentModel;

  useEffect(() => {
    if (!client) return;
    client.request('config.list_models', {})
      .then((result: any) => {
        const list = (result?.models || result || []) as ModelInfo[];
        setModels(list);
        setLoading(false);
      })
      .catch((e: any) => {
        setError(e.message);
        setLoading(false);
      });
  }, [client]);

  const filtered = models.filter(m =>
    m.name.toLowerCase().includes(filter.toLowerCase())
  );

  // Reset selection when filter changes
  useEffect(() => {
    setSelectedIndex(0);
  }, [filter]);

  useInput((input, key) => {
    if (loading || error) {
      if (key.escape || key.return) {
        uiStore.setView('chat');
      }
      return;
    }

    if (key.escape) {
      uiStore.setView('chat');
      return;
    }

    if (key.upArrow) {
      setSelectedIndex(i => Math.max(0, i - 1));
      return;
    }

    if (key.downArrow) {
      setSelectedIndex(i => Math.min(filtered.length - 1, i + 1));
      return;
    }

    if (key.return) {
      const selected = filtered[selectedIndex];
      if (selected && client && sessionStore.currentSessionId) {
        client.request('session.model.set', {
          session_id: sessionStore.currentSessionId,
          model: selected.name,
        }).then(() => {
          sessionStore.setModel(selected.name);
          uiStore.setView('chat');
        }).catch((e: any) => {
          setError(e.message);
        });
      }
      return;
    }

    // Backspace: remove last char from filter
    if (key.backspace || key.delete) {
      setFilter(f => f.slice(0, -1));
      return;
    }

    // Typing: add to filter
    if (input && !key.ctrl && !key.meta && input.length === 1 && input >= ' ') {
      setFilter(f => f + input);
    }
  });

  return (
    <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.textMuted} paddingX={1}>
      <Box marginBottom={1}>
        <Text bold color={TUI_COLORS.accent}>Switch Model</Text>
        <Text color={TUI_COLORS.textMuted}>{` ${PREFIX.sep} type to filter ${PREFIX.sep} ↑↓ select ${PREFIX.sep} Enter switch ${PREFIX.sep} Esc cancel`}</Text>
      </Box>

      {loading && <Text color={TUI_COLORS.textMuted}>loading</Text>}
      {error && <Text color={TUI_COLORS.error}>{error}</Text>}

      {!loading && !error && (
        <>
          {filter && (
            <Box marginBottom={1}>
              <Text color={TUI_COLORS.warning}>{`filter ${PREFIX.sep} ${filter}`}</Text>
            </Box>
          )}
          <Box flexDirection="column">
            {filtered.length === 0 ? (
              <Text color={TUI_COLORS.textMuted}>no match</Text>
            ) : (
              filtered.map((m, i) => (
                <Box key={m.name}>
                  <Text color={i === selectedIndex ? TUI_COLORS.accent : TUI_COLORS.textMuted} bold={i === selectedIndex}>
                    {i === selectedIndex ? `${PREFIX.running} ` : '  '}
                    {m.name === currentModel ? `${PREFIX.done} ` : '  '}
                    {m.name}
                  </Text>
                  {m.context_window ? (
                    <Text color={TUI_COLORS.textMuted}>{` ${PREFIX.sep} ctx=${m.context_window > 1000 ? `${m.context_window / 1000}k` : m.context_window}`}</Text>
                  ) : null}
                  {m.protocol ? (
                    <Text color={TUI_COLORS.textMuted}>{` ${PREFIX.sep} [${m.protocol}]`}</Text>
                  ) : null}
                </Box>
              ))
            )}
          </Box>
        </>
      )}
    </Box>
  );
}
