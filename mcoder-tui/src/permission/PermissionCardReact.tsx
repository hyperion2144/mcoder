// DESIGN.md §3 / §6: 权限审批卡片（React 共享版本，供 Desktop/Mobile 用）
// 标题：▸ permission · STD · 等待确认
// 样式与 mcoder-desktop/styles.css 中的 .ask-card 完全对称（warning border + warning-dim bg）

import React, { useState } from 'react';
import {
  usePermissionStore,
  serializeDecision,
  type PermissionRequest,
  type PermissionDecision,
  type PermissionLevel,
} from './store.js';
import { CSS_COLORS, LEVEL_BADGE, DECISION_BADGE } from './tokens.js';
import { PREFIX } from '../theme.js';
import { t } from '../i18n.js';

interface Props {
  request_id: string;
  tool_call_id: string;
  session_id: string;
  client: {
    request: (method: string, params?: any) => Promise<any>;
  };
  onError?: (msg: string) => void;
}

/** 格式化 tool_args */
function formatToolArgs(args: unknown): string {
  if (!args || typeof args !== 'object') return String(args);
  const obj = args as Record<string, unknown>;
  const parts: string[] = [];
  if (typeof obj.command === 'string') parts.push(`cmd: ${obj.command}`);
  if (typeof obj.file === 'string') parts.push(`file: ${obj.file}`);
  if (typeof obj.path === 'string') parts.push(`path: ${obj.path}`);
  if (typeof obj.pattern === 'string') parts.push(`pattern: ${obj.pattern}`);
  if (typeof obj.query === 'string') parts.push(`query: ${obj.query}`);
  if (typeof obj.url === 'string') parts.push(`url: ${obj.url}`);
  if (typeof obj.action === 'string') parts.push(`action: ${obj.action}`);
  if (parts.length === 0) {
    const full = JSON.stringify(obj);
    return full.length > 200 ? full.slice(0, 200) + '...' : full;
  }
  return parts.join(` ${PREFIX.sep} `);
}

export function PermissionCard({
  request_id, tool_call_id, session_id, client, onError,
}: Props) {
  const pending = usePermissionStore((s) => s.pending[session_id]);
  const setResolved = usePermissionStore((s) => s.setResolved);
  const [submitting, setSubmitting] = useState(false);

  const req = pending && pending.request_id === request_id ? pending : null;
  if (!req) return null;

  const badge = LEVEL_BADGE[req.level];

  const submit = async (decision: PermissionDecision) => {
    if (submitting) return;
    setSubmitting(true);
    try {
      await client.request('permission.submit', {
        session_id,
        response: {
          request_id,
          session_id,
          decision: serializeDecision(decision),
        },
      });
      setResolved(session_id, request_id, decision);
    } catch (e: any) {
      onError?.(`permission.submit failed: ${e.message ?? e}`);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="permission-card" data-tool-call-id={tool_call_id}>
      <div className="permission-card-header">
        <span className="permission-card-title">{PREFIX.selected} permission</span>
        <span className="permission-level-badge" style={{ backgroundColor: badge.css }}>
          {badge.text}
        </span>
        <span className="permission-card-status">{t('ui.waiting_confirm')}</span>
      </div>
      <div className="permission-card-body">
        <div className="permission-row">
          <span className="permission-label">tool</span>
          <code className="permission-tool-name">{req.tool_name}</code>
        </div>
        <div className="permission-row">
          <span className="permission-label">args</span>
          <span className="permission-args">{formatToolArgs(req.tool_args)}</span>
        </div>
        <div className="permission-row">
          <span className="permission-label">reason</span>
          <span className="permission-reason">{req.reason}</span>
        </div>
      </div>
      <div className="permission-card-actions">
        <button
          className="permission-btn permission-btn-allow"
          disabled={submitting}
          onClick={() => submit({ type: 'allow' })}
        >
          Allow
        </button>
        <button
          className="permission-btn permission-btn-deny"
          disabled={submitting}
          onClick={() => submit({ type: 'deny', reason: 'denied by user' })}
        >
          Deny
        </button>
        <button
          className="permission-btn permission-btn-always"
          disabled={submitting}
          onClick={() => submit({ type: 'always_allow' })}
          title="永久通过（仅 standard/strict 模式生效）"
        >
          Always Allow
        </button>
      </div>
    </div>
  );
}

/** 已决议摘要 */
export function PermissionCardSummary({
  tool_call_id, decision,
}: {
  request_id: string;
  tool_call_id: string;
  session_id: string;
  decision: PermissionDecision;
}) {
  const kind = decision.type;
  const b = DECISION_BADGE[kind];
  return (
    <div className="permission-card permission-summary" data-tool-call-id={tool_call_id}>
      <div className="permission-card-header" style={{ color: b.css }}>
        <span className="permission-card-title">permission · {t(b.text)}</span>
      </div>
    </div>
  );
}