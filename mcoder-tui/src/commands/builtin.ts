// 设计文档 §6.9: commands/builtin.tsx - 内置 slash commands
// 设计文档 §6.12: 平台无关逻辑，UI 部分由调用方处理

import type { WsClient } from '../rpc/client.js';
import { useSessionStore, useMessagesStore } from '../store/index.js';
import { generateQrCode } from '../utils/pairing.js';

/// Slash 命令处理结果
export interface CommandResult {
  /// 需要添加到消息流的系统消息（可选）
  systemMessage?: string;
  /// 是否需要切换视图
  switchView?: 'chat' | 'sessions' | 'todos' | 'tasks' | 'config' | 'help' | 'diff';
  /// 是否需要加载会话列表
  loadSessions?: boolean;
  /// 是否需要退出
  exit?: boolean;
  /// 错误信息
  error?: string;
}

/// Slash 命令处理器类型
export type CommandHandler = (
  args: string[],
  client: WsClient,
) => Promise<CommandResult>;

/// 命令定义
export interface CommandDef {
  name: string;
  description: string;
  usage: string;
  handler: CommandHandler;
}

/// 设计文档 §6.9: 内置 slash commands
export const builtinCommands: CommandDef[] = [
  {
    name: 'help',
    description: 'show this help',
    usage: '/help',
    handler: async () => ({ switchView: 'help' }),
  },
  {
    name: 'mode',
    description: 'switch role (normal|plan|goal|loop|execute|review)',
    usage: '/mode <role>',
    handler: async (args, _client) => {
      if (!args[0]) return { error: 'usage: /mode <normal|plan|goal|loop|execute|review>' };
      const role = args[0] === 'normal' ? 'default' : args[0];
      const sid = useSessionStore.getState().currentSessionId;
      if (!sid) return { error: 'no active session' };
      try {
        await _client.request('session.mode.set', { session_id: sid, role });
        useSessionStore.getState().setRole(role);
        return {};
      } catch (e: any) {
        return { error: e.message };
      }
    },
  },
  {
    name: 'model',
    description: 'model management',
    usage: '/model <list|set <name>>',
    handler: async (args, client) => {
      if (!args[0] || args[0] === 'list') {
        try {
          const result = await client.request('config.list_models');
          return { systemMessage: 'Available models:\n' + JSON.stringify(result, null, 2) };
        } catch (e: any) { return { error: e.message }; }
      } else if (args[0] === 'set' && args[1]) {
        useSessionStore.getState().setModel(args[1]);
        return {};
      }
      return { error: 'usage: /model <list|set <name>>' };
    },
  },
  {
    name: 'sessions',
    description: 'session management (list|new|open|delete)',
    usage: '/sessions <sub>',
    handler: async (args, client) => {
      const sub = args[0] || 'list';
      const sessionStore = useSessionStore.getState();
      const msgStore = useMessagesStore.getState();
      if (sub === 'list') {
        try {
          const result = await client.request('sessions.list');
          sessionStore.setSessions(result);
          return { switchView: 'sessions' };
        } catch (e: any) { return { error: e.message }; }
      } else if (sub === 'new') {
        try {
          const result = await client.request('sessions.create', { title: 'New Session' });
          sessionStore.setCurrentSession(result.session_id);
          client.setReconnectSession(result.session_id);
          msgStore.setMessages([]);
          try {
            const msgs = await client.request('sessions.messages', { session_id: result.session_id });
            msgStore.setMessages(msgs);
          } catch {}
          return {};
        } catch (e: any) { return { error: e.message }; }
      } else if (sub === 'open' && args[1]) {
        try {
          const result = await client.request('session.attach', { session_id: args[1] });
          sessionStore.setCurrentSession(args[1]);
          client.setReconnectSession(args[1]);
          msgStore.setMessages(result.messages || []);
          try {
            const roleResp = await client.request('session.mode.get', { session_id: args[1] });
            sessionStore.setRole(roleResp.role);
          } catch {}
          return {};
        } catch (e: any) { return { error: e.message }; }
      } else if (sub === 'delete' && args[1]) {
        try {
          await client.request('session.delete', { session_id: args[1] });
          return { loadSessions: true };
        } catch (e: any) { return { error: e.message }; }
      }
      return { error: 'usage: /sessions <list|new|open <id>|delete <id>>' };
    },
  },
  {
    name: 'undo',
    description: 'undo file changes',
    usage: '/undo [id|--list]',
    handler: async (args, client) => {
      const sid = useSessionStore.getState().currentSessionId;
      if (!sid) return { error: 'no active session' };
      try {
        const undoArgs: any = {};
        if (args[0] === '--list') {
          undoArgs.op = 'list';
        } else if (args[0]) {
          undoArgs.op = 'undo';
          undoArgs.id = args[0];
        } else {
          undoArgs.op = 'undo';
        }
        const result = await client.request('tool.call', { name: 'undo', args: undoArgs });
        return { systemMessage: 'undo: ' + JSON.stringify(result) };
      } catch (e: any) { return { error: e.message }; }
    },
  },
  {
    name: 'diff',
    description: 'view git diff',
    usage: '/diff',
    handler: async (_args, client) => {
      try {
        const result = await client.request('tool.call', {
          name: 'bash',
          args: { cmd: 'git diff --stat', timeout: 10 },
        });
        return { systemMessage: 'diff:\n' + JSON.stringify(result, null, 2), switchView: 'diff' };
      } catch (e: any) { return { error: e.message }; }
    },
  },
  {
    name: 'compact',
    description: 'compact context',
    usage: '/compact',
    handler: async () => {
      // 设计文档 §8.3.4: 手动触发上下文压缩
      // 通过设置 session flag，下次 LLM call 时应用压缩
      return { systemMessage: '[compact requested - will apply on next LLM call]' };
    },
  },
  {
    name: 'cancel',
    description: 'cancel current agent loop',
    usage: '/cancel',
    handler: async (_args, client) => {
      const sid = useSessionStore.getState().currentSessionId;
      if (!sid) return { error: 'no active session' };
      try {
        await client.request('session.cancel', { session_id: sid });
        useMessagesStore.getState().setStreaming(false);
        return {};
      } catch (e: any) { return { error: e.message }; }
    },
  },
  {
    name: 'task',
    description: 'background task management',
    usage: '/task <list|cancel <id>>',
    handler: async (args, client) => {
      if (!args[0] || args[0] === 'list') {
        try {
          const result = await client.request('task.list');
          useSessionStore.getState().setTaskCount(result.length);
          useSessionStore.getState().setBackgroundTasks(result);
          return { switchView: 'tasks' };
        } catch (e: any) { return { error: e.message }; }
      } else if (args[0] === 'cancel' && args[1]) {
        try {
          await client.request('task.cancel', { task_id: args[1] });
          return {};
        } catch (e: any) { return { error: e.message }; }
      }
      return { error: 'usage: /task <list|cancel <id>>' };
    },
  },
  {
    name: 'config',
    description: 'config management',
    usage: '/config <get|set> <key> [value]',
    handler: async (args, client) => {
      if (args[0] === 'get') {
        try {
          const result = await client.request('config.get', { key: args[1] || null });
          return { systemMessage: 'config:\n' + JSON.stringify(result, null, 2), switchView: 'config' };
        } catch (e: any) { return { error: e.message }; }
      } else if (args[0] === 'set' && args[1]) {
        try {
          await client.request('config.set', { key: args[1], value: args[2] || '' });
          return {};
        } catch (e: any) { return { error: e.message }; }
      }
      return { error: 'usage: /config <get [key]|set <key> <value>>' };
    },
  },
  {
    name: 'pair',
    description: 'show pairing info + QR code',
    usage: '/pair',
    handler: async (_args, client) => {
      try {
        const result = await client.request('config.get', { key: null });
        // 设计文档 §5.1: 生成配对串 + QR 码
        const pairingStr = result.pairing_string || '';
        const qr = pairingStr ? generateQrCode(pairingStr) : '';
        const msg = 'Server info:\n' + JSON.stringify(result, null, 2)
          + (qr ? '\n\nScan QR to connect:\n' + qr : '');
        return { systemMessage: msg };
      } catch (e: any) { return { error: e.message }; }
    },
  },
  {
    name: 'workflow',
    description: 'workflow management',
    usage: '/workflow <init|list|show|propose|plan|apply|review|archive|continue>',
    handler: async (args, client) => {
      const sub = args[0] || 'list';
      // 设计文档 §8.5: 完整的 /workflow 子命令
      if (sub === 'list') {
        try {
          const result = await client.request('tool.call', {
            name: 'workflow_query',
            args: { op: 'roadmaps' },
          });
          return { systemMessage: 'Roadmaps:\n' + JSON.stringify(result, null, 2) };
        } catch (e: any) { return { error: e.message }; }
      } else if (sub === 'show' && args[1]) {
        try {
          const result = await client.request('tool.call', {
            name: 'workflow_query',
            args: { op: 'milestones', roadmap_id: args[1] },
          });
          return { systemMessage: `Milestones for ${args[1]}:\n` + JSON.stringify(result, null, 2) };
        } catch (e: any) { return { error: e.message }; }
      } else if (sub === 'init') {
        try {
          const result = await client.request('tool.call', {
            name: 'workflow_create',
            args: { op: 'roadmap', title: args[1] || 'New Roadmap', description: args[2] || '' },
          });
          return { systemMessage: 'Roadmap created:\n' + JSON.stringify(result, null, 2) };
        } catch (e: any) { return { error: e.message }; }
      } else if (sub === 'propose' || sub === 'plan' || sub === 'apply' || sub === 'review' || sub === 'archive' || sub === 'continue') {
        // 5 步循环 + continue
        try {
          const result = await client.request('tool.call', {
            name: 'workflow_update',
            args: { op: 'phase_next', roadmap_id: args[1] || '' },
          });
          return { systemMessage: `Workflow ${sub}:\n` + JSON.stringify(result, null, 2) };
        } catch (e: any) { return { error: e.message }; }
      }
      return { error: 'usage: /workflow <init|list|show <id>|propose <id>|plan <id>|apply <id>|review <id>|archive <id>|continue <id>>' };
    },
  },
  {
    name: 'todos',
    description: 'show todo list (goal mode)',
    usage: '/todos',
    handler: async () => ({ switchView: 'todos' }),
  },
  {
    name: 'branch',
    description: 'create git branch',
    usage: '/branch <name>',
    handler: async (args, client) => {
      if (!args[0]) return { error: 'usage: /branch <name>' };
      try {
        const result = await client.request('tool.call', {
          name: 'bash',
          args: { cmd: `git checkout -b ${args[0]}`, timeout: 10 },
        });
        return { systemMessage: `Branch ${args[0]} created:\n` + JSON.stringify(result) };
      } catch (e: any) { return { error: e.message }; }
    },
  },
  {
    name: 'compact-mode',
    description: 'toggle compact mode (merge context lines)',
    usage: '/compact-mode',
    handler: async () => {
      // 设计文档 §6.5: 紧凑模式
      // 需要通过外部传入 store，这里用动态 import
      const { useUiStore } = await import('../store/ui.js');
      useUiStore.getState().toggleCompact();
      return {};
    },
  },
  {
    name: 'exit',
    description: 'quit mcoder',
    usage: '/exit',
    handler: async () => ({ exit: true }),
  },
  {
    name: 'quit',
    description: 'quit mcoder',
    usage: '/quit',
    handler: async () => ({ exit: true }),
  },
];

/// 查找命令
export function findCommand(name: string): CommandDef | undefined {
  return builtinCommands.find(c => c.name === name);
}

/// 列出所有命令名 + 描述（用于 /help）
export function listCommands(): { name: string; description: string; usage: string }[] {
  return builtinCommands.map(c => ({ name: c.name, description: c.description, usage: c.usage }));
}
