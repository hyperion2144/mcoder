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

// per-model 参数编辑字段定义
interface ParamField {
  name: string;
  type: string;  // 'number' | 'integer' | 'string' | 'enum' | 'boolean'
  options?: string[];
}

/// 从 protocol schema JSON 解析出可编辑字段列表
/// M2 修复: 后端统一返回 { fields: [{ name, type, options, default, ... }] } 格式
/// 不再 fallback 到 DEFAULT_PARAM_FIELDS 或 JSON Schema properties
function parseSchemaFields(schema: any): ParamField[] | null {
  if (!schema) return null;
  if (Array.isArray(schema.fields)) {
    return schema.fields.map((f: any) => ({
      name: f.name,
      type: f.type,
      options: f.options,
    }));
  }
  return null;
}

/// 将编辑缓冲区的字符串值转换为字段对应类型
function convertParamValue(field: ParamField, strVal: string): any {
  if (strVal === '' || strVal === 'null') return null;
  if (field.type === 'integer') {
    const n = parseInt(strVal, 10);
    return isNaN(n) ? null : n;
  }
  if (field.type === 'number') {
    const n = parseFloat(strVal);
    return isNaN(n) ? null : n;
  }
  if (field.type === 'boolean') {
    return strVal === 'true';
  }
  return strVal;
}

