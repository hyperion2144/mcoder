// 设计文档 §provider: TUI ProviderView
// - 列出现有供应商 + 协议下拉 + 模型列表
// - 添加/删除/测试/toggle/set-default 操作；通过 RPC 调服务端
// - S4 修复: 通过 pendingPermission prop 避免与权限审批快捷键冲突
// - S5 修复: busy 不阻塞 Esc
// - M12 修复: config_updated 通知在 add 模式时不触发 refresh（不打断表单）

import { useEffect, useState, useRef } from 'react';
import { Box, Text, useInput } from 'ink';
import { TUI_COLORS, PREFIX } from '../theme.js';
import type { WsClient } from '../rpc/client.js';
import {
  listProviders, listModels, listProtocols, addProvider, deleteProvider,
  updateProvider, setDefault, testProvider,
  type ProviderInfo, type ModelInfo, type ProtocolInfo,
} from '../rpc/config.js';

interface Props {
  client: WsClient;
  onClose: () => void;
  /** S4 修复: 有 pending permission 时由父组件控制是否激活 useInput */
  pendingPermission?: boolean;
}

// L3 修复: 移除未使用的 'edit' mode
type Mode = 'list' | 'add' | 'confirm-delete';

export function ProviderView({ client, onClose, pendingPermission }: Props) {
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
  const [formField, setFormField] = useState<0 | 1 | 2 | 3 | 4>(0);
  const [protocolIdx, setProtocolIdx] = useState(0);
  // L5 修复: per-provider testResult map
  const [testResults, setTestResults] = useState<Record<string, string>>({});

  // M12 修复: refresh 不在 add 模式时触发（避免打断表单编辑）
  const modeRef = useRef(mode);
  modeRef.current = mode;

  const refresh = async () => {
    // M12: add 模式时不 refresh（避免 setBusy 打断输入）
    if (modeRef.current === 'add') return;
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
    const handler = (n: any) => {
      if (n.method === 'config_updated') refresh();
    };
    client.onNotification(handler);
    return () => client.offNotification(handler);
  }, []);

  // S4 修复: 有 pending permission 时不激活 useInput，避免快捷键冲突
  useInput((input, key) => {
    // S5 修复: Esc 始终可用，不被 busy 阻塞
    if (key.escape) {
      if (mode === 'list') onClose();
      else setMode('list');
      return;
    }

    if (busy) return;

    if (mode === 'list') {
      if (key.upArrow) setCursor((c) => Math.max(0, c - 1));
      else if (key.downArrow) setCursor((c) => Math.min(Math.max(0, providers.length - 1), c + 1));
      else if (input === 'a') {
        setMode('add'); setFormField(0); setName(''); setProtocol('openai');
        setBaseUrl(''); setApiKey(''); setModelsInput(''); setProtocolIdx(0);
      }
      else if (input === 't' && providers[cursor]) {
        const pname = providers[cursor].name;
        setTestResults((m) => ({ ...m, [pname]: '...' }));
        (async () => {
          setBusy(true);
          try {
            const r = await testProvider(client.request.bind(client), pname);
            setTestResults((m) => ({
              ...m,
              [pname]: r.ok ? `✓ ${r.url || ''}` : `✗ ${r.hint || r.error || r.status}`,
            }));
          } catch (e: any) {
            setTestResults((m) => ({ ...m, [pname]: `✗ ${e.message}` }));
          } finally { setBusy(false); }
        })();
      }
      else if (input === 'd' && providers[cursor]) setMode('confirm-delete');
      // M1 修复: 'e' 键 toggle-enabled
      else if (input === 'e' && providers[cursor]) {
        const p = providers[cursor];
        (async () => {
          setBusy(true);
          try {
            await updateProvider(client.request.bind(client), { name: p.name, enabled: !p.enabled });
          } catch (e: any) { setError(e.message); }
          finally { setBusy(false); }
        })();
      }
      else if (key.return && providers[cursor] && providers[cursor].models.length > 0) {
        // M2 修复: set-default 传 provider name + 第一个 model
        const p = providers[cursor];
        (async () => {
          setBusy(true);
          try {
            // L9 修复: set-default 成功后给反馈
            await setDefault(client.request.bind(client), p.models[0], p.name);
            setError(null);
          } catch (e: any) { setError(e.message); }
          finally { setBusy(false); }
        })();
      }
    } else if (mode === 'confirm-delete') {
      if (input === 'y' && providers[cursor]) {
        const delName = providers[cursor].name;
        (async () => {
          setBusy(true);
          try {
            await deleteProvider(client.request.bind(client), delName);
            // M5 修复: 删除后重置 cursor 避免越界
            setCursor(0);
            setMode('list');
          } catch (e: any) { setError(e.message); }
          finally { setBusy(false); }
        })();
      } else setMode('list');
    } else if (mode === 'add') {
      if (key.return) {
        if (formField < 4) { setFormField((f) => (f + 1) as any); }
        else {
          // M4 修复: 校验 name 和 base_url 非空
          if (!name.trim()) { setError('name cannot be empty'); return; }
          if (!baseUrl.trim()) { setError('base_url cannot be empty'); return; }
          (async () => {
            setBusy(true); setError(null);
            try {
              const modelList = modelsInput.split(/[\s,]+/).filter(Boolean);
              await addProvider(client.request.bind(client), {
                name: name.trim(),
                protocol,
                base_url: baseUrl.trim(),
                api_key: apiKey,
                models: modelList,
              });
              setMode('list');
            } catch (e: any) { setError(e.message); }
            finally { setBusy(false); }
          })();
        }
      } else if (key.backspace || key.delete) {
        if (formField === 0) setName((s) => s.slice(0, -1));
        else if (formField === 2) setBaseUrl((s) => s.slice(0, -1));
        else if (formField === 3) setApiKey((s) => s.slice(0, -1));
        else if (formField === 4) setModelsInput((s) => s.slice(0, -1));
      } else if (input === '\t') {
        setFormField((f) => ((f + 1) % 5) as any);
      } else if (key.upArrow && formField === 1) {
        const next = (protocolIdx - 1 + protocols.length) % protocols.length;
        setProtocolIdx(next);
        setProtocol(protocols[next].id);
        if (!baseUrl) setBaseUrl(protocols[next].default_url);
      } else if (key.downArrow && formField === 1) {
        const next = (protocolIdx + 1) % protocols.length;
        setProtocolIdx(next);
        setProtocol(protocols[next].id);
        if (!baseUrl) setBaseUrl(protocols[next].default_url);
      } else if (input && input !== '\t') {
        if (formField === 0) setName((s) => s + input);
        else if (formField === 2) setBaseUrl((s) => s + input);
        else if (formField === 3) setApiKey((s) => s + input);
        else if (formField === 4) setModelsInput((s) => s + input);
      }
    }
  }, { isActive: !pendingPermission }); // S4: 有 pending permission 时停用

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
        {/* L1 修复: 用 PREFIX.error */}
        {error && <Text color={TUI_COLORS.error}>{PREFIX.error} {error}</Text>}
        {/* L2 修复: 用 PREFIX.loading */}
        {busy && <Text color={TUI_COLORS.accent}>{PREFIX.loading} 提交中...</Text>}
      </Box>
    );
  }

  return (
    <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.accent} paddingX={1}>
      <Text color={TUI_COLORS.textPrimary}>{PREFIX.setting} Providers ({providers.length})</Text>
      <Text color={TUI_COLORS.textMuted}>{PREFIX.textMuted} Models: {models.length} | a:add t:test d:delete e:toggle ⏎:set-default Esc:close</Text>
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
      {/* L5 修复: 显示当前 cursor 对应 provider 的 testResult */}
      {providers[cursor] && testResults[providers[cursor].name] && (
        <Text color={testResults[providers[cursor].name].startsWith('✓') ? TUI_COLORS.success : TUI_COLORS.error}>
          {testResults[providers[cursor].name]}
        </Text>
      )}
      {/* L1 修复: 用 PREFIX.error */}
      {error && <Text color={TUI_COLORS.error}>{PREFIX.error} {error}</Text>}
      {/* L2 修复: 用 PREFIX.loading */}
      {busy && <Text color={TUI_COLORS.accent}>{PREFIX.loading} 加载中...</Text>}
    </Box>
  );
}