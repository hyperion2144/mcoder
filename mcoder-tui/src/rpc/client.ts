// 设计文档 §6.12: rpc/client.ts - 平台无关的 WS 客户端
// TUI 用 ws 库；Tauri/Capacitor 可替换为原生 WebSocket

import type { JsonRpcRequest, JsonRpcResponse, JsonRpcNotification } from './types.js';

type ResponseHandler = (resp: JsonRpcResponse) => void;
type NotificationHandler = (notif: JsonRpcNotification) => void;

// 设计文档 §5.6: 心跳与重连常量
const HEARTBEAT_INTERVAL_MS = 30_000;
const MAX_RECONNECT_ATTEMPTS = 5;
const RECONNECT_BASE_DELAY_MS = 1_000;

export interface ReconnectOptions {
  sessionId?: string;
  onReconnect?: () => void;
}

/// 抽象的传输层接口（平台无关）
/// TUI 用 ws；Tauri 可用 Tauri WebSocket；Capacitor 用 @capacitor-community/http
export interface Transport {
  send(data: string): void;
  close(): void;
  onOpen?: () => void;
  onMessage?: (data: string) => void;
  onClose?: () => void;
  onError?: (err: Error) => void;
}

export class WsClient {
  private transport!: Transport;
  private reqId = 0;
  private pending = new Map<number, ResponseHandler>();
  private notifHandlers: NotificationHandler[] = [];
  private onConnect: () => void;
  private onDisconnect: () => void;
  private heartbeatTimer: any | null = null;
  private reconnectAttempts = 0;
  private intentionallyClosed = false;
  private reconnectOpts: ReconnectOptions = {};
  private url: string;
  private token: string;
  private transportFactory: (url: string, handlers: {
    onOpen: () => void;
    onMessage: (data: string) => void;
    onClose: () => void;
    onError: (err: Error) => void;
  }) => Transport;

  constructor(
    url: string,
    token: string,
    onConnect: () => void,
    onDisconnect: () => void,
    transportFactory?: (url: string, handlers: {
      onOpen: () => void;
      onMessage: (data: string) => void;
      onClose: () => void;
      onError: (err: Error) => void;
    }) => Transport,
  ) {
    this.url = url;
    this.token = token;
    this.onConnect = onConnect;
    this.onDisconnect = onDisconnect;
    this.transportFactory = transportFactory || defaultWsTransportFactory;
  }

  /// 设计文档 §5.2: 连接握手
  connect(): Promise<void> {
    this.intentionallyClosed = false;
    return new Promise((resolve, reject) => {
      let authResolved = false;
      this.transport = this.transportFactory(this.url, {
        onOpen: () => {
          // 发送 auth 请求
          const authReq: JsonRpcRequest = {
            jsonrpc: '2.0',
            id: 0,
            method: 'auth',
            params: { token: this.token },
          };
          this.transport.send(JSON.stringify(authReq));
        },
        onMessage: (data: string) => {
          try {
            const msg = JSON.parse(data);
            if (msg.id !== undefined && msg.id !== null) {
              const handler = this.pending.get(Number(msg.id));
              if (handler) {
                handler(msg as JsonRpcResponse);
                this.pending.delete(Number(msg.id));
              } else if (msg.id === 0 && msg.result && msg.result.authenticated) {
                // auth ack
                this.reconnectAttempts = 0;
                this.startHeartbeat();
                this.onConnect();
                authResolved = true;
                resolve();
              }
            } else if (msg.method) {
              const notif = msg as JsonRpcNotification;
              for (const h of this.notifHandlers) {
                h(notif);
              }
            }
          } catch {}
        },
        onClose: () => {
          this.stopHeartbeat();
          this.onDisconnect();
          if (!this.intentionallyClosed) {
            this.scheduleReconnect();
          }
        },
        onError: (err: Error) => {
          if (!authResolved) reject(err);
        },
      });
    });
  }

  /// 设计文档 §5.6: 客户端每 30s 发 ping
  private startHeartbeat() {
    this.stopHeartbeat();
    this.heartbeatTimer = setInterval(() => {
      try {
        const req: JsonRpcRequest = { jsonrpc: '2.0', id: ++this.reqId, method: 'ping' };
        this.transport.send(JSON.stringify(req));
      } catch {}
    }, HEARTBEAT_INTERVAL_MS);
  }

