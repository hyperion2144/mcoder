// 设计文档 §provider: Mobile ProviderScreen
// - 列出现有 supplier + 添加/删除/测试操作（移动端友好：全屏列表 + 抽屉式表单）
// - 通过 RPC 调服务端
// - 订阅 config_updated 自动刷新

import { useEffect, useState } from 'react';
import {
  listProviders, listModels, listProtocols,
  addProvider, deleteProvider, updateProvider, setDefault, testProvider,
  type ProviderInfo, type ModelInfo, type ProtocolInfo,
} from '../rpc/config.js';
import { Check, AlertCircle, ArrowLeft, Plus, ChevronDown, ChevronRight, Star, Save } from './icons.js';

interface Props {
  /** WS client request 函数 */
  req: (method: string, params?: any) => Promise<any>;
  /** 订阅 config_updated 通知的 handler；返回 unsubscribe */
  onConfigUpdated: (cb: () => void) => () => void;
}

type Mode = 'list' | 'add';

export function ProviderScreen({ req, onConfigUpdated }: Props) {
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [protocols, setProtocols] = useState<ProtocolInfo[]>([]);
  const [mode, setMode] = useState<Mode>('list');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expandedProvider, setExpandedProvider] = useState<string | null>(null);

  const [name, setName] = useState('');
  const [protocol, setProtocol] = useState('openai');
  const [baseUrl, setBaseUrl] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [modelsInput, setModelsInput] = useState('');
  const [testResults, setTestResults] = useState<Record<string, string>>({});
  // 参数编辑
  const [editingParams, setEditingParams] = useState<{ provider: string; model: string; protocol: string } | null>(null);
  const [paramValues, setParamValues] = useState<any>({});
  const [originalParams, setOriginalParams] = useState<any>({});
  const [protocolSchema, setProtocolSchema] = useState<any>(null);

  const refresh = async () => {
    setBusy(true);
    try {
      const [ps, ms, pt] = await Promise.all([
        listProviders(req), listModels(req), listProtocols(req),
      ]);
      setProviders(ps);
      setModels(ms);
      setProtocols(pt);
      setError(null);
      // L7 修复: 清理悬空的 expandedProvider（外部 refresh 可能删除了 provider）
      setExpandedProvider((cur) => cur && ps.some((p) => p.name === cur) ? cur : null);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    refresh();
    return onConfigUpdated(refresh);
  }, []);

  const submitAdd = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const modelList = modelsInput.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);
      await addProvider(req, {
        name: name.trim(),
        protocol,
        base_url: baseUrl.trim(),
        api_key: apiKey,
        models: modelList,
      });
      setMode('list');
      setName(''); setProtocol('openai'); setBaseUrl(''); setApiKey(''); setModelsInput('');
      await refresh();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };

  const handleDelete = async (p: ProviderInfo) => {
    if (!confirm(`Delete provider "${p.name}"?`)) return;
    setBusy(true); setError(null);
    try {
      await deleteProvider(req, p.name);
      if (expandedProvider === p.name) setExpandedProvider(null);
      await refresh();
    } catch (e: any) { setError(e.message); }
    finally { setBusy(false); }
  };

  const handleTest = async (p: ProviderInfo) => {
    setTestResults((m) => ({ ...m, [p.name]: '...' }));
    try {
      const r = await testProvider(req, p.name);
      const detail = r.ok ? 'OK' : (r.hint || r.error || ('HTTP ' + r.status));
      setTestResults((m) => ({
        ...m,
        [p.name]: r.ok ? `ok:${detail}` : `fail:${detail}`,
      }));
    } catch (e: any) {
      setTestResults((m) => ({ ...m, [p.name]: `fail:${e.message}` }));
    }
  };

  const handleToggle = async (p: ProviderInfo) => {
    // M6 修复: 设 busy 避免并发
    setBusy(true); setError(null);
    try {
      await updateProvider(req, { name: p.name, enabled: !p.enabled });
      await refresh();
    } catch (e: any) { setError(e.message); }
    finally { setBusy(false); }
  };

  const handleSetDefault = async (modelName: string, providerName: string) => {
    // M3 修复: 传 provider name，避免清空 default_provider
    setBusy(true); setError(null);
    try {
      await setDefault(req, modelName, providerName);
      await refresh();
    } catch (e: any) { setError(e.message); }
    finally { setBusy(false); }
  };

  const handleEditParams = async (provider: string, model: string, protocol: string) => {
    setBusy(true); setError(null);
    try {
      const [schema, params] = await Promise.all([
        req('config.get_protocol_schema', { protocol }),
        req('config.get_model_params', { provider, model }),
      ]);
      setProtocolSchema(schema);
      const initial = params || {};
      setParamValues(initial);
      setOriginalParams(initial);
      setEditingParams({ provider, model, protocol });
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };

  const handleSaveParams = async () => {
    if (!editingParams) return;
    setBusy(true); setError(null);
    try {
      await req('config.set_model_params', {
        provider: editingParams.provider,
        model: editingParams.model,
        params: paramValues,
      });
      await refresh();
      setEditingParams(null);
      setProtocolSchema(null);
      setParamValues({});
      setOriginalParams({});
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  };

  if (editingParams && protocolSchema) {
    return (
      <div className="provider-screen">
        <div className="provider-screen-header">
          <button className="back-btn" onClick={() => setEditingParams(null)}><ArrowLeft size={18} /></button>
          <span>Params: {editingParams.model}</span>
        </div>
        <form className="provider-form" onSubmit={(e) => { e.preventDefault(); handleSaveParams(); }}>
          {Object.entries(protocolSchema).map(([key, schema]: [string, any]) => {
            const isModified = JSON.stringify(paramValues[key]) !== JSON.stringify(originalParams[key]);
            return (
            <div className={`form-row ${isModified ? 'form-row-modified' : ''}`} key={key}>
              <label>{key} {schema.description ? `(${schema.description})` : ''}</label>
              {schema.type === 'enum' ? (
                <select value={paramValues[key] ?? ''} onChange={(e) => setParamValues({ ...paramValues, [key]: e.target.value || undefined })}>
                  <option value="">(default)</option>
                  {schema.values.map((v: string) => <option key={v} value={v}>{v}</option>)}
                </select>
              ) : schema.type === 'float' || schema.type === 'int' ? (
                <input type="number" step={schema.type === 'float' ? 0.1 : 1} min={schema.min} max={schema.max}
                  value={paramValues[key] ?? ''} onChange={(e) => setParamValues({ ...paramValues, [key]: e.target.value === '' ? undefined : Number(e.target.value) })} />
              ) : schema.type === 'string_list' ? (
                <input type="text" placeholder="comma-separated"
                  value={(paramValues[key] || []).join(', ')}
                  onChange={(e) => setParamValues({ ...paramValues, [key]: e.target.value.split(',').map(s => s.trim()).filter(Boolean) })} />
              ) : schema.type === 'object' ? (
                <textarea rows={3} placeholder='JSON'
                  value={typeof paramValues[key] === 'object' ? JSON.stringify(paramValues[key], null, 2) : (paramValues[key] || '')}
                  onChange={(e) => { try { setParamValues({ ...paramValues, [key]: JSON.parse(e.target.value) }); } catch { setParamValues({ ...paramValues, [key]: e.target.value }); } }} />
              ) : null}
            </div>
            );
          })}
          {error && <div className="error-banner">{error}</div>}
          <div className="form-actions">
            <button type="button" className="secondary-btn" onClick={() => setEditingParams(null)}>Cancel</button>
            <button type="submit" className="primary-btn" disabled={busy}>{busy ? 'Saving...' : (<><Save size={14} /> Save</>)}</button>
          </div>
        </form>
      </div>
    );
  }

  if (mode === 'add') {
    return (
      <div className="provider-screen">
        <div className="provider-screen-header">
          <button className="back-btn" onClick={() => setMode('list')}><ArrowLeft size={18} /></button>
          <span>Add Provider</span>
        </div>
        <form className="provider-form" onSubmit={submitAdd}>
          <div className="form-row">
            <label>Name</label>
            <input type="text" value={name} onChange={(e) => setName(e.target.value)} required
              placeholder="e.g. openai-official" />
          </div>
          <div className="form-row">
            <label>Protocol</label>
            <select value={protocol} onChange={(e) => {
              const v = e.target.value;
              setProtocol(v);
              if (!baseUrl) {
                const p = protocols.find((x) => x.id === v);
                if (p) setBaseUrl(p.default_url);
              }
            }}>
              {protocols.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
            </select>
          </div>
          <div className="form-row">
            <label>Base URL</label>
            <input type="text" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="https://api.openai.com/v1" required />
          </div>
          <div className="form-row">
            <label>API Key</label>
            <input type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)}
              placeholder="sk-..." />
            <span className="form-hint">支持 {'${ENV_VAR}'} 环境变量语法</span>
          </div>
          <div className="form-row">
            <label>Models</label>
            <input type="text" value={modelsInput} onChange={(e) => setModelsInput(e.target.value)}
              placeholder="gpt-4o, gpt-4o-mini (逗号分隔)" />
          </div>
          {error && <div className="error-banner">{error}</div>}
          <div className="form-actions">
            <button type="button" className="secondary-btn" onClick={() => setMode('list')}>Cancel</button>
            <button type="submit" className="primary-btn" disabled={busy || !name.trim() || !baseUrl.trim()}>
              {busy ? 'Adding...' : 'Add'}
            </button>
          </div>
        </form>
      </div>
    );
  }

  return (
    <div className="provider-screen">
      <div className="provider-screen-header">
        <span>Providers</span>
        <button className="primary-btn" disabled={busy} onClick={() => setMode('add')}><Plus size={14} /> Add</button>
      </div>

      <div className="provider-summary">
        {providers.length} provider{providers.length === 1 ? '' : 's'}
        {' · '}{models.length} model{models.length === 1 ? '' : 's'}
      </div>

      {error && <div className="error-banner">{error}</div>}

      {providers.length === 0 && (
        <div className="empty-state">
          <p>No providers configured.</p>
          <p>Tap "Add" to set up OpenAI / Anthropic / Ollama / etc.</p>
        </div>
      )}

      <div className="provider-list">
        {providers.map((p) => (
          <div key={p.name} className={`provider-card ${p.enabled ? '' : 'disabled'}`}>
            <div
              className="provider-card-header"
              onClick={() => setExpandedProvider(expandedProvider === p.name ? null : p.name)}
            >
              <div className="provider-name-block">
                <span className="provider-name">{p.name}</span>
                <span className="provider-protocol">{p.protocol}</span>
              </div>
              <div className="provider-status">
                {!p.enabled && <span className="badge disabled">disabled</span>}
                {p.has_api_key ? <span className="badge ok">key</span> : <span className="badge warn">no key</span>}
                <span className="caret">{expandedProvider === p.name ? <ChevronDown size={14} /> : <ChevronRight size={14} />}</span>
              </div>
            </div>
            {expandedProvider === p.name && (
              <div className="provider-card-body">
                <div className="provider-row"><span>URL:</span><code>{p.base_url}</code></div>
                <div className="provider-row">
                  <span>Models:</span>
                  <ul className="provider-models">
                    {p.models.map((m) => (
                      <li key={m}>
                        <code>{m}</code>
                        <button className="link-btn" disabled={busy} onClick={() => handleSetDefault(m, p.name)} title="Set as default"><Star size={14} /></button>
                        <button className="link-btn" disabled={busy} onClick={() => handleEditParams(p.name, m, p.protocol)} title="Edit params">Params</button>
                      </li>
                    ))}
                  </ul>
                </div>
                {testResults[p.name] && (
                  <div className={`provider-test ${testResults[p.name].startsWith('ok:') ? 'ok' : 'fail'}`}>
                    {testResults[p.name].startsWith('ok:') ? (
                      <><Check size={14} /> {testResults[p.name].slice(3)}</>
                    ) : testResults[p.name] === '...' ? (
                      <>{testResults[p.name]}</>
                    ) : (
                      <><AlertCircle size={14} /> {testResults[p.name].slice(5)}</>
                    )}
                  </div>
                )}
                <div className="provider-card-actions">
                  <button className="secondary-btn" disabled={busy} onClick={() => handleTest(p)}>Test</button>
                  <button className="secondary-btn" disabled={busy} onClick={() => handleToggle(p)}>
                    {p.enabled ? 'Disable' : 'Enable'}
                  </button>
                  <button className="danger-btn" disabled={busy} onClick={() => handleDelete(p)}>Delete</button>
                </div>
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}