// Phase 3: Mobile Resume 入口（消息区下方、输入框上方；非模态）
// 共用逻辑：@mcoder/shared/resume/state.ts

import React, { useState } from 'react';
import {
  computeResumeEntry,
  hasResumeEntry,
  type ResumeEntry,
} from '@mcoder/shared/resume/state.js';
import { useSessionStore, useMessagesStore } from '@mcoder/shared/store/index.js';
import type { WsClient } from '@mcoder/shared/rpc/client.js';
import { Play } from './icons.js';

interface Props {
  client: WsClient | null;
  sessionId: string | null;
}

export function ResumeBar({ client, sessionId }: Props) {
  const sessionStore = useSessionStore();
  const msgStore = useMessagesStore();
  const [busy, setBusy] = useState(false);

  if (!sessionId) return null;

  const entry: ResumeEntry = computeResumeEntry({
    loop_state: sessionStore.loopState,
    stop_reason: sessionStore.stopReason,
    has_unfinished_todo: ((sessionStore.pendingTodos ?? []) as any[]).some(
      (t) => t.status === 'pending' || t.status === 'in_progress',
    ),
    loop_running: !sessionStore.canResume,
    // Phase 5c: 5 参数与 Rust 同步
    has_interrupted_tasks: ((sessionStore.backgroundTasks ?? []) as any[]).some(
      (t: any) => t.status === 'Interrupted' || t.status === 'interrupted',
    ),
  });

  if (!hasResumeEntry(entry)) return null;

  const labelKind = entry.kind === 'auto_resume'
    ? 'Resume (auto)'
    : entry.kind === 'requires_input'
      ? 'Resume (waiting)'
      : 'Resume (ask)';

  const onClick = async () => {
    if (!client || !sessionId || busy) return;
    setBusy(true);
    try {
      const result: any = await client.request('session.resume', { session_id: sessionId });
      if (result && result.started) {
        sessionStore.setLoopState('running', null);
        sessionStore.setCanResume(false);
        msgStore.setStreaming(true);
      } else if (result && result.requires_user_input) {
        // 无工作：UI 提示即可
      } else if (result && result.waiting_for_user) {
        // 保留 ask 流程
      }
    } catch (e: any) {
      msgStore.setError(`session.resume failed: ${e.message}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="resume-bar">
      <button
        className="resume-bar-button"
        onClick={onClick}
        disabled={busy}
        title={entry.reason}
      >
        <Play size={12} /> {labelKind}
      </button>
    </div>
  );
}