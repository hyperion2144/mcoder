// 设计文档 §8.6.2: 移动客户端主应用
// 以项目为入口，项目内多会话用 tab 组织
// 单栏布局，触摸友好，弱网友好
// 复用 TUI 的 rpc/store/commands/utils 逻辑层

import React, { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { WsClient } from '@mcoder/shared/rpc/client.js';
import { useSessionStore, useMessagesStore } from '@mcoder/shared/store/index.js';
import { findCommand, listCommands } from '@mcoder/shared/commands/index.js';
import { parsePairingString } from '@mcoder/shared/utils/pairing.js';
import { NetworkMonitor } from './network.js';
import { PairingScreen } from './components/PairingScreen.js';
import { Drawer } from './components/Drawer.js';
import { MessageList } from './components/MessageList.js';
import { InputBar } from './components/InputBar.js';
import { StatusBar } from './components/StatusBar.js';
import { ProjectList } from './components/ProjectList.js';
import { SessionTabs } from './components/SessionTabs.js';

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
        <span className="plan-panel-title">Plan pending approval</span>
        <button className="plan-panel-close" onClick={onDismiss} aria-label="close">×</button>
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
        <button className="plan-btn plan-btn-approve" onClick={handleApprove}>Approve</button>
        <button className="plan-btn plan-btn-reject" onClick={handleReject}>Reject</button>
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
        <span className="todo-panel-title">Todos</span>
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
              <span className="todo-check">{isDone ? '✓' : '☐'}</span>
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

export function App() {
  const [client, setClient] = useState<WsClient | null>(null);
  const [pairing, setPairing] = useState<string>('');
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [networkStatus, setNetworkStatus] = useState<'online' | 'offline'>('online');
  const [pendingQueue, setPendingQueue] = useState<string[]>([]);
  // 项目入口视图状态：projects 为项目选择页，sessions 为项目内会话 tab 页
  const [view, setView] = useState<'projects' | 'sessions'>('projects');
  const [currentProject, setCurrentProject] = useState<string | null>(null);
  // 当前项目内打开为 tab 的会话 ID 列表
  const [openTabs, setOpenTabs] = useState<string[]>([]);
  const sessionStore = useSessionStore();
  const msgStore = useMessagesStore();
  const networkMonitor = useRef<NetworkMonitor | null>(null);

  // P1-5: 用 ref 保存最新的 sendMessage 和 client，避免闭包过期
  const sendMessageRef = useRef<(content: string) => void>(() => {});
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
        queue.forEach((msg) => sendMessageRef.current(msg));
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
    } catch (e: any) {
      msgStore.setError(`Connection failed: ${e.message}`);
    }

    // 通知处理
    c.onNotification((notif) => {
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
        case 'session.plan_created':
          sessionStore.setPendingPlan(notif.params.plan);
          break;
        case 'session.todo_updated':
          sessionStore.setPendingTodos(notif.params.todos);
          break;
        case 'error':
          msgStore.setError(notif.params.message);
          msgStore.setStreaming(false);
          break;
      }
    });
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

  // attach 到指定会话并加载消息
  const attachSession = useCallback(async (id: string) => {
    if (!client) return;
    try {
      const result = await client.request('session.attach', { session_id: id });
      sessionStore.setCurrentSession(id);
      client.setReconnectSession(id);
      msgStore.setMessages(result.messages || []);
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  }, [client]);

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

  const sendMessage = useCallback(async (content: string) => {
    if (!client || !content.trim()) return;

    // 弱网检测：离线时排队
    if (networkStatus === 'offline') {
      setPendingQueue((q) => [...q, content]);
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
    msgStore.addMessage({ role: 'user', content: [{ type: 'text', text: content }] });
    msgStore.setStreaming(true);
    try {
      await client.request('sessions.send', { session_id: sid, content });
    } catch (e: any) {
      // 发送失败，排队重试
      setPendingQueue((q) => [...q, content]);
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
    const parts = cmd.slice(1).split(/\s+/);
    const cmdDef = findCommand(parts[0]);
    if (!cmdDef) {
      msgStore.setError(`unknown command: /${parts[0]}`);
      return;
    }
    try {
      const result = await cmdDef.handler(parts.slice(1), client);
      if (result.error) msgStore.setError(result.error);
      if (result.systemMessage) {
        msgStore.addMessage({ role: 'system', content: [{ type: 'text', text: result.systemMessage }] });
      }
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  }, [client]);

  const onSubmit = useCallback((value: string) => {
    if (value.startsWith('/')) {
      handleSlash(value);
    } else {
      sendMessage(value);
    }
  }, [handleSlash, sendMessage]);

  // 断开连接
  const disconnect = useCallback(() => {
    client?.close();
    setClient(null);
    setPairing('');
    setView('projects');
    setCurrentProject(null);
    setOpenTabs([]);
    sessionStore.reset();
    msgStore.setMessages([]);
    prefs.remove('mcoder_pairing');
  }, [client]);

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
  const { connected, currentModel, currentRole, contextUsed, contextWindow, sessionCost } = sessionStore;

  return (
    <div className="app">
      <StatusBar
        connected={connected}
        networkStatus={networkStatus}
        role={currentRole}
        model={currentModel}
        contextUsed={contextUsed}
        contextWindow={contextWindow}
        cost={sessionCost}
        onMenuClick={() => setDrawerOpen(true)}
      />

      <div className="session-nav-bar">
        <button
          className="back-button"
          onClick={handleBackToProjects}
          aria-label="back to projects"
        >
          ←
        </button>
        <SessionTabs
          sessions={currentProjectSessions}
          currentSessionId={sessionStore.currentSessionId}
          onSelectSession={attachSession}
          onCloseSession={handleCloseTab}
          onNewSession={handleNewSessionInTabs}
        />
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
      />

      <InputBar
        onSubmit={onSubmit}
        onCancel={cancelStreaming}
        streaming={msgStore.streaming}
        disabled={networkStatus === 'offline'}
      />

      <Drawer
        open={drawerOpen}
        onClose={() => setDrawerOpen(false)}
        sessions={sessionStore.sessions}
        currentSessionId={sessionStore.currentSessionId}
        onSelectSession={handleDrawerSelectSession}
        onNewSession={handleNewSessionInTabs}
        onDisconnect={disconnect}
        commands={listCommands()}
      />
    </div>
  );
}
