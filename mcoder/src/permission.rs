//! 设计文档 §8.8: 权限审批网关
//!
//! 三级别权限（yolo / standard / strict）
//! - yolo: 全部自动；除 yolo_deny 外无需审批
//! - standard: 只读工具自动；其他需要审批
//! - strict: 所有非只读工具都需审批
//!
//! 流程：
//!   1. ToolRegistry.execute() 在执行前调 PermissionGate.check()
//!   2. 如需要审批 → 发 PermissionRequest 到 client（ws broadcast）
//!   3. 等 PermissionResponse（client 主动 push；超时自动 deny）
//!   4. allow → 执行；deny → 返回 ToolOutput::Error
//!
//! 与 ask_user 同模式：notify 唤醒 + per-session registry。

use crate::types::{PermissionConfig, PermissionLevel, ToolCall, ToolOutput};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

/// 设计文档 §8.8: 待审批请求（推到 client）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub request_id: String,
    pub session_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub tool_args: serde_json::Value,
    /// 触发审批的原因（"tool modifies state" / "in strict mode" / 等）
    pub reason: String,
    /// 当前权限级别（client 用这个渲染徽章）
    pub level: PermissionLevel,
}

/// 设计文档 §8.8: 客户端回复（client → server）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionDecision {
    /// 同意（仅当前这一次 tool call）
    Allow,
    /// 拒绝（cancel 当前 tool call；agent 拿到 ToolOutput::Error）
    Deny { reason: Option<String> },
    /// 同意并加入 yolo mode 临时白名单（本 session 内未来同类自动放行）
    /// 仅 standard/strict 模式下 client 可选；yolo 模式无效
    AlwaysAllow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionResponse {
    pub request_id: String,
    pub session_id: String,
    pub decision: PermissionDecision,
}

/// 审批等待项（per-session，按 request_id 索引）
struct PendingPermission {
    #[allow(dead_code)]
    request: PermissionRequest,
    notify: Arc<Notify>,
    decision: Option<PermissionDecision>,
}

#[derive(Default)]
pub struct PermissionRegistry {
    /// session_id → request_id → pending
    pending: Mutex<HashMap<String, HashMap<String, PendingPermission>>>,
    /// event sink：把 PermissionEvent 转发给 SessionManager 的 ServerEvent
    /// 设计：使用 callback 而非 broadcast，避免双重 channel + spawned task
    event_sink: Mutex<Option<Box<dyn Fn(PermissionEvent) + Send + Sync>>>,
}

/// 设计文档 §8.8: 权限事件（server → client）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionEvent {
    /// 新审批请求
    Pending {
        session_id: String,
        request: PermissionRequest,
    },
    /// 已决议（client 显示卡片消失）
    Resolved {
        session_id: String,
        request_id: String,
        decision: PermissionDecision,
    },
    /// 取消（session cancel 触发）
    Cancelled {
        session_id: String,
        request_id: String,
    },
}

