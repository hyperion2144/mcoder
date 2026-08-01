// 设计文档 §8.6.2: 移动客户端主应用
// 以项目为入口，项目内多会话用 tab 组织
// 单栏布局，触摸友好，弱网友好
// 复用 TUI 的 rpc/store/commands/utils 逻辑层

import React, { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { WsClient } from '@mcoder/shared/rpc/client.js';
import { useSessionStore, useMessagesStore } from '@mcoder/shared/store/index.js';
import { dispatchSlashCommand } from '@mcoder/shared/commands/index.js';
import { parsePairingString } from '@mcoder/shared/utils/pairing.js';
import { AskCard, useAskStore } from '@mcoder/shared/ask/index.js';
import { usePermissionStore } from '@mcoder/shared/permission/store.js';
import { hasToolUse } from '@mcoder/shared/ask/messages.js';
import { ASK_USER_TOOL } from '@mcoder/shared/ask/types.js';
/// 设计文档 §8.8: 权限审批占位 tool name（虚拟）
const PERMISSION_TOOL_NAME = '__permission_pending__';
import { hydrateSnapshot, type SessionSnapshot } from '@mcoder/shared/rpc/sessionSnapshot.js';
import { clearSessionUiState } from '@mcoder/shared/store/clearSessionUiState.js';
import { NetworkMonitor } from './network.js';
import { PairingScreen } from './components/PairingScreen.js';
import { Drawer } from './components/Drawer.js';
import { MessageList } from './components/MessageList.js';
import { InputBar, type PendingImage } from './components/InputBar.js';
import { ProjectList } from './components/ProjectList.js';
import { SessionTabs } from './components/SessionTabs.js';
import { TodoSummaryBar } from './components/TodoSummaryBar.js';
import { ResumeBar } from './components/ResumeBar.js';
import { SubagentBar } from './components/SubagentBar.js';
import { TreeView } from './components/TreeView.js';
import { ProviderScreen } from './components/ProviderScreen.js';
import { CommandPicker } from './components/CommandPicker.js';
import { t, setLang, getLang, loadLang } from './i18n.js';
import {
  Brain, X, Check, ChevronDown, ChevronRight, ChevronUp, ArrowLeft, Settings,
  Plus, Trash2, Star, Play, Square, CircleDot, Circle, AlertCircle, CornerDownRight,
} from 'lucide-react';

// 设计文档 §6.2/§6.7: Plan 审批 + Todo 显示（移动端触摸友好版）
function MobilePlanPanel({
  plan,
  client,
  sessionId,
  onDismiss,
}: {
  plan: any;
  client: WsClient;
  sessionId: string;
  onDismiss: () => void;
}) {
  const handleApprove = async () => {
    try {
      await client.request('session.approve', { session_id: sessionId, action: 'approve' });
      onDismiss();
    } catch {}
  };
  const handleReject = async () => {
    try {
      await client.request('session.approve', { session_id: sessionId, action: 'reject' });
      onDismiss();
    } catch {}
  };
  const steps: any[] = Array.isArray(plan.steps) ? plan.steps : [];
  return (
    <div className="plan-panel">
      <div className="plan-panel-header">
        <span className="plan-panel-title">{t('ui.plan_pending')}</span>
        <button className="plan-panel-close" onClick={onDismiss} aria-label={t('ui.close')}><X size={18} /></button>
      </div>
      {plan.title && <div className="plan-panel-name">{plan.title}</div>}
      <ol className="plan-steps">
        {steps.map((step: any, i: number) => (
          <li key={i} className="plan-step">
            <span className="plan-step-index">{i + 1}.</span>
            <span className="plan-step-text">{step.description || step.text || JSON.stringify(step)}</span>
          </li>
        ))}
      </ol>
      <div className="plan-panel-actions">
        <button className="plan-btn plan-btn-approve" onClick={handleApprove}>{t('ui.approve')}</button>
        <button className="plan-btn plan-btn-reject" onClick={handleReject}>{t('ui.reject')}</button>
      </div>
    </div>
  );
}

function MobileTodoPanel({ todos }: { todos: any[] }) {
  const done = todos.filter((t) => t.done || t.status === 'done').length;
  const total = todos.length;
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;
  return (
    <div className="todo-panel">
      <div className="todo-panel-header">
        <span className="todo-panel-title">{t('ui.todos')}</span>
        <span className="todo-panel-progress">{done}/{total} · {pct}%</span>
      </div>
      <div className="todo-progress-bar">
        <div className="todo-progress-fill" style={{ width: `${pct}%` }} />
      </div>
      <ul className="todo-list">
        {todos.map((todo: any, i: number) => {
          const isDone = todo.done || todo.status === 'done';
          return (
            <li key={i} className={`todo-item ${isDone ? 'todo-done' : ''}`}>
              <span className="todo-check">{isDone ? <Check size={16} /> : <Square size={16} />}</span>
              <span className="todo-text">{todo.text || todo.description}</span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

// P2-3: 使用 Capacitor Preferences 替代 localStorage（更可靠的原生持久化）
// 在 Web 环境下自动 fallback 到 localStorage
const prefs = {
  async get(key: string): Promise<string | null> {
    try {
      const Preferences = (await import('@capacitor/preferences')).Preferences;
      const { value } = await Preferences.get({ key });
      return value;
    } catch {
      // Web 环境或 Preferences 不可用时 fallback
      return localStorage.getItem(key);
    }
  },
  async set(key: string, value: string): Promise<void> {
    try {
      const Preferences = (await import('@capacitor/preferences')).Preferences;
      await Preferences.set({ key, value });
    } catch {
      localStorage.setItem(key, value);
    }
  },
  async remove(key: string): Promise<void> {
    try {
      const Preferences = (await import('@capacitor/preferences')).Preferences;
      await Preferences.remove({ key });
    } catch {
      localStorage.removeItem(key);
    }
  },
};

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
  const [pairing, setPairing] = useState<string>('');
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [showTree, setShowTree] = useState(false);
  const [networkStatus, setNetworkStatus] = useState<'online' | 'offline'>('online');
  const [pendingQueue, setPendingQueue] = useState<{content: string; images: PendingImage[]}[]>([]);
  // 项目入口视图状态：projects 为项目选择页，sessions 为项目内会话 tab 页
  const [view, setView] = useState<'projects' | 'sessions'>('projects');
  const [currentProject, setCurrentProject] = useState<string | null>(null);
  // 当前项目内打开为 tab 的会话 ID 列表
  const [openTabs, setOpenTabs] = useState<string[]>([]);
  // 从服务端获取的命令列表（供 Drawer 展示）
  const [commands, setCommands] = useState<{ name: string; description: string }[]>([]);
  // 模型选择 sheet
  const [showModelSheet, setShowModelSheet] = useState(false);
  const [availableModels, setAvailableModels] = useState<{ name: string; description?: string }[]>([]);
  // 思考深度快捷切换
  const [currentThinking, setCurrentThinking] = useState('none');
  const [showThinkingSheet, setShowThinkingSheet] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [settingsTab, setSettingsTab] = useState<'general' | 'providers'>('general');
  const [configValues, setConfigValues] = useState<Record<string, any>>({});
  // 命令选择面板
  const [input, setInput] = useState('');
  const [showCommandPicker, setShowCommandPicker] = useState(false);
  // 语言版本号：语言变更时递增以触发重渲染
  const [, setLangVersion] = useState(0);
  const sessionStore = useSessionStore();
  const msgStore = useMessagesStore();
  const networkMonitor = useRef<NetworkMonitor | null>(null);

  // P1-5: 用 ref 保存最新的 sendMessage 和 client，避免闭包过期
  const sendMessageRef = useRef<(content: string, images?: PendingImage[]) => void>(() => {});
  const clientRef = useRef<WsClient | null>(null);
  clientRef.current = client;

  // 初始化网络监听
  useEffect(() => {
    networkMonitor.current = new NetworkMonitor();
    networkMonitor.current.onStatusChange((status) => {
      setNetworkStatus(status);
      if (status === 'online' && pendingQueue.length > 0 && clientRef.current) {
        // P1-5: 弱网恢复后重发排队消息，用 ref 获取最新 sendMessage
        const queue = [...pendingQueue];
        setPendingQueue([]);
        queue.forEach((item) => sendMessageRef.current(item.content, item.images));
      }
    });
    return () => networkMonitor.current?.destroy();
  }, [pendingQueue]);

  // 连接 server
  const connect = useCallback(async (pairingStr: string) => {
    const parsed = parsePairingString(pairingStr);
    if (!parsed) {
      msgStore.setError('Invalid pairing string');
      return;
    }

    const c = new WsClient(
      parsed.url,
      parsed.token,
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

    try {
      await c.connect();
      setPairing(pairingStr);
      // P2-3: 持久化配对串（Capacitor Preferences）
      try {
        await prefs.set('mcoder_pairing', pairingStr);
      } catch {}
      // 加载会话，进入项目选择页
      try {
        const sessions = await c.request('sessions.list');
        sessionStore.setSessions(sessions);
        setView('projects');
      } catch {}
      // 从服务端获取命令列表（供 Drawer 展示）
      try {
        const cmds = await c.request('command.list');
        setCommands(cmds);
      } catch {}
      // 从后端加载语言设置
      loadLang(c).catch(() => {});
    } catch (e: any) {
      msgStore.setError(`Connection failed: ${e.message}`);
    }

    // 通知处理
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
        case 'error':
          msgStore.setError(notif.params.message);
          msgStore.setStreaming(false);
          break;
        case 'config_updated': {
          // 重新加载语言设置（后端 config.set_language 会广播此通知）
          loadLang(c).then(() => setLangVersion(v => v + 1)).catch(() => {});
          break;
        }
      }
    };
    c.onNotification(notifHandler);
    // 注意：connect 通常只在 App 挂载时调用一次；offNotification 由 disconnect() / close() 处理
  }, []);

  // 启动时尝试自动连接
  useEffect(() => {
    (async () => {
      const saved = (await prefs.get('mcoder_pairing')) || '';
      if (saved) {
        connect(saved);
      }
    })();
  }, []);

  // 刷新会话列表（创建/关闭会话后调用，保持项目分组最新）
  const refreshSessions = useCallback(async () => {
    if (!client) return;
    try {
      const sessions = await client.request('sessions.list');
      sessionStore.setSessions(sessions);
    } catch {}
  }, [client]);

  // attach 到指定会话并加载消息：调用 session.attach 拿 SessionSnapshot，再用 hydrateSnapshot hydrate
  // Phase 2: 不再单独调 ask.pending / todo.list / task.list —— 全部来自 snapshot
  // Phase 5c: 切 session 前先 clearSessionUiState 旧 session 避免闪旧 Todo/Plan/Ask
  const attachSession = useCallback(async (id: string) => {
    if (!client) return;
    // Phase 5c: 切 session 时先清掉旧 session 的 UI
    if (sessionStore.currentSessionId && sessionStore.currentSessionId !== id) {
      clearSessionUiState({ sessionId: sessionStore.currentSessionId });
    }
    try {
      const snapshot = await client.request('session.attach', { session_id: id }) as SessionSnapshot;
      client.setReconnectSession(id);
      hydrateSnapshot({
        sessionId: id,
        snapshot,
        store: {
          setCurrentSessionId: (sid) => sessionStore.setCurrentSession(sid),
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
              askStore.clearSession(id);
              return;
            }
            askStore.setPendingAskFromSnapshot(ask);
          },
          clearAskSession: (sid) => {
            useAskStore.getState().clearSession(sid);
          },
          replaceTodosFromSnapshot: (_todos) => {
            // setPendingTodos 已替换全部
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
      msgStore.setError(e.message);
    }
  }, [client, sessionStore, msgStore]);

  // 进入项目：设置 currentProject，打开该项目所有会话为 tab，自动 attach 第一个（最新）
  const enterProject = useCallback(async (projectPath: string) => {
    const projectSessions = sessionStore.sessions
      .filter((s) => s.project_path === projectPath)
      .sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());
    const tabIds = projectSessions.map((s) => s.session_id);
    setOpenTabs(tabIds);
    setCurrentProject(projectPath);
    setView('sessions');
    // 自动 attach 最新的一条会话
    if (tabIds.length > 0) {
      await attachSession(tabIds[tabIds.length - 1]);
    } else {
      sessionStore.setCurrentSession(null);
      msgStore.setMessages([]);
    }
  }, [sessionStore.sessions, attachSession]);

  // 从项目列表新建会话（指定工作目录），创建后进入该项目会话页
  const handleNewSessionForProject = useCallback(async (projectPath: string) => {
    if (!client) return;
    try {
      const result = await client.request('sessions.create', {
        project: projectPath,
        title: 'Mobile Session',
      });
      await refreshSessions();
      setOpenTabs([result.session_id]);
      setCurrentProject(projectPath);
      setView('sessions');
      await attachSession(result.session_id);
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  }, [client, refreshSessions, attachSession]);

  // 在会话页 tab 栏 "+" 新建会话（使用当前项目）
  const handleNewSessionInTabs = useCallback(async () => {
    if (!client || !currentProject) return;
    try {
      const result = await client.request('sessions.create', {
        project: currentProject,
        title: 'Mobile Session',
      });
      await refreshSessions();
      setOpenTabs((tabs) => (tabs.includes(result.session_id) ? tabs : [...tabs, result.session_id]));
      await attachSession(result.session_id);
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  }, [client, currentProject, refreshSessions, attachSession]);

  // 关闭 tab：从 openTabs 移除，若关闭的是当前会话则切到剩余的第一个
  const handleCloseTab = useCallback(async (id: string) => {
    const newTabs = openTabs.filter((t) => t !== id);
    setOpenTabs(newTabs);
    if (sessionStore.currentSessionId === id) {
      if (newTabs.length > 0) {
        await attachSession(newTabs[0]);
      } else {
        // Phase 5c: 切换 session 时调用 clearSessionUiState 避免闪旧 Todo/Plan/Resume/Ask/Tasks
        clearSessionUiState({ sessionId: id });
        sessionStore.setCurrentSession(null);
        msgStore.setMessages([]);
      }
    }
  }, [openTabs, sessionStore.currentSessionId, attachSession]);

  // 返回项目选择页
  const handleBackToProjects = useCallback(() => {
    setView('projects');
    setCurrentProject(null);
    setOpenTabs([]);
    // Phase 5c: 返回项目选择前清掉旧 session 的 UI（防止再进入时闪旧 Todo/Plan/Ask）
    clearSessionUiState({ sessionId: sessionStore.currentSessionId ?? undefined });
    sessionStore.setCurrentSession(null);
    msgStore.setMessages([]);
  }, [sessionStore, msgStore]);

  // Drawer 中跨项目切换会话：切换项目上下文并 attach
  const handleDrawerSelectSession = useCallback(async (id: string) => {
    const session = sessionStore.sessions.find((s) => s.session_id === id);
    if (!session) return;
    const projectPath = session.project_path;
    if (projectPath !== currentProject) {
      // 切换到目标项目，打开该项目所有会话为 tab
      const projectSessions = sessionStore.sessions.filter((s) => s.project_path === projectPath);
      setOpenTabs(projectSessions.map((s) => s.session_id));
      setCurrentProject(projectPath);
    } else if (!openTabs.includes(id)) {
      // 同项目但 tab 未打开，补开
      setOpenTabs((tabs) => (tabs.includes(id) ? tabs : [...tabs, id]));
    }
    setView('sessions');
    setDrawerOpen(false);
    await attachSession(id);
  }, [sessionStore.sessions, currentProject, openTabs, attachSession]);

  const sendMessage = useCallback(async (content: string, images: PendingImage[] = []) => {
    if (!client || (!content.trim() && images.length === 0)) return;

    // issue 9: 离线 pending answer 不作为普通消息重发
    // 若 session 当前有 pending Ask，则此文本本应作为 ask answer 提交
    // 离线时不应排进普通消息队列（否则恢复后会创建新 loop，污染上下文）
    if (networkStatus === 'offline') {
      const sid = sessionStore.currentSessionId;
      const hasPendingAsk = sid && useAskStore.getState().pending[sid];
      if (hasPendingAsk) {
        msgStore.setError('当前有 ask 等待回答，请先在 ask 卡片上交互；离线消息会丢失');
        return;
      }
      setPendingQueue((q) => [...q, { content, images }]);
      msgStore.addMessage({
        role: 'system',
        content: [{ type: 'text', text: `[queued, will send when online] ${content}` }],
      });
      return;
    }

    let sid = sessionStore.currentSessionId;
    if (!sid) {
      try {
        const project = currentProject || '';
        const result = await client.request('sessions.create', {
          project,
          title: 'Mobile Session',
        });
        sessionStore.setCurrentSession(result.session_id);
        client.setReconnectSession(result.session_id);
        setOpenTabs((tabs) => (tabs.includes(result.session_id) ? tabs : [...tabs, result.session_id]));
        refreshSessions();
        sid = result.session_id;
      } catch (e: any) {
        msgStore.setError(e.message);
        return;
      }
    }
    // 构建用户消息内容块（含图片，乐观渲染用 data URL）
    const userBlocks: any[] = [];
    if (content.trim()) {
      userBlocks.push({ type: 'text', text: content });
    }
    for (const img of images) {
      userBlocks.push({ type: 'image', path: img.preview, media_type: img.media_type });
    }
    msgStore.addMessage({ role: 'user', content: userBlocks });
    msgStore.setStreaming(true);
    try {
      if (images.length > 0) {
        await client.request('sessions.send', {
          session_id: sid,
          content,
          images: images.map(im => ({ data: im.data, media_type: im.media_type })),
        });
      } else {
        await client.request('sessions.send', { session_id: sid, content });
      }
    } catch (e: any) {
      // 发送失败，排队重试（但 ask answer 不入队，见上面）
      const hasPendingAsk = sid ? useAskStore.getState().pending[sid] : null;
      if (hasPendingAsk) {
        msgStore.setError('当前有 ask 等待回答，请先在 ask 卡片上交互；离线消息会丢失');
        msgStore.setStreaming(false);
        return;
      }
      setPendingQueue((q) => [...q, { content, images: [] }]);
      msgStore.setError(`send failed (queued): ${e.message}`);
      msgStore.setStreaming(false);
    }
  }, [client, networkStatus, sessionStore.currentSessionId, currentProject, refreshSessions]);

  // P1-5: 保持 ref 为最新的 sendMessage
  sendMessageRef.current = sendMessage;

  // P1-4: 取消流式响应
  const cancelStreaming = useCallback(async () => {
    if (!client || !sessionStore.currentSessionId) return;
    try {
      await client.request('session.cancel', { session_id: sessionStore.currentSessionId });
      msgStore.setStreaming(false);
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  }, [client, sessionStore.currentSessionId]);

  const handleSlash = useCallback(async (cmd: string) => {
    if (!client) return;
    // /handoff <desc>: 将当前 session 移交给子代理处理指定任务
    if (cmd.startsWith('/handoff ')) {
      const taskPrompt = cmd.slice('/handoff '.length).trim();
      if (!taskPrompt || !client || !sessionStore.currentSessionId) return;
      try {
        const result = await client.request('session.handoff', {
          session_id: sessionStore.currentSessionId,
          task_prompt: taskPrompt,
        });
        msgStore.addMessage({ role: 'system', content: [{ type: 'text', text: `${t('ui.handoff_to')} ${result.new_session_id}\n\n${result.handoff_doc}` }] });
      } catch (e: any) { msgStore.setError(e.message); }
      return;
    }
    // /handoff-back: 从当前子代理返回到父 session
    if (cmd === '/handoff-back') {
      if (!client || !sessionStore.currentSessionId) return;
      try {
        const result = await client.request('session.handoff_back', {
          from_session_id: sessionStore.currentSessionId,
        });
        msgStore.addMessage({ role: 'system', content: [{ type: 'text', text: `${t('ui.handoff_back_to')} ${result.to_session_id}:\n\n${result.back_doc}` }] });
      } catch (e: any) { msgStore.setError(e.message); }
      return;
    }
    // /lang <en|zh>: 设置语言（Mobile 本地拦截，确保 mobile i18n 模块同步）
    if (cmd === '/lang' || cmd.startsWith('/lang ')) {
      const lang = cmd.slice('/lang'.length).trim();
      if (lang === 'en' || lang === 'zh') {
        try {
          await client.request('config.set_language', { language: lang });
          setLang(lang);
          setLangVersion(v => v + 1);
          msgStore.addMessage({ role: 'system', content: [{ type: 'text', text: `${t('cmd.lang_set')} ${lang}` }] });
        } catch (e: any) { msgStore.setError(e.message); }
      } else if (!lang) {
        try {
          const result = await client.request('config.get_language');
          msgStore.addMessage({ role: 'system', content: [{ type: 'text', text: `${t('cmd.lang_current')} ${result.language}` }] });
        } catch (e: any) { msgStore.setError(e.message); }
      } else {
        msgStore.setError(t('cmd.lang_usage'));
      }
      return;
    }
    // 所有 slash command 转发到服务端分发（commands/mod.rs::CommandDispatcher）
    try {
      const result = await dispatchSlashCommand(cmd, client);
      if (result.error) msgStore.setError(result.error);
      else msgStore.setError(null);
      if (result.systemMessage) {
        msgStore.addMessage({ role: 'system', content: [{ type: 'text', text: result.systemMessage }] });
      }
      if (result.switchView === 'tree') {
        setShowTree(true);
      }
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  }, [client, sessionStore, msgStore]);

  const handleInputChange = useCallback((val: string) => {
    setInput(val);
    setShowCommandPicker(val.startsWith('/') && !val.includes(' '));
  }, []);

  const onSubmit = useCallback((value: string, images: PendingImage[] = []) => {
    setShowCommandPicker(false);
    if (value.startsWith('/')) {
      handleSlash(value);
    } else {
      sendMessage(value, images);
    }
  }, [handleSlash, sendMessage]);

  // 模型选择：点击 model 名称拉取可用模型列表，弹出 sheet
  const handleModelTap = useCallback(async () => {
    if (!client) return;
    try {
      const result: any = await client.request('config.list_models', {});
      setAvailableModels(result.models || result || []);
      setShowThinkingSheet(false);
      setShowModelSheet(true);
    } catch (e: any) {
      msgStore.setError(`fetch models failed: ${e.message}`);
    }
  }, [client, msgStore]);

  // 选择模型：调用 session.model.set RPC，更新 store，关闭 sheet
  const handleModelSelect = useCallback(async (name: string) => {
    if (!client || !sessionStore.currentSessionId) {
      setShowModelSheet(false);
      return;
    }
    try {
      await client.request('session.model.set', {
        session_id: sessionStore.currentSessionId,
        model: name,
      });
      sessionStore.setModel(name);
    } catch (e: any) {
      msgStore.setError(`set model failed: ${e.message}`);
    }
    setShowModelSheet(false);
  }, [client, sessionStore, msgStore]);

  // 断开连接
  // 终审修复 #17：断开连接时同时清 ask store，避免残留卡片
  // Phase 5c: 改用 clearSessionUiState({ clearAll: true }) 统一清理
  const disconnect = useCallback(() => {
    client?.close();
    setClient(null);
    setPairing('');
    setView('projects');
    setCurrentProject(null);
    setOpenTabs([]);
    sessionStore.reset();
    // Phase 5c: 统一 helper 清理 ask / todo / plan / task / messages
    try {
      clearSessionUiState({ clearAll: true });
    } catch (e) {
      console.warn('mobile disconnect: clearSessionUiState failed', e);
    }
    // 兼容旧路径：resetAll 仍调用以兜底
    try {
      useAskStore.getState().resetAll();
    } catch (e) {
      console.warn('mobile disconnect: ask store reset failed', e);
    }
    prefs.remove('mcoder_pairing');
  }, [client]);

  // Settings: 拉取 config 值（loop_max_iters / compact / memory）
  useEffect(() => {
    if (showSettings && client && sessionStore.currentSessionId) {
      Promise.all([
        client.request('config.get', { key: 'loop_max_iters' }).catch(() => null),
        client.request('config.get', { key: 'compact' }).catch(() => null),
        client.request('config.get', { key: 'memory' }).catch(() => null),
      ]).then(([iters, compact, memory]) => {
        setConfigValues({ loop_max_iters: iters, compact, memory });
      });
    }
  }, [showSettings, client, sessionStore.currentSessionId]);

  const handleConfigSet = useCallback(async (key: string, value: any) => {
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
  }, [client, msgStore]);

  const handleRoleChange = useCallback(async (role: string) => {
    if (!client || !sessionStore.currentSessionId) return;
    try {
      await client.request('session.mode.set', { session_id: sessionStore.currentSessionId, role });
      sessionStore.setRole(role);
    } catch (e: any) {
      msgStore.setError(`set role failed: ${e.message}`);
    }
  }, [client, sessionStore, msgStore]);

  // 当前项目内 tab 对应的会话列表（按创建时间排序）
  const currentProjectSessions = useMemo(() => {
    if (!currentProject) return [];
    const idSet = new Set(openTabs);
    return sessionStore.sessions
      .filter((s) => s.project_path === currentProject && idSet.has(s.session_id))
      .sort((a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime());
  }, [sessionStore.sessions, currentProject, openTabs]);

  // 未配对时显示配对界面
  if (!client || !pairing) {
    return <PairingScreen onConnect={connect} />;
  }

  // 项目选择页（入口）
  if (view === 'projects') {
    return (
      <div className="app">
        <ProjectList
          sessions={sessionStore.sessions}
          onSelectProject={enterProject}
          onNewSession={handleNewSessionForProject}
          onDisconnect={disconnect}
        />
      </div>
    );
  }

  // 会话页（tab 组织）
  const { connected, currentModel, contextUsed, contextWindow, sessionCost, version, lspServers, projectPath } = sessionStore;

  return (
    <div className="app">
      <div className="session-nav-bar">
        <button
          className="back-button"
          onClick={handleBackToProjects}
          aria-label="back to projects"
        >
          <ArrowLeft size={18} />
        </button>
        <SessionTabs
          sessions={currentProjectSessions}
          currentSessionId={sessionStore.currentSessionId}
          onSelectSession={attachSession}
          onCloseSession={handleCloseTab}
          onNewSession={handleNewSessionInTabs}
        />
        <button
          className="settings-button"
          onClick={() => setShowSettings(true)}
          aria-label="settings"
        >
          <Settings size={18} />
        </button>
      </div>

      {/* Plan/Todo 浮层（设计 §6.2/§6.7） */}
      {sessionStore.pendingPlan && sessionStore.currentSessionId && client && (
        <MobilePlanPanel
          plan={sessionStore.pendingPlan}
          client={client}
          sessionId={sessionStore.currentSessionId}
          onDismiss={() => sessionStore.setPendingPlan(null)}
        />
      )}
      {sessionStore.pendingTodos && sessionStore.pendingTodos.length > 0 && (
        <MobileTodoPanel todos={sessionStore.pendingTodos} />
      )}

      <MessageList
        messages={msgStore.messages}
        streaming={msgStore.streaming}
        error={msgStore.error}
        pendingCount={pendingQueue.length}
        client={client}
        currentSessionId={sessionStore.currentSessionId}
        onError={(m) => msgStore.setError(m)}
        version={sessionStore.version}
        model={currentModel}
        projectPath={sessionStore.projectPath}
        lspServers={sessionStore.lspServers}
        resultsById={(() => {
          const m = new Map<string, any>();
          for (const msg of msgStore.messages) {
            for (const block of (msg?.content || [])) {
              if (block.type === 'tool_result' && block.id && !m.has(block.id)) {
                m.set(block.id, block);
              }
            }
          }
          return m;
        })()}
      />

      {/* Todo 摘要条（消息区下方、输入框上方）；全部完成时隐藏 */}
      <TodoSummaryBar />

      {/* Phase 3: Resume 入口（固定状态提示附近；非模态） */}
      <ResumeBar client={client} sessionId={sessionStore.currentSessionId} />

      {/* 子代理实时 chip 栏：水平滚动，无子代理时隐藏 */}
      {client && sessionStore.currentSessionId && (
        <SubagentBar
          client={client}
          currentSessionId={sessionStore.currentSessionId}
          onSwitchSession={(sid) => attachSession(sid)}
        />
      )}

      {/* BottomStatus: 连接状态 / model / ctx / cost / running */}
      <div className="bottom-status">
        <span className={connected ? 'status-connected' : 'status-disconnected'}>
          {connected ? <CircleDot size={14} /> : <Circle size={14} />}
        </span>
        {currentModel && (
          <span className="status-model" onClick={handleModelTap}>
            {currentModel} <ChevronDown size={14} />
          </span>
        )}
        <span className="status-thinking" onClick={() => { setShowModelSheet(false); setShowThinkingSheet(true); }}>
          <Brain size={14} />{currentThinking !== 'none' ? currentThinking : ''}
        </span>
        <span className="status-ctx">
          {contextUsed > 1000 ? `${(contextUsed / 1000).toFixed(1)}k` : contextUsed}/{contextWindow > 1000 ? `${(contextWindow / 1000).toFixed(0)}k` : contextWindow}
        </span>
        {sessionCost > 0 && <span className="status-cost">${sessionCost.toFixed(3)}</span>}
        {msgStore.streaming && <span className="status-running">running</span>}
      </div>

      <InputBar
        value={input}
        onValueChange={setInput}
        onSubmit={onSubmit}
        onCancel={cancelStreaming}
        onChange={handleInputChange}
        streaming={msgStore.streaming}
        disabled={networkStatus === 'offline'}
      />

      {/* 命令选择面板：输入 / 时弹出 */}
      {showCommandPicker && client && (
        <CommandPicker
          client={client}
          filter={input}
          onSelect={(cmd) => {
            // 选中后把命令加一个空格写到输入框，handleInputChange 看到空格会自动关闭面板
            setInput(cmd + ' ');
            setShowCommandPicker(false);
          }}
          onClose={() => setShowCommandPicker(false)}
        />
      )}

      {/* 模型选择 sheet */}
      {showModelSheet && (
        <div className="model-sheet-overlay" onClick={() => setShowModelSheet(false)}>
          <div className="model-sheet" onClick={(e) => e.stopPropagation()}>
            <div className="model-sheet-header">
              <span className="model-sheet-title">Select Model</span>
              <button className="model-sheet-close" onClick={() => setShowModelSheet(false)}><X size={18} /></button>
            </div>
            <div className="model-sheet-list">
              {availableModels.map((m) => (
                <button
                  key={m.name}
                  className={`model-sheet-option ${m.name === currentModel ? 'model-sheet-option-active' : ''}`}
                  onClick={() => handleModelSelect(m.name)}
                >
                  <span className="model-sheet-option-name">{m.name}</span>
                  {(m as any).model && <span className="model-sheet-option-desc">{(m as any).model}</span>}
                </button>
              ))}
            </div>
          </div>
        </div>
      )}

      {/* 思考深度选择 sheet */}
      {showThinkingSheet && (
        <div className="model-sheet-overlay" onClick={() => setShowThinkingSheet(false)}>
          <div className="model-sheet" onClick={(e) => e.stopPropagation()}>
            <div className="model-sheet-header">
              <span className="model-sheet-title">Thinking Depth</span>
              <button className="model-sheet-close" onClick={() => setShowThinkingSheet(false)}><X size={18} /></button>
            </div>
            <div className="model-sheet-list">
              {['none', 'low', 'medium', 'high', 'max'].map((d) => (
                <button
                  key={d}
                  className={`model-sheet-option ${d === currentThinking ? 'model-sheet-option-active' : ''}`}
                  onClick={async () => {
                    if (!client) return;
                    try {
                      await client.request('config.quick_thinking', { session_id: sessionStore.currentSessionId, depth: d });
                      setCurrentThinking(d);
                      setShowThinkingSheet(false);
                    } catch (e: any) {
                      msgStore.setError(`set thinking depth failed: ${e.message}`);
                    }
                  }}
                >
                  <span className="model-sheet-option-name">
                    <Brain size={14} /> {d === 'none' ? 'Off' : d.charAt(0).toUpperCase() + d.slice(1)}
                  </span>
                  {d === currentThinking && <span className="model-sheet-option-desc"><Check size={14} /></span>}
                </button>
              ))}
            </div>
          </div>
        </div>
      )}

      {/* 消息树模态 */}
      {showTree && client && (
        <TreeView client={client} onClose={() => setShowTree(false)} />
      )}

      <Drawer
        open={drawerOpen}
        onClose={() => setDrawerOpen(false)}
        sessions={sessionStore.sessions}
        currentSessionId={sessionStore.currentSessionId}
        onSelectSession={handleDrawerSelectSession}
        onNewSession={handleNewSessionInTabs}
        onDisconnect={disconnect}
        onOpenSettings={() => { setDrawerOpen(false); setShowSettings(true); }}
        commands={commands}
      />

      {/* Settings 全屏页 */}
      {showSettings && (
        <div className="settings-page">
          <div className="settings-header">
            <button onClick={() => setShowSettings(false)}><ArrowLeft size={18} /></button>
            <span>Settings</span>
            <div className="settings-tabs">
              <button className={settingsTab === 'general' ? 'tab active' : 'tab'} onClick={() => setSettingsTab('general')}>General</button>
              <button className={settingsTab === 'providers' ? 'tab active' : 'tab'} onClick={() => setSettingsTab('providers')}>Providers</button>
            </div>
          </div>
          <div className="settings-body">
            <div className="form-row">
              <label>{t('ui.language')}</label>
              <select value={getLang()} onChange={async (e) => {
                const lang = e.target.value === 'zh' ? 'zh' : 'en';
                if (client) {
                  try { await client.request('config.set_language', { language: lang }); } catch {}
                }
                setLang(lang);
                setLangVersion(v => v + 1);
              }}>
                <option value="en">English</option>
                <option value="zh">中文</option>
              </select>
            </div>
            {settingsTab === 'providers' && client && (
              <ProviderScreen
                req={client.request.bind(client)}
                onConfigUpdated={(cb) => {
                  const handler = (n: any) => { if (n.method === 'config_updated') cb(); };
                  client.onNotification(handler);
                  return () => client.offNotification(handler);
                }}
              />
            )}
            {settingsTab === 'general' && (<>
            <div className="setting-row">
              <div className="setting-label">
                <span className="setting-name">Model</span>
                <span className="setting-desc">LLM model for this session</span>
              </div>
              <div className="setting-control">
                <button className="setting-model-btn" onClick={() => { setShowSettings(false); handleModelTap(); }}>
                  {currentModel || '(not set)'} <ChevronDown size={14} />
                </button>
              </div>
            </div>
            <div className="setting-row">
              <div className="setting-label">
                <span className="setting-name">Role</span>
                <span className="setting-desc">Agent role / mode</span>
              </div>
              <div className="setting-control">
                <select value={sessionStore.currentRole} onChange={(e) => handleRoleChange(e.target.value)}>
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
      )}
    </div>
  );
}
