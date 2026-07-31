// 设计文档 §8.6.1: 桌面端主应用
// 两阶段视图：项目选择页 → 会话页（tab 组织同项目的多个会话）
// 会话页内保留原三栏布局：左（文件树）| 中（聊天）| 右（图谱/Diff/文件预览 标签页）
// 复用 TUI 的 rpc/store/commands/utils 逻辑层

import React, { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { WsClient } from '@mcoder/shared/rpc/client.js';
import { useSessionStore, useMessagesStore } from '@mcoder/shared/store/index.js';
import { dispatchSlashCommand } from '@mcoder/shared/commands/index.js';
import { parsePairingString } from '@mcoder/shared/utils/pairing.js';
import { AskCard, AskCardSummary, useAskStore } from '@mcoder/shared/ask/index.js';
import { usePermissionStore, PermissionCardReact } from '@mcoder/shared/permission/index.js';
import { hasToolUse } from '@mcoder/shared/ask/messages.js';
import { ASK_USER_TOOL } from '@mcoder/shared/ask/types.js';
/// 设计文档 §8.8: 权限审批占位 tool name（虚拟，仅用于 desktop/mobile 渲染识别）
const PERMISSION_TOOL_NAME = '__permission_pending__';
import type { SessionMeta } from '@mcoder/shared/rpc/types.js';
import { hydrateSnapshot, type SessionSnapshot } from '@mcoder/shared/rpc/sessionSnapshot.js';
import { clearSessionUiState } from '@mcoder/shared/store/clearSessionUiState.js';
import { useDesktopStore } from './store/index.js';
import { FileTree } from './components/FileTree.js';
import { GraphView } from './components/GraphView.js';
import { DiffViewer } from './components/DiffViewer.js';
import { ProjectList } from './components/ProjectList.js';
import { SessionTabs } from './components/SessionTabs.js';
import { PlanPanel } from './components/PlanPanel.js';
import { TodoPanel } from './components/TodoPanel.js';
import { TodoSummaryBar } from './components/TodoSummaryBar.js';
import { ResumeBar } from './components/ResumeBar.js';
import { TreeView } from './components/TreeView.js';
import { ProviderPanel } from './components/ProviderPanel.js';
import { ToolCard } from '@mcoder/shared/toolCard/ToolCardHtml.js';
import { formatUsageDelta } from '@mcoder/shared/utils/format.js';

type RightPanel = 'graph' | 'diff' | 'file' | 'tree' | 'none';

// 按创建时间倒序（最近在前）
function sortByRecent(list: SessionMeta[]): SessionMeta[] {
  return [...list].sort((a, b) => {
    const ta = new Date(a.created_at).getTime() || 0;
    const tb = new Date(b.created_at).getTime() || 0;
    return tb - ta;
  });
}

// 简单的 markdown 行内渲染：`code` → <code>，**bold** → <strong>
function renderInline(text: string): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  const regex = /(`[^`]+`|\*\*[^*]+\*\*)/g;
  let last = 0;
  let match: RegExpExecArray | null;
  let key = 0;
  while ((match = regex.exec(text)) !== null) {
    if (match.index > last) {
      nodes.push(<span key={key++}>{text.slice(last, match.index)}</span>);
    }
    const token = match[0];
    if (token.startsWith('`')) {
      nodes.push(<code key={key++} className="md-code">{token.slice(1, -1)}</code>);
    } else {
      nodes.push(<strong key={key++}>{token.slice(2, -2)}</strong>);
    }
    last = match.index + token.length;
  }
  if (last < text.length) {
    nodes.push(<span key={key++}>{text.slice(last)}</span>);
  }
  return nodes;
}

// 单条消息渲染
function MessageItem({
  msg,
  index,
  client,
  currentSessionId,
  resultsById,
}: {
  msg: any;
  index: number;
  client: WsClient | null;
  currentSessionId: string | null;
  resultsById: Map<string, any>;
}) {
  const roleLabel: Record<string, string> = {
    user: 'You', assistant: 'AI', system: 'SYS', tool: 'TOOL',
  };
  const label = roleLabel[msg.role] || msg.role;
  return (
    <div className={`message message-${msg.role}`}>
      <div className="message-avatar">{label.charAt(0)}</div>
      <div className="message-body">
        <div className="message-header">
          <span className="message-role">{label}</span>
        </div>
        <div className="message-content">
          {msg.content.map((block: any, j: number) => {
            if (block.type === 'text') {
              return (
                <div key={j} className="msg-text">
                  {block.text.split('\n').map((line: string, k: number) => (
                    <div key={k}>{renderInline(line)}</div>
                  ))}
                </div>
              );
            }
            if (block.type === 'tool_use') {
              if (block.name === ASK_USER_TOOL && client && currentSessionId) {
                return (
                  <AskCard
                    key={j}
                    ask_id={block.id || ''}
                    tool_call_id={block.id || ''}
                    session_id={currentSessionId}
                    client={client}
                    onError={(m) => useMessagesStore.getState().setError(m)}
                  />
                );
              }
              // 设计文档 §8.8: 权限审批卡片
              if (block.name === PERMISSION_TOOL_NAME && client && currentSessionId) {
                return (
                  <PermissionCardReact
                    key={j}
                    request_id={block.id || ''}
                    tool_call_id={block.id || ''}
                    session_id={currentSessionId}
                    client={client}
                    onError={(m: string) => useMessagesStore.getState().setError(m)}
                  />
                );
              }
              const result = block.id ? resultsById.get(block.id) || null : null;
              return (
                <ToolCard
                  key={j}
                  block={block}
                  resultBlock={result}
                />
              );
            }
            if (block.type === 'tool_result') {
              // tool_result 由 ToolCard 内联显示，不单独渲染
              return null;
            }
            if (block.type === 'image' && block.path) {
              const src = block.path.startsWith('data:') || block.path.startsWith('http')
                ? block.path
                : convertFileSrc(block.path);
              return (
                <div key={j} className="msg-image-wrap">
                  <img src={src} className="msg-image" alt={block.path} />
                </div>
              );
            }
            return null;
          })}
        </div>
        {msg.role === 'assistant' && msg.usage && formatUsageDelta(msg.usage) && (
          <div className="message-usage">↳ {formatUsageDelta(msg.usage)}</div>
        )}
      </div>
    </div>
  );
}

