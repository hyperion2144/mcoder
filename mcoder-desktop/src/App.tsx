// 设计文档 §8.6.1: 桌面端主应用
// 两阶段视图：项目选择页 → 会话页（tab 组织同项目的多个会话）
// 会话页内保留原三栏布局：左（文件树）| 中（聊天）| 右（图谱/Diff/文件预览 标签页）
// 复用 TUI 的 rpc/store/commands/utils 逻辑层

import React, { useState, useEffect, useCallback } from 'react';
import { WsClient } from '@mcoder/shared/rpc/client.js';
import { useSessionStore, useMessagesStore } from '@mcoder/shared/store/index.js';
import { dispatchSlashCommand } from '@mcoder/shared/commands/index.js';
import { parsePairingString } from '@mcoder/shared/utils/pairing.js';
import type { SessionMeta } from '@mcoder/shared/rpc/types.js';
import { useDesktopStore } from './store/index.js';
import { FileTree } from './components/FileTree.js';
import { GraphView } from './components/GraphView.js';
import { DiffViewer } from './components/DiffViewer.js';
import { ProjectList } from './components/ProjectList.js';
import { SessionTabs } from './components/SessionTabs.js';
import { PlanPanel } from './components/PlanPanel.js';
import { TodoPanel } from './components/TodoPanel.js';

type RightPanel = 'graph' | 'diff' | 'file' | 'none';

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

// 工具调用结果折叠组件
function ToolResultBlock({ output }: { output: any }) {
  const [expanded, setExpanded] = useState(false);
  const outputStr = typeof output === 'string' ? output : JSON.stringify(output, null, 2);
  const isLong = outputStr.length > 300;
  const preview = isLong ? outputStr.slice(0, 300) : outputStr;
  return (
    <div className="tool-result">
      <pre className="tool-result-content">
        {expanded ? outputStr : preview}
        {isLong && !expanded && <span className="tool-result-ellipsis">... ({outputStr.length} chars)</span>}
      </pre>
      {isLong && (
        <button className="tool-result-toggle" onClick={() => setExpanded(!expanded)}>
          {expanded ? '▾ Collapse' : '▸ Expand'}
        </button>
      )}
    </div>
  );
}

// 单条消息渲染
function MessageItem({ msg, index }: { msg: any; index: number }) {
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
              return (
                <div key={j} className="tool-call">
                  <span className="tool-name">{block.name}</span>
                  <span className="tool-args">{JSON.stringify(block.args).slice(0, 200)}</span>
                </div>
              );
            }
            if (block.type === 'tool_result') {
              return <ToolResultBlock key={j} output={block.output} />;
            }
            return null;
          })}
        </div>
      </div>
    </div>
  );
}

