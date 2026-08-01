// 设计文档 §6.9: commands/index.ts - Slash command 客户端
//
// 所有 slash command 的解析和分发在服务端进行（commands/mod.rs::CommandDispatcher）
// 客户端只负责：
//   1. 把 /xxx 输入转发到服务端 (command.call)
//   2. 根据返回的 DispatchResult 执行对应的 UI 动作
//      - Meta: 结构化指令，由客户端执行对应 RPC（如 session.mode.set）
//      - CustomCommand / Skill: 服务端已渲染好的提示词，注入对话流
//      - Unknown: 报错
//
// 自定义命令和 skill 的加载/渲染全在服务端，客户端零感知

import type { WsClient } from '../rpc/client.js';
import { useSessionStore, useMessagesStore } from '../store/index.js';
import { generateQrCode } from '../utils/pairing.js';
import { useAskStore } from '../ask/store.js';
import { hydrateSnapshot, type SessionSnapshot } from '../rpc/sessionSnapshot.js';

/// 服务端返回的 DispatchResult（对应 commands/mod.rs::DispatchResult）
export type DispatchResult =
  | { kind: 'meta'; result: MetaCommandResult }
  | { kind: 'custom_command'; name: string; prompt: string }
  | { kind: 'skill'; name: string; prompt: string }
  | { kind: 'unknown'; name: string };

/// 元命令结果（对应 commands/mod.rs::MetaCommandResult）
export type MetaCommandResult =
  | { type: 'mode'; role: string }
  | { type: 'model'; action: string; model: string | null }
  | { type: 'sessions'; action: string; session_id: string | null }
  | { type: 'undo'; id: string | null }
  | { type: 'diff' }
  | { type: 'cancel' }
  | { type: 'task'; action: string; task_id: string | null }
  | { type: 'config'; key: string; value: string | null }
  | { type: 'pair' }
  | { type: 'exit' }
  | { type: 'help' }
  | { type: 'tree' }
  | { type: 'setting' }
  | { type: 'provider' }
  | { type: 'providers' }
  | { type: 'thinking' }
  | { type: 'remote'; args: string[] }
  | { type: 'workflow'; action: string; change_id: string | null; args: string[]; prompt: string };

/// 客户端处理结果：告诉调用方需要做什么 UI 动作
export interface CommandResult {
  /// 需要添加到消息流的系统消息（可选）
  systemMessage?: string;
  /// 是否需要切换视图
  switchView?: 'chat' | 'sessions' | 'todos' | 'tasks' | 'config' | 'help' | 'diff' | 'tree' | 'model' | 'setting' | 'provider' | 'thinking';
  /// 是否需要重新加载会话列表
  loadSessions?: boolean;
  /// 是否需要退出
  exit?: boolean;
  /// 是否需要重新连接到远程服务器
  reconnect?: string;
  /// 错误信息
  error?: string;
}

/// 分发 slash command
/// 输入：完整的 /xxx args 字符串
/// 流程：转发到服务端 → 根据 DispatchResult 执行对应动作
export async function dispatchSlashCommand(
  input: string,
  client: WsClient,
): Promise<CommandResult> {
  // 去掉前导 /
  const stripped = input.startsWith('/') ? input.slice(1) : input;
  if (!stripped.trim()) {
    return { error: 'empty command' };
  }

  let result: DispatchResult;
  try {
    // 客户端拦截 /thinking 和 /think：服务端无此 meta 命令，直接返回 thinking 视图
    const firstWord = stripped.split(/\s+/)[0]?.toLowerCase();
    if (firstWord === 'thinking' || firstWord === 'think') {
      result = { kind: 'meta', result: { type: 'thinking' } };
    } else if (firstWord === 'handoff') {
      // /handoff <task description> -> session.handoff RPC
      const taskPrompt = stripped.slice(firstWord.length).trim();
      if (!taskPrompt) return { error: 'Usage: /handoff <task description>' };
      const sid = useSessionStore.getState().currentSessionId;
      if (!sid) return { error: 'no active session' };
      try {
        const handoffResult = await client.request('session.handoff', {
          session_id: sid,
          task_prompt: taskPrompt,
        });
        return {
          systemMessage: `Handoff -> ${handoffResult.new_session_id}\n\n${handoffResult.handoff_doc}`,
        };
      } catch (e: any) {
        return { error: e.message };
      }
    } else if (firstWord === 'handoff-back') {
      // /handoff-back -> session.handoff_back RPC
      const sid = useSessionStore.getState().currentSessionId;
      if (!sid) return { error: 'no active session' };
      try {
        const backResult = await client.request('session.handoff_back', {
          from_session_id: sid,
        });
        return {
          systemMessage: `Handoff back to ${backResult.to_session_id}:\n\n${backResult.back_doc}`,
        };
      } catch (e: any) {
        return { error: e.message };
      }
    } else {
      result = await client.request('command.call', { input: stripped });
    }
  } catch (e: any) {
    return { error: e.message };
  }

  return handleDispatchResult(result, client);
}

