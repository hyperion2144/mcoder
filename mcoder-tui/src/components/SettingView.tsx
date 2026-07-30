// 设计文档 §6.7: components/SettingView.tsx - 交互式设置面板

import React, { useState, useEffect } from 'react';
import { Box, Text, useInput } from 'ink';
import { useSessionStore } from '../store/session.js';
import { useUiStore } from '../store/ui.js';
import type { WsClient } from '../rpc/client.js';

interface SettingItem {
  key: string;
  label: string;
  description: string;
  type: 'bool' | 'number' | 'float' | 'text' | 'readonly' | 'role';
  value: string;
  rpcKey?: string;  // for config.set RPC
}

export function SettingView({ client }: { client: WsClient | null }) {
  const sessionStore = useSessionStore();
  const uiStore = useUiStore();
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [editing, setEditing] = useState(false);
  const [editValue, setEditValue] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);

  const sid = sessionStore.currentSessionId;

  // Fetch current config values on mount
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

  const settings: SettingItem[] = [
    { key: 'model', label: 'Model', description: 'LLM model for this session', type: 'text', value: sessionStore.currentModel || '-' },
    { key: 'role', label: 'Role', description: 'Agent role (default/plan/execute/review/goal/loop)', type: 'role', value: sessionStore.currentRole || 'default' },
    { key: 'loop_max_iters', label: 'Max Iterations', description: 'Max agent loop iterations', type: 'number', value: String(configValues.loop_max_iters ?? '?'), rpcKey: 'loop_max_iters' },
    { key: 'compact.threshold', label: 'Compact Threshold', description: 'Context usage threshold to trigger compaction (0-1)', type: 'float', value: String(configValues.compact?.threshold ?? '?'), rpcKey: 'compact.threshold' },
    { key: 'compact.keep_recent', label: 'Compact Keep Recent', description: 'Messages to keep during compaction', type: 'number', value: String(configValues.compact?.keep_recent ?? '?'), rpcKey: 'compact.keep_recent' },
    { key: 'memory.auto_recall', label: 'Memory Auto Recall', description: 'Automatically recall relevant memories', type: 'bool', value: String(configValues.memory?.auto_recall ?? '?'), rpcKey: 'memory.auto_recall' },
    { key: 'memory.auto_capture', label: 'Memory Auto Capture', description: 'Automatically capture decisions to memory', type: 'bool', value: String(configValues.memory?.auto_capture ?? '?'), rpcKey: 'memory.auto_capture' },
    { key: 'version', label: 'Version', description: 'mcoder version', type: 'readonly', value: sessionStore.version || '-' },
    { key: 'project', label: 'Project', description: 'Project path', type: 'readonly', value: sessionStore.projectPath || '-' },
    { key: 'lsp', label: 'LSP Servers', description: 'Active LSP servers', type: 'readonly', value: sessionStore.lspServers.join(', ') || 'none' },
  ];

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

  return (
    <Box flexDirection="column" borderStyle="single" borderColor="cyan" paddingX={1}>
      <Box marginBottom={1}>
        <Text bold color="cyan">Settings</Text>
        <Text color="gray">  (↑↓ navigate, Enter edit/toggle, Esc close)</Text>
      </Box>

      {error && <Text color="red">Error: {error}</Text>}
      {success && <Text color="green">✓ {success}</Text>}

      <Box flexDirection="column">
        {settings.map((item, i) => (
          <Box key={item.key}>
            <Text color={i === selectedIndex ? 'cyan' : undefined} bold={i === selectedIndex}>
              {i === selectedIndex ? '▸ ' : '  '}
            </Text>
            <Box width={22}><Text bold>{item.label}</Text></Box>
            {editing && i === selectedIndex ? (
              <Text color="yellow">{editValue}_</Text>
            ) : (
              <Text color={item.type === 'bool' ? (item.value === 'true' ? 'green' : 'red') : 'gray'}>
                {item.value}
              </Text>
            )}
            <Text color="gray">  {item.description}</Text>
          </Box>
        ))}
      </Box>
    </Box>
  );
}
