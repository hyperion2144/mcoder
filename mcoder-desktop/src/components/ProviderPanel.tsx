// 设计文档 §provider: Desktop ProviderPanel
// - 列出现有 supplier + 添加/删除/测试操作
// - 通过 RPC 调服务端 (add/update/delete_provider / set_default / test_provider)
// - 订阅 config_updated 自动刷新

import { useEffect, useState } from 'react';
import {
  listProviders, listModels, listProtocols,
  addProvider, deleteProvider, updateProvider, setDefault, testProvider,
  type ProviderInfo, type ModelInfo, type ProtocolInfo,
} from '../rpc/config.js';
import { X, Check, Star, Plus } from './icons.js';
import { t } from '../i18n.js';

interface Props {
  /** WS client request 函数 */
  req: (method: string, params?: any) => Promise<any>;
  /** 订阅 config_updated 通知的 handler；返回 unsubscribe */
  onConfigUpdated: (cb: () => void) => () => void;
  onClose?: () => void;
}

type Mode = 'list' | 'add';

export function ProviderPanel({ req, onConfigUpdated, onClose }: Props) {
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [protocols, setProtocols] = useState<ProtocolInfo[]>([]);
  const [mode, setMode] = useState<Mode>('list');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [name, setName] = useState('');
  const [protocol, setProtocol] = useState('openai');
  const [baseUrl, setBaseUrl] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [modelsInput, setModelsInput] = useState('');
  const [testResults, setTestResults] = useState<Record<string, { ok: boolean; text: string }>>({});
  const [editingParams, setEditingParams] = useState<{ provider: string; model: string; protocol: string } | null>(null);
  const [paramValues, setParamValues] = useState<any>({});
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
      await refresh();
    } catch (e: any) { setError(e.message); }
    finally { setBusy(false); }
  };

  const handleTest = async (p: ProviderInfo) => {
    setTestResults((m) => ({ ...m, [p.name]: { ok: false, text: '...' } }));
    try {
      const r = await testProvider(req, p.name);
      setTestResults((m) => ({
        ...m,
        [p.name]: {
          ok: r.ok,
          text: r.ok ? 'OK' : (r.hint || r.error || ('HTTP ' + r.status)),
        },
      }));
    } catch (e: any) {
      setTestResults((m) => ({ ...m, [p.name]: { ok: false, text: e.message } }));
    }
  };

  const handleToggle = async (p: ProviderInfo) => {
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

  const handleEditParams = async (providerName: string, modelName: string, protocol: string) => {
    try {
      const [params, schema] = await Promise.all([
        req('config.get_model_params', { provider: providerName, model: modelName }),
        req('config.get_protocol_schema', { protocol }),
      ]);
      setParamValues(params || {});
      setProtocolSchema(schema);
      setEditingParams({ provider: providerName, model: modelName, protocol });
    } catch (e: any) { setError(e.message); }
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
      setEditingParams(null);
      await refresh();
    } catch (e: any) { setError(e.message); }
    finally { setBusy(false); }
  };

  return (
    <div className="provider-panel">
      <div className="provider-header">
        <h2>{t('ui.providers')}</h2>
        {onClose && <button className="icon-btn" onClick={onClose} title={t('ui.close')}><X size={14} /></button>}
      </div>
      {error && <div className="error-banner">{error}</div>}

      {mode === 'list' && (
        <>
          <div className="provider-toolbar">
            <span className="provider-count">
              {providers.length} provider{providers.length === 1 ? '' : 's'}
              {' · '}{models.length} model{models.length === 1 ? '' : 's'}
            </span>
            <button className="primary-btn" disabled={busy} onClick={() => setMode('add')}>
              <Plus size={14} /> {t('ui.add_provider')}
            </button>
          </div>

          {providers.length === 0 && (
            <div className="empty-state">
              <p>{t('ui.no_providers')}</p>
              <p>{t('ui.no_providers_hint')}</p>
            </div>
          )}

          <div className="provider-list">
            {providers.map((p) => (
              <div key={p.name} className={`provider-card ${p.enabled ? '' : 'disabled'}`}>
                <div className="provider-card-header">
                  <div className="provider-name-block">
                    <span className="provider-name">{p.name}</span>
                    <span className="provider-protocol">{p.protocol}</span>
                  </div>
                  <div className="provider-status">
                    {!p.enabled && <span className="badge disabled">{t('ui.disabled')}</span>}
                    {p.has_api_key ? <span className="badge ok">{t('ui.key_set')}</span> : <span className="badge warn">{t('ui.no_key')}</span>}
                  </div>
                </div>
                <div className="provider-card-body">
                  <div className="provider-row"><span>URL:</span><code>{p.base_url}</code></div>
                  <div className="provider-row">
                    <span>{t('ui.models')}:</span>
                    <ul className="provider-models">
                      {p.models.map((m) => (
                        <li key={m}>
                          <code>{m}</code>
                          <button className="link-btn" disabled={busy} onClick={() => handleSetDefault(m, p.name)} title={t('ui.set_default')}>
                            <Star size={14} />
                          </button>
                          <button className="link-btn" disabled={busy} onClick={() => handleEditParams(p.name, m, p.protocol)}>{t('ui.params')}</button>
                        </li>
                      ))}
                    </ul>
                  </div>
                  {testResults[p.name] && (
                    <div className={`provider-test ${testResults[p.name].ok ? 'ok' : 'fail'}`}>
                      {testResults[p.name].ok
                        ? <Check size={14} />
                        : <X size={14} />}
                      {testResults[p.name].text}
                    </div>
                  )}
                </div>
                <div className="provider-card-actions">
                  <button className="secondary-btn" disabled={busy} onClick={() => handleTest(p)}>{t('ui.test')}</button>
                  <button className="secondary-btn" disabled={busy} onClick={() => handleToggle(p)}>
                    {p.enabled ? t('ui.disable') : t('ui.enable')}
                  </button>
                  <button className="danger-btn" disabled={busy} onClick={() => handleDelete(p)}>{t('ui.delete')}</button>
                </div>
              </div>
            ))}
          </div>
        </>
      )}

      {mode === 'add' && (
        <form className="provider-form" onSubmit={submitAdd}>
          <h3>{t('ui.add_provider')}</h3>
          <div className="form-row">
            <label>{t('ui.name')}</label>
            <input type="text" value={name} onChange={(e) => setName(e.target.value)} required
              placeholder="e.g. openai-official" />
          </div>
          <div className="form-row">
            <label>{t('ui.protocol')}</label>
            <select value={protocol} onChange={(e) => {
              const v = e.target.value;
              setProtocol(v);
              // 切换协议时填默认 URL（仅在 baseUrl 为空时）
              if (!baseUrl) {
                const p = protocols.find((x) => x.id === v);
                if (p) setBaseUrl(p.default_url);
              }
            }}>
              {protocols.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
            </select>
          </div>
          <div className="form-row">
            <label>{t('ui.base_url')}</label>
            <input type="text" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="https://api.openai.com/v1" required />
          </div>
          <div className="form-row">
            <label>{t('ui.api_key')}</label>
            <input type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)}
              placeholder="sk-..." />
            <span className="form-hint">{t('ui.env_var_hint')}</span>
          </div>
          <div className="form-row">
            <label>{t('ui.models')}</label>
            <input type="text" value={modelsInput} onChange={(e) => setModelsInput(e.target.value)}
              placeholder={'gpt-4o, gpt-4o-mini (' + t('ui.comma_separated') + ')'} />
          </div>
          <div className="form-actions">
            <button type="button" className="secondary-btn" onClick={() => setMode('list')}>{t('ui.cancel')}</button>
            <button type="submit" className="primary-btn" disabled={busy || !name.trim() || !baseUrl.trim()}>
              {busy ? t('ui.adding') : t('ui.add')}
            </button>
          </div>
        </form>
      )}

      {editingParams && protocolSchema && (
        <div className="settings-overlay" onClick={(e) => { if (e.target === e.currentTarget) setEditingParams(null); }}>
          <div className="settings-panel" style={{ maxWidth: '500px' }}>
            <div className="settings-header">
              <span>Model Parameters: {editingParams.model}</span>
              <button onClick={() => setEditingParams(null)}><X size={14} /></button>
            </div>
            <div className="settings-body">
              {Object.entries(protocolSchema).map(([key, schema]: [string, any]) => (
                <div className="setting-row" key={key}>
                  <div className="setting-label">
                    <span className="setting-name">{key}</span>
                    <span className="setting-desc">{schema.description || `${schema.type}${schema.min !== undefined ? ` (${schema.min}-${schema.max})` : ''}`}</span>
                  </div>
                  <div className="setting-control">
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
                      <textarea rows={2} placeholder='JSON'
                        value={typeof paramValues[key] === 'object' ? JSON.stringify(paramValues[key], null, 2) : (paramValues[key] || '')}
                        onChange={(e) => { try { setParamValues({ ...paramValues, [key]: JSON.parse(e.target.value) }); } catch { setParamValues({ ...paramValues, [key]: e.target.value }); } }} />
                    ) : null}
                  </div>
                </div>
              ))}
              <div className="form-actions" style={{ marginTop: '12px' }}>
                <button className="secondary-btn" onClick={() => setEditingParams(null)}>{t('ui.cancel')}</button>
                <button className="primary-btn" onClick={handleSaveParams} disabled={busy}>{t('ui.save')}</button>
              </div>
            </div>
          </div>
        </div>
      )}
      {busy && <div className="overlay-loading">{t('ui.working')}</div>}
    </div>
  );
}