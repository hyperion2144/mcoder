// 设计文档 §6.7: components/ModelView.tsx - 模型选择视图（底部 sheet）

import { useState, useEffect, useMemo } from 'react';
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

const THINKING_LEVELS = ['none', 'low', 'medium', 'high', 'max'] as const;

type ProviderNode = { provider: string; models: ModelInfo[] };
type TreeRow =
  | { kind: 'provider'; provider: string; count: number }
  | { kind: 'model'; model: ModelInfo; globalIndex: number };

function deriveProvider(m: ModelInfo): string {
  const name = (m.model || m.name || '').toLowerCase();
  if (name.startsWith('claude')) return 'anthropic';
  if (name.startsWith('gpt') || name.startsWith('o1') || name.startsWith('o3') || name.startsWith('o4') || name.startsWith('chatgpt')) return 'openai';
  if (name.startsWith('deepseek')) return 'deepseek';
  if (name.startsWith('gemini')) return 'google';
  if (name.startsWith('qwen')) return 'qwen';
  if (name.startsWith('llama') || name.startsWith('mistral') || name.startsWith('phi')) return 'ollama';
  if (m.protocol) return m.protocol;
  return 'other';
}

function formatCtx(cw?: number): string {
  if (!cw) return '';
  return cw >= 1000 ? `${Math.round(cw / 1000)}k ctx` : `${cw} ctx`;
}

export function ModelView({ client }: { client: WsClient | null }) {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [filter, setFilter] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [thinkingIndex, setThinkingIndex] = useState(2);
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

  // Group filtered models by provider, preserving insertion order
  const providerTree: ProviderNode[] = useMemo(() => {
    const order: string[] = [];
    const map = new Map<string, ModelInfo[]>();
    for (const m of filtered) {
      const p = deriveProvider(m);
      if (!map.has(p)) {
        map.set(p, []);
        order.push(p);
      }
      map.get(p)!.push(m);
    }
    return order.map(p => ({ provider: p, models: map.get(p)! }));
  }, [filtered]);

  // Flatten the tree into renderable rows, tracking the global model index
  const rows: TreeRow[] = useMemo(() => {
    const out: TreeRow[] = [];
    let gi = 0;
    for (const { provider, models: pmodels } of providerTree) {
      out.push({ kind: 'provider', provider, count: pmodels.length });
      for (const m of pmodels) {
        out.push({ kind: 'model', model: m, globalIndex: gi++ });
      }
    }
    return out;
  }, [providerTree]);

  const thinkingDepth = THINKING_LEVELS[thinkingIndex];

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

    if (key.tab) {
      setThinkingIndex(i => (i + 1) % THINKING_LEVELS.length);
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

  const blue = TUI_COLORS.brand;
  const muted = TUI_COLORS.textMuted;
  const mauve = TUI_COLORS.mauve;

  return (
    <Box flexDirection="column" borderStyle="round" borderColor={blue} paddingX={2} paddingY={1}>
      {/* Sheet header */}
      <Box flexDirection="column" marginBottom={1}>
        <Text bold color={blue}>{'MODEL SWITCHER'}</Text>
        <Text color={muted}>
          {'Current: '}
          <Text color={blue}>{currentModel || '-'}</Text>
          {` ${PREFIX.sep} `}
          <Text color={mauve}>{`thinking: ${thinkingDepth}`}</Text>
        </Text>
      </Box>

      {/* Models card */}
      <Box flexDirection="column" borderStyle="round" borderColor={muted} marginBottom={1}>
        <Box paddingLeft={1} paddingRight={1}>
          <Text color={muted}>models</Text>
          <Box flexGrow={1} />
          <Text color={muted}>provider tree</Text>
        </Box>
        <Box flexDirection="column" paddingLeft={1} paddingRight={1} paddingBottom={1}>
          {loading && <Text color={muted}>loading</Text>}
          {error && <Text color={TUI_COLORS.error}>{error}</Text>}
          {!loading && !error && filter.length > 0 && (
            <Text color={TUI_COLORS.warning}>{`filter ${PREFIX.sep} ${filter}`}</Text>
          )}
          {!loading && !error && filtered.length === 0 && (
            <Text color={muted}>no match</Text>
          )}
          {!loading && !error && rows.map(row => {
            if (row.kind === 'provider') {
              return (
                <Box key={`p-${row.provider}`}>
                  <Text color={blue}>{PREFIX.expanded}</Text>
                  <Text bold>{` ${row.provider}`}</Text>
                  <Text color={muted}>{` (${row.count})`}</Text>
                </Box>
              );
            }
            const m = row.model;
            const isSelected = row.globalIndex === selectedIndex;
            const isCurrent = m.name === currentModel;
            const markerColor = isSelected ? blue : muted;
            const nameColor = isSelected ? blue : undefined;
            const stateColor = isSelected ? blue : muted;
            return (
              <Box key={`m-${m.name}`}>
                <Text color={muted}>{PREFIX.branch}</Text>
                <Text color={markerColor}>{` ${isSelected ? PREFIX.dot : PREFIX.selected}`}</Text>
                <Text color={nameColor} bold={isSelected}>{` ${m.name}`}</Text>
                {isCurrent && <Text color={stateColor}>{' current'}</Text>}
                <Box flexGrow={1} />
                {m.context_window ? (
                  <Text color={muted}>{formatCtx(m.context_window)}</Text>
                ) : null}
              </Box>
            );
          })}
        </Box>
      </Box>

      {/* Thinking depth card */}
      <Box flexDirection="column" borderStyle="round" borderColor={muted} marginBottom={1}>
        <Box paddingLeft={1} paddingRight={1}>
          <Text color={muted}>thinking depth</Text>
          <Box flexGrow={1} />
          <Text color={muted}>5 levels</Text>
        </Box>
        <Box flexDirection="column" paddingLeft={1} paddingRight={1} paddingBottom={1}>
          {THINKING_LEVELS.map((level, i) => {
            const isSelected = i === thinkingIndex;
            const markerColor = isSelected ? blue : muted;
            const nameColor = isSelected ? blue : undefined;
            const stateColor = isSelected ? blue : muted;
            return (
              <Box key={level}>
                <Text color={markerColor}>{isSelected ? PREFIX.dot : PREFIX.selected}</Text>
                <Text color={nameColor} bold={isSelected}>{` ${level}`}</Text>
                {isSelected && <Text color={stateColor}>{' current'}</Text>}
              </Box>
            );
          })}
        </Box>
      </Box>

      {/* Sheet dock */}
      <Box>
        <Text color={muted}>
          {`[↑↓] navigate ${PREFIX.sep} [Tab] switch thinking ${PREFIX.sep} [Enter] select ${PREFIX.sep} [Esc] close`}
        </Text>
      </Box>
    </Box>
  );
}
