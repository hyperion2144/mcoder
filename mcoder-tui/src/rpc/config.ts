// 设计文档 §provider: Provider CRUD RPC 客户端封装
// 三端共享，platform-agnostic

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

/** 列出所有供应商 */
export async function listProviders(req: RequestFn): Promise<ProviderInfo[]> {
  return (await req('config.list_providers')) || [];
}

/** 列出所有模型（含从 providers 展开） */
export async function listModels(req: RequestFn): Promise<ModelInfo[]> {
  return (await req('config.list_models')) || [];
}

/** 列出支持的协议 */
export async function listProtocols(req: RequestFn): Promise<ProtocolInfo[]> {
  return (await req('config.list_protocols')) || [];
}

/** 添加供应商 */
export async function addProvider(
  req: RequestFn,
  args: { name: string; protocol: string; base_url: string; api_key: string; models: string[] },
): Promise<void> {
  await req('config.add_provider', args);
}

/** 更新供应商字段 */
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

/** 删除供应商 */
export async function deleteProvider(req: RequestFn, name: string): Promise<void> {
  await req('config.delete_provider', { name });
}

/** 设置默认模型 */
export async function setDefault(req: RequestFn, model: string, provider?: string): Promise<void> {
  await req('config.set_default', { model, provider: provider ?? null });
}

/** 测试供应商连通性 */
export async function testProvider(
  req: RequestFn,
  name: string,
): Promise<{ ok: boolean; status?: number; url?: string; error?: string; hint?: string }> {
  return await req('config.test_provider', { name });
}