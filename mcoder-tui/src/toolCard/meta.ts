// 统一工具卡片元数据提取（三端共享）
// 从 tool_use block 的 name + args 提取人类可读的标题与输入摘要

import type { ContentBlock } from '../rpc/types.js';

/** 工具类别（决定左边框颜色） */
export type ToolCategory =
  | 'thinking'
  | 'file'
  | 'command'
  | 'code'
  | 'graph'
  | 'subagent'
  | 'plan'
  | 'workflow'
  | 'other';

/** 工具执行状态 */
export type ToolStatus = 'loading' | 'done' | 'failed' | 'cancelled';

/** 折叠状态 */
export type FoldState = 'collapsed' | 'semi' | 'expanded';

/** 工具元数据 */
export interface ToolMeta {
  /** 工具名（原样） */
  name: string;
  /** 类别 */
  category: ToolCategory;
  /** 标题（如 "read · src/App.tsx"） */
  title: string;
  /** 输入摘要（半折叠时显示，1-2 行） */
  inputSummary: string;
  /** 默认折叠状态 */
  defaultFold: FoldState;
}

/** 按工具名 + args 提取元数据 */
export function extractToolMeta(block: ContentBlock): ToolMeta {
  const name = block.name || 'unknown';
  const args = block.args || {};
  const category = categorize(name);

  switch (name) {
    case 'thinking':
      return { name, category, title: 'thinking', inputSummary: '', defaultFold: 'expanded' };

    case 'read':
    case 'read_file': {
      const path = args.path || args.file || '';
      return {
        name, category: 'file',
        title: `read · ${shortPath(path)}`,
        inputSummary: `path: ${path}`,
        defaultFold: 'collapsed',
      };
    }
    case 'write':
    case 'write_file': {
      const path = args.path || args.file || '';
      return {
        name, category: 'file',
        title: `write · ${shortPath(path)}`,
        inputSummary: `path: ${path}`,
        defaultFold: 'collapsed',
      };
    }
    case 'edit': {
      const path = args.path || args.file || '';
      return {
        name, category: 'file',
        title: `edit · ${shortPath(path)}`,
        inputSummary: `path: ${path}`,
        defaultFold: 'expanded',
      };
    }
    case 'bash': {
      const cmd = args.command || args.cmd || '';
      return {
        name, category: 'command',
        title: `bash · ${truncate(cmd, 50)}`,
        inputSummary: `$ ${cmd}`,
        defaultFold: 'semi',
      };
    }
    case 'code_exec': {
      const lang = args.language || args.lang || 'text';
      const code = args.code || '';
      return {
        name, category: 'code',
        title: `code_exec · ${lang}`,
        inputSummary: `${lang}: ${truncate(code.split('\n')[0] || '', 60)}`,
        defaultFold: 'semi',
      };
    }
    case 'ast_rename':
      return {
        name, category: 'graph',
        title: `ast_rename · ${args.old_name || ''} → ${args.new_name || ''}`,
        inputSummary: `old: ${args.old_name || ''}\nnew: ${args.new_name || ''}`,
        defaultFold: 'semi',
      };
    case 'ast_inline':
      return {
        name, category: 'graph',
        title: `ast_inline · ${args.function || args.fn || ''}`,
        inputSummary: `fn: ${args.function || args.fn || ''}`,
        defaultFold: 'semi',
      };
    case 'ast_extract': {
      const file = args.file || args.path || '';
      return {
        name, category: 'graph',
        title: `ast_extract · ${shortPath(file)}`,
        inputSummary: `file: ${file}`,
        defaultFold: 'semi',
      };
    }
    case 'graph_query':
      return {
        name, category: 'graph',
        title: `graph_query · ${args.op || args.name || ''}`,
        inputSummary: `op: ${args.op || args.name || ''}`,
        defaultFold: 'semi',
      };
    case 'subagent':
      return {
        name, category: 'subagent',
        title: `subagent · ${args.role || ''}`,
        inputSummary: `role: ${args.role || ''}`,
        defaultFold: 'semi',
      };
    case 'task':
      return {
        name, category: 'other',
        title: `task · ${args.op || ''}`,
        inputSummary: `op: ${args.op || ''}`,
        defaultFold: 'semi',
      };
    case 'plan_create':
      return {
        name, category: 'plan',
        title: `plan_create · ${countSteps(args)} steps`,
        inputSummary: `steps: ${countSteps(args)}`,
        defaultFold: 'semi',
      };
    case 'plan_update':
      return {
        name, category: 'plan',
        title: `plan_update · step ${args.step_id || ''}`,
        inputSummary: `step: ${args.step_id || ''} → ${args.status || ''}`,
        defaultFold: 'semi',
      };
    case 'plan_query':
      return {
        name, category: 'plan',
        title: 'plan_query',
        inputSummary: 'read current plan',
        defaultFold: 'semi',
      };
    case 'todo':
      return {
        name, category: 'plan',
        title: `todo · ${args.action || ''}`,
        inputSummary: `action: ${args.action || ''}`,
        defaultFold: 'semi',
      };
    case 'journal':
      return {
        name, category: 'other',
        title: `journal · ${args.op || ''}`,
        inputSummary: `op: ${args.op || ''}`,
        defaultFold: 'semi',
      };
    case 'workflow_create':
    case 'workflow_query':
    case 'workflow_update':
      return {
        name, category: 'workflow',
        title: `${name} · ${args.action || args.op || ''}`,
        inputSummary: `action: ${args.action || args.op || ''}`,
        defaultFold: 'semi',
      };
    default:
      return {
        name, category,
        title: name,
        inputSummary: truncate(JSON.stringify(args), 80),
        defaultFold: 'semi',
      };
  }
}

