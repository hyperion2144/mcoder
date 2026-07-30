// 设计文档 §6.12: rpc/types.ts - 平台无关的 JSON-RPC 类型定义
// TUI / Tauri / Capacitor 共用

import type { LLMUsage } from './sessionSnapshot.js';

export interface ContentBlock {
  type: 'text' | 'tool_use' | 'tool_result' | 'image';
  text?: string;
  id?: string;
  name?: string;
  args?: any;
  output?: any;
  /** 图片文件路径（type='image' 时存在） */
  path?: string;
  /** 图片 MIME 类型，如 image/png（type='image' 时存在） */
  media_type?: string;
}

export interface Message {
  /** 消息唯一 id（服务端生成；客户端占位消息可缺省） */
  id?: string;
  /** 父消息 id（消息树分叉用；客户端占位消息可缺省） */
  parent_id?: string | null;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: ContentBlock[];
  /** 该轮 LLM 调用的 token 用量（仅 assistant 消息携带） */
  usage?: LLMUsage;
}

export interface SessionMeta {
  session_id: string;
  project_path: string;
  title: string;
  created_at: string;
  model: string;
  /** 当前消息树分支末端消息 id（null=空会话） */
  current_head_id?: string | null;
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

/// 消息树节点（session.tree 返回）
export interface MessageTreeNode {
  id: string;
  parent_id: string | null;
  role: string;
  preview: string;
  is_head: boolean;
}

/// 消息树（session.tree 返回）
export interface MessageTree {
  nodes: MessageTreeNode[];
  head_id: string | null;
}