// Phase 5c: 构造 hydrateSnapshot 需要的 store 注入（reconnect 复用 attach 的逻辑）
function buildHydrateStore() {
  const sessionStore = useSessionStore.getState();
  const msgStore = useMessagesStore.getState();
  return {
    setCurrentSessionId: (id: string) => sessionStore.setCurrentSession(id),
    setMessages: (m: any[]) => msgStore.setMessages(m),
    appendMessages: (m: any[]) => msgStore.appendMessages(m),
    getMessages: () => msgStore.messages,
    setRole: (r: string) => sessionStore.setRole(r),
    setModel: (m: string) => sessionStore.setModel(m),
    setProjectPath: (p: string) => sessionStore.setProjectPath(p),
    setContextUsage: (used: number, _w: number) =>
      sessionStore.setContextUsage(used, sessionStore.contextWindow || 0),
    setPendingPlan: (p: any) => sessionStore.setPendingPlan(p),
    setPendingTodos: (t: any[]) => sessionStore.setPendingTodos(t),
    setBackgroundTasks: (t: any[]) => sessionStore.setBackgroundTasks(t),
    setPendingAskFromSnapshot: (ask: any) => {
      const askStore = useAskStore.getState();
      if (ask === null) {
        const cur = useSessionStore.getState().currentSessionId;
        if (cur) askStore.clearSession(cur);
        return;
      }
      askStore.setPendingAskFromSnapshot(ask);
    },
    clearAskSession: (sid: string) => useAskStore.getState().clearSession(sid),
    replaceTodosFromSnapshot: (_todos: any[]) => {
      // setPendingTodos 已替换全部，无需额外 replace
    },
  };
}

