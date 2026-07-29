// 设计文档 §6.2: Plan 审批面板
// 当 pendingPlan 存在时显示，支持 approve/reject/edit

import React from 'react';
import type { WsClient } from '@mcoder/shared/rpc/client.js';

interface Props {
  plan: any;
  client: WsClient;
  sessionId: string;
  onDismiss: () => void;
}

export function PlanPanel({ plan, client, sessionId, onDismiss }: Props) {
  if (!plan) return null;

  const handleApprove = async () => {
    try {
      await client.request('session.approve', {
        session_id: sessionId,
        action: 'approve',
      });
      onDismiss();
    } catch {}
  };

  const handleReject = async () => {
    try {
      await client.request('session.approve', {
        session_id: sessionId,
        action: 'reject',
      });
      onDismiss();
    } catch {}
  };

  const steps: any[] = Array.isArray(plan.steps) ? plan.steps : [];

  return (
    <div className="plan-panel">
      <div className="plan-panel-header">
        <span className="plan-panel-title">Plan pending approval</span>
        <button className="plan-panel-close" onClick={onDismiss} aria-label="close">
          ×
        </button>
      </div>
      <div className="plan-panel-body">
        {plan.title && <div className="plan-panel-name">{plan.title}</div>}
        {steps.length > 0 ? (
          <ol className="plan-steps">
            {steps.map((step: any, i: number) => (
              <li key={i} className="plan-step">
                <span className="plan-step-index">{i + 1}.</span>
                <span className="plan-step-text">
                  {step.description || step.text || JSON.stringify(step)}
                </span>
              </li>
            ))}
          </ol>
        ) : (
          <pre className="plan-raw">{JSON.stringify(plan, null, 2)}</pre>
        )}
      </div>
      <div className="plan-panel-actions">
        <button className="plan-btn plan-btn-approve" onClick={handleApprove}>
          Approve
        </button>
        <button className="plan-btn plan-btn-reject" onClick={handleReject}>
          Reject
        </button>
      </div>
    </div>
  );
}