  private stopHeartbeat() {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  /// 设计文档 §5.6: 断线自动重连（指数退避，最多 5 次）
  private scheduleReconnect() {
    if (this.reconnectAttempts >= MAX_RECONNECT_ATTEMPTS) return;
    this.reconnectAttempts += 1;
    const delay = RECONNECT_BASE_DELAY_MS * Math.pow(2, this.reconnectAttempts - 1);
    setTimeout(() => {
      if (this.intentionallyClosed) return;
      this.doReconnect().catch(() => {
        this.scheduleReconnect();
      });
    }, delay);
  }

  private async doReconnect(): Promise<void> {
    await this.connect();
    if (this.reconnectOpts.sessionId) {
      try {
        await this.request('session.attach', { session_id: this.reconnectOpts.sessionId });
      } catch {}
    }
    this.reconnectOpts.onReconnect?.();
  }

  setReconnectSession(sessionId: string | undefined) {
    this.reconnectOpts.sessionId = sessionId;
  }

  request(method: string, params?: any): Promise<any> {
    return new Promise((resolve, reject) => {
      const id = ++this.reqId;
      const req: JsonRpcRequest = { jsonrpc: '2.0', id, method, params };
      this.pending.set(id, (resp: JsonRpcResponse) => {
        if (resp.error) {
          reject(new Error(resp.error.message));
        } else {
          resolve(resp.result);
        }
      });
      try {
        this.transport.send(JSON.stringify(req));
      } catch (e: any) {
        reject(new Error(`transport send failed: ${e.message}`));
      }
    });
  }

  onNotification(handler: NotificationHandler) {
    this.notifHandlers.push(handler);
  }

  close() {
    this.intentionallyClosed = true;
    this.stopHeartbeat();
    try { this.transport?.close(); } catch {}
  }
}

/// 默认的 Transport 工厂
/// 浏览器/Tauri/Capacitor：优先用全局 WebSocket
/// Node.js (TUI)：全局无 WebSocket 时 fallback 到 ws 库
/// 调用方也可传入自定义 factory（如需 Tauri 专属 API）
export function defaultWsTransportFactory(
  url: string,
  handlers: {
    onOpen: () => void;
    onMessage: (data: string) => void;
    onClose: () => void;
    onError: (err: Error) => void;
  },
): Transport {
  // 优先用全局 WebSocket（浏览器/Tauri webview 原生支持，Node 18+ 也有全局 WebSocket）
  const G: any = (typeof globalThis !== 'undefined' ? globalThis : undefined) as any;
  const GlobalWebSocket = G?.WebSocket;
  if (GlobalWebSocket) {
    const ws = new GlobalWebSocket(url);
    ws.onopen = handlers.onOpen;
    ws.onmessage = (e: any) => {
      const d = e?.data;
      handlers.onMessage(typeof d === 'string' ? d : d?.toString?.() || '');
    };
    ws.onclose = handlers.onClose;
    ws.onerror = () => handlers.onError(new Error('WebSocket error'));
    return {
      send: (data: string) => ws.send(data),
      close: () => ws.close(),
    };
  }

  // Node.js fallback：用 ws 库
  let ws: any;
  try {
    const g: any = (typeof globalThis !== 'undefined' ? globalThis : undefined) as any;
    if (typeof g?.require === 'undefined') {
      handlers.onError(new Error('no WebSocket implementation available'));
      return { send: () => {}, close: () => {} };
    }
    const WebSocket = g.require('ws');
    ws = new WebSocket(url);
    ws.on('open', handlers.onOpen);
    ws.on('message', (data: any) => handlers.onMessage(data.toString()));
    ws.on('close', handlers.onClose);
    ws.on('error', handlers.onError);
  } catch (e: any) {
    handlers.onError(new Error(`failed to create WebSocket: ${e.message}`));
  }

  return {
    send: (data: string) => ws?.send(data),
    close: () => ws?.close(),
  };
}
