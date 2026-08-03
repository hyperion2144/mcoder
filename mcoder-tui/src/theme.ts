// mcoder UI Redesign v2 - Tokyo Night palette
// Inspired by Tokyo Night, refined for agent orchestration tool.
// TUI color names mapped to ink-supported colors; Desktop/Mobile use CSS vars.

export const TUI_COLORS = {
  // Brand primary (blue family)
  brand: 'blue',           // #7aa2f7
  accent: 'blue',           // #7aa2f7 (alias)
  // Text
  textPrimary: 'white',     // #c0caf5
  textSecondary: 'gray',    // #a9b1d6
  textMuted: 'gray',        // #565f89
  // State colors
  success: 'green',         // #9ece6a
  warning: 'yellow',        // #e0af68
  error: 'red',             // #f7768e
  mauve: 'magenta',         // #bb9af7
  orange: 'yellow',         // #ff9e64 (closest ink color)
  cyan: 'cyan',             // #7dcfff
} as const;

/// Card role colors (5 categories per design system)
export const ROLE_COLOR = {
  interaction: TUI_COLORS.warning,   // ask_user / permission / plan
  execution: TUI_COLORS.accent,      // write / bash / read / search
  thinking: TUI_COLORS.mauve,        // LLM reasoning
  done: TUI_COLORS.textMuted,        // completed
  error: TUI_COLORS.error,           // failed
} as const;

/// Status prefix glyphs
export const PREFIX = {
  pending: '▸',      // waiting / collapsed
  running: '▶',      // executing
  done: '✓',         // completed
  failed: '✗',       // failed
  approval: '?',     // pending approval
  sep: '·',          // separator
  expanded: '▾',     // expanded fold
  selected: '▸',     // selected item
  branch: '│',       // tree branch line
  dot: '●',          // status dot
  open: '○',         // open circle
  // Legacy aliases (used by older components)
  setting: '⚙',      // settings gear
  error: '✗',        // error (alias of failed)
  loading: '·',      // loading dot
  textMuted: '·',    // muted separator (alias of sep)
  thinking: '⚙',     // thinking gear
} as const;

/// Spacing (8pt grid)
export const SPACING = {
  xs: 1,   // 4px
  sm: 2,   // 8px
  md: 3,   // 12px
  lg: 4,   // 16px
  xl: 6,   // 24px
} as const;

/// Border styles
export const BORDER = {
  card: 'round',
  panel: 'single',
  summary: 'single',
} as const;
