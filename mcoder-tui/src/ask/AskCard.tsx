// AskCard - 跨 TUI/Desktop/Mobile 的 Ask 交互卡片（React 版本）
// TUI（TUI 用 ink）有自己的 AskUserCard.tsx
// Desktop/Mobile（React）共用此文件
// 设计：在消息流中作为普通 tool_use 卡片渲染（非模态/非 Sheet）
// 回答后原位置显示 AskCardSummary

import React, { useState, useEffect, useRef } from 'react';
import { useAskStore } from './store.js';
import { formatAskFullSummary } from './summary.js';
import { serializeSubmission } from './validation.js';
import type { AskQuestionAnswer, AskRequest, AskSubmission } from './types.js';

interface AskCardProps {
  ask_id: string;
  tool_call_id: string;
  session_id: string;
  client: {
    request: (method: string, params?: any) => Promise<any>;
  };
  onError?: (msg: string) => void;
}

/** pending 状态：交互式卡片
 *  防重复提交按钮（issue 9）：用 submitting ref 阻止 Enter/Submit 期间重复点击 */
export function AskCard({ ask_id, tool_call_id, session_id, client, onError }: AskCardProps) {
  const pending = useAskStore((s) => s.pending[session_id]);
  const setPending = useAskStore((s) => s.setPending);
  const toggleSelection = useAskStore((s) => s.toggleSelection);
  const setNote = useAskStore((s) => s.setNote);
  const setFocus = useAskStore((s) => s.setFocus);
  const draftSelections = useAskStore((s) => s.draftSelections[session_id] || {});
  const draftNotes = useAskStore((s) => s.draftNotes[session_id] || {});
  const draftFocus = useAskStore((s) => s.draftFocus[session_id] ?? 0);
  const submittingRef = useRef(false);

  // 验证：必须是当前 session 的 ask_id
  const request: AskRequest | undefined = pending && pending.ask_id === ask_id ? pending.request : undefined;

  // fallback：服务端 pending 已清空但我们仍有 lastSubmission（已经答完）。这种情况显示摘要。
  const last = useAskStore((s) => s.lastSubmission[session_id]);
  if (last && last.ask_id === ask_id) {
    return <AskCardSummary request={request || { questions: [] }} submission={last.submission} />;
  }
  if (!request) {
    return null;
  }

  const handleSelect = (qIndex: number, optionLabel: string) => {
    toggleSelection(session_id, qIndex, optionLabel);
  };

  const handleNoteChange = (qIndex: number, note: string) => {
    setNote(session_id, qIndex, note);
  };

  const handleFocus = (qIndex: number) => {
    setFocus(session_id, qIndex);
  };

  const handleSubmit = async () => {
    if (submittingRef.current) return;
    submittingRef.current = true;
    const answers: Record<number, AskQuestionAnswer> = {};
    let allFilled = true;
    for (let i = 0; i < request.questions.length; i++) {
      const q = request.questions[i];
      const isMulti = !!q.multi_select;
      const sel = draftSelections[i] || [];
      const note = draftNotes[i];
      if (isMulti) {
        if (sel.length === 0 && !note) {
          allFilled = false;
          continue;
        }
        answers[i] = {
          kind: 'multi',
          options: sel,
          ...(note ? { note } : {}),
        };
      } else {
        if (sel.length === 0 && !note) {
          allFilled = false;
          continue;
        }
        answers[i] = {
          kind: 'single',
          option: sel[0] || '',
          ...(note ? { note } : {}),
        };
      }
    }
    if (!allFilled) {
      onError?.('请回答所有问题');
      submittingRef.current = false;
      return;
    }
    const submission: AskSubmission = serializeSubmission({ cancelled: false, answers });
    try {
      await client.request('ask.answer', {
        session_id,
        ask_id,
        submission,
      });
    } catch (e: any) {
      onError?.(`ask.answer failed: ${e.message}`);
    } finally {
      submittingRef.current = false;
    }
  };

  const handleCancel = async () => {
    if (submittingRef.current) return;
    submittingRef.current = true;
    try {
      await client.request('ask.cancel', { session_id });
    } catch (e: any) {
      onError?.(`ask.cancel failed: ${e.message}`);
    } finally {
      submittingRef.current = false;
    }
  };

  return (
    <div className="ask-card">
      <div className="ask-card-header">
        <span className="ask-card-title">▸ ask_user</span>
        <span className="ask-card-status">等待输入</span>
      </div>
      {request.questions.map((q, qi) => {
        const sel = draftSelections[qi] || [];
        const isMulti = !!q.multi_select;
        const isFocused = draftFocus === qi;
        return (
          <div key={qi} className={`ask-question ${isFocused ? 'focused' : ''}`}>
            <div className="ask-question-title" onClick={() => handleFocus(qi)}>
              Q{qi + 1}. {q.question}
            </div>
            <div className="ask-options">
              {q.options.map((opt, oi) => {
                const checked = sel.includes(opt.label);
                return (
                  <label key={oi} className={`ask-option ${checked ? 'checked' : ''}`}>
                    <input
                      type={isMulti ? 'checkbox' : 'radio'}
                      name={`ask-q-${qi}`}
                      checked={checked}
                      onChange={() => handleSelect(qi, opt.label)}
                    />
                    <span>{opt.label}</span>
                    {opt.description ? <span className="ask-option-desc"> · {opt.description}</span> : null}
                  </label>
                );
              })}
            </div>
            {isMulti && (
              <div className="ask-card-multi-select">multi-select</div>
            )}
            <div className="ask-note-row">
              <input
                className="ask-note-input"
                type="text"
                placeholder={isFocused ? 'note (optional)' : 'click question above to add note'}
                value={draftNotes[qi] || ''}
                onChange={(e: any) => handleNoteChange(qi, e.target.value)}
                onFocus={() => handleFocus(qi)}
              />
            </div>
          </div>
        );
      })}
      <div className="ask-actions">
        <button className="ask-btn-approve" onClick={handleSubmit} disabled={submittingRef.current}>
          Submit
        </button>
        <button className="ask-btn-cancel" onClick={handleCancel} disabled={submittingRef.current}>
          Cancel
        </button>
      </div>
    </div>
  );
}

