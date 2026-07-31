// 设计文档 §8.8: 三端 UI 设计 Token（权限卡片）
// 与 mcoder-desktop/styles.css 中的 catppuccin 主题保持一致：
//   --warning: #f9e2af   (ask_user 边框/标题)
//   --warning-dim: rgba(249, 226, 175, 0.12)
//   --border-subtle: #45475a
//   --text-primary: #cdd6f4
//   --text-secondary: #a6adc8
//   --text-muted: #6c7086
//   --accent: #89b4fa
//   --success: #a6e3a1
//   --error: #f38ba8
//   --peach: #fab387

import type { PermissionLevel } from './store.js';

/// TUI ink 端颜色名（ink 调色板；与 catppuccin 对应）
export const TUI_COLORS = {
  warning: 'yellow',        // ask_user 标题 / pending border
  warningDim: 'gray',       // 已决议状态
  success: 'green',
  error: 'red',
  accent: 'cyan',           // 工具名高亮
  textPrimary: 'white',
  textSecondary: 'gray',
  textMuted: 'gray',
  border: 'yellow',
  borderSubtle: 'gray',
} as const;

/// Desktop/Mobile CSS 端 hex 颜色（与 styles.css 中 --warning 等对齐）
export const CSS_COLORS = {
  warning: '#f9e2af',
  warningDim: 'rgba(249, 226, 175, 0.12)',
  success: '#a6e3a1',
  successDim: 'rgba(166, 227, 161, 0.12)',
  error: '#f38ba8',
  errorDim: 'rgba(243, 139, 168, 0.12)',
  accent: '#89b4fa',
  textPrimary: '#cdd6f4',
  textSecondary: '#a6adc8',
  textMuted: '#6c7086',
  borderSubtle: '#45475a',
  bgSurface: '#181825',
  bgElevated: '#313244',
} as const;

/// 三种权限级别的视觉标识（统一为：yolo=红/strict=青/standard=黄）
/// 注意：yolo 是高风险（自动执行所有），所以用 warning 颜色（红/橙）
///       strict 是最保守，用 accent（青）表示安全
///       standard 是默认平衡，用 warning（黄）表示需要留意
export const LEVEL_BADGE: Record<PermissionLevel, { text: string; tui: string; css: string }> = {
  yolo:     { text: 'YOLO',     tui: 'red',    css: CSS_COLORS.error },
  standard: { text: 'STD',      tui: 'yellow', css: CSS_COLORS.warning },
  strict:   { text: 'STRICT',   tui: 'cyan',   css: CSS_COLORS.accent },
};

/// 决议状态的视觉标识
export const DECISION_BADGE: Record<'allow' | 'deny' | 'always_allow', { text: string; tui: string; css: string }> = {
  allow:       { text: '已通过',   tui: 'green',  css: CSS_COLORS.success },
  deny:        { text: '已拒绝',   tui: 'red',    css: CSS_COLORS.error },
  always_allow:{ text: '永久通过', tui: 'green',  css: CSS_COLORS.success },
};