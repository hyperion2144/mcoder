// DESIGN.md §2.1: TUI 颜色 Token
// ink 支持的色名映射到 catppuccin mocha
// 三端统一：TUI 用 TUI_COLORS 导出色名；Desktop/Mobile 用 var(--*)

export const TUI_COLORS = {
  // 文字
  textPrimary: 'white',      // #cdd6f4
  textSecondary: 'gray',     // #a6adc8
  textMuted: 'gray',         // #6c7086

  // 角色色
  accent: 'cyan',            // #89b4fa
  success: 'green',          // #a6e3a1
  warning: 'yellow',         // #f9e2af
  error: 'red',              // #f38ba8
  mauve: 'magenta',          // #cba6f7
} as const;

/// DESIGN.md §3: 卡片角色色（按用途分 5 类）
export const ROLE_COLOR = {
  interaction: TUI_COLORS.warning,   // ask_user / permission / plan
  execution: TUI_COLORS.accent,      // 写/执行类工具
  thinking: TUI_COLORS.mauve,        // LLM 推理
  done: TUI_COLORS.textMuted,        // 已完成
  error: TUI_COLORS.error,           // 失败
} as const;

/// DESIGN.md §6.1: 状态前缀符号
export const PREFIX = {
  pending: '▸',      // 待操作 / 折叠
  running: '▶',      // 执行中
  done: '✓',         // 已完成
  failed: '✗',       // 失败
  approval: '?',     // 待审批
  textMuted: '·',
  setting: '⚙',
  loading: '·',
  error: '✗',
  thinking: '⚙',  // 思考深度图标（统一用 setting 前缀）
  expanded: '▾',     // 折叠展开（折角朝下）
  selected: '▸',     // 列表/选项被选中（语义化别名，与 pending 同字符）
  sep: '·',          // 标签分隔符（与 textMuted/loading 区分用途）
} as const;

/// DESIGN.md §5: 间距节奏（8 倍数）
export const SPACING = {
  xs: 1,   // 4px（最小间距）
  sm: 2,   // 8px
  md: 3,   // 12px
  lg: 4,   // 16px
  xl: 6,   // 24px
} as const;

/// DESIGN.md §4: 边框风格
export const BORDER = {
  card: 'round',          // 卡片（交互/执行/思考）
  panel: 'single',        // 面板（session/todo/setting/tree）
  summary: 'single',      // 摘要（已完成/已决议）
} as const;