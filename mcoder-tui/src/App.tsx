// 设计文档 §6.2 / §6.3: 主应用组件
// 布局：消息区（可滚动） + 固定区（ContextLine + ProjectLine + 输入框）
// 设计文档 §6.7: 多视图切换（chat/sessions/todos/tasks/config/help）

import { useState, useEffect } from 'react';
import { Box, Text, useInput, useApp } from 'ink';
import { useSessionStore, useMessagesStore, useUiStore } from './store/index.js';
import { findCommand } from './commands/index.js';
import type { WsClient } from './rpc/client.js';
import type { Message } from './rpc/types.js';
import {
  ContextLine, ProjectLine, CompactLine,
  MessageList, PlanApproval,
  SessionList, TodoView, TaskMonitor, ConfigView, HelpView,
  InputBox,
} from './components/index.js';

interface Props {
  client: WsClient;
}

export function App({ client }: Props) {
  const [input, setInput] = useState('');
  const uiStore = useUiStore();
  const sessionStore = useSessionStore();
  const msgStore = useMessagesStore();
  const { exit } = useApp();

  // 通知处理
  useEffect(() => {
    client.onNotification((notif) => {
      switch (notif.method) {
        case 'message':
          msgStore.addMessage(notif.params.message as Message);
          if (notif.params.message.role === 'assistant') {
            msgStore.setStreaming(false);
          }
          break;
        case 'tool_call_start':
          msgStore.setStreaming(true);
          break;
        case 'tool_call_done':
          break;
        case 'session_created':
          loadSessions();
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

  const loadSessions = async () => {
    try {
      const result = await client.request('sessions.list');
      sessionStore.setSessions(result);
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  };

  const sendMessage = async (content: string) => {
    let sid = sessionStore.currentSessionId;
    if (!sid) {
      // 自动创建会话
      try {
        const result = await client.request('sessions.create', { title: 'New Session' });
        sessionStore.setCurrentSession(result.session_id);
        client.setReconnectSession(result.session_id);
        sid = result.session_id;
      } catch (e: any) {
        msgStore.setError(e.message);
        return;
      }
    }
    msgStore.addMessage({ role: 'user', content: [{ type: 'text', text: content }] });
    msgStore.setStreaming(true);
    setInput('');
    try {
      await client.request('sessions.send', { session_id: sid, content });
    } catch (e: any) {
      msgStore.setError(e.message);
      msgStore.setStreaming(false);
    }
  };

  const handleSlashCommand = async (cmd: string) => {
    const parts = cmd.slice(1).split(/\s+/);
    const commandName = parts[0];
    const args = parts.slice(1);
    const cmdDef = findCommand(commandName);
    if (!cmdDef) {
      msgStore.setError(`unknown command: /${commandName} (try /help)`);
      return;
    }
    try {
      const result = await cmdDef.handler(args, client);
      if (result.error) msgStore.setError(result.error);
      else msgStore.setError(null);
      if (result.systemMessage) {
        msgStore.addMessage({
          role: 'system',
          content: [{ type: 'text', text: result.systemMessage }],
        });
      }
      if (result.switchView) uiStore.setView(result.switchView);
      if (result.loadSessions) await loadSessions();
      if (result.exit) exit();
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  };

  const onSubmit = (value: string) => {
    if (value.startsWith('/')) {
      handleSlashCommand(value);
      setInput('');
    } else {
      sendMessage(value);
    }
  };

  // 设计文档 §6.2 / §6.7 / §6.8: 全局快捷键
  useInput((inputChar: string, key: any) => {
    // Ctrl+C 退出
    if (key.ctrl && inputChar === 'c') {
      exit();
      return;
    }
    // ESC 关闭覆盖层
    if (key.escape) {
      uiStore.setView('chat');
      return;
    }
    // Ctrl+S 会话列表
    if (key.ctrl && inputChar === 's') {
      loadSessions();
      uiStore.setView('sessions');
      return;
    }
    // Ctrl+T Todo 视图
    if (key.ctrl && inputChar === 't') {
      uiStore.setView('todos');
      return;
    }
    // Ctrl+K 任务监控
    if (key.ctrl && inputChar === 'k') {
      client.request('task.list').then((tasks) => {
        sessionStore.setTaskCount(tasks.length);
        sessionStore.setBackgroundTasks(tasks);
        uiStore.setView('tasks');
      }).catch(() => {});
      return;
    }
    // Ctrl+, 配置视图
    if (key.ctrl && inputChar === ',') {
      uiStore.setView('config');
      return;
    }
    // 设计文档 §6.2: PgUp/PgDn 滚动消息
    if (key.pageUp) {
      uiStore.scrollUp(10);
      return;
    }
    if (key.pageDown) {
      uiStore.scrollDown(10);
      return;
    }
  });

  const currentView = uiStore.currentView;

  return (
    <Box flexDirection="column" height="100%">
      {/* Header */}
      <Box justifyContent="space-between" paddingX={1}>
        <Text bold color="cyan">mcoder</Text>
        <Text color={sessionStore.connected ? 'green' : 'red'}>
          {sessionStore.connected ? '● connected' : '● disconnected'}
        </Text>
      </Box>

      {/* 消息区（可滚动） */}
      <MessageList />

      {/* Plan 审批 */}
      <PlanApproval client={client} />

      {/* 覆盖层视图 */}
      {currentView === 'sessions' && <SessionList />}
      {currentView === 'todos' && <TodoView />}
      {currentView === 'tasks' && <TaskMonitor />}
      {currentView === 'config' && <ConfigView />}
      {currentView === 'help' && <HelpView />}

      {/* 设计文档 §6.5: 紧凑模式 */}
      {uiStore.compact ? (
        <CompactLine />
      ) : (
        <>
          <ContextLine />
          <ProjectLine />
        </>
      )}

      {/* 输入框 */}
      <InputBox value={input} onChange={setInput} onSubmit={onSubmit} />
    </Box>
  );
}
