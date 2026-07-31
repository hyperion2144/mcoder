// 设计文档 §8.8: permission 模块统一导出
export {
  usePermissionStore,
  serializeDecision,
  type PermissionLevel,
  type PermissionRequest,
  type PermissionDecision,
} from './store.js';
export {
  PermissionCard,
  PermissionSummary,
  PermissionLevelBadge,
} from './PermissionCard.js';
// React 共享版本（Desktop/Mobile 用）
export {
  PermissionCard as PermissionCardReact,
  PermissionCardSummary as PermissionCardSummaryReact,
} from './PermissionCardReact.js';