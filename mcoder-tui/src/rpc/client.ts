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
  onReconnect?: (snapshot?: unknown) => void;
  /// Phase 5c: 拿到 reconnect 拿到的最新消息数（用于 hydrate 增量 append）
  getCurrentMessageCount?: () => number;
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
  /** notification 去重键：method + (主要 id 字段)。已用过的丢弃。 */
  private seenNotifications = new Set<string>();
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
    // 每次连接建立时清空去重缓存：
    // - 重连后可以重新接收同一 ask 的 pending；
    // - 二次 review（issue 9）：不要依赖 seenNotifications 持久去重来保证消息正确性。
    //   Ask 通知由 store 幂等保护（setPendingIdempotent）；服务端真实的 Message
    //   事件按 server 顺序到达，客户端不要私自丢弃。
    this.seenNotifications.clear();
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
              // 二次 review（issue 9）：
              // 不再用客户端持久去重阻挡消息流 —— 这会让重连/网络抖动期间
              // 服务端的真实 Message 事件被错误丢弃，导致消息历史不完整。
              // Ask 相关通知的幂等由 store 内部保证
              // （setPendingIdempotent / setSubmissionIfMatch / clearPendingByIds）。
              for (const h of this.notifHandlers.slice()) {
                try {
                  h(notif);
                } catch {
                  /* swallow handler errors */
                }
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

  /// 生成 notification 去重键
  private notificationKey(notif: JsonRpcNotification): string {
    const m = notif.method;
    const p = notif.params || {};
    // 用 method + 关键 id 字段构造 key
    if (m === 'session.ask_pending' || m === 'session.ask_answered' || m === 'session.ask_cancelled') {
      const sid = (p as any).session_id || '';
      const aid = (p as any).ask_id || '';
      const tcid = (p as any).tool_call_id || '';
      return `${m}|${sid}|${aid}|${tcid}`;
    }
    if (m === 'message') {
      const sid = (p as any).session_id || '';
      const msg = (p as any).message || {};
      return `${m}|${sid}|${msg.role || ''}|${JSON.stringify(msg.content || [])}`;
    }
    if (m === 'tool_call_start' || m === 'tool_call_done') {
      const sid = (p as any).session_id || '';
      const name = (p as any).name || '';
      return `${m}|${sid}|${name}`;
    }
    // 全局事件：直接以 method 去重
    return `${m}|`;
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
    let snapshot: unknown = undefined;
    if (this.reconnectOpts.sessionId) {
      try {
        // Phase 5c: 重新 attach 拿 snapshot，**仅在同 session 时**用 offset
        // 增量 hydrate，避免重连瞬间闪旧消息
        const params: Record<string, unknown> = { session_id: this.reconnectOpts.sessionId };
        const cur = this.reconnectOpts.getCurrentMessageCount?.();
        if (typeof cur === 'number' && cur > 0) {
          params.offset = cur;
        }
        snapshot = await this.request('session.attach', params);
      } catch {
        // ignore — 调用方按需 fallback
      }
    }
    this.reconnectOpts.onReconnect?.(snapshot);
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

  /** 取消订阅（issue 6）：返回 unsubscribe 函数 */
  offNotification(handler: NotificationHandler): void {
    const idx = this.notifHandlers.indexOf(handler);
    if (idx >= 0) this.notifHandlers.splice(idx, 1);
  }

  /** 移除所有 notification handler；用于 client 重连或销毁时清理 */
  clearNotificationHandlers(): void {
    this.notifHandlers.length = 0;
  }

  close() {
    this.intentionallyClosed = true;
    this.stopHeartbeat();
    this.clearNotificationHandlers();
    this.pending.clear();
    this.seenNotifications.clear();
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