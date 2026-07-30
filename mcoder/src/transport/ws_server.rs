use crate::session_manager::{ServerEvent, SessionManager};
use crate::transport::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::transport::pairing::{self, PairingInfo};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::protocol::Message;

/// 设计文档 §5.6: 心跳与超时
/// 客户端每 30s 发 ping，server 回 pong
/// 60s 无任何消息 → server 视为断开
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(60);

pub struct WsServer {
    pub addr: SocketAddr,
    pub pairing: PairingInfo,
    pub session_mgr: Arc<SessionManager>,
    /// 设计文档 §8.6: TLS acceptor（wss 支持）
    /// None = 不使用 TLS（本地 ws://）
    pub tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
}

impl WsServer {
    /// 设计文档 §8.6: 启动 WebSocket 服务器
    /// tls_acceptor: 外部传入的 TLS acceptor（如 ACME 证书），None 则根据 tls 模式自动决定
    pub async fn start_with_tls(
        host: &str,
        port: u16,
        session_mgr: Arc<SessionManager>,
        tls_acceptor_override: Option<tokio_rustls::TlsAcceptor>,
    ) -> Result<Arc<Self>> {
        let pairing = pairing::generate_pairing(host, port)?;
        let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
        let listener = TcpListener::bind(&addr).await
            .with_context(|| format!("binding to {}", addr))?;

        // 设计文档 §8.6: TLS 决策
        // 1. 如果外部传入了 ACME 证书 → 直接使用
        // 2. 否则根据 tls 模式决定（auto/on → 自签证书，off → 无 TLS）
        let tls_acceptor = if let Some(acceptor) = tls_acceptor_override {
            tracing::info!("TLS enabled (wss://) - ACME certificate");
            Some(acceptor)
        } else {
            let use_tls = crate::transport::tls::should_use_tls(&pairing.tls, host);
            if use_tls {
                match crate::transport::tls::build_tls_acceptor() {
                    Ok(acceptor) => {
                        tracing::info!("TLS enabled (wss://) - self-signed cert");
                        Some(acceptor)
                    }
                    Err(e) => {
                        tracing::warn!("failed to build TLS acceptor, falling back to ws://: {}", e);
                        None
                    }
                }
            } else {
                tracing::info!("TLS disabled (ws://) - local connection");
                None
            }
        };

        let server = Arc::new(Self {
            addr: listener.local_addr()?,
            pairing,
            session_mgr,
            tls_acceptor,
        });

        let srv = server.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer)) => {
                        tracing::info!("client connected from {}", peer);
                        let srv = srv.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, srv).await {
                                tracing::warn!("connection error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("accept error: {}", e);
                    }
                }
            }
        });

        Ok(server)
    }

    pub fn pairing_info(&self) -> &PairingInfo {
        &self.pairing
    }
}

/// 设计文档 §8.6: 支持 TLS 和非 TLS 两种连接
/// TLS 连接：先 TLS handshake，再 WebSocket upgrade
/// 非 TLS 连接：直接 WebSocket upgrade
async fn handle_connection(stream: TcpStream, server: Arc<WsServer>) -> Result<()> {
    // 设计文档 §5.2: 先读取 HTTP 握手请求，验证 token，再升级为 WebSocket
    // 简化实现：用 accept_async 升级，token 在连接后通过第一条消息验证
    // 真正的 token 验证应该在握手阶段，但 tungstenite 的 accept_hdr_async 有 HRTB 问题
    // 这里折中：接受连接后立即检查第一条消息是否为 auth 请求

    // 设计文档 §8.6: 如果有 TLS acceptor，先做 TLS handshake
    if let Some(ref acceptor) = server.tls_acceptor {
        let tls_stream = acceptor.accept(stream).await
            .context("TLS handshake")?;
        let ws = tokio_tungstenite::accept_async(tls_stream).await
            .context("websocket handshake over TLS")?;
        return handle_ws_connection(ws, server).await;
    }

    let ws = tokio_tungstenite::accept_async(stream).await
        .context("websocket handshake")?;
    handle_ws_connection(ws, server).await
}

