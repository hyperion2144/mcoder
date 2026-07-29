// 设计文档 §6.12: utils/format.ts - 格式化工具
// 平台无关

/// 缩写路径：/Users/mutou/projects/myapp → ~/projects/myapp
// 平台无关：Node 环境用 process.env，浏览器/Tauri 环境无 home 概念则原样返回
export function shortenPath(path: string): string {
  const g: any = (typeof globalThis !== 'undefined' ? globalThis : undefined) as any;
  const env = g?.process?.env || {};
  const home = env.HOME || env.USERPROFILE || '';
  if (home && path.startsWith(home)) {
    return '~' + path.slice(home.length);
  }
  return path;
}

/// 格式化 token 使用量：1234 / 128000 → "1.2k/128k"
export function formatContext(used: number, window: number): string {
  const usedStr = used > 1000 ? `${(used / 1000).toFixed(1)}k` : `${used}`;
  const windowStr = window > 1000 ? `${(window / 1000).toFixed(0)}k` : `${window}`;
  return `${usedStr}/${windowStr}`;
}

/// 格式化成本：0.0342 → "$0.034"
export function formatCost(cost: number): string {
  if (cost <= 0) return '';
  if (cost < 0.01) return `$${cost.toFixed(4)}`;
  return `$${cost.toFixed(3)}`;
}

/// 截断字符串
export function truncate(s: string, maxLen: number): string {
  return s.length > maxLen ? s.slice(0, maxLen) + '...' : s;
}

/// 格式化 JSON 输出（用于 tool result 预览）
export function formatToolOutput(output: any, maxLen: number = 200): string {
  const str = typeof output === 'string'
    ? output
    : JSON.stringify(output, null, 2);
  return truncate(str, maxLen);
}
