// 设计文档 §provider: TUI ProviderView
// - 列出现有供应商 + 协议下拉 + 模型列表
// - 添加/删除/测试操作；通过 RPC 调服务端
// - 配置错误（无 default_model）时 Setup Mode 引导

import { useEffect, useState } from 'react';
import { Box, Text, useInput } from 'ink';
import { TUI_COLORS, PREFIX } from '../theme.js';
import type { WsClient } from '../rpc/client.js';
import {
  listProviders, listModels, listProtocols, addProvider, deleteProvider, testProvider,
  type ProviderInfo, type ModelInfo, type ProtocolInfo,
} from '../rpc/config.js';

interface Props {
  client: WsClient;
  onClose: () => void;
}

type Mode = 'list' | 'add' | 'edit' | 'confirm-delete';

export function ProviderView({ client, onClose }: Props) {
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [protocols, setProtocols] = useState<ProtocolInfo[]>([]);
  const [mode, setMode] = useState<Mode>('list');
  const [cursor, setCursor] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 添加表单字段
  const [name, setName] = useState('');
  const [protocol, setProtocol] = useState('openai');
  const [baseUrl, setBaseUrl] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [modelsInput, setModelsInput] = useState('');
  const [formField, setFormField] = useState<0 | 1 | 2 | 3 | 4>(0); // 5 字段：name/protocol/baseUrl/apiKey/models
  const [protocolIdx, setProtocolIdx] = useState(0);
  const [testResult, setTestResult] = useState<string | null>(null);

  const refresh = async () => {
    try {
      setBusy(true);
      const [ps, ms, pt] = await Promise.all([
        listProviders(client.request.bind(client)),
        listModels(client.request.bind(client)),
        listProtocols(client.request.bind(client)),
      ]);
      setProviders(ps);
      setModels(ms);
      setProtocols(pt);
      setError(null);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    refresh();
    // 订阅 config_updated 通知 → 自动刷新
    const handler = (n: any) => {
      if (n.method === 'config_updated') refresh();
    };
    client.onNotification(handler);
    return () => client.offNotification(handler);
  }, []);

  useInput((input, key) => {
    if (busy) return;

    // 全局：Esc 返回
    if (key.escape) {
      if (mode === 'list') onClose();
      else setMode('list');
      return;
    }

    if (mode === 'list') {
      if (key.upArrow) setCursor((c) => Math.max(0, c - 1));
      else if (key.downArrow) setCursor((c) => Math.min(providers.length - 1, c + 1));
      else if (input === 'a') { setMode('add'); setFormField(0); setName(''); setProtocol('openai'); setBaseUrl(''); setApiKey(''); setModelsInput(''); setProtocolIdx(0); setTestResult(null); }
      else if (input === 't' && providers[cursor]) {
        // 测试
        (async () => {
          setBusy(true); setTestResult(null);
          try {
            const r = await testProvider(client.request.bind(client), providers[cursor].name);
            setTestResult(r.ok ? `✓ ${r.url || ''}` : `✗ ${r.hint || r.error || r.status}`);
          } catch (e: any) { setTestResult(`✗ ${e.message}`); }
          finally { setBusy(false); }
        })();
      }
      else if (input === 'd' && providers[cursor]) setMode('confirm-delete');
      else if (key.return && providers[cursor]) {
        // 设为默认
        (async () => {
          setBusy(true);
          try {
            await client.request('config.set_default', { model: providers[cursor].models[0] || providers[cursor].name, provider: providers[cursor].name });
            setError(null);
          } catch (e: any) { setError(e.message); }
          finally { setBusy(false); }
        })();
      }
    } else if (mode === 'confirm-delete') {
      if (input === 'y') {
        (async () => {
          setBusy(true);
          try {
            await deleteProvider(client.request.bind(client), providers[cursor].name);
            await refresh();
            setMode('list');
          } catch (e: any) { setError(e.message); }
          finally { setBusy(false); }
        })();
      } else setMode('list');
    } else if (mode === 'add') {
      // 表单编辑
      if (key.return) {
        if (formField < 4) { setFormField((f) => (f + 1) as any); }
        else {
          // 提交
          (async () => {
            setBusy(true); setError(null);
            try {
              const models = modelsInput.split(/[\s,]+/).filter(Boolean);
              await addProvider(client.request.bind(client), {
                name: name.trim(),
                protocol: protocol,
                base_url: baseUrl.trim(),
                api_key: apiKey,
                models,
              });
              await refresh();
              setMode('list');
            } catch (e: any) { setError(e.message); }
            finally { setBusy(false); }
          })();
        }
      } else if (key.backspace || key.delete) {
        // 删字符
        if (formField === 0) setName((s) => s.slice(0, -1));
        else if (formField === 2) setBaseUrl((s) => s.slice(0, -1));
        else if (formField === 3) setApiKey((s) => s.slice(0, -1));
        else if (formField === 4) setModelsInput((s) => s.slice(0, -1));
      } else if (input === '\t') {
        // M5 修复: tab 在所有字段间循环切换（0->1->2->3->4->0）
        setFormField((f) => ((f + 1) % 5) as any);
      } else if (key.upArrow && formField === 1) {
        // 协议字段：上切换
        const next = (protocolIdx - 1 + protocols.length) % protocols.length;
        setProtocolIdx(next);
        setProtocol(protocols[next].id);
        if (!baseUrl) setBaseUrl(protocols[next].default_url);
      } else if (key.downArrow && formField === 1) {
        // 协议字段：下切换
        const next = (protocolIdx + 1) % protocols.length;
        setProtocolIdx(next);
        setProtocol(protocols[next].id);
        if (!baseUrl) setBaseUrl(protocols[next].default_url);
      } else if (input) {
        // 字符输入（除 tab/backspace）
        if (formField === 0) setName((s) => s + input);
        else if (formField === 2) setBaseUrl((s) => s + input);
        else if (formField === 3) setApiKey((s) => s + input);
        else if (formField === 4) setModelsInput((s) => s + input);
      }
    }
  });

  if (mode === 'add') {
    const labels = ['name', 'protocol', 'base_url', 'api_key', 'models (逗号分隔)'];
    const vals = [name, protocol, baseUrl, apiKey, modelsInput];
    return (
      <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.accent} paddingX={1}>
        <Text color={TUI_COLORS.textPrimary}>{PREFIX.pending} Add Provider</Text>
        {labels.map((label, i) => (
          <Box key={label}>
            <Text color={i === formField ? TUI_COLORS.accent : TUI_COLORS.textMuted}>{i === formField ? '▸ ' : '  '}{label.padEnd(20)}</Text>
            <Text color={TUI_COLORS.textPrimary}>{vals[i] || (i === formField ? '▏' : '')}</Text>
          </Box>
        ))}
        <Text color={TUI_COLORS.textMuted}>Tab: 下一字段 · ↑↓: 协议切换 · Enter: 下一字段/提交 · Esc: 取消</Text>
        {error && <Text color={TUI_COLORS.error}>{PREFIX.error} {error}</Text>}
        {busy && <Text color={TUI_COLORS.accent}>{PREFIX.loading} 提交中...</Text>}
      </Box>
    );
  }

  return (
    <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.accent} paddingX={1}>
      <Text color={TUI_COLORS.textPrimary}>{PREFIX.setting} Providers ({providers.length})</Text>
      <Text color={TUI_COLORS.textMuted}>{PREFIX.textMuted} Models: {models.length} | a:add t:test d:delete ⏎:set-default Esc:close</Text>
      <Text color={TUI_COLORS.textMuted}>{'─'.repeat(60)}</Text>
      {providers.length === 0 && (
        <Text color={TUI_COLORS.textMuted}>  无供应商。按 a 添加。</Text>
      )}
      {providers.map((p, i) => (
        <Box key={p.name}>
          <Text color={i === cursor ? TUI_COLORS.accent : TUI_COLORS.textMuted}>{i === cursor ? '▸ ' : '  '}</Text>
          <Text color={TUI_COLORS.textPrimary}>{p.name.padEnd(20)} </Text>
          <Text color={TUI_COLORS.textMuted}>{p.protocol.padEnd(14)} </Text>
          <Text color={TUI_COLORS.textMuted}>{p.models.length} models  </Text>
          {!p.enabled && <Text color={TUI_COLORS.warning}>disabled</Text>}
        </Box>
      ))}
      {mode === 'confirm-delete' && providers[cursor] && (
        <Box marginTop={1}>
          <Text color={TUI_COLORS.error}>删除 "{providers[cursor].name}"? y/n</Text>
        </Box>
      )}
      {testResult && (
        <Text color={testResult.startsWith('✓') ? TUI_COLORS.success : TUI_COLORS.error}>
          {testResult}
        </Text>
      )}
      {error && <Text color={TUI_COLORS.error}>{PREFIX.pending} {error}</Text>}
      {busy && <Text color={TUI_COLORS.accent}>{PREFIX.pending} 加载中...</Text>}
    </Box>
  );
}