// L3 修复: 移除未使用的 'edit' mode
type Mode = 'list' | 'add' | 'confirm-delete' | 'params';

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
  // 测试结果状态（结构化：ok 标志 + text 文本）；UI 渲染时根据 ok 选择 PREFIX.done / PREFIX.failed
  const [testResults, setTestResults] = useState<Record<string, { ok: boolean; text: string }>>({});

  // per-model 参数编辑状态
  const [paramsProvider, setParamsProvider] = useState<ProviderInfo | null>(null);
  const [paramsModelIdx, setParamsModelIdx] = useState(0);
  const [paramsFields, setParamsFields] = useState<ParamField[]>([]);
  const [paramsBuffer, setParamsBuffer] = useState<Record<string, string>>({});
  const [paramsFieldIdx, setParamsFieldIdx] = useState(0);
  const [paramsLoadError, setParamsLoadError] = useState<string | null>(null);

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
        setTestResults((m) => ({ ...m, [pname]: { ok: false, text: '...' } }));
        (async () => {
          setBusy(true);
          try {
            const r = await testProvider(client.request.bind(client), pname);
            setTestResults((m) => ({
              ...m,
              [pname]: r.ok
                ? { ok: true, text: r.url || '' }
                : { ok: false, text: r.hint || r.error || String(r.status || '') },
            }));
          } catch (e: any) {
            setTestResults((m) => ({ ...m, [pname]: { ok: false, text: e.message } }));
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
      // 'p' 键: 进入 per-model 参数编辑
      else if (input === 'p' && providers[cursor] && providers[cursor].models.length > 0) {
        const p = providers[cursor];
        setParamsProvider(p);
        setParamsModelIdx(0);
        setBusy(true);
        (async () => {
          try {
            const [schema, params] = await Promise.all([
              client.request('config.get_protocol_schema', { protocol: p.protocol }).catch(() => null),
              client.request('config.get_model_params', { provider: p.name, model: p.models[0] }).catch(() => ({})),
            ]);
            const fields = parseSchemaFields(schema);
            // M2 修复: schema 缺失 fields 数组时显式提示用户（不再 fallback）
            if (!fields) {
              setParamsLoadError('无法获取协议字段');
              return;
            }
            setParamsLoadError(null);
            setParamsFields(fields);
            // 把 params 值转为字符串存入编辑缓冲区
            const buf: Record<string, string> = {};
            for (const f of fields) {
              const v = (params as any)?.[f.name];
              buf[f.name] = v === null || v === undefined ? '' : String(v);
            }
            setParamsBuffer(buf);
            setParamsFieldIdx(0);
            setMode('params');
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
    } else if (mode === 'params') {
      // per-model 参数编辑：↑↓/Tab 导航字段，←/→ 切换枚举，Enter 保存，m 切换模型
      if (key.upArrow) {
        setParamsFieldIdx((i) => Math.max(0, i - 1));
      } else if (key.downArrow) {
        setParamsFieldIdx((i) => Math.min(paramsFields.length - 1, i + 1));
      } else if (input === '\t') {
        setParamsFieldIdx((i) => (i + 1) % Math.max(1, paramsFields.length));
      } else if (input === 'M' && paramsProvider && paramsProvider.models.length > 1) {
        const nextIdx = (paramsModelIdx + 1) % paramsProvider.models.length;
        setParamsModelIdx(nextIdx);
        setBusy(true);
        (async () => {
          try {
            const params = await client.request('config.get_model_params', {
              provider: paramsProvider.name,
              model: paramsProvider.models[nextIdx],
            }).catch(() => ({}));
            const buf: Record<string, string> = {};
            for (const f of paramsFields) {
              const v = (params as any)?.[f.name];
              buf[f.name] = v === null || v === undefined ? '' : String(v);
            }
            setParamsBuffer(buf);
            setParamsFieldIdx(0);
          } catch (e: any) { setError(e.message); }
          finally { setBusy(false); }
        })();
      } else if (key.return) {
        if (!paramsProvider) return;
        const model = paramsProvider.models[paramsModelIdx];
        const paramsObj: Record<string, any> = {};
        for (const f of paramsFields) {
          paramsObj[f.name] = convertParamValue(f, paramsBuffer[f.name] ?? '');
        }
        setBusy(true);
        (async () => {
          try {
            await client.request('config.set_model_params', {
              provider: paramsProvider.name,
              model,
              params: paramsObj,
            });
            setMode('list');
          } catch (e: any) { setError(e.message); }
          finally { setBusy(false); }
        })();
      } else if (key.backspace || key.delete) {
        const field = paramsFields[paramsFieldIdx];
        if (field && field.type !== 'enum') {
          setParamsBuffer((b) => ({
            ...b,
            [field.name]: (b[field.name] ?? '').slice(0, -1),
          }));
        }
      } else if (key.leftArrow || key.rightArrow) {
        const field = paramsFields[paramsFieldIdx];
        if (field && field.type === 'enum' && field.options && field.options.length > 0) {
          const curVal = paramsBuffer[field.name] ?? '';
          let curIdx = field.options.indexOf(curVal);
          if (curIdx === -1) curIdx = 0;
          const nextIdx = key.leftArrow
            ? (curIdx - 1 + field.options.length) % field.options.length
            : (curIdx + 1) % field.options.length;
          setParamsBuffer((b) => ({ ...b, [field.name]: field.options![nextIdx] }));
        }
      } else if (input && input !== '\t' && input.length === 1) {
        const field = paramsFields[paramsFieldIdx];
        if (field && field.type !== 'enum') {
          setParamsBuffer((b) => ({
            ...b,
            [field.name]: (b[field.name] ?? '') + input,
          }));
        }
      }
    }
  }, { isActive: !pendingPermission }); // S4: 有 pending permission 时停用

  if (mode === 'add') {
    const labels = ['name', 'protocol', 'base_url', 'api_key', 'models (逗号分隔)'];
    const vals = [name, protocol, baseUrl, apiKey, modelsInput];
    return (
      <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.accent} paddingX={1}>
        <Text color={TUI_COLORS.textPrimary}>{PREFIX.setting} Add Provider</Text>
        {labels.map((label, i) => (
          <Box key={label}>
            <Text color={i === formField ? TUI_COLORS.accent : TUI_COLORS.textMuted}>{i === formField ? `${PREFIX.selected} ` : '  '}{label.padEnd(20)}</Text>
            <Text color={TUI_COLORS.textPrimary}>{vals[i] || (i === formField ? '▏' : '')}</Text>
          </Box>
        ))}
        <Text color={TUI_COLORS.textMuted}>{`Tab: 下一字段 ${PREFIX.sep} ↑↓: 协议切换 ${PREFIX.sep} Enter: 下一字段/提交 ${PREFIX.sep} Esc: 取消`}</Text>
        {/* L1 修复: 用 PREFIX.error */}
        {error && <Text color={TUI_COLORS.error}>{PREFIX.error} {error}</Text>}
        {/* L2 修复: 用 PREFIX.loading */}
        {busy && <Text color={TUI_COLORS.accent}>{PREFIX.loading} 提交中...</Text>}
      </Box>
    );
  }

  if (mode === 'params' && paramsProvider) {
    const model = paramsProvider.models[paramsModelIdx];
    if (paramsLoadError) {
      return (
        <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.error} paddingX={1}>
          <Text color={TUI_COLORS.textPrimary}>{PREFIX.setting} Params: {paramsProvider.name} / {model}</Text>
          <Text color={TUI_COLORS.error}>{PREFIX.error} {paramsLoadError}</Text>
          <Text color={TUI_COLORS.textMuted}>{PREFIX.sep} Esc: 关闭</Text>
        </Box>
      );
    }
    return (
      <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.accent} paddingX={1}>
        <Text color={TUI_COLORS.textPrimary}>{PREFIX.setting} Params: {paramsProvider.name} / {model} ({paramsModelIdx + 1}/{paramsProvider.models.length})</Text>
        <Text color={TUI_COLORS.textMuted}>{PREFIX.textMuted} protocol: {paramsProvider.protocol}</Text>
        <Text color={TUI_COLORS.textMuted}>{'─'.repeat(60)}</Text>
        {paramsFields.map((f, i) => (
          <Box key={f.name}>
            <Text color={i === paramsFieldIdx ? TUI_COLORS.accent : TUI_COLORS.textMuted}>
              {i === paramsFieldIdx ? `${PREFIX.selected} ` : '  '}
              {f.name.padEnd(20)}
            </Text>
            <Text color={TUI_COLORS.textPrimary}>
              {f.type === 'enum'
                ? (paramsBuffer[f.name] != null && paramsBuffer[f.name] !== ''
                  ? `[${paramsBuffer[f.name]}]`
                  : `<default>`)
                : (paramsBuffer[f.name] || '▏')}
            </Text>
            <Text color={TUI_COLORS.textMuted}> ({f.type}{f.options ? `: ${f.options.join('/')}` : ''})</Text>
          </Box>
        ))}
        <Text color={TUI_COLORS.textMuted}>{`↑↓/Tab: 切换字段 ${PREFIX.sep} ←/→: 枚举切换 ${PREFIX.sep} M: 切换模型 ${PREFIX.sep} Enter: 保存 ${PREFIX.sep} Esc: 取消`}</Text>
        {error && <Text color={TUI_COLORS.error}>{PREFIX.error} {error}</Text>}
        {busy && <Text color={TUI_COLORS.accent}>{PREFIX.loading} 加载中...</Text>}
      </Box>
    );
  }

  return (
    <Box flexDirection="column" borderStyle="single" borderColor={TUI_COLORS.accent} paddingX={1}>
      <Text color={TUI_COLORS.textPrimary}>{PREFIX.setting} Providers ({providers.length})</Text>
      <Text color={TUI_COLORS.textMuted}>{`${PREFIX.textMuted} Models: ${models.length} | a:add t:test d:delete e:toggle p:params ⏎:set-default Esc:close`}</Text>
      <Text color={TUI_COLORS.textMuted}>{'─'.repeat(60)}</Text>
      {providers.length === 0 && (
        <Text color={TUI_COLORS.textMuted}>  无供应商。按 a 添加。</Text>
      )}
      {providers.map((p, i) => (
        <Box key={p.name}>
          <Text color={i === cursor ? TUI_COLORS.accent : TUI_COLORS.textMuted}>{i === cursor ? `${PREFIX.selected} ` : '  '}</Text>
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
      {/* L5 修复: 显示当前 cursor 对应 provider 的 testResult（结构化 ok/text） */}
      {providers[cursor] && testResults[providers[cursor].name] && (() => {
        const tr = testResults[providers[cursor].name];
        return (
          <Text color={tr.ok ? TUI_COLORS.success : TUI_COLORS.error}>
            {tr.ok ? PREFIX.done : PREFIX.failed} {tr.text}
          </Text>
        );
      })()}
      {/* L1 修复: 用 PREFIX.error */}
      {error && <Text color={TUI_COLORS.error}>{PREFIX.error} {error}</Text>}
      {/* L2 修复: 用 PREFIX.loading */}
      {busy && <Text color={TUI_COLORS.accent}>{PREFIX.loading} 加载中...</Text>}
    </Box>
  );
}