/// 设计文档 §5.2: WebSocket 连接处理（TLS 和非 TLS 共用）
async fn handle_ws_connection<S>(ws: tokio_tungstenite::WebSocketStream<S>, server: Arc<WsServer>) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut write, mut read) = ws.split();
    let mut rx = server.session_mgr.subscribe();

    // 设计文档 §5.5: 跟踪此 client 当前 attach 的 session_id
    // 用于多 client 隔离：只接收已 attach session 的事件
    let mut attached_session: Option<String> = None;

    // 设计文档 §5.2: 等待第一条消息验证 token
    // 客户端连接后必须发送 {"method":"auth","params":{"token":"xxx"}}
    let expected_token = server.pairing.token.clone();
    let first_msg = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read.next()
    ).await;
    let authenticated = match first_msg {
        Ok(Some(Ok(Message::Text(text)))) => {
            if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(&text) {
                if req.method == "auth" {
                    let token = req.params.as_ref()
                        .and_then(|p| p["token"].as_str())
                        .unwrap_or("");
                    token == expected_token
                } else {
                    false
                }
            } else {
                false
            }
        }
        _ => false,
    };

    if !authenticated {
        let err = make_notification("error", serde_json::json!({
            "message": "authentication failed: invalid or missing token"
        }));
        let _ = write.send(Message::Text(err)).await;
        let _ = write.send(Message::Close(None)).await;
        anyhow::bail!("authentication failed");
    }

    // 认证成功，发送 ack
    let ack = serde_json::to_string(&JsonRpcResponse::ok(
        Some(serde_json::Value::from(0)),
        serde_json::json!({"authenticated": true})
    )).unwrap_or_default();
    let _ = write.send(Message::Text(ack)).await;

    // 设计文档 §5.2: 连接成功后推送 session.welcome
    {
        let welcome_params = serde_json::json!({
            "server_version": env!("CARGO_PKG_VERSION"),
            "sessions": server.session_mgr.list_sessions().await.unwrap_or_default(),
            "capabilities": {
                "roles": server.session_mgr.list_roles(),
                "tools": server.session_mgr.list_tools().await.into_iter().map(|t| t.name).collect::<Vec<_>>(),
            }
        });
        let notif = make_notification("session.welcome", welcome_params);
        let _ = write.send(Message::Text(notif)).await;
    }

    // 设计文档 §5.6: 心跳与超时
    // 记录最后一次收到客户端消息的时间，60s 无任何消息视为断开
    let mut last_activity = Instant::now();

    loop {
        // 设计文档 §5.6: 距离下次心跳超时的剩余时间
        let remaining = HEARTBEAT_TIMEOUT
            .checked_sub(last_activity.elapsed())
            .unwrap_or(Duration::ZERO);

        tokio::select! {
            // 设计文档 §5.6: 60s 无心跳 → 视为断开
            _ = tokio::time::sleep(remaining), if !remaining.is_zero() => {
                if last_activity.elapsed() >= HEARTBEAT_TIMEOUT {
                    tracing::info!("client heartbeat timeout, closing connection");
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        last_activity = Instant::now();
                        if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(&text) {
                            // 设计文档 §5.5: 拦截 session.attach 以跟踪 client 的 session
                            if req.method == "session.attach" {
                                let sid = req.params.as_ref()
                                    .and_then(|p| p["session_id"].as_str())
                                    .unwrap_or("")
                                    .to_string();
                                // 设计文档 §5.6: 支持 offset 参数用于断线重连补推
                                let offset = req.params.as_ref()
                                    .and_then(|p| p["offset"].as_u64())
                                    .map(|n| n as usize);
                                if !sid.is_empty() {
                                    // 若已有 attach 的 session，先 detach
                                    if let Some(old) = &attached_session {
                                        let _ = server.session_mgr.detach_client(old).await;
                                    }
                                    // attach 新 session（会从 jsonl 重放若不在内存）
                                    // Phase 2: 返回统一 SessionSnapshot；messages 仍按 offset 增量，
                                    // 其它字段始终为 session 当前全量最新值
                                    match server.session_mgr.attach_session_with_offset(&sid, offset).await {
                                        Ok(snapshot) => {
                                            let _ = server.session_mgr.attach_client(&sid).await;
                                            attached_session = Some(sid.clone());
                                            let resp = JsonRpcResponse::ok(req.id, serde_json::to_value(&snapshot).unwrap_or(serde_json::json!({
                                                "session_id": sid,
                                                "attached": true
                                            })));
                                            if let Ok(json) = serde_json::to_string(&resp) {
                                                let _ = write.send(Message::Text(json)).await;
                                            }
                                            continue;
                                        }
                                        Err(e) => {
                                            let resp = JsonRpcResponse::err(req.id, -1, e.to_string());
                                            if let Ok(json) = serde_json::to_string(&resp) {
                                                let _ = write.send(Message::Text(json)).await;
                                            }
                                            continue;
                                        }
                                    }
                                }
                            }
                            let resp = handle_request(req, &server.session_mgr, &attached_session).await;
                            if let Ok(json) = serde_json::to_string(&resp) {
                                let _ = write.send(Message::Text(json)).await;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        last_activity = Instant::now();
                        let _ = write.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        last_activity = Instant::now();
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {
                        last_activity = Instant::now();
                    }
                }
            }
            event = rx.recv() => {
                if let Ok(event) = event {
                    // 设计文档 §5.5: 多 client 隔离
                    // 只推送与当前 client attached session 相关的事件
                    // 全局事件（session_created/session_list/error）推送给所有 client
                    let (notif, target_session) = match event {
                        ServerEvent::Message { session_id, message } => (
                            make_notification("message", serde_json::json!({
                                "session_id": session_id,
                                "message": message
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::ToolCallStart { session_id, name } => (
                            make_notification("tool_call_start", serde_json::json!({
                                "session_id": session_id,
                                "name": name
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::ToolCallDone { session_id, name, success } => (
                            make_notification("tool_call_done", serde_json::json!({
                                "session_id": session_id,
                                "name": name,
                                "success": success
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::SessionCreated { session_id, title } => (
                            make_notification("session_created", serde_json::json!({
                                "session_id": session_id,
                                "title": title
                            })),
                            None, // 广播给所有 client
                        ),
                        ServerEvent::SessionList { sessions } => (
                            make_notification("session_list", serde_json::json!({
                                "sessions": sessions
                            })),
                            None,
                        ),
                        ServerEvent::PlanCreated { session_id, plan } => (
                            make_notification("session.plan_created", serde_json::json!({
                                "session_id": session_id,
                                "plan": plan
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::RoleChanged { session_id, role } => (
                            make_notification("session.mode_event", serde_json::json!({
                                "session_id": session_id,
                                "role": role
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::ModelChanged { session_id, model } => (
                            make_notification("session.model_changed", serde_json::json!({
                                "session_id": session_id,
                                "model": model
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::SessionDone { session_id, reason, unfinished_todos } => (
                            make_notification("session.done", serde_json::json!({
                                "session_id": session_id,
                                "reason": reason,
                                "unfinished_todos": unfinished_todos,
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::TodoUpdated { session_id, todos, summary } => (
                            make_notification("session.todo_updated", serde_json::json!({
                                "session_id": session_id,
                                "todos": todos,
                                "summary": summary,
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::AskPending { session_id, ask_id, tool_call_id, request } => (
                            make_notification("session.ask_pending", serde_json::json!({
                                "session_id": session_id,
                                "ask_id": ask_id,
                                "tool_call_id": tool_call_id,
                                "request": request,
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::AskAnswered { session_id, ask_id, tool_call_id, submission, result } => (
                            make_notification("session.ask_answered", serde_json::json!({
                                "session_id": session_id,
                                "ask_id": ask_id,
                                "tool_call_id": tool_call_id,
                                "submission": submission,
                                "result": result,
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::AskCancelled { session_id, ask_id, tool_call_id } => (
                            make_notification("session.ask_cancelled", serde_json::json!({
                                "session_id": session_id,
                                "ask_id": ask_id,
                                "tool_call_id": tool_call_id,
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::UsageUpdated { session_id, delta, cumulative, context_window } => (
                            make_notification("session.usage_updated", serde_json::json!({
                                "session_id": session_id,
                                "delta": delta,
                                "cumulative": cumulative,
                                "context_window": context_window,
                            })),
                            Some(session_id),
                        ),
                        ServerEvent::Error { message } => (
                            make_notification("error", serde_json::json!({
                                "message": message
                            })),
                            None,
                        ),
                    };
                    // 过滤：session 事件只推给 attached 的 client；全局事件推给所有
                    let should_send = match (&attached_session, target_session) {
                        (_, None) => true, // 全局事件
                        (Some(sid), Some(target)) => sid == &target, // 只推 attached session
                        (None, Some(_)) => false, // 未 attach 的 client 不收 session 事件
                    };
                    if should_send {
                        let _ = write.send(Message::Text(notif)).await;
                    }
                }
            }
        }
    }

    // 设计文档 §5.5: 连接断开时 detach client
    if let Some(sid) = &attached_session {
        let _ = server.session_mgr.detach_client(sid).await;
        tracing::debug!("client detached from session {}", sid);
    }
    Ok(())
}

async fn handle_request(
    req: JsonRpcRequest,
    mgr: &Arc<SessionManager>,
    attached_session: &Option<String>,
) -> JsonRpcResponse {
    match req.method.as_str() {
        "ping" => JsonRpcResponse::ok(req.id, serde_json::json!("pong")),

        // ===== 会话管理 =====
        "sessions.list" => {
            match mgr.list_sessions().await {
                Ok(sessions) => JsonRpcResponse::ok(req.id, serde_json::json!(sessions)),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        "sessions.create" => {
            let params = req.params.unwrap_or_default();
            let title = params["title"].as_str().unwrap_or("New Session");
            let model_name = params["model"].as_str();
            // 多项目支持：从 params 获取 project 路径，默认用当前工作目录
            let project = params["project"].as_str()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

            match mgr.create_session(&project, title, model_name).await {
                Ok(sid) => JsonRpcResponse::ok(req.id, serde_json::json!({
                    "session_id": sid,
                    "title": title
                })),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        // session.attach 在 handle_connection 中拦截处理（含 client 跟踪）
        "session.attach" => {
            JsonRpcResponse::err(req.id, -32603, "session.attach should be handled by connection layer".to_string())
        }

        "session.close" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            match mgr.close_session(session_id).await {
                Ok(_) => JsonRpcResponse::ok(req.id, serde_json::json!({"closed": true})),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        "session.delete" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            match mgr.delete_session(session_id).await {
                Ok(_) => JsonRpcResponse::ok(req.id, serde_json::json!({"deleted": true})),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        "sessions.messages" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            match mgr.get_messages(session_id).await {
                Ok(msgs) => JsonRpcResponse::ok(req.id, serde_json::json!(msgs)),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        "sessions.send" | "session.user_input" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            let content = params["content"].as_str().unwrap_or("");

            // 支持附带图片（base64）：params.images = [{data, media_type}, ...]
            let imgs: Vec<(String, String)> = params.get("images")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter()
                    .filter_map(|img| {
                        let data = img.get("data").and_then(|v| v.as_str())?.to_string();
                        let media_type = img.get("media_type").and_then(|v| v.as_str())
                            .unwrap_or("image/png").to_string();
                        Some((data, media_type))
                    })
                    .collect())
                .unwrap_or_default();

            if imgs.is_empty() {
                match mgr.send_message(session_id, content).await {
                    Ok(_) => JsonRpcResponse::ok(req.id, serde_json::json!({"accepted": true})),
                    Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
                }
            } else {
                match mgr.send_message_with_images(session_id, content, imgs).await {
                    Ok(_) => JsonRpcResponse::ok(req.id, serde_json::json!({"accepted": true})),
                    Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
                }
            }
        }

        "session.cancel" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            match mgr.cancel_session(session_id).await {
                Ok(_) => JsonRpcResponse::ok(req.id, serde_json::json!({"cancelled": true})),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        // ===== Phase 3: session.resume =====
        // 决策：unfinished todos / blocked / cancelled / failed → Start；
        // running / waiting_for_user → Conflict；无工作 → NoWork
        // 详见 SessionManager::resume_session 文档
        "session.resume" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            if session_id.is_empty() {
                return JsonRpcResponse::err(
                    req.id,
                    -1,
                    "missing 'session_id' for session.resume".to_string(),
                );
            }
            match mgr.resume_session(session_id).await {
                Ok(result) => JsonRpcResponse::ok(req.id, result),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        // ===== Message Tree (分叉/切换) =====
        "session.tree" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            if session_id.is_empty() {
                return JsonRpcResponse::err(
                    req.id,
                    -32602,
                    "session_id required for session.tree".to_string(),
                );
            }
            if let Err(reason) = check_attached_session(attached_session, session_id) {
                return JsonRpcResponse::err(req.id, -32602, reason);
            }
            match mgr.get_message_tree(session_id).await {
                Ok(tree) => JsonRpcResponse::ok(req.id, serde_json::json!(tree)),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        "session.checkout" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            let message_id = params["message_id"].as_str().unwrap_or("");
            if session_id.is_empty() {
                return JsonRpcResponse::err(
                    req.id,
                    -32602,
                    "session_id required for session.checkout".to_string(),
                );
            }
            if message_id.is_empty() {
                return JsonRpcResponse::err(
                    req.id,
                    -32602,
                    "message_id required for session.checkout".to_string(),
                );
            }
            if let Err(reason) = check_attached_session(attached_session, session_id) {
                return JsonRpcResponse::err(req.id, -32602, reason);
            }
            match mgr.checkout(session_id, message_id).await {
                Ok(snapshot) => JsonRpcResponse::ok(req.id, serde_json::to_value(&snapshot).unwrap_or(serde_json::json!({"checked_out": true}))),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        // ===== Mode / Role =====
        "session.mode.set" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            let role = params["role"].as_str().unwrap_or("default");
            match mgr.set_role(session_id, role).await {
                Ok(_) => {
                    let _ = mgr.broadcast_role_changed(session_id, role).await;
                    JsonRpcResponse::ok(req.id, serde_json::json!({"role": role}))
                }
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        "session.mode.get" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            match mgr.current_role(session_id).await {
                Ok(role) => JsonRpcResponse::ok(req.id, serde_json::json!({"role": role})),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        },

        "session.model.set" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            let model = params["model"].as_str().unwrap_or("");
            if session_id.is_empty() {
                return JsonRpcResponse::err(
                    req.id,
                    -32602,
                    "session_id required for session.model.set".to_string(),
                );
            }
            if model.is_empty() {
                return JsonRpcResponse::err(
                    req.id,
                    -32602,
                    "model required for session.model.set".to_string(),
                );
            }
            match mgr.set_model(session_id, model).await {
                Ok(_) => {
                    // set_model 内部已广播 ModelChanged 事件，此处不再重复
                    JsonRpcResponse::ok(req.id, serde_json::json!({"model": model}))
                }
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        },

        "session.model.get" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            match mgr.current_model(session_id).await {
                Ok(model) => JsonRpcResponse::ok(req.id, serde_json::json!({"model": model})),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        },

        "session.mode.list" => {
            let roles = mgr.list_roles();
            JsonRpcResponse::ok(req.id, serde_json::json!({"roles": roles}))
        }

        // ===== Plan Approve =====
        "session.approve" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            let action = params["action"].as_str().unwrap_or("approve"); // approve | reject | edit
            match mgr.approve_plan(session_id, action, params.get("edited_plan").cloned()).await {
                Ok(result) => JsonRpcResponse::ok(req.id, result),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        // ===== AskUser RPC =====
        "ask.pending" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            // 终审修复 #14 + Phase 5c：caller 必须 attach 到 session
            if session_id.is_empty() {
                return JsonRpcResponse::err(
                    req.id,
                    -32602,
                    "session_id required for ask.pending (caller must be attached to a session)".to_string(),
                );
            }
            // 校验 attached_session == session_id（防止越权读取）
            if let Err(reason) = check_attached_session(attached_session, session_id) {
                return JsonRpcResponse::err(req.id, -32602, reason);
            }
            match mgr.peek_ask(session_id).await {
                Some(pending) => JsonRpcResponse::ok(req.id, pending),
                None => JsonRpcResponse::ok(req.id, serde_json::json!({"pending": null})),
            }
        }

        "ask.answer" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            let ask_id = params["ask_id"].as_str().unwrap_or("");
            // submission 可以是对象 {cancelled, answers} 或 null（cancelled）
            let submission: crate::ask_user::AskSubmission = match params.get("submission") {
                Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
                None => crate::ask_user::AskSubmission { cancelled: true, ..Default::default() },
            };
            match mgr.answer_ask(session_id, ask_id, submission).await {
                Ok(result) => JsonRpcResponse::ok(req.id, result),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        "ask.cancel" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            match mgr.cancel_ask(session_id).await {
                Ok(result) => JsonRpcResponse::ok(req.id, result.unwrap_or(serde_json::json!({"cancelled": false}))),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        // ===== Tools =====
        "tool.call" => {
            let params = req.params.unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            let tool_name = params["name"].as_str().unwrap_or("");
            let tool_args = params["args"].clone();

            if session_id.is_empty() {
                return JsonRpcResponse::err(req.id, -1, "missing 'session_id' for tool.call".to_string());
            }
            // Phase 5c: 校验 attached_session == session_id
            if let Err(reason) = check_attached_session(attached_session, session_id) {
                return JsonRpcResponse::err(req.id, -32602, reason);
            }

            match mgr.call_tool(session_id, tool_name, tool_args).await {
                Ok(output) => JsonRpcResponse::ok(req.id, serde_json::json!(output)),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        "tools.list" => {
            let schemas = mgr.list_tools().await;
            JsonRpcResponse::ok(req.id, serde_json::json!(schemas))
        }

        // ===== Task =====
        "task.list" => {
            // 终审修复 #16 + Phase 5c: 强制 session_id；caller 必须 attach 到 session 才可调用
            let params = req.params.clone().unwrap_or_default();
            let session_id = params["session_id"].as_str().unwrap_or("");
            if session_id.is_empty() {
                return JsonRpcResponse::err(
                    req.id,
                    -32602,
                    "session_id required for task.list (caller must be attached to a session)".to_string(),
                );
            }
            // 校验 attached_session == session_id
            if let Err(reason) = check_attached_session(attached_session, session_id) {
                return JsonRpcResponse::err(req.id, -32602, reason);
            }
            let tasks = mgr.list_tasks_for_session(session_id).await;
            JsonRpcResponse::ok(req.id, serde_json::json!(tasks))
        }

        "task.cancel" => {
            // 终审修复 #16 + Phase 5c: 强制 session_id；不再走跨会话兼容路径
            let params = req.params.clone().unwrap_or_default();
            let task_id = params["task_id"].as_str().unwrap_or("");
            let session_id = params["session_id"].as_str().unwrap_or("");
            if session_id.is_empty() {
                return JsonRpcResponse::err(
                    req.id,
                    -32602,
                    "session_id required for task.cancel (caller must be attached to a session)".to_string(),
                );
            }
            if task_id.is_empty() {
                return JsonRpcResponse::err(
                    req.id,
                    -32602,
                    "task_id required for task.cancel".to_string(),
                );
            }
            // 校验 attached_session == session_id
            if let Err(reason) = check_attached_session(attached_session, session_id) {
                return JsonRpcResponse::err(req.id, -32602, reason);
            }
            match mgr.cancel_task_for_session(session_id, task_id).await {
                Ok(_) => JsonRpcResponse::ok(req.id, serde_json::json!({"cancelled": true})),
                Err(e) => JsonRpcResponse::err(req.id, -403, e.to_string()),
            }
        }

        // ===== Config =====
        "config.get" => {
            let params = req.params.unwrap_or_default();
            let key = params["key"].as_str();
            let result = mgr.get_config(key).await;
            JsonRpcResponse::ok(req.id, serde_json::json!(result))
        }

        "config.set" => {
            let params = req.params.unwrap_or_default();
            let key = params["key"].as_str().unwrap_or("");
            let value = params["value"].clone();
            match mgr.set_config(key, value).await {
                Ok(_) => JsonRpcResponse::ok(req.id, serde_json::json!({"set": true})),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        "config.list_models" => {
            let models = mgr.list_models();
            JsonRpcResponse::ok(req.id, serde_json::json!(models))
        }

        // ===== Slash Commands =====
        // 客户端转发 /xxx 输入到服务端，由服务端解析和分发
        // 返回 DispatchResult：Meta（结构化指令）/ CustomCommand（提示词）/ Skill（提示词）/ Unknown
        "command.call" => {
            let params = req.params.unwrap_or_default();
            let input = params["input"].as_str().unwrap_or("");
            if input.is_empty() {
                return JsonRpcResponse::err(req.id, -1, "missing 'input' field".to_string());
            }
            // 去掉前导 /
            let input = input.strip_prefix('/').unwrap_or(input);
            match mgr.dispatch_command(input).await {
                Ok(result) => JsonRpcResponse::ok(req.id, serde_json::json!(result)),
                Err(e) => JsonRpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        "command.list" => {
            let commands = mgr.list_commands().await;
            JsonRpcResponse::ok(req.id, serde_json::json!(commands))
        }

        // ===== Server =====
        "server.stats" => {
            let stats = mgr.server_stats().await;
            JsonRpcResponse::ok(req.id, serde_json::json!(stats))
        }

        "server.shutdown" => {
            tracing::info!("shutdown requested by client");
            // 异步触发 shutdown
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                std::process::exit(0);
            });
            JsonRpcResponse::ok(req.id, serde_json::json!({"shutting_down": true}))
        }

        method => JsonRpcResponse::err(
            req.id,
            -32601,
            format!("method not found: {}", method),
        ),
    }
}

pub fn make_notification(method: &str, params: serde_json::Value) -> String {
    let notif = JsonRpcNotification::new(method, Some(params));
    serde_json::to_string(&notif).unwrap_or_default()
}

// ==================== Phase 5c: attached_session 校验 ====================
//
// 把"caller 是否 attach 到指定 session"的判定抽成纯函数，便于单测。
// 返回 `Result<(), String>`：Ok 表示放行；Err 表示拒绝（带拒绝原因）。
//
// 规则：
// 1. caller 未 attach（None）→ 拒（session-scoped RPC 必传 attached）
// 2. param session_id 与 attached 不一致 → 拒
// 3. param session_id 为空 → 放行（非 session-scoped RPC，不走该 helper）
pub fn check_attached_session(
    attached_session: &Option<String>,
    param_session_id: &str,
) -> Result<(), String> {
    // 1. caller 必须 attach
    let attached = match attached_session.as_deref() {
        Some(s) => s,
        None => {
            return Err(format!(
                "caller not attached to any session; refusing session-scoped RPC"
            ));
        }
    };
    // 2. param 为空：放行（让外层 caller 决定走哪条分支）
    if param_session_id.is_empty() {
        return Ok(());
    }
    // 3. 必须匹配
    if attached != param_session_id {
        return Err(format!(
            "caller attached to '{}' but params.session_id='{}' (cross-session denied)",
            attached, param_session_id
        ));
    }
    Ok(())
}
