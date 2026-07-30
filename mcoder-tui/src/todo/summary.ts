// 共享 Todo 摘要条选择函数（三端 TUI/Desktop/Mobile 共用）
//
// 行为：
//   - 输入未完成 todo 列表（pending + in_progress）；若为空返回 null（前端应隐藏摘要条）
//   - 默认折叠（collapseLimit=1）：仅展示 1 条 + "..." 标记；展开后展示最多 maxVisible 条
//   - TUI/Desktop: maxVisible = 3，Mobile: maxVisible = 1（折叠）/ 3（展开）
//   - 排序: 服务端已排好（in_progress → pending，再 priority/order），前端不再排序
//
// 单一职责：纯函数，无副作用，便于三端共用 + TS 测试覆盖。

export type TodoStatus = 'pending' | 'in_progress' | 'completed' | 'cancelled';
export type TodoPriority = 'high' | 'medium' | 'low';

export interface TodoItem {
  id: string;
  session_id: string;
  content: string;
  status: string;
  priority: string;
  order: number;
  created_at: string;
  updated_at: string;
}

export interface TodoSummary {
  total: number;
  pending: number;
  in_progress: number;
  completed: number;
  cancelled: number;
}

export interface TodoSummaryItem {
  id: string;
  content: string;
  status: TodoStatus;
  priority: TodoPriority;
}

export interface TodoSummaryView {
  /// 显示的 todo 列表（最多 maxVisible 条）
  visible: TodoSummaryItem[];
  /// 未在 visible 中展示的剩余未完成 todo 数
  remaining: number;
  /// 全部未完成数 = visible.length + remaining
  totalUnfinished: number;
  /// 是否完全隐藏（全部完成 / 取消时返回 null）
  hidden: false;
}

/** 返回 null 表示无未完成 todo → 前端隐藏整个摘要条 */
export type TodoSummaryResult = TodoSummaryView | null;

export const MAX_VISIBLE_DESKTOP = 3;
export const MAX_VISIBLE_MOBILE_COLLAPSED = 1;
export const MAX_VISIBLE_MOBILE_EXPANDED = 3;

/**
 * 平台配置：默认折叠时显示几条，展开后显示几条。
 * - TUI/Desktop: 都显示 3 条（无折叠）
 * - Mobile: 默认 1 条（可点击展开为 3 条）
 */
export interface TodoSummaryPlatform {
  maxVisibleCollapsed: number;
  maxVisibleExpanded: number;
}

export const PLATFORM_DESKTOP: TodoSummaryPlatform = {
  maxVisibleCollapsed: MAX_VISIBLE_DESKTOP,
  maxVisibleExpanded: MAX_VISIBLE_DESKTOP,
};
export const PLATFORM_MOBILE: TodoSummaryPlatform = {
  maxVisibleCollapsed: MAX_VISIBLE_MOBILE_COLLAPSED,
  maxVisibleExpanded: MAX_VISIBLE_MOBILE_EXPANDED,
};
export const PLATFORM_TUI: TodoSummaryPlatform = PLATFORM_DESKTOP;

/**
 * 过滤未完成（pending / in_progress）；服务端 list_unfinished 已经过滤，但保留这步
 * 以保证在客户端独立使用时的正确性。
 */
export function filterUnfinished(items: TodoItem[]): TodoSummaryItem[] {
  return items
    .filter((it) => it.status === 'pending' || it.status === 'in_progress')
    .map<TodoSummaryItem>((it) => ({
      id: it.id,
      content: it.content,
      status: it.status as TodoStatus,
      priority: it.priority as TodoPriority,
    }));
}

/**
 * 构造摘要条视图。
 *
 * @param items 服务端推送的完整 todos（已按稳定顺序排列）
 * @param platform 平台配置（默认 TUI/Desktop：3 条；Mobile：折叠 1 / 展开 3）
 * @param expanded 是否展开（仅 Mobile 生效）
 */
export function selectTodoSummary(
  items: TodoItem[],
  platform: TodoSummaryPlatform = PLATFORM_DESKTOP,
  expanded: boolean = false,
): TodoSummaryResult {
  const unfinished = filterUnfinished(items);
  if (unfinished.length === 0) return null;

  const limit = expanded ? platform.maxVisibleExpanded : platform.maxVisibleCollapsed;
  const visible = unfinished.slice(0, limit);
  const remaining = unfinished.length - visible.length;
  return {
    visible,
    remaining,
    totalUnfinished: unfinished.length,
    hidden: false,
  };
}

/**
 * 摘要文案（用于 Mobile 折叠态：1 条 + "+N more"）
 */
export function formatRemaining(view: TodoSummaryView): string {
  return view.remaining > 0 ? `+${view.remaining} more` : '';
}