function categorize(name: string): ToolCategory {
  if (name === 'thinking') return 'thinking';
  if (['read', 'read_file', 'write', 'write_file', 'edit'].includes(name)) return 'file';
  if (['bash'].includes(name)) return 'command';
  if (['code_exec'].includes(name)) return 'code';
  if (['ast_rename', 'ast_inline', 'ast_extract', 'graph_query'].includes(name)) return 'graph';
  if (['subagent'].includes(name)) return 'subagent';
  if (['plan_create', 'plan_update', 'plan_query', 'todo'].includes(name)) return 'plan';
  if (name.startsWith('workflow')) return 'workflow';
  return 'other';
}

function shortPath(p: string): string {
  if (!p) return '';
  const parts = p.split('/');
  if (parts.length <= 2) return p;
  return parts.slice(-2).join('/');
}

function truncate(s: string, max: number): string {
  if (!s) return '';
  return s.length > max ? s.slice(0, max) + '...' : s;
}

function countSteps(args: any): number {
  if (Array.isArray(args.steps)) return args.steps.length;
  return 0;
}

/** 格式化工具结果为人类可读字符串 */
export function formatToolResult(output: any): string {
  if (output == null) return '';
  if (typeof output === 'string') return output;
  if (typeof output === 'object') {
    // 常见字段优先
    if (output.error) return `Error: ${output.error}`;
    // output.result 为字符串时直接返回，避免冗余 JSON 包装
    if (typeof output.result === 'string') return output.result;
    if (output.result && typeof output.result === 'object') {
      return JSON.stringify(output.result, null, 2);
    }
    return JSON.stringify(output, null, 2);
  }
  return String(output);
}

/** 半折叠下每行最大字符数（超出横向截断） */
const SUMMARY_MAX_WIDTH = 80;

/** 截取结果摘要（半折叠用），每行横向截断到 SUMMARY_MAX_WIDTH */
export function summarizeResult(output: any, maxLines: number = 3): { text: string; truncated: boolean; totalLines: number } {
  const full = formatToolResult(output);
  const rawLines = full.split('\n');
  // 横向截断：每行超过 SUMMARY_MAX_WIDTH 时截断并加省略号
  const truncLine = (line: string): string =>
    line.length > SUMMARY_MAX_WIDTH ? line.slice(0, SUMMARY_MAX_WIDTH - 1) + '…' : line;
  const lines = rawLines.map(truncLine);
  const totalLines = lines.length;
  if (totalLines <= maxLines) {
    return { text: lines.join('\n'), truncated: false, totalLines };
  }
  return {
    text: lines.slice(0, maxLines).join('\n'),
    truncated: true,
    totalLines,
  };
}