/// 处理服务端返回的 DispatchResult
async function handleDispatchResult(
  result: DispatchResult,
  client: WsClient,
): Promise<CommandResult> {
  switch (result.kind) {
    case 'meta':
      return handleMetaCommand(result.result, client);

    case 'custom_command':
    case 'skill': {
      // 服务端已渲染好提示词，作为用户消息注入对话流
      const prompt = result.prompt;
      const msgStore = useMessagesStore.getState();
      const sessionStore = useSessionStore.getState();
      const sid = sessionStore.currentSessionId;
      if (!sid) {
        return { error: 'no active session' };
      }
      msgStore.addMessage({
        role: 'user',
        content: [{ type: 'text', text: prompt }],
      });
      msgStore.setStreaming(true);
      try {
        await client.request('sessions.send', { session_id: sid, content: prompt });
      } catch (e: any) {
        msgStore.setError(e.message);
        msgStore.setStreaming(false);
      }
      return {};
    }

    case 'unknown':
      return { error: `unknown command: /${result.name} (try /help)` };
  }
}

/// 处理元命令：客户端执行对应的 RPC 和 UI 动作
/// 元命令是内置命令，需要客户端配合做 UI 切换或调用特定 RPC
async function handleMetaCommand(
  meta: MetaCommandResult,
  client: WsClient,
): Promise<CommandResult> {
  const sessionStore = useSessionStore.getState();
  const msgStore = useMessagesStore.getState();

  switch (meta.type) {
    case 'help':
      return { switchView: 'help' };

    case 'tree':
      return { switchView: 'tree' };

    case 'setting':
      return { switchView: 'setting' };

    case 'exit':
      return { exit: true };

    case 'mode': {
      const sid = sessionStore.currentSessionId;
      if (!sid) return { error: 'no active session' };
      try {
        await client.request('session.mode.set', { session_id: sid, role: meta.role });
        sessionStore.setRole(meta.role);
        return {};
      } catch (e: any) {
        return { error: e.message };
      }
    }

    case 'model': {
      const sid = sessionStore.currentSessionId;
      if (meta.action === 'picker') {
        // /model (no args) -> open model picker view
        return { switchView: 'model' };
      } else if (meta.action === 'list') {
        try {
          const result = await client.request('config.list_models', {});
          const models = (result as any)?.models || result || [];
          const lines = Array.isArray(models)
            ? models.map((m: any) => `  ${m.name}${m.model ? `  (${m.model})` : ''}${m.context_window ? `  ctx=${m.context_window}` : ''}`)
            : [JSON.stringify(models, null, 2)];
          return { systemMessage: 'Available models:\n' + lines.join('\n') };
        } catch (e: any) {
          return { error: e.message };
        }
      } else if (meta.action === 'set' && meta.model) {
        if (!sid) return { error: 'no active session' };
        try {
          await client.request('session.model.set', { session_id: sid, model: meta.model });
          sessionStore.setModel(meta.model);
          return {};
        } catch (e: any) {
          return { error: e.message };
        }
      }
      return { error: 'usage: /model [list|set <name>]' };
    }

    case 'provider':
    case 'providers': {
      return { switchView: 'provider' };
    }

    case 'thinking': {
      return { switchView: 'thinking' };
    }

    case 'sessions': {
      const action = meta.action;
      if (action === 'list') {
        try {
          const result = await client.request('sessions.list');
          sessionStore.setSessions(result);
          return { switchView: 'sessions' };
        } catch (e: any) {
          return { error: e.message };
        }
      } else if (action === 'new') {
        try {
          const result = await client.request('sessions.create', { title: 'New Session' });
          sessionStore.setCurrentSession(result.session_id);
          client.setReconnectSession(result.session_id);
          msgStore.setMessages([]);
          try {
            const msgs = await client.request('sessions.messages', {
              session_id: result.session_id,
            });
            msgStore.setMessages(msgs);
          } catch {}
          return {};
        } catch (e: any) {
          return { error: e.message };
        }
      } else if (action === 'open' && meta.session_id) {
        const targetSid = meta.session_id;
        try {
          // Phase 2: 直接用 SessionSnapshot hydrate，不再单独 ask.pending / session.mode.get
          const snapshot = await client.request('session.attach', {
            session_id: targetSid,
          }) as SessionSnapshot;
          client.setReconnectSession(targetSid);
          hydrateSnapshot({
            sessionId: targetSid,
            snapshot,
            store: {
              setCurrentSessionId: (id) => useSessionStore.getState().setCurrentSession(id),
              setMessages: (m) => useMessagesStore.getState().setMessages(m),
              setRole: (r) => useSessionStore.getState().setRole(r),
              setModel: (m) => useSessionStore.getState().setModel(m),
              setProjectPath: (p) => useSessionStore.getState().setProjectPath(p),
              setContextUsage: (used, _w) => useSessionStore.getState().setContextUsage(used, useSessionStore.getState().contextWindow || 0),
              setPendingPlan: (p) => useSessionStore.getState().setPendingPlan(p),
              setPendingTodos: (t) => useSessionStore.getState().setPendingTodos(t),
              setBackgroundTasks: (t) => useSessionStore.getState().setBackgroundTasks(t),
              setPendingAskFromSnapshot: (ask) => {
                const askStore = useAskStore.getState();
                if (ask === null) {
                  askStore.clearSession(targetSid);
                  return;
                }
                askStore.setPendingAskFromSnapshot(ask);
              },
              clearAskSession: (sid) => useAskStore.getState().clearSession(sid),
              replaceTodosFromSnapshot: (_t) => {
                // setPendingTodos 已替换全部
              },
            },
          });
          useSessionStore.getState().setLoopState(snapshot.session.loop_state, snapshot.session.stop_reason);
          useSessionStore.getState().setCanResume(snapshot.can_resume);
          useSessionStore.getState().setVersion(snapshot.session.version);
          useSessionStore.getState().setLspServers(snapshot.session.lsp_servers);
          return {};
        } catch (e: any) {
          return { error: e.message };
        }
      } else if (action === 'delete' && meta.session_id) {
        try {
          await client.request('session.delete', { session_id: meta.session_id });
          return { loadSessions: true };
        } catch (e: any) {
          return { error: e.message };
        }
      }
      return { error: 'usage: /sessions <list|new|open <id>|delete <id>>' };
    }

    case 'undo': {
      const sid = sessionStore.currentSessionId;
      if (!sid) return { error: 'no active session' };
      try {
        const undoArgs: any = {};
        if (meta.id) {
          undoArgs.op = 'undo';
          undoArgs.id = meta.id;
        } else {
          undoArgs.op = 'undo';
        }
        const result = await client.request('tool.call', { name: 'undo', args: undoArgs });
        return { systemMessage: 'undo: ' + JSON.stringify(result) };
      } catch (e: any) {
        return { error: e.message };
      }
    }

    case 'diff': {
      try {
        const result = await client.request('tool.call', {
          name: 'bash',
          args: { cmd: 'git diff --stat', timeout: 10 },
        });
        return { systemMessage: 'diff:\n' + JSON.stringify(result, null, 2), switchView: 'diff' };
      } catch (e: any) {
        return { error: e.message };
      }
    }

    case 'cancel': {
      const sid = sessionStore.currentSessionId;
      if (!sid) return { error: 'no active session' };
      try {
        await client.request('session.cancel', { session_id: sid });
        msgStore.setStreaming(false);
        return {};
      } catch (e: any) {
        return { error: e.message };
      }
    }

    case 'task': {
      if (meta.action === 'list') {
        try {
          const result = await client.request('task.list');
          sessionStore.setTaskCount(result.length);
          sessionStore.setBackgroundTasks(result);
          return { switchView: 'tasks' };
        } catch (e: any) {
          return { error: e.message };
        }
      } else if (meta.action === 'cancel' && meta.task_id) {
        try {
          await client.request('task.cancel', { task_id: meta.task_id });
          return {};
        } catch (e: any) {
          return { error: e.message };
        }
      }
      return { error: 'usage: /task <list|cancel <id>>' };
    }

    case 'config': {
      if (meta.key === 'get' || meta.value === null) {
        // /config get <key>
        try {
          const result = await client.request('config.get', { key: meta.key });
          return {
            systemMessage: 'config:\n' + JSON.stringify(result, null, 2),
            switchView: 'config',
          };
        } catch (e: any) {
          return { error: e.message };
        }
      } else {
        // /config set <key> <value>
        try {
          await client.request('config.set', { key: meta.key, value: meta.value });
          return {};
        } catch (e: any) {
          return { error: e.message };
        }
      }
    }

    case 'pair': {
      try {
        const result = await client.request('config.get', { key: null });
        const pairingStr = result.pairing_string || '';
        const qr = pairingStr ? generateQrCode(pairingStr) : '';
        const msg =
          'Server info:\n' +
          JSON.stringify(result, null, 2) +
          (qr ? '\n\nScan QR to connect:\n' + qr : '');
        return { systemMessage: msg };
      } catch (e: any) {
        return { error: e.message };
      }
    }

    case 'remote': {
      // /remote mcoder://token@host:port
      // /remote ws://host:port token
      const raw = meta.args.join(' ');
      return { reconnect: raw };
    }

    case 'workflow': {
      // /workflow slash command
      const { action, prompt } = meta;
      // prompt 非空时直接注入为 system message，让 agent 执行编排步骤
      if (prompt) {
        return { systemMessage: prompt };
      }
      // 仅 list 没有编排 prompt，直接查询工作流概览。
      if (action !== 'list') {
        return { error: `workflow action ${action} did not return an orchestration prompt` };
      }
      try {
        const result = await client.request('tool.call', {
          name: 'workflow_query',
          args: { op: 'list' },
        });
        return { systemMessage: 'workflow: ' + JSON.stringify(result, null, 2) };
      } catch (e: any) {
        return { error: e.message };
      }
    }
  }
}