export function App() {
  const [client, setClient] = useState<WsClient | null>(null);
  const [input, setInput] = useState('');
  const [rightPanel, setRightPanel] = useState<RightPanel>('none');
  const [previewFile, setPreviewFile] = useState<{ path: string; content: string } | null>(null);
  const [pendingImages, setPendingImages] = useState<{data: string; media_type: string; name: string; preview: string}[]>([]);
  const [showModelDropdown, setShowModelDropdown] = useState(false);
  const [availableModels, setAvailableModels] = useState<{name: string; description?: string; model?: string; context_window?: number}[]>([]);
  const [showSettings, setShowSettings] = useState(false);
  const [settingsTab, setSettingsTab] = useState<'general' | 'providers'>('general');
  const [remoteInput, setRemoteInput] = useState('');
  const [configValues, setConfigValues] = useState<Record<string, any>>({});
  const fileInputRef = useRef<HTMLInputElement>(null);
  const sessionStore = useSessionStore();
  const msgStore = useMessagesStore();
  const desktop = useDesktopStore();
  // Phase 5c: 跟踪当前 session id（用于切 session 时清旧）
  const currentSessionIdRef = useRef<string | null>(null);
  // 暴露 useEffect 内的 setupClient，供设置面板切换远程服务器时复用
  const setupClientRef = useRef<((url: string, token: string) => void) | null>(null);

  // 平台检测：用于区分 macOS（交通灯在左）和 Windows（窗口按钮在右）
  const platform = useMemo(() => {
    const ua = (typeof navigator !== 'undefined' ? navigator.userAgent : '').toLowerCase();
    if (ua.includes('mac')) return 'mac';
    if (ua.includes('win')) return 'win';
    return 'other';
  }, []);

  const { view, currentProject, openTabs } = desktop;
  const { sessions, currentSessionId, pendingPlan, pendingTodos } = sessionStore;

  // attach 到指定会话：调用 session.attach 拿 SessionSnapshot，再用 hydrateSnapshot 一次性 hydrate
  // Phase 2: 不再单独调 ask.pending / todo.list / task.list —— 全部来自 snapshot
  // Phase 5c: 切 session 前先 clearSessionUiState 旧 session 避免闪旧 Todo/Plan/Ask
  const attachSession = useCallback(async (sessionId: string) => {
    if (!client) return;
    // Phase 5c: 切 session 时先清掉旧 session 的 UI
    const oldSid = currentSessionIdRef.current;
    if (oldSid && oldSid !== sessionId) {
      clearSessionUiState({ sessionId: oldSid });
    }
    try {
      const snapshot = await client.request('session.attach', { session_id: sessionId }) as SessionSnapshot;
      client.setReconnectSession(sessionId);
      // Phase 5c: 切完才把"current"切到新 session；避免旧 ask 流影响新 UI
      currentSessionIdRef.current = sessionId;
      hydrateSnapshot({
        sessionId,
        snapshot,
        store: {
          setCurrentSessionId: (id) => sessionStore.setCurrentSession(id),
          setMessages: (m) => msgStore.setMessages(m),
          appendMessages: (m) => msgStore.appendMessages?.(m),
          getMessages: () => msgStore.messages,
          setRole: (r) => sessionStore.setRole(r),
          setModel: (m) => sessionStore.setModel(m),
          setProjectPath: (p) => sessionStore.setProjectPath(p),
          setContextUsage: (used, window) => sessionStore.setContextUsage(used, window || sessionStore.contextWindow || 0),
          setUsage: (usage, cost) => sessionStore.setUsage(usage, cost),
          setPendingPlan: (p) => sessionStore.setPendingPlan(p),
          setPendingTodos: (t) => sessionStore.setPendingTodos(t),
          setBackgroundTasks: (t) => sessionStore.setBackgroundTasks(t),
          setPendingAskFromSnapshot: (ask) => {
            const askStore = useAskStore.getState();
            if (ask === null) {
              // 清空当前 session 的 pending
              askStore.clearSession(sessionId);
              return;
            }
            askStore.setPendingAskFromSnapshot(ask);
          },
          clearAskSession: (sid) => {
            useAskStore.getState().clearSession(sid);
          },
          replaceTodosFromSnapshot: (_todos) => {
            // 当前 store 设计：setPendingTodos 已替换全部，无需额外 replace
          },
        },
      });
      // 同步 loop_state / can_resume（由 hydrateSnapshot 不在 store 上的字段）
      sessionStore.setLoopState(snapshot.session.loop_state, snapshot.session.stop_reason);
      sessionStore.setCanResume(snapshot.can_resume);
      sessionStore.setVersion(snapshot.session.version);
      sessionStore.setLspServers(snapshot.session.lsp_servers);
      // 若 snapshot 带 pending ask 且消息流中没有 tool_use，补一条占位 assistant message
      const ask = snapshot.pending_ask;
      if (ask && !hasToolUse(msgStore.messages, ask.tool_call_id)) {
        msgStore.addMessage({
          role: 'assistant',
          content: [{ type: 'tool_use', id: ask.tool_call_id, name: ASK_USER_TOOL, args: ask.request }],
        });
      }
    } catch (e: any) {
      msgStore.setError(`session.attach failed: ${e.message}`);
    }
  }, [client, sessionStore, msgStore]);

  // 进入某项目的会话页
  const enterProject = useCallback(async (
    projectPath: string,
    allSessions: SessionMeta[],
    selectSessionId?: string,
  ) => {
    const projSessions = sortByRecent(allSessions.filter((s) => s.project_path === projectPath));
    desktop.setCurrentProject(projectPath);
    desktop.setOpenTabs(projSessions.map((s) => s.session_id));
    desktop.setView('sessions');
    msgStore.setMessages([]);
    setRightPanel('none');
    setPreviewFile(null);
    const idToSelect = selectSessionId || projSessions[0]?.session_id;
    if (idToSelect) {
      await attachSession(idToSelect);
    } else {
      sessionStore.setCurrentSession(null);
      client?.setReconnectSession(undefined);
    }
  }, [desktop, attachSession, msgStore, sessionStore, client]);

  useEffect(() => {
    let cancelled = false;
    let cleanupFn: (() => void) | undefined;

    // 公共的客户端初始化逻辑：创建 WsClient、注册 reconnect/notif handler、连接
    function setupClient(url: string, token: string) {
      if (cancelled) return;

      const c = new WsClient(
        url,
        token,
        () => sessionStore.setConnected(true),
        () => sessionStore.setConnected(false),
      );
      setClient(c);

      // Phase 5c: 注册 reconnect handler，断线重连后用 offset 增量 hydrate
      (c as any).reconnectOpts = {
        sessionId: undefined, // 由 setReconnectSession 设置
        onReconnect: (snapshot: unknown) => {
          if (!snapshot) return;
          const sid = useSessionStore.getState().currentSessionId;
          if (!sid) return;
          try {
            hydrateSnapshot({
              sessionId: sid,
              snapshot: snapshot as SessionSnapshot,
              currentMessageCount: useMessagesStore.getState().messages.length,
              store: buildHydrateStore(),
            });
          } catch (e: any) {
            msgStore.setError(`reconnect hydrate failed: ${e.message}`);
          }
        },
        getCurrentMessageCount: () => useMessagesStore.getState().messages.length,
      };

      c.connect().then(async () => {
        try {
          const allSessions: SessionMeta[] = await c.request('sessions.list');
          sessionStore.setSessions(allSessions);
          desktop.setView('projects');
        } catch {}
      }).catch(e => {
        msgStore.setError(`Connection failed: ${e.message}`);
      });

      const notifHandler = (notif: any) => {
        switch (notif.method) {
          case 'message':
            msgStore.addMessage(notif.params.message);
            if (notif.params.message.role === 'assistant') {
              msgStore.setStreaming(false);
            }
            break;
          case 'session.mode_event':
            sessionStore.setRole(notif.params.role);
            break;
          case 'session.model_changed':
            sessionStore.setModel(notif.params.model);
            break;
          case 'session.plan_created':
            sessionStore.setPendingPlan(notif.params.plan);
            break;
          case 'session.todo_updated':
            sessionStore.setPendingTodos(notif.params.todos);
            break;
          case 'session.ask_pending': {
            // 二次 review（issue 6/9）：仅更新 store；仅当消息流中无对应 tool_use 时才追加
            const p = notif.params;
            useAskStore.getState().setPendingIdempotent({
              ask_id: p.ask_id,
              tool_call_id: p.tool_call_id,
              session_id: p.session_id,
              request: p.request,
              created_at: Date.now(),
            });
            if (!hasToolUse(msgStore.messages, p.tool_call_id)) {
              msgStore.addMessage({
                role: 'assistant',
                content: [{ type: 'tool_use', id: p.tool_call_id, name: ASK_USER_TOOL, args: p.request }],
              });
            }
            break;
          }
          case 'permission.pending': {
            // 设计文档 §8.8: 权限审批 pending
            const p = notif.params;
            if (p && p.request) {
              usePermissionStore.getState().setPending(p.session_id, p.request);
              if (!hasToolUse(msgStore.messages, p.request.tool_call_id)) {
                // 插入占位 tool_use block，用虚拟工具名 PERMISSION_TOOL_NAME 让渲染分支识别
                msgStore.addMessage({
                  role: 'assistant',
                  content: [{
                    type: 'tool_use',
                    id: p.request.tool_call_id,
                    name: PERMISSION_TOOL_NAME,
                    args: { real_tool_name: p.request.tool_name, ...p.request },
                  }],
                });
              }
            }
            break;
          }
          case 'permission.resolved': {
            // 设计文档 §8.8: 权限审批决议
            const p = notif.params;
            if (p && p.request_id && p.decision) {
              const decision = p.decision;
              usePermissionStore.getState().setResolved(p.session_id, p.request_id, {
                type: decision.type === 'Allow' ? 'allow'
                  : decision.type === 'AlwaysAllow' ? 'always_allow'
                  : 'deny',
                reason: decision.reason,
              });
            }
            break;
          }
          case 'session.ask_answered': {
            const p = notif.params;
            const ok = useAskStore.getState().setSubmissionIfMatch(
              p.session_id,
              p.ask_id,
              p.tool_call_id,
              p.submission,
            );
            if (ok) {
              const haveResult = msgStore.messages.some((m) =>
                m.content.some(
                  (b: any) => b.type === 'tool_result' && b.id === p.tool_call_id,
                ),
              );
              if (!haveResult) {
                msgStore.addMessage({
                  role: 'tool',
                  content: [{ type: 'tool_result', id: p.tool_call_id, output: p.result }],
                });
              }
            }
            break;
          }
          case 'session.ask_cancelled': {
            // 校验 ask_id + tool_call_id 后清空 pending（issue 8）
            const p = notif.params;
            if (p && p.ask_id && p.tool_call_id) {
              useAskStore.getState().clearPendingByIds(
                p.session_id,
                p.ask_id,
                p.tool_call_id,
              );
            }
            break;
          }
          case 'session.done':
            sessionStore.setLoopState('stopped', notif.params.reason);
            sessionStore.setCanResume(true);
            msgStore.setStreaming(false);
            break;
          case 'session.usage_updated': {
            const p = notif.params;
            if (p && p.cumulative) {
              const used = (p.cumulative.prompt_tokens || 0)
                + (p.cumulative.cache_read_input_tokens || 0)
                + (p.cumulative.cache_creation_input_tokens || 0);
              sessionStore.setContextUsage(used, p.context_window || 0);
              sessionStore.setUsage(p.cumulative, sessionStore.sessionCost);
            }
            break;
          }
          case 'error':
            msgStore.setError(notif.params.message);
            msgStore.setStreaming(false);
            break;
        }
      };
      c.onNotification(notifHandler);

      cleanupFn = () => {
        c.offNotification(notifHandler);
        c.close();
      };
    }

    // 暴露 setupClient 供设置面板切换远程服务器复用
    setupClientRef.current = setupClient;

    async function init() {
      // 1. 优先通过 Tauri 后端自动检测/拉起本地 server
      try {
        const info = await invoke<{ url: string; token: string }>('get_server_info');
        setupClient(info.url, info.token);
        return;
      } catch (e) {
        // 非 Tauri 环境（浏览器）或 server 启动失败 -> 降级到配对串
        console.warn('Tauri get_server_info failed, falling back to pairing string:', e);
      }

      // 2. 降级：从 URL 参数或 localStorage 读取配对串（远程 server 场景）
      const params = new URLSearchParams(window.location.search);
      const pairingStr = params.get('pairing') || localStorage.getItem('mcoder_pairing') || '';
      if (!pairingStr) {
        msgStore.setError('No server available. Install mcoder or pass ?pairing=mcoder://...');
        return;
      }
      const parsed = parsePairingString(pairingStr);
      if (!parsed) {
        msgStore.setError('Invalid pairing string.');
        return;
      }
      setupClient(parsed.url, parsed.token);
    }

    init();
    return () => {
      cancelled = true;
      cleanupFn?.();
    };
  }, []);

  const refreshSessions = useCallback(async (): Promise<SessionMeta[]> => {
    if (!client) return [];
    const allSessions: SessionMeta[] = await client.request('sessions.list');
    sessionStore.setSessions(allSessions);
    return allSessions;
  }, [client, sessionStore]);

  const handleSelectProject = useCallback((projectPath: string) => {
    const all = useSessionStore.getState().sessions;
    enterProject(projectPath, all);
  }, [enterProject]);

  const handleCreateFromProjectList = useCallback(async (projectPath: string) => {
    if (!client) return;
    try {
      const result = await client.request('sessions.create', { project: projectPath });
      const newId: string = result.session_id;
      const allSessions = await refreshSessions();
      await enterProject(projectPath, allSessions, newId);
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  }, [client, refreshSessions, enterProject, msgStore]);

  const handleSelectTab = useCallback((id: string) => {
    attachSession(id);
  }, [attachSession]);

  const handleCloseTab = useCallback((id: string) => {
    const remaining = openTabs.filter((t) => t !== id);
    desktop.closeTab(id);
    if (id === currentSessionId) {
      if (remaining.length > 0) {
        attachSession(remaining[0]);
      } else {
        // Phase 5c: 切换 session 时调用 clearSessionUiState 避免闪旧 Todo/Plan/Resume/Ask/Tasks
        clearSessionUiState({ sessionId: id });
        sessionStore.setCurrentSession(null);
        client?.setReconnectSession(undefined);
        msgStore.setMessages([]);
      }
    }
  }, [openTabs, currentSessionId, desktop, attachSession, sessionStore, client, msgStore]);

  const handleNewTab = useCallback(async () => {
    if (!client || !currentProject) return;
    try {
      const result = await client.request('sessions.create', { project: currentProject });
      const newId: string = result.session_id;
      await refreshSessions();
      desktop.openTab(newId);
      await attachSession(newId);
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  }, [client, currentProject, refreshSessions, desktop, attachSession, msgStore]);

  const handleBack = useCallback(() => {
    desktop.setView('projects');
    desktop.setCurrentProject(null);
    desktop.setOpenTabs([]);
    // Phase 5c: 返回项目选择前清掉旧 session 的 UI（防止再进入时闪旧 Todo/Plan/Ask）
    clearSessionUiState({ sessionId: currentSessionId ?? undefined });
    sessionStore.setCurrentSession(null);
    client?.setReconnectSession(undefined);
    msgStore.setMessages([]);
    setPreviewFile(null);
    setRightPanel('none');
    refreshSessions();
  }, [desktop, sessionStore, client, msgStore, refreshSessions, currentSessionId]);

  // FileTree 选中文件回调
  const handleFileSelect = useCallback((path: string, content: string) => {
    setPreviewFile({ path, content });
    setRightPanel('file');
  }, []);

  const sendMessage = async () => {
    if (!client || (!input.trim() && pendingImages.length === 0)) return;
    let sid = currentSessionId;
    if (!sid) {
      if (!currentProject) {
        msgStore.setError('Select a project first.');
        return;
      }
      try {
        const result = await client.request('sessions.create', { project: currentProject });
        const newSid: string = result.session_id;
        sessionStore.setCurrentSession(newSid);
        client.setReconnectSession(newSid);
        desktop.openTab(newSid);
        sid = newSid;
      } catch (e: any) {
        msgStore.setError(e.message);
        return;
      }
    }
    // 构建用户消息内容块（含图片，乐观渲染用 data URL）
    const userBlocks: any[] = [];
    if (input.trim()) {
      userBlocks.push({ type: 'text', text: input });
    }
    for (const img of pendingImages) {
      userBlocks.push({ type: 'image', path: img.preview, media_type: img.media_type });
    }
    msgStore.addMessage({ role: 'user', content: userBlocks });
    msgStore.setStreaming(true);
    const text = input;
    const imgs = pendingImages;
    setInput('');
    setPendingImages([]);
    try {
      if (imgs.length > 0) {
        await client.request('sessions.send', {
          session_id: sid,
          content: text,
          images: imgs.map(im => ({ data: im.data, media_type: im.media_type })),
        });
      } else {
        await client.request('sessions.send', { session_id: sid, content: text });
      }
    } catch (e: any) {
      msgStore.setError(e.message);
      msgStore.setStreaming(false);
    }
  };

  // 读取图片文件为 base64
  const handleImageSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files;
    if (!files) return;
    Array.from(files).forEach(file => {
      const reader = new FileReader();
      reader.onload = () => {
        const result = reader.result as string;
        const base64 = result.split(',')[1] || '';
        const media_type = file.type || 'image/png';
        setPendingImages(prev => [...prev, {
          data: base64,
          media_type,
          name: file.name,
          preview: result,
        }]);
      };
      reader.readAsDataURL(file);
    });
    // 清空 input 以便重复选择同一文件
    e.target.value = '';
  };

  const cancelStreaming = async () => {
    if (!client || !currentSessionId) return;
    try {
      await client.request('session.cancel', { session_id: currentSessionId });
      msgStore.setStreaming(false);
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  };

  const handleSlash = async (cmd: string) => {
    if (!client) return;
    // 所有 slash command 转发到服务端分发（commands/mod.rs::CommandDispatcher）
    try {
      const result = await dispatchSlashCommand(cmd, client);
      if (result.error) msgStore.setError(result.error);
      else msgStore.setError(null);
      if (result.systemMessage) {
        msgStore.addMessage({ role: 'system', content: [{ type: 'text', text: result.systemMessage }] });
      }
      if (result.switchView === 'tree') {
        setRightPanel('tree');
      }
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  };

  const handleModelClick = async () => {
    if (showModelDropdown) { setShowModelDropdown(false); return; }
    if (!client) return;
    try {
      const result: any = await client.request('config.list_models', {});
      setAvailableModels(result.models || result || []);
      setShowModelDropdown(true);
    } catch (e: any) {
      msgStore.setError(`fetch models failed: ${e.message}`);
    }
  };

  const handleModelSelect = async (modelName: string) => {
    if (!client || !currentSessionId) {
      setShowModelDropdown(false);
      return;
    }
    try {
      await client.request('session.model.set', { session_id: currentSessionId, model: modelName });
      sessionStore.setModel(modelName);
    } catch (e: any) {
      msgStore.setError(`set model failed: ${e.message}`);
    }
    setShowModelDropdown(false);
  };

  // Settings: 拉取 config 值（loop_max_iters / compact / memory）
  useEffect(() => {
    if (showSettings && client && currentSessionId) {
      Promise.all([
        client.request('config.get', { key: 'loop_max_iters' }).catch(() => null),
        client.request('config.get', { key: 'compact' }).catch(() => null),
        client.request('config.get', { key: 'memory' }).catch(() => null),
      ]).then(([iters, compact, memory]) => {
        setConfigValues({ loop_max_iters: iters, compact, memory });
      });
    }
  }, [showSettings, client, currentSessionId]);

  // M2: Escape 键关闭 settings overlay
  useEffect(() => {
    if (!showSettings) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setShowSettings(false);
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [showSettings]);

  const handleConfigSet = async (key: string, value: any) => {
    if (!client) return;
    try {
      await client.request('config.set', { key, value });
      setConfigValues((prev) => {
        const next = { ...prev };
        if (key === 'loop_max_iters') {
          next.loop_max_iters = value;
        } else if (key.startsWith('compact.')) {
          next.compact = { ...(next.compact || {}), [key.slice('compact.'.length)]: value };
        } else if (key.startsWith('memory.')) {
          next.memory = { ...(next.memory || {}), [key.slice('memory.'.length)]: value };
        }
        return next;
      });
    } catch (e: any) {
      msgStore.setError(`config.set failed: ${e.message}`);
    }
  };

  const handleRoleChange = async (role: string) => {
    if (!client || !currentSessionId) return;
    try {
      await client.request('session.mode.set', { session_id: currentSessionId, role });
      sessionStore.setRole(role);
    } catch (e: any) {
      msgStore.setError(`set role failed: ${e.message}`);
    }
  };

  // 设置面板：切换到远程服务器（复用 setupClient 重建 WsClient）
  const handleRemoteConnect = (raw: string) => {
    let url = '';
    let token = '';

    if (raw.startsWith('mcoder://')) {
      // Parse mcoder://token@host:port
      const match = raw.match(/^mcoder:\/\/(.+)@(.+)$/);
      if (!match) {
        msgStore.setError('Invalid pairing string');
        return;
      }
      token = match[1];
      url = `ws://${match[2]}`;
    } else if (raw.startsWith('ws://') || raw.startsWith('wss://')) {
      const parts = raw.split(/\s+/);
      url = parts[0];
      token = parts[1] || '';
    }

    if (!url || !token) {
      msgStore.setError('Usage: mcoder://token@host:port or ws://host:port token');
      return;
    }

    // Close old connection
    client?.close();

    // Reset stores
    sessionStore.reset();
    desktop.reset();
    msgStore.setMessages([]);
    currentSessionIdRef.current = null;

    // Create new client using setupClient
    setupClientRef.current?.(url, token);
    setShowSettings(false);
  };

  const onInputKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      if (input.startsWith('/')) {
        handleSlash(input);
        setInput('');
      } else {
        sendMessage();
      }
    }
  };

  const { messages, streaming, error } = msgStore;
  const { connected, currentModel, currentRole, contextUsed, contextWindow, sessionCost, version, lspServers, projectPath } = sessionStore;

  // 全局配对 tool_use → tool_result（按 id）
  const resultsById = new Map<string, any>();
  for (const msg of messages) {
    for (const block of (msg?.content || [])) {
      if (block.type === 'tool_result' && block.id && !resultsById.has(block.id)) {
        resultsById.set(block.id, block);
      }
    }
  }

  const projectSessions = currentProject
    ? sessions.filter((s) => s.project_path === currentProject)
    : [];

  return (
    <div className={`app platform-${platform}`}>
      {/* Header：minimal — title + connection + navigation */}
      <div className="header" data-tauri-drag-region>
        <div className="header-traffic-light" data-tauri-drag-region aria-hidden />
        <span className="header-title" data-tauri-drag-region>mcoder</span>
        <span className={`header-status ${connected ? 'connected' : 'disconnected'}`}>
          {connected ? '●' : '○'}
        </span>
        <button className="header-settings" onClick={() => setShowSettings(true)} title="Settings">
          ⚙
        </button>
        {view === 'sessions' && currentProject && (
          <>
            <button className="header-back" onClick={handleBack} title="Back to projects">←</button>
            <span className="header-project" title={currentProject}>
              {currentProject.split('/').pop() || currentProject}
            </span>
          </>
        )}
        {/* Windows 窗口按钮右侧占位（仅 platform-win 显示） */}
        <div className="header-window-controls" data-tauri-drag-region aria-hidden />
      </div>

      {view === 'projects' ? (
        <div className="projects-view">
          {error && <div className="error">{error}</div>}
          <ProjectList
            sessions={sessions}
            onSelectProject={handleSelectProject}
            onCreateSession={handleCreateFromProjectList}
          />
        </div>
      ) : (
        <div className="sessions-view">
          <SessionTabs
            sessions={projectSessions}
            openTabs={openTabs}
            activeSessionId={currentSessionId || ''}
            onSelect={handleSelectTab}
            onClose={handleCloseTab}
            onNew={handleNewTab}
          />
          {/* Plan/Todo 浮层 */}
          {pendingPlan && currentSessionId && client && (
            <PlanPanel
              plan={pendingPlan}
              client={client}
              sessionId={currentSessionId}
              onDismiss={() => sessionStore.setPendingPlan(null)}
            />
          )}
          {pendingTodos && pendingTodos.length > 0 && (
            <TodoPanel todos={pendingTodos} />
          )}
          {error && <div className="error">{error}</div>}
          <div className="main">
            {/* 左栏：文件树 */}
            <div className="sidebar">
              {client && <FileTree client={client} onFileSelect={handleFileSelect} />}
            </div>

            {/* 中栏：聊天 */}
            <div className="chat">
              <div className="messages">
                {messages.length === 0 && !streaming && (
                  <div className="messages-empty">
                    <div className="messages-empty-text">No messages yet</div>
                  </div>
                )}
                {messages.map((msg, i) => (
                  <MessageItem key={i} msg={msg} index={i} client={client} currentSessionId={currentSessionId} resultsById={resultsById} />
                ))}
                {streaming && (
                  <div className="message message-assistant">
                    <div className="message-avatar">A</div>
                    <div className="message-body">
                      <div className="message-header"><span className="message-role">AI</span></div>
                      <div className="message-content">
                        <div className="streaming-dots">
                          <span className="dot" /><span className="dot" /><span className="dot" />
                        </div>
                      </div>
                    </div>
                  </div>
                )}
                {!currentSessionId && !streaming && (
                  <div className="streaming">No session selected. Press + to create one.</div>
                )}
              </div>

              <div className="input-area">
                {/* Todo 摘要条（消息区下方、输入框上方）；全部完成时隐藏 */}
                <TodoSummaryBar />
                {/* Phase 3: Resume 入口（固定状态提示附近；非模态） */}
                <ResumeBar client={client} sessionId={currentSessionId} />
                {pendingImages.length > 0 && (
                  <div className="pending-images">
                    {pendingImages.map((img, i) => (
                      <div key={i} className="pending-image-item">
                        <img src={img.preview} alt={img.name} />
                        <button
                          className="pending-image-remove"
                          onClick={() => setPendingImages(prev => prev.filter((_, idx) => idx !== i))}
                        >x</button>
                      </div>
                    ))}
                  </div>
                )}
                <textarea
                  value={input}
                  onChange={e => setInput(e.target.value)}
                  onKeyDown={onInputKeyDown}
                  placeholder="type a message or /help for commands (Shift+Enter for newline)"
                  rows={3}
                />
                <div className="input-toolbar">
                  <span className="input-hint">Enter to send · Shift+Enter for newline</span>
                  <input
                    ref={fileInputRef}
                    type="file"
                    accept="image/*"
                    multiple
                    style={{ display: 'none' }}
                    onChange={handleImageSelect}
                  />
                  <button
                    className="attach-btn"
                    onClick={() => fileInputRef.current?.click()}
                    title="Attach image"
                  >Image</button>
                  {streaming && (
                    <button className="cancel-btn" onClick={cancelStreaming}>Cancel</button>
                  )}
                </div>
              </div>
              <div className="bottom-status">
                <span className={`status-dot ${connected ? 'connected' : 'disconnected'}`}>
                  {connected ? '●' : '○'}
                </span>
                {currentRole !== 'default' && <span className="status-role">{currentRole}</span>}
                {currentModel && (
                  <span className="status-model" onClick={handleModelClick} style={{ cursor: 'pointer' }}>
                    {currentModel} ▾
                  </span>
                )}
                {showModelDropdown && (
                  <div className="model-dropdown">
                    {availableModels.map((m) => (
                      <div
                        key={m.name}
                        className={`model-option ${m.name === currentModel ? 'model-option-active' : ''}`}
                        onClick={() => handleModelSelect(m.name)}
                      >
                        <span className="model-option-name">{m.name}</span>
                        {m.model && <span className="model-option-desc">{m.model}</span>}
                      </div>
                    ))}
                  </div>
                )}
                <span className="status-ctx" title={`${contextUsed}/${contextWindow} tokens`}>
                  {contextUsed > 1000 ? `${(contextUsed / 1000).toFixed(1)}k` : contextUsed}/{contextWindow > 1000 ? `${(contextWindow / 1000).toFixed(0)}k` : contextWindow}
                </span>
                {sessionCost > 0 && <span className="status-cost">${sessionCost.toFixed(3)}</span>}
                {streaming && <span className="status-running">running</span>}
              </div>
            </div>

            {/* 右栏：图谱/Diff/文件预览 标签页 */}
            <div className="right-panel">
              <div className="right-panel-tabs">
                <button
                  className={rightPanel === 'graph' ? 'active' : ''}
                  onClick={() => setRightPanel(rightPanel === 'graph' ? 'none' : 'graph')}
                >Graph</button>
                <button
                  className={rightPanel === 'diff' ? 'active' : ''}
                  onClick={() => setRightPanel(rightPanel === 'diff' ? 'none' : 'diff')}
                >Diff</button>
                <button
                  className={rightPanel === 'tree' ? 'active' : ''}
                  onClick={() => setRightPanel(rightPanel === 'tree' ? 'none' : 'tree')}
                >Tree</button>
                {previewFile && (
                  <button
                    className={rightPanel === 'file' ? 'active' : ''}
                    onClick={() => setRightPanel('file')}
                    title={previewFile.path}
                  >
                    {previewFile.path.split('/').pop()}
                  </button>
                )}
              </div>
              <div className="right-panel-content">
                {rightPanel === 'graph' && client && <GraphView client={client} />}
                {rightPanel === 'diff' && client && <DiffViewer client={client} />}
                {rightPanel === 'tree' && client && <TreeView client={client} />}
                {rightPanel === 'file' && previewFile && (
                  <div className="file-preview">
                    <div className="file-preview-header">
                      <span className="file-preview-path">{previewFile.path}</span>
                      <button className="file-preview-close" onClick={() => { setRightPanel('none'); setPreviewFile(null); }}>×</button>
                    </div>
                    <pre className="file-preview-content">{previewFile.content}</pre>
                  </div>
                )}
                {rightPanel === 'none' && (
                  <div className="right-panel-empty">
                    <div>Select Graph, Diff, or click a file</div>
                  </div>
                )}
              </div>
            </div>
          </div>
        </div>
      )}
      {showSettings && (
        <div className="settings-overlay" onClick={(e) => { if (e.target === e.currentTarget) setShowSettings(false); }}>
          <div className="settings-panel">
            <div className="settings-header">
              <span>Settings</span>
              <div className="settings-tabs">
                <button className={settingsTab === 'general' ? 'tab active' : 'tab'} onClick={() => setSettingsTab('general')}>General</button>
                <button className={settingsTab === 'providers' ? 'tab active' : 'tab'} onClick={() => setSettingsTab('providers')}>Providers</button>
              </div>
              <button onClick={() => setShowSettings(false)}>✕</button>
            </div>
            <div className="settings-body">
              {settingsTab === 'providers' && client && (
                <ProviderPanel
                  req={client.request.bind(client)}
                  onConfigUpdated={(cb) => {
                    const handler = (n: any) => { if (n.method === 'config_updated') cb(); };
                    client.onNotification(handler);
                    return () => client.offNotification(handler);
                  }}
                />
              )}
              {settingsTab === 'general' && (<>
              {/* Server Connection section */}
              <div className="setting-section-title">Server Connection</div>
              <div className="setting-row">
                <div className="setting-label">
                  <span className="setting-name">Remote Server</span>
                  <span className="setting-desc">Connect to a remote mcoder server</span>
                </div>
                <div className="setting-control">
                  <input
                    type="text"
                    className="setting-control-text"
                    placeholder="mcoder://token@host:port"
                    value={remoteInput}
                    onChange={(e) => setRemoteInput(e.target.value)}
                  />
                  <button
                    className="setting-connect-btn"
                    onClick={() => handleRemoteConnect(remoteInput)}
                  >
                    Connect
                  </button>
                </div>
              </div>
              <div className="setting-row">
                <div className="setting-label">
                  <span className="setting-name">Model</span>
                  <span className="setting-desc">LLM model for this session</span>
                </div>
                <div className="setting-control">
                  <button className="setting-model-btn" onClick={() => { setShowSettings(false); handleModelClick(); }}>
                    {currentModel || '(not set)'} ▾
                  </button>
                </div>
              </div>
              <div className="setting-row">
                <div className="setting-label">
                  <span className="setting-name">Role</span>
                  <span className="setting-desc">Agent role / mode</span>
                </div>
                <div className="setting-control">
                  <select value={currentRole} onChange={(e) => handleRoleChange(e.target.value)}>
                    <option value="default">default</option>
                    <option value="coder">coder</option>
                    <option value="plan">plan</option>
                    <option value="goal">goal</option>
                    <option value="loop">loop</option>
                  </select>
                </div>
              </div>
              <div className="setting-row">
                <div className="setting-label">
                  <span className="setting-name">Max Iterations</span>
                  <span className="setting-desc">Max agent loop iterations</span>
                </div>
                <div className="setting-control">
                  <input type="number" min={1} key={`iters-${configValues.loop_max_iters}`} defaultValue={configValues.loop_max_iters ?? ''} onBlur={(e) => { const v = e.target.value; if (v !== '') handleConfigSet('loop_max_iters', Number(v)); }} />
                </div>
              </div>
              <div className="setting-row">
                <div className="setting-label">
                  <span className="setting-name">Compact Threshold</span>
                  <span className="setting-desc">Context fill ratio (0-1) to trigger compaction</span>
                </div>
                <div className="setting-control">
                  <input type="number" min={0} max={1} step={0.1} key={`threshold-${configValues.compact?.threshold}`} defaultValue={configValues.compact?.threshold ?? ''} onBlur={(e) => { const v = e.target.value; if (v !== '') handleConfigSet('compact.threshold', Number(v)); }} />
                </div>
              </div>
              <div className="setting-row">
                <div className="setting-label">
                  <span className="setting-name">Compact Keep Recent</span>
                  <span className="setting-desc">Messages to keep after compaction</span>
                </div>
                <div className="setting-control">
                  <input type="number" min={0} key={`keeprecent-${configValues.compact?.keep_recent}`} defaultValue={configValues.compact?.keep_recent ?? ''} onBlur={(e) => { const v = e.target.value; if (v !== '') handleConfigSet('compact.keep_recent', Number(v)); }} />
                </div>
              </div>
              <div className="setting-row">
                <div className="setting-label">
                  <span className="setting-name">Memory Auto Recall</span>
                  <span className="setting-desc">Automatically recall relevant memories</span>
                </div>
                <div className="setting-control">
                  <button className={`setting-toggle ${configValues.memory?.auto_recall ? 'on' : 'off'}`} onClick={() => handleConfigSet('memory.auto_recall', !configValues.memory?.auto_recall)} />
                </div>
              </div>
              <div className="setting-row">
                <div className="setting-label">
                  <span className="setting-name">Memory Auto Capture</span>
                  <span className="setting-desc">Automatically capture memories from conversations</span>
                </div>
                <div className="setting-control">
                  <button className={`setting-toggle ${configValues.memory?.auto_capture ? 'on' : 'off'}`} onClick={() => handleConfigSet('memory.auto_capture', !configValues.memory?.auto_capture)} />
                </div>
              </div>
              <div className="setting-section-title">Info</div>
              <div className="setting-row setting-row-info">
                <div className="setting-label">
                  <span className="setting-name">Version</span>
                </div>
                <div className="setting-control setting-control-text">{version || '-'}</div>
              </div>
              <div className="setting-row setting-row-info">
                <div className="setting-label">
                  <span className="setting-name">Project Path</span>
                </div>
                <div className="setting-control setting-control-text" title={projectPath}>{projectPath || '-'}</div>
              </div>
              <div className="setting-row setting-row-info">
                <div className="setting-label">
                  <span className="setting-name">LSP Servers</span>
                </div>
                <div className="setting-control setting-control-text">{lspServers.length > 0 ? lspServers.join(', ') : '-'}</div>
              </div>
              </>)}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
