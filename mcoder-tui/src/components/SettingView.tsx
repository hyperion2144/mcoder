// 设计文档 §6.7: components/SettingView.tsx - 交互式设置面板 (prototype redesign)
// Full-screen settings view: header-bar + title + sidebar/settings-pane grid.
// All RPC logic (config.get/set, set_language, session.mode.set, model picker)
// and keyboard handling (navigate / edit / toggle) preserved from the original.

import React, { useState, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';
import { useSessionStore } from '../store/session.js';
import { useUiStore } from '../store/ui.js';
import type { WsClient } from '../rpc/client.js';
import { TUI_COLORS, PREFIX } from '../theme.js';
import { t, getLang, setLang } from '../i18n.js';

interface SettingItem {
  key: string;
  label: string;
  description: string;
  type: 'bool' | 'number' | 'float' | 'text' | 'readonly' | 'role';
  value: string;
  rpcKey?: string;  // for config.set RPC
}

// Prototype sidebar categories (General is the active pane shown).
const CATEGORIES = [
  'General', 'Providers', 'Models', 'Permissions', 'Hooks',
  'MCP Servers', 'Memory & Context', 'Sub-agents', 'Workflow', 'Display', 'Updates',
];

const KEYBINDS: [string, string][] = [
  ['Ctrl+C', 'quit'], ['Ctrl+S', 'sessions'], ['Ctrl+T', 'todos'], ['Ctrl+K', 'tasks'],
  ['Ctrl+,', 'config'], ['Ctrl+R', 'resume'], ['/', 'cmd'], ['@', 'files'], ['?', 'help'],
];

const PERMISSION_OPTIONS = [
  { name: 'YOLO', note: '(auto-approve all)' },
  { name: 'Standard', note: '(approve writes - recommended)', active: true },
  { name: 'Strict', note: '(approve everything)' },
];

const THEME_OPTIONS = [
  { name: 'Catppuccin Mocha' },
  { name: 'Tokyo Night', active: true },
  { name: 'Tokyo Night Storm' },
  { name: 'Gruvbox Dark' },
];

export function SettingView({ client }: { client: WsClient | null }) {
  const sessionStore = useSessionStore();
  const uiStore = useUiStore();
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [editing, setEditing] = useState(false);
  const [editValue, setEditValue] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);

  const sid = sessionStore.currentSessionId;

  // Fetch current config values on mount (preserved)
  const [configValues, setConfigValues] = useState<Record<string, any>>({});
  useEffect(() => {
    if (!client || !sid) return;
    const keys = ['loop_max_iters', 'compact', 'memory'];
    Promise.all(keys.map(k =>
      client.request('config.get', { key: k }).catch(() => null)
    )).then(([iters, compact, memory]) => {
      setConfigValues({
        loop_max_iters: iters,
        compact: compact,
        memory: memory,
      });
    });
  }, [client, sid]);

  // Navigable settings items (ordered to match the visual section layout).
  const settings: SettingItem[] = [
    { key: 'project', label: t('ui.project_path'), description: '', type: 'readonly', value: sessionStore.projectPath || '-' },
    { key: 'version', label: t('ui.version'), description: '', type: 'readonly', value: sessionStore.version || '-' },
    { key: 'lsp', label: t('ui.lsp_servers'), description: '', type: 'readonly', value: sessionStore.lspServers.join(', ') || 'none' },
    { key: 'model', label: t('ui.model'), description: t('ui.model_desc'), type: 'text', value: sessionStore.currentModel || '-' },
    { key: 'role', label: t('ui.role'), description: t('ui.role_desc'), type: 'role', value: sessionStore.currentRole || 'default' },
    { key: 'loop_max_iters', label: t('ui.max_iterations'), description: t('ui.max_iterations_desc'), type: 'number', value: String(configValues.loop_max_iters ?? '?'), rpcKey: 'loop_max_iters' },
    { key: 'compact.threshold', label: t('ui.compact_threshold'), description: t('ui.compact_threshold_desc'), type: 'float', value: String(configValues.compact?.threshold ?? '?'), rpcKey: 'compact.threshold' },
    { key: 'compact.keep_recent', label: t('ui.compact_keep_recent'), description: t('ui.compact_keep_recent_desc'), type: 'number', value: String(configValues.compact?.keep_recent ?? '?'), rpcKey: 'compact.keep_recent' },
    { key: 'memory.auto_recall', label: t('ui.memory_auto_recall'), description: t('ui.memory_auto_recall_desc'), type: 'bool', value: String(configValues.memory?.auto_recall ?? '?'), rpcKey: 'memory.auto_recall' },
    { key: 'memory.auto_capture', label: t('ui.memory_auto_capture'), description: t('ui.memory_auto_capture_desc'), type: 'bool', value: String(configValues.memory?.auto_capture ?? '?'), rpcKey: 'memory.auto_capture' },
    { key: 'language', label: t('ui.language'), description: '', type: 'readonly', value: getLang() === 'zh' ? '中文' : 'English' },
  ];

  // Keyboard handling (preserved verbatim from original).
  useInput((input, key) => {
    if (editing) {
      // Editing mode: type new value
      if (key.escape) {
        setEditing(false);
        return;
      }
      if (key.return) {
        // Save the value
        const item = settings[selectedIndex];
        if (item.rpcKey && client && sid) {
          let value: any = editValue;
          if (item.type === 'bool') value = editValue === 'true' || editValue === '1';
          else if (item.type === 'number') value = parseInt(editValue);
          else if (item.type === 'float') value = parseFloat(editValue);

          client.request('config.set', { key: item.rpcKey, value })
            .then(() => {
              setSuccess(`${item.label} updated to ${editValue}`);
              setError(null);
              // Refresh config
              setConfigValues(prev => {
                const next = { ...prev };
                if (item.rpcKey === 'loop_max_iters') next.loop_max_iters = value;
                else if (item.rpcKey?.startsWith('compact.')) {
                  next.compact = { ...next.compact, [item.rpcKey.split('.')[1]]: value };
                }
                else if (item.rpcKey?.startsWith('memory.')) {
                  next.memory = { ...next.memory, [item.rpcKey.split('.')[1]]: value };
                }
                return next;
              });
            })
            .catch((e: any) => { setError(e.message); setSuccess(null); });
        }
        setEditing(false);
        return;
      }
      if (key.backspace || key.delete) {
        setEditValue(v => v.slice(0, -1));
        return;
      }
      if (input && !key.ctrl && !key.meta && input.length === 1 && input >= ' ') {
        setEditValue(v => v + input);
      }
      return;
    }

    // Navigation mode
    if (key.escape) {
      uiStore.setView('chat');
      return;
    }
    if (key.upArrow) {
      setSelectedIndex(i => Math.max(0, i - 1));
      return;
    }
    if (key.downArrow) {
      setSelectedIndex(i => Math.min(settings.length - 1, i + 1));
      return;
    }
    if (key.return) {
      const item = settings[selectedIndex];
      if (item.key === 'language') {
        // Toggle language between en/zh
        const newLang = getLang() === 'en' ? 'zh' : 'en';
        if (client) {
          client.request('config.set_language', { language: newLang })
            .then(() => { setLang(newLang); setSuccess(`${t('ui.language')} -> ${newLang}`); setError(null); })
            .catch((e: any) => { setError(e.message); setSuccess(null); });
        }
        return;
      }
      if (item.type === 'readonly') return;

      if (item.type === 'bool') {
        // Toggle immediately
        const newVal = item.value === 'true' ? 'false' : 'true';
        if (item.rpcKey && client && sid) {
          client.request('config.set', { key: item.rpcKey, value: newVal === 'true' })
            .then(() => {
              setSuccess(`${item.label} -> ${newVal}`);
              setError(null);
              setConfigValues(prev => {
                const next = { ...prev };
                if (item.rpcKey?.startsWith('memory.')) {
                  next.memory = { ...next.memory, [item.rpcKey.split('.')[1]]: newVal === 'true' };
                }
                return next;
              });
            })
            .catch((e: any) => { setError(e.message); setSuccess(null); });
        }
      } else if (item.type === 'role') {
        // Cycle through roles
        const roles = ['default', 'plan', 'execute', 'review', 'goal', 'loop'];
        const currentIdx = roles.indexOf(item.value);
        const nextRole = roles[(currentIdx + 1) % roles.length];
        if (client && sid) {
          client.request('session.mode.set', { session_id: sid, role: nextRole })
            .then(() => {
              sessionStore.setRole(nextRole);
              setSuccess(`Role -> ${nextRole}`);
              setError(null);
            })
            .catch((e: any) => { setError(e.message); setSuccess(null); });
        }
      } else if (item.type === 'text' && item.key === 'model') {
        // Open model picker
        uiStore.setView('model');
      } else {
        // Enter edit mode for numbers/floats
        setEditValue(item.value === '?' ? '' : item.value);
        setEditing(true);
      }
    }
  });

  // ---- render helpers ----
  const KEY_W = 22;

  // Render a navigable settings row (bound to settings[selectedIndex] navigation).
  const navRow = (itemKey: string): React.ReactNode => {
    const idx = settings.findIndex(s => s.key === itemKey);
    if (idx < 0) return null;
    const item = settings[idx];
    const isSel = idx === selectedIndex;
    const isEditingThis = editing && isSel;
    const marker = isSel ? `${PREFIX.running} ` : '  ';
    const valueColor =
      item.type === 'bool'
        ? (item.value === 'true' ? TUI_COLORS.success : item.value === 'false' ? TUI_COLORS.error : TUI_COLORS.textMuted)
        : item.type === 'role' ? TUI_COLORS.accent
        : item.type === 'text' ? TUI_COLORS.accent
        : item.type === 'readonly' ? TUI_COLORS.textMuted
        : TUI_COLORS.textPrimary;

    let valueText: React.ReactNode;
    if (isEditingThis) {
      valueText = <Text color={TUI_COLORS.warning}>{editValue}<Text color={TUI_COLORS.textMuted}>▏</Text></Text>;
    } else if (item.type === 'bool') {
      valueText = item.value === 'true'
        ? <Text color={TUI_COLORS.success}>✓ on</Text>
        : item.value === 'false'
          ? <Text color={TUI_COLORS.error}>✗ off</Text>
          : <Text color={TUI_COLORS.textMuted}>{item.value}</Text>;
    } else {
      valueText = <Text color={valueColor}>{item.value}</Text>;
    }

    return (
      <Box key={item.key}>
        <Text color={isSel ? TUI_COLORS.accent : TUI_COLORS.textMuted} bold={isSel}>{marker}</Text>
        <Box width={KEY_W}><Text color={isSel ? TUI_COLORS.textPrimary : TUI_COLORS.textMuted} bold={isSel}>{item.label}</Text></Box>
        {valueText}
      </Box>
    );
  };

  // Render a static (non-navigable) display row.
  const staticRow = (label: string, valueNode: React.ReactNode, id: string): React.ReactNode => (
    <Box key={id}>
      <Text color={TUI_COLORS.textMuted}>{'  '}</Text>
      <Box width={KEY_W}><Text color={TUI_COLORS.textMuted}>{label}</Text></Box>
      {valueNode}
    </Box>
  );

  // Render an indented options sub-list (aligned under the value column).
  const optionsList = (options: { name: string; note?: string; active?: boolean }[], idPrefix: string): React.ReactNode => (
    <Box flexDirection="column" marginLeft={KEY_W + 2} key={idPrefix}>
      {options.map(o => (
        <Box key={o.name}>
          <Text color={o.active ? TUI_COLORS.accent : TUI_COLORS.textMuted}>{PREFIX.selected} </Text>
          <Box width={18}><Text color={o.active ? TUI_COLORS.accent : TUI_COLORS.textMuted} bold={o.active}>{o.name}</Text></Box>
          {o.note ? <Text color={TUI_COLORS.textMuted}> {o.note}</Text> : null}
        </Box>
      ))}
    </Box>
  );

  // Render a settings-section card (round border, head with fill line, body rows).
  const section = (name: string, rows: React.ReactNode[]): React.ReactNode => (
    <Box flexDirection="column" borderStyle="round" borderColor={TUI_COLORS.textMuted} flexShrink={0} marginBottom={1}>
      <Box paddingLeft={1} paddingRight={1}>
        <Text bold color={TUI_COLORS.textPrimary}>{name}</Text>
        <Text color={TUI_COLORS.textMuted}>{' ────────'}</Text>
        <Box flexGrow={1} />
      </Box>
      <Box paddingLeft={1} paddingRight={1} flexDirection="column">
        {rows}
      </Box>
    </Box>
  );

  const connected = sessionStore.connected;

  return (
    <Box flexDirection="column" flexGrow={1} overflow="hidden" paddingX={1}>
      {/* header-bar: connection status + path + branch | [Esc] back [s] save */}
      <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.textMuted} flexShrink={0}>
        <Box paddingLeft={1} paddingRight={1}>
          <Text color={connected ? TUI_COLORS.success : TUI_COLORS.error}>{PREFIX.dot}</Text>
          <Text color={connected ? TUI_COLORS.success : TUI_COLORS.error}> {connected ? t('ui.connected') : t('ui.disconnected')}</Text>
          <Text color={TUI_COLORS.textMuted}> {PREFIX.sep} </Text>
          <Text color={TUI_COLORS.textPrimary}>{sessionStore.projectPath || '~'}</Text>
          <Text color={TUI_COLORS.textMuted}> {PREFIX.sep} </Text>
          <Text color={TUI_COLORS.textMuted}>{sessionStore.gitBranch || '-'}</Text>
          <Box flexGrow={1} />
          <Text color={TUI_COLORS.orange}>[Esc]</Text>
          <Text color={TUI_COLORS.textMuted}> back </Text>
          <Text color={TUI_COLORS.textMuted}>{PREFIX.sep} </Text>
          <Text color={TUI_COLORS.orange}>[s]</Text>
          <Text color={TUI_COLORS.textMuted}> save</Text>
        </Box>
      </Box>

      {/* title row */}
      <Box paddingLeft={1} flexShrink={0} justifyContent="space-between">
        <Box>
          <Text bold color={TUI_COLORS.textPrimary}>{t('ui.settings')}</Text>
          <Text color={TUI_COLORS.textMuted}> Configure mcoder preferences</Text>
        </Box>
        <Text color={TUI_COLORS.textMuted}>↑↓ navigate {PREFIX.sep} Enter edit/toggle {PREFIX.sep} Esc back</Text>
      </Box>

      {/* two-column grid: sidebar + settings pane */}
      <Box flexDirection="row" flexGrow={1} overflow="hidden" marginTop={1}>
        {/* sidebar */}
        <Box flexDirection="column" borderStyle="round" borderColor={TUI_COLORS.textMuted} width={24} flexShrink={0} paddingX={1}>
          {CATEGORIES.map((cat, i) => {
            const active = i === 0;
            return (
              <Box key={cat}>
                <Text color={active ? TUI_COLORS.accent : TUI_COLORS.textMuted}>{active ? PREFIX.selected : '▹'}</Text>
                <Text> </Text>
                <Text color={active ? TUI_COLORS.accent : TUI_COLORS.textMuted} bold={active}>{cat}</Text>
              </Box>
            );
          })}
        </Box>

        {/* settings pane */}
        <Box flexDirection="column" flexGrow={1} paddingLeft={1} overflow="hidden">
          {error ? <Text color={TUI_COLORS.error}>{error}</Text> : null}
          {success ? <Text color={TUI_COLORS.success}>{success}</Text> : null}

          {section('Server Connection', [
            staticRow('Server URL', <Text color={TUI_COLORS.textPrimary}>ws://127.0.0.1:7654</Text>, 'url'),
            staticRow('Auth token', <Text color={TUI_COLORS.textMuted}>••••••••</Text>, 'token'),
            staticRow('TLS mode', <Text color={TUI_COLORS.textPrimary}>auto <Text color={TUI_COLORS.textMuted}>▾</Text></Text>, 'tls'),
            staticRow('Status',
              <Text color={connected ? TUI_COLORS.success : TUI_COLORS.error}>{connected ? '● connected' : '● disconnected'}</Text>,
              'status'),
            navRow('project'),
            navRow('version'),
            navRow('lsp'),
          ])}

          {section('Agent Behavior', [
            navRow('model'),
            navRow('role'),
            staticRow('Permission level', <Text color={TUI_COLORS.textPrimary}>Standard <Text color={TUI_COLORS.textMuted}>▾</Text></Text>, 'perm'),
            optionsList(PERMISSION_OPTIONS, 'perm-opt'),
            navRow('memory.auto_recall'),
            navRow('memory.auto_capture'),
            navRow('loop_max_iters'),
            navRow('compact.threshold'),
            navRow('compact.keep_recent'),
          ])}

          {section('UI Preferences', [
            staticRow('Theme', <Text color={TUI_COLORS.textPrimary}>Tokyo Night <Text color={TUI_COLORS.textMuted}>▾</Text></Text>, 'theme'),
            optionsList(THEME_OPTIONS, 'theme-opt'),
            navRow('language'),
            staticRow('Date format', <Text color={TUI_COLORS.textPrimary}>2026-08-03</Text>, 'datefmt'),
            staticRow('Time format', <Text color={TUI_COLORS.textPrimary}>14:23:08</Text>, 'timefmt'),
            staticRow('Streaming indicator', <Text color={TUI_COLORS.success}>● on</Text>, 'stream'),
            staticRow('Compact mode', <Text color={TUI_COLORS.error}>✗ off</Text>, 'compact-ui'),
            staticRow('Verbose tool output', <Text color={TUI_COLORS.success}>✓ on</Text>, 'verbose'),
          ])}

          {section('Quick Shortcuts', [
            <Box flexDirection="column" key="kb">
              {Array.from({ length: Math.ceil(KEYBINDS.length / 2) }, (_, r) => (
                <Box key={r}>
                  {[0, 1].map(c => {
                    const idx = r * 2 + c;
                    if (idx >= KEYBINDS.length) return <Box key={idx} width={26} />;
                    const [k, lbl] = KEYBINDS[idx];
                    return (
                      <Box key={idx} width={26}>
                        <Text color={TUI_COLORS.orange}>[{k}]</Text>
                        <Text color={TUI_COLORS.textMuted}> {lbl}</Text>
                      </Box>
                    );
                  })}
                </Box>
              ))}
            </Box>,
          ])}
        </Box>
      </Box>
    </Box>
  );
}