/** 提交后只读摘要：原位置显示 */
export function AskCardSummary({
  request,
  submission,
}: {
  request: AskRequest;
  submission: AskSubmission;
}) {
  const text = formatAskFullSummary(request, submission);
  return (
    <div className="ask-card ask-card-summary">
      <div className="ask-card-header">ask_user · 已回答</div>
      <pre className="ask-card-summary-text">{text}</pre>
    </div>
  );
}

/** helper hook：在通知处理中调用，把服务端事件同步到 store
 *  - ask_pending：幂等插入（issue 7）
 *  - ask_answered：仅当 ask_id + tool_call_id 匹配当前 pending 时写（issue 4）
 *  - ask_cancelled：仅当 ask_id + tool_call_id 匹配当前 pending 时清空（issue 8）
 *
 *  二次 review（issue 6/9）：Ask 通知 **只更新 store**，**不** 主动在消息流中追加
 *  tool_use / tool_result 占位。消息历史由服务端真实的 Message 事件负责；
 *  通知到来时使用 hasToolUse(messages, tool_call_id) 检测是否已存在，
 *  避免重复制造 block。attach 时同理（issue 6）。
 */
export function useAskEventBridge(
  notif: { method: string; params: any } | null,
  session_id: string | null,
  messages?: ReadonlyArray<{ role: string; content: any[] }> | null,
) {
  const setPendingIdempotent = useAskStore((s) => s.setPendingIdempotent);
  const setSubmissionIfMatch = useAskStore((s) => s.setSubmissionIfMatch);
  const clearPendingByIds = useAskStore((s) => s.clearPendingByIds);
  useEffect(() => {
    if (!notif || !session_id) return;
    if (notif.method === 'session.ask_pending' && notif.params) {
      const p = notif.params;
      if (p.session_id === session_id && p.ask_id && p.request) {
        // 仅更新 store；占位 tool_use 由后续真实 Message 事件落地（issue 6/9）
        setPendingIdempotent({
          ask_id: p.ask_id,
          tool_call_id: p.tool_call_id,
          session_id: p.session_id,
          request: p.request,
          created_at: Date.now(),
        });
      }
    } else if (notif.method === 'session.ask_answered' && notif.params) {
      const p = notif.params;
      if (p.session_id === session_id && p.ask_id && p.tool_call_id) {
        // 仅更新 store；tool_result 由后续真实 Message 事件落地
        setSubmissionIfMatch(session_id, p.ask_id, p.tool_call_id, p.submission);
      }
    } else if (notif.method === 'session.ask_cancelled' && notif.params) {
      const p = notif.params;
      if (p.session_id === session_id && p.ask_id && p.tool_call_id) {
        // 校验 ask_id + tool_call_id 后清空 pending（issue 8：防止误清其他 ask）
        clearPendingByIds(session_id, p.ask_id, p.tool_call_id);
      }
    }
  }, [notif, session_id, messages]);
}