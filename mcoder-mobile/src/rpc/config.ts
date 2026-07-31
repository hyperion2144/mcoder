// 设计文档 §provider: Mobile 端 Provider CRUD RPC 客户端封装
// 与 TUI/Desktop 端的同名模块 API 等价；各自独立维护、不跨包共享

export interface ProviderInfo {
  name: string;
  display_name: string;
  protocol: string;
  base_url: string;
  has_api_key: boolean;
  enabled: boolean;
  models: string[];
}

export interface ModelInfo {
  name: string;
  display_name: string;
  protocol: string;
  context_window?: number;
  max_tokens?: number;
  source: 'models' | 'provider';
  provider?: string;
}

export interface ProtocolInfo {
  id: string;
  name: string;
  default_url: string;
}

type RequestFn = (method: string, params?: any) => Promise<any>;

export async function listProviders(req: RequestFn): Promise<ProviderInfo[]> {
  return (await req('config.list_providers')) || [];
}

export async function listModels(req: RequestFn): Promise<ModelInfo[]> {
  return (await req('config.list_models')) || [];
}

export async function listProtocols(req: RequestFn): Promise<ProtocolInfo[]> {
  return (await req('config.list_protocols')) || [];
}

export async function addProvider(
  req: RequestFn,
  args: { name: string; protocol: string; base_url: string; api_key: string; models: string[] },
): Promise<void> {
  await req('config.add_provider', args);
}

export async function updateProvider(
  req: RequestFn,
  args: {
    name: string;
    protocol?: string;
    base_url?: string;
    api_key?: string;
    models?: string[];
    enabled?: boolean;
  },
): Promise<void> {
  await req('config.update_provider', args);
}

export async function deleteProvider(req: RequestFn, name: string): Promise<void> {
  await req('config.delete_provider', { name });
}

export async function setDefault(req: RequestFn, model: string, provider?: string): Promise<void> {
  await req('config.set_default', { model, provider: provider ?? null });
}

export async function testProvider(
  req: RequestFn,
  name: string,
): Promise<{ ok: boolean; status?: number; url?: string; error?: string; hint?: string }> {
  return await req('config.test_provider', { name });
}