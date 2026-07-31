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
  const [testResults, setTestResults] = useState<Record<string, string>>({});

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
    setTestResults((m) => ({ ...m, [p.name]: '...' }));
    try {
      const r = await testProvider(req, p.name);
      setTestResults((m) => ({
        ...m,
        [p.name]: r.ok ? '✓ OK' : `✗ ${r.hint || r.error || ('HTTP ' + r.status)}`,
      }));
    } catch (e: any) {
      setTestResults((m) => ({ ...m, [p.name]: `✗ ${e.message}` }));
    }
  };

  const handleToggle = async (p: ProviderInfo) => {
    try {
      await updateProvider(req, { name: p.name, enabled: !p.enabled });
      await refresh();
    } catch (e: any) { setError(e.message); }
  };

  const handleSetDefault = async (modelName: string) => {
    setBusy(true); setError(null);
    try {
      await setDefault(req, modelName);
      await refresh();
    } catch (e: any) { setError(e.message); }
    finally { setBusy(false); }
  };

  return (
    <div className="provider-panel">
      <div className="provider-header">
        <h2>Providers</h2>
        {onClose && <button className="icon-btn" onClick={onClose} title="Close">✕</button>}
      </div>
      {error && <div className="error-banner">{error}</div>}

      {mode === 'list' && (
        <>
          <div className="provider-toolbar">
            <span className="provider-count">
              {providers.length} provider{providers.length === 1 ? '' : 's'}
              {' · '}{models.length} model{models.length === 1 ? '' : 's'}
            </span>
            <button className="primary-btn" disabled={busy} onClick={() => setMode('add')}>+ Add Provider</button>
          </div>

          {providers.length === 0 && (
            <div className="empty-state">
              <p>No providers configured.</p>
              <p>Click "Add Provider" to set up OpenAI / Anthropic / Ollama / etc.</p>
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
                    {!p.enabled && <span className="badge disabled">disabled</span>}
                    {p.has_api_key ? <span className="badge ok">key set</span> : <span className="badge warn">no key</span>}
                  </div>
                </div>
                <div className="provider-card-body">
                  <div className="provider-row"><span>URL:</span><code>{p.base_url}</code></div>
                  <div className="provider-row">
                    <span>Models:</span>
                    <ul className="provider-models">
                      {p.models.map((m) => (
                        <li key={m}>
                          <code>{m}</code>
                          <button className="link-btn" disabled={busy} onClick={() => handleSetDefault(m)} title="Set as default">★</button>
                        </li>
                      ))}
                    </ul>
                  </div>
                  {testResults[p.name] && (
                    <div className={`provider-test ${testResults[p.name].startsWith('✓') ? 'ok' : 'fail'}`}>
                      {testResults[p.name]}
                    </div>
                  )}
                </div>
                <div className="provider-card-actions">
                  <button className="secondary-btn" disabled={busy} onClick={() => handleTest(p)}>Test</button>
                  <button className="secondary-btn" disabled={busy} onClick={() => handleToggle(p)}>
                    {p.enabled ? 'Disable' : 'Enable'}
                  </button>
                  <button className="danger-btn" disabled={busy} onClick={() => handleDelete(p)}>Delete</button>
                </div>
              </div>
            ))}
          </div>
        </>
      )}

      {mode === 'add' && (
        <form className="provider-form" onSubmit={submitAdd}>
          <h3>Add Provider</h3>
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
          <div className="form-actions">
            <button type="button" className="secondary-btn" onClick={() => setMode('list')}>Cancel</button>
            <button type="submit" className="primary-btn" disabled={busy || !name.trim() || !baseUrl.trim()}>
              {busy ? 'Adding...' : 'Add'}
            </button>
          </div>
        </form>
      )}

      {busy && <div className="overlay-loading">working...</div>}
    </div>
  );
}