impl PermissionRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 注入 event sink（在 SessionManager 启动后调用）
    pub async fn set_event_sink<F>(&self, sink: F)
    where
        F: Fn(PermissionEvent) + Send + Sync + 'static,
    {
        let mut guard = self.event_sink.lock().await;
        *guard = Some(Box::new(sink));
    }

    /// 注入 event sink（boxed 版，用于需要 move closure 的场景）
    pub async fn set_event_tx_boxed(&self, sink: Box<dyn Fn(PermissionEvent) + Send + Sync>) {
        let mut guard = self.event_sink.lock().await;
        *guard = Some(sink);
    }

    async fn emit(&self, event: PermissionEvent) {
        let guard = self.event_sink.lock().await;
        if let Some(sink) = guard.as_ref() {
            sink(event);
        }
    }

    /// 设计文档 §8.8: 检查 + 等待审批
    /// 返回 Ok(()) 表示允许执行；Err 表示 deny 或 timeout
    pub async fn check_and_wait(
        &self,
        cfg: &PermissionConfig,
        session_id: &str,
        call: &ToolCall,
    ) -> Result<()> {
        // 1. 决策
        let reason = match cfg.requires_approval(&call.name) {
            None => return Ok(()), // 不需要审批
            Some(r) => r,
        };

        // 2. 注册 pending
        let request_id = uuid::Uuid::new_v4().to_string();
        let tool_call_id = call.id.clone();
        let request = PermissionRequest {
            request_id: request_id.clone(),
            session_id: session_id.to_string(),
            tool_call_id: tool_call_id.clone(),
            tool_name: call.name.clone(),
            tool_args: call.args.clone(),
            reason,
            level: cfg.level,
        };
        let notify = Arc::new(Notify::new());
        let pending = PendingPermission {
            request: request.clone(),
            notify: notify.clone(),
            decision: None,
        };

        {
            let mut map = self.pending.lock().await;
            map.entry(session_id.to_string())
                .or_default()
                .insert(request_id.clone(), pending);
        }

        // 3. 广播 Pending（让 client 渲染审批卡片）
        self.emit(PermissionEvent::Pending {
            session_id: session_id.to_string(),
            request,
        })
        .await;

        // 4. 等待决议（带 timeout：60s 默认；超时自动 deny）
        let timeout = std::time::Duration::from_secs(60);
        let decision = tokio::select! {
            _ = notify.notified() => {
                let mut map = self.pending.lock().await;
                if let Some(p) = map.get_mut(session_id).and_then(|m| m.get_mut(&request_id)) {
                    p.decision.take()
                } else {
                    None
                }
            }
            _ = tokio::time::sleep(timeout) => {
                tracing::warn!(
                    "permission request {} timed out, auto-denying tool {}",
                    request_id, call.name
                );
                Some(PermissionDecision::Deny {
                    reason: Some("permission request timed out".into()),
                })
            }
        };

        // 5. 清理 + 广播 Resolved
        {
            let mut map = self.pending.lock().await;
            if let Some(m) = map.get_mut(session_id) {
                m.remove(&request_id);
                if m.is_empty() {
                    map.remove(session_id);
                }
            }
        }
        if let Some(d) = &decision {
            self.emit(PermissionEvent::Resolved {
                session_id: session_id.to_string(),
                request_id: request_id.clone(),
                decision: d.clone(),
            })
            .await;
        }

        // 6. 决策
        match decision {
            Some(PermissionDecision::Allow) | Some(PermissionDecision::AlwaysAllow) => Ok(()),
            Some(PermissionDecision::Deny { reason }) => Err(anyhow::anyhow!(
                "permission denied: {}",
                reason.unwrap_or_else(|| "user denied".into())
            )),
            None => Err(anyhow::anyhow!("permission request lost (no decision)")),
        }
    }

    /// 设计文档 §8.8: 提交决议（client → server）
    /// 注：此函数可能在 ws handler 的同步上下文调用，用 blocking_lock
    pub fn submit_blocking(&self, session_id: &str, resp: PermissionResponse) -> Result<()> {
        let mut map = self.pending.blocking_lock();
        if let Some(p) = map
            .get_mut(session_id)
            .and_then(|m| m.get_mut(&resp.request_id))
        {
            p.decision = Some(resp.decision.clone());
            p.notify.notify_one();
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "no pending permission request {} for session {}",
                resp.request_id,
                session_id
            ))
        }
    }

    /// 设计文档 §8.8: 异步版（与 submit_blocking 同效果）
    pub async fn submit(&self, session_id: &str, resp: PermissionResponse) -> Result<()> {
        let mut map = self.pending.lock().await;
        if let Some(p) = map
            .get_mut(session_id)
            .and_then(|m| m.get_mut(&resp.request_id))
        {
            p.decision = Some(resp.decision.clone());
            p.notify.notify_one();
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "no pending permission request {} for session {}",
                resp.request_id,
                session_id
            ))
        }
    }

    /// 设计文档 §8.8: session cancel 时清理
    pub async fn cancel_session(&self, session_id: &str) {
        let items: Vec<(String, Arc<Notify>)> = {
            let mut map = self.pending.lock().await;
            if let Some(m) = map.remove(session_id) {
                m.into_iter().map(|(id, p)| (id, p.notify)).collect()
            } else {
                Vec::new()
            }
        };
        for (request_id, notify) in items {
            notify.notify_one();
            self.emit(PermissionEvent::Cancelled {
                session_id: session_id.to_string(),
                request_id,
            })
            .await;
        }
    }
}

/// 设计文档 §8.8: 包装 ToolRegistry.execute：执行前查权限
/// session_manager 在调用 ToolRegistry.execute 前调 PermissionRegistry.check_and_wait
pub fn needs_approval(cfg: &PermissionConfig, tool_name: &str) -> Option<String> {
    cfg.requires_approval(tool_name)
}

/// 设计文档 §8.8: 标准工具 ToolOutput（permission denied）
pub fn denied_output(reason: &str) -> ToolOutput {
    ToolOutput::Error {
        message: format!("Permission denied: {}", reason),
    }
}