export function App() {
  const [client, setClient] = useState<WsClient | null>(null);
  const [input, setInput] = useState('');
  const [rightPanel, setRightPanel] = useState<RightPanel>('none');
  const [previewFile, setPreviewFile] = useState<{ path: string; content: string } | null>(null);
  const sessionStore = useSessionStore();
  const msgStore = useMessagesStore();
  const desktop = useDesktopStore();

  const { view, currentProject, openTabs } = desktop;
  const { sessions, currentSessionId, pendingPlan, pendingTodos } = sessionStore;

  // attach 到指定会话：加载消息历史并切换 currentSession
  const attachSession = useCallback(async (sessionId: string) => {
    if (!client) return;
    try {
      const result = await client.request('session.attach', { session_id: sessionId });
      sessionStore.setCurrentSession(sessionId);
      client.setReconnectSession(sessionId);
      msgStore.setMessages(result.messages || []);
      if (result.role) sessionStore.setRole(result.role);
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
    const params = new URLSearchParams(window.location.search);
    const pairingStr = params.get('pairing') || localStorage.getItem('mcoder_pairing') || '';
    if (!pairingStr) {
      msgStore.setError('No pairing info. Pass ?pairing=mcoder://... in URL.');
      return;
    }
    const parsed = parsePairingString(pairingStr);
    if (!parsed) {
      msgStore.setError('Invalid pairing string.');
      return;
    }

    const c = new WsClient(
      parsed.url,
      parsed.token,
      () => sessionStore.setConnected(true),
      () => sessionStore.setConnected(false),
    );
    setClient(c);

    c.connect().then(async () => {
      try {
        const allSessions: SessionMeta[] = await c.request('sessions.list');
        sessionStore.setSessions(allSessions);
        desktop.setView('projects');
      } catch {}
    }).catch(e => {
      msgStore.setError(`Connection failed: ${e.message}`);
    });

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

    return () => c.close();
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
    sessionStore.setCurrentSession(null);
    client?.setReconnectSession(undefined);
    msgStore.setMessages([]);
    setPreviewFile(null);
    setRightPanel('none');
    refreshSessions();
  }, [desktop, sessionStore, client, msgStore, refreshSessions]);

  // FileTree 选中文件回调
  const handleFileSelect = useCallback((path: string, content: string) => {
    setPreviewFile({ path, content });
    setRightPanel('file');
  }, []);

  const sendMessage = async () => {
    if (!client || !input.trim()) return;
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
    msgStore.addMessage({ role: 'user', content: [{ type: 'text', text: input }] });
    msgStore.setStreaming(true);
    setInput('');
    try {
      await client.request('sessions.send', { session_id: sid, content: input });
    } catch (e: any) {
      msgStore.setError(e.message);
      msgStore.setStreaming(false);
    }
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
    } catch (e: any) {
      msgStore.setError(e.message);
    }
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
  const { connected, currentModel, currentRole, contextUsed, contextWindow, sessionCost } = sessionStore;
  const ctxPct = contextWindow > 0 ? Math.round((contextUsed / contextWindow) * 100) : 0;

  const projectSessions = currentProject
    ? sessions.filter((s) => s.project_path === currentProject)
    : [];

  return (
    <div className="app">
      {/* Header */}
      <div className="header">
        <span className="header-title">mcoder</span>
        <span className={`header-status ${connected ? 'connected' : 'disconnected'}`}>
          {connected ? '●' : '○'}
        </span>
        {view === 'sessions' && currentProject && (
          <>
            <button className="header-back" onClick={handleBack} title="Back to projects">
              ←
            </button>
            <span className="header-project" title={currentProject}>
              {currentProject.split('/').pop() || currentProject}
            </span>
          </>
        )}
        <span className="header-info">
          {currentRole !== 'default' && <span className="header-role">{currentRole}</span>}
          {currentModel && <span className="header-model">{currentModel}</span>}
          <span className="header-ctx" title={`${contextUsed}/${contextWindow} tokens`}>
            <span className="ctx-text">{contextUsed > 1000 ? `${(contextUsed / 1000).toFixed(1)}k` : contextUsed}/{contextWindow > 1000 ? `${(contextWindow / 1000).toFixed(0)}k` : contextWindow}</span>
            <span className="ctx-bar"><span className="ctx-bar-fill" style={{ width: `${Math.min(ctxPct, 100)}%` }} /></span>
          </span>
          {sessionCost > 0 && <span className="header-cost">${sessionCost.toFixed(3)}</span>}
        </span>
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
                  <MessageItem key={i} msg={msg} index={i} />
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
                <textarea
                  value={input}
                  onChange={e => setInput(e.target.value)}
                  onKeyDown={onInputKeyDown}
                  placeholder="type a message or /help for commands (Shift+Enter for newline)"
                  rows={3}
                />
                <div className="input-toolbar">
                  <span className="input-hint">Enter to send · Shift+Enter for newline</span>
                  {streaming && (
                    <button className="cancel-btn" onClick={cancelStreaming}>Cancel</button>
                  )}
                </div>
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
    </div>
  );
}
