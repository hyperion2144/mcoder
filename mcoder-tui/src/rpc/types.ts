// 设计文档 §6.12: rpc/types.ts - 平台无关的 JSON-RPC 类型定义
// TUI / Tauri / Capacitor 共用

export interface ContentBlock {
  type: 'text' | 'tool_use' | 'tool_result';
  text?: string;
  id?: string;
  name?: string;
  args?: any;
  output?: any;
}

export interface Message {
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: ContentBlock[];
}

export interface SessionMeta {
  session_id: string;
  project_path: string;
  title: string;
  created_at: string;
  model: string;
}

export interface JsonRpcRequest {
  jsonrpc: '2.0';
  id: number | string;
  method: string;
  params?: any;
}

export interface JsonRpcResponse {
  jsonrpc: '2.0';
  id: number | string | null;
  result?: any;
  error?: { code: number; message: string; data?: any };
}

export interface JsonRpcNotification {
  jsonrpc: '2.0';
  method: string;
  params?: any;
}
