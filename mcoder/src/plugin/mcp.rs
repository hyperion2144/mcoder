// 设计文档 §8.3.2: MCP (Model Context Protocol) 客户端
// 通过 stdio 与 MCP server 通信，把 server 暴露的 tools 注册到 ToolRegistry
//
// 协议:
//   - 启动: spawn 子进程，通过 stdin/stdout 交换 JSON-RPC 2.0 消息
//   - 初始化: initialize → initialized notification
//   - 发现工具: tools/list → 返回工具 schema 列表
//   - 调用工具: tools/call → 返回执行结果
//
// 配置示例 (config.toml):
//   [mcp_servers.filesystem]
//   command = "npx"
//   args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

use crate::tools::{SharedTool, Tool};
use crate::types::{McpServerConfig, ToolOutput, ToolSchema};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, RwLock};

/// 设计文档 §8.3.2: SSE 模式下每个 server name 对应的 (post_url, http_client)
/// 用于 request_sse / notify_sse 发送 POST 请求
static SSE_ENDPOINTS: tokio::sync::OnceCell<RwLock<HashMap<String, (String, reqwest::Client)>>> =
    tokio::sync::OnceCell::const_new();

async fn sse_endpoints() -> &'static RwLock<HashMap<String, (String, reqwest::Client)>> {
    SSE_ENDPOINTS.get_or_init(|| async { RwLock::new(HashMap::new()) }).await
}

/// JSON-RPC 2.0 请求
#[derive(Debug, Clone, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

/// JSON-RPC 2.0 通知（无 id）
#[derive(Debug, Clone, Serialize)]
struct JsonRpcNotification {
    jsonrpc: &'static str,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

/// JSON-RPC 2.0 响应
#[derive(Debug, Clone, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    pub id: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Deserialize)]
struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[allow(dead_code)]
    pub data: Option<Value>,
}

/// MCP 工具定义（来自 tools/list 响应）
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
}

/// MCP 客户端：与单个 MCP server 的连接
pub struct McpClient {
    name: String,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    // 共享 reader，由后台 task 读消息分发到 pending requests
    next_id: Mutex<u64>,
    pending: Arc<RwLock<HashMap<u64, tokio::sync::oneshot::Sender<JsonRpcResponse>>>>,
    tools: RwLock<Vec<McpToolDef>>,
}

impl McpClient {
    /// 启动 MCP server 并完成 initialize 握手
    pub async fn start(name: String, config: McpServerConfig) -> Result<Arc<Self>> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        let mut child = cmd.spawn()
            .with_context(|| format!("failed to spawn MCP server '{}': {}", name, config.command))?;

        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;

        let client = Arc::new(Self {
            name: name.clone(),
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            next_id: Mutex::new(1),
            pending: Arc::new(RwLock::new(HashMap::new())),
            tools: RwLock::new(Vec::new()),
        });

        // 启动后台 reader task
        client.spawn_reader(stdout);

        // MCP initialize 握手
        let init_params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "mcoder",
                "version": env!("CARGO_PKG_VERSION")
            }
        });
        let _init_resp = client.request("initialize", Some(init_params)).await
            .context("MCP initialize failed")?;

        // 发送 initialized 通知
        client.notify("notifications/initialized", None).await?;

        // 发现工具
        let tools_resp = client.request("tools/list", None).await
            .context("MCP tools/list failed")?;
        let tools: Vec<McpToolDef> = if let Some(result) = tools_resp.result {
            serde_json::from_value(result.get("tools").cloned().unwrap_or(Value::Array(vec![])))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        tracing::info!("MCP server '{}' exposed {} tools", name, tools.len());
        *client.tools.write().await = tools;

        Ok(client)
    }

    /// 设计文档 §8.3.2: 通过 SSE transport 启动 MCP client
    /// MCP SSE 协议:
    ///   1. GET /sse 建立 EventSource，server 返回 endpoint 事件
    ///   2. POST 到 endpoint 发送 JSON-RPC 请求
    ///   3. 响应通过 SSE 流推回
    pub async fn start_sse(name: String, config: McpServerConfig) -> Result<Arc<Self>> {
        let base_url = config.url.clone();
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        // 1. 建立 SSE 流，获取 endpoint
        let sse_url = base_url.clone();
        let (endpoint_tx, endpoint_rx) = tokio::sync::oneshot::channel::<String>();
        let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel::<JsonRpcResponse>();

        // 启动 SSE reader task
        let sse_url_clone = sse_url.clone();
        let http_clone = http.clone();
        tokio::spawn(async move {
            let mut endpoint_announced = false;
            let mut endpoint_tx_opt = Some(endpoint_tx);
            loop {
                tracing::debug!("connecting to SSE: {}", sse_url_clone);
                let resp = match http_clone.get(&sse_url_clone).send().await {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("SSE connect failed: {}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };
                if !resp.status().is_success() {
                    tracing::warn!("SSE returned status {}", resp.status());
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
                // 逐行读取 SSE 事件
                let mut buf = String::new();
                let mut byte_stream = resp.bytes_stream();
                use futures_util::StreamExt;
                while let Some(chunk_result) = byte_stream.next().await {
                    match chunk_result {
                        Ok(chunk) => {
                            buf.push_str(&String::from_utf8_lossy(&chunk));
                            // 按换行分割处理 SSE 事件
                            while let Some(pos) = buf.find('\n') {
                                let line = buf[..pos].trim_end_matches('\r').to_string();
                                buf.drain(..=pos);
                                if line.starts_with("data: ") {
                                    let data = &line[6..];
                                    if !endpoint_announced && data.contains("\"endpoint\"") {
                                        if let Ok(v) = serde_json::from_str::<Value>(data) {
                                            if let Some(ep) = v["endpoint"].as_str() {
                                                if let Some(tx) = endpoint_tx_opt.take() {
                                                    let _ = tx.send(ep.to_string());
                                                }
                                                endpoint_announced = true;
                                            }
                                        }
                                    } else if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(data) {
                                        let _ = msg_tx.send(resp);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("SSE read error: {}", e);
                            break;
                        }
                    }
                }
                tracing::info!("SSE stream closed, will retry in 5s");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });

        // 等待 endpoint
        let endpoint = tokio::time::timeout(std::time::Duration::from_secs(30), endpoint_rx)
            .await
            .map_err(|_| anyhow!("SSE endpoint timeout"))?
            .map_err(|_| anyhow!("SSE endpoint sender dropped"))?;

        // 解析 endpoint 为完整 URL
        let post_url = if endpoint.starts_with("http") {
            endpoint
        } else {
            // 相对路径，拼接 base_url
            let base = base_url.trim_end_matches("/sse").trim_end_matches('/');
            format!("{}{}", base, endpoint)
        };
        tracing::info!("MCP SSE '{}' endpoint: {}", name, post_url);

        // 构造 client（stdio 字段用占位，SSE 模式不使用）
        // 用 piped 启动一个占位进程，取其 stdin 作为 dummy ChildStdin
        let mut dummy_child = Command::new("true")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let dummy_stdin = dummy_child.stdin.take().context("dummy child stdin")?;
        let client = Arc::new(Self {
            name: name.clone(),
            child: Mutex::new(dummy_child),
            stdin: Mutex::new(dummy_stdin),
            next_id: Mutex::new(1),
            pending: Arc::new(RwLock::new(HashMap::new())),
            tools: RwLock::new(Vec::new()),
        });

        // 启动消息分发 task
        let pending = client.pending.clone();
        let client_name = name.clone();
        tokio::spawn(async move {
            while let Some(resp) = msg_rx.recv().await {
                if let Some(id) = resp.id {
                    if let Some(tx) = pending.write().await.remove(&id) {
                        let _ = tx.send(resp);
                    }
                }
            }
            tracing::debug!("MCP SSE '{}' message dispatcher exited", client_name);
        });

        // 存 post_url 到 client 便于 request 使用（用 static map 暂存）
        sse_endpoints().await.write().await.insert(name.clone(), (post_url.clone(), http.clone()));

        // MCP initialize 握手
        let init_params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "mcoder", "version": env!("CARGO_PKG_VERSION") }
        });
        let _init_resp = client.request_sse(&name, "initialize", Some(init_params)).await
            .context("MCP SSE initialize failed")?;
        client.notify_sse(&name, "notifications/initialized", None).await?;

        // 发现工具
        let tools_resp = client.request_sse(&name, "tools/list", None).await
            .context("MCP SSE tools/list failed")?;
        let tools: Vec<McpToolDef> = if let Some(result) = tools_resp.result {
            serde_json::from_value(result.get("tools").cloned().unwrap_or(Value::Array(vec![])))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        tracing::info!("MCP SSE server '{}' exposed {} tools", name, tools.len());
        *client.tools.write().await = tools;

        Ok(client)
    }

    fn spawn_reader(self: &Arc<Self>, stdout: ChildStdout) {
        let pending = self.pending.clone();
        let name = self.name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        tracing::info!("MCP server '{}' stdout closed", name);
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<JsonRpcResponse>(trimmed) {
                            Ok(resp) => {
                                if let Some(id) = resp.id {
                                    if let Some(tx) = pending.write().await.remove(&id) {
                                        let _ = tx.send(resp);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("MCP '{}' invalid JSON: {}", name, e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("MCP '{}' read error: {}", name, e);
                        break;
                    }
                }
            }
        });
    }

    async fn next_id(&self) -> u64 {
        let mut id = self.next_id.lock().await;
        let v = *id;
        *id += 1;
        v
    }

    async fn request(&self, method: &str, params: Option<Value>) -> Result<JsonRpcResponse> {
        let id = self.next_id().await;
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        let json = serde_json::to_string(&req)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.write().await.insert(id, tx);

        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(json.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }

        let resp = match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(r) => r.map_err(|_| anyhow!("MCP request '{}' sender dropped (id={})", method, id))?,
            Err(_) => {
                // 设计文档 §8.3.2: 超时后清理 pending map，避免泄漏
                self.pending.write().await.remove(&id);
                anyhow::bail!("MCP request '{}' timeout (id={})", method, id);
            }
        };

        if let Some(err) = resp.error {
            anyhow::bail!("MCP '{}' error: code={} msg={}", method, err.code, err.message);
        }
        Ok(resp)
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
        };
        let json = serde_json::to_string(&notif)?;
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    pub async fn list_tools(&self) -> Vec<McpToolDef> {
        self.tools.read().await.clone()
    }

    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Value> {
        let params = serde_json::json!({
            "name": name,
            "arguments": args,
        });
        // 优先用 SSE 模式（若已注册），否则用 stdio
        let resp = {
            let sse_map = sse_endpoints().await.read().await;
            if sse_map.contains_key(&self.name) {
                drop(sse_map);
                self.request_sse(&self.name, "tools/call", Some(params)).await
            } else {
                self.request("tools/call", Some(params)).await
            }
        }.with_context(|| format!("MCP tools/call '{}' failed", name))?;
        resp.result.ok_or_else(|| anyhow!("MCP tools/call returned no result"))
    }

    pub async fn shutdown(&self) -> Result<()> {
        let _ = self.notify("shutdown", None).await;
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        // 清理 SSE endpoint 注册
        sse_endpoints().await.write().await.remove(&self.name);
        Ok(())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// 设计文档 §8.3.2: SSE 模式发送请求（POST 到 endpoint）
    async fn request_sse(&self, server_name: &str, method: &str, params: Option<Value>) -> Result<JsonRpcResponse> {
        let id = self.next_id().await;
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        let (post_url, http) = {
            let map = sse_endpoints().await.read().await;
            map.get(server_name)
                .cloned()
                .ok_or_else(|| anyhow!("SSE endpoint not found for server '{}'", server_name))?
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.write().await.insert(id, tx);

        let resp = http.post(&post_url).json(&req).send().await
            .with_context(|| format!("SSE POST to {} failed", post_url))?;
        if !resp.status().is_success() {
            self.pending.write().await.remove(&id);
            anyhow::bail!("SSE POST returned status {}", resp.status());
        }

        let result = tokio::time::timeout(std::time::Duration::from_secs(30), rx).await
            .map_err(|_| {
                // 超时清理
                let pending = self.pending.clone();
                let id = id;
                tokio::spawn(async move {
                    pending.write().await.remove(&id);
                });
                anyhow!("SSE request '{}' timeout (id={})", method, id)
            })?
            .map_err(|_| anyhow!("SSE request '{}' sender dropped (id={})", method, id))?;

        if let Some(err) = result.error {
            anyhow::bail!("SSE '{}' error: code={} msg={}", method, err.code, err.message);
        }
        Ok(result)
    }

    /// 设计文档 §8.3.2: SSE 模式发送通知（POST 到 endpoint，无响应）
    async fn notify_sse(&self, server_name: &str, method: &str, params: Option<Value>) -> Result<()> {
        let (post_url, http) = {
            let map = sse_endpoints().await.read().await;
            map.get(server_name)
                .cloned()
                .ok_or_else(|| anyhow!("SSE endpoint not found for server '{}'", server_name))?
        };
        let notif = JsonRpcNotification {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
        };
        let _ = http.post(&post_url).json(&notif).send().await?;
        Ok(())
    }
}

/// MCP 工具包装器：把单个 MCP server 工具暴露为 mcoder Tool
pub struct McpToolWrapper {
    pub server_name: String,
    pub tool_def: McpToolDef,
    pub client: Arc<McpClient>,
    pub registered_name: String, // mcp__{server}__{tool}
}

impl McpToolWrapper {
    pub fn new(server_name: String, tool_def: McpToolDef, client: Arc<McpClient>) -> Self {
        let registered_name = format!("mcp__{}__{}", server_name, tool_def.name);
        Self {
            server_name,
            tool_def,
            client,
            registered_name,
        }
    }
}

#[async_trait]
impl Tool for McpToolWrapper {
    fn name(&self) -> &str {
        // 命名规范: mcp__{server}__{tool}，避免与内置工具冲突
        &self.registered_name
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.registered_name.clone(),
            description: if self.tool_def.description.is_empty() {
                format!("[MCP/{}] {}", self.server_name, self.tool_def.name)
            } else {
                self.tool_def.description.clone()
            },
            parameters: self.tool_def.input_schema.clone(),
        }
    }

    async fn execute(&self, args: Value, _ctx: &crate::tools::ToolContext) -> Result<ToolOutput> {
        // 调用 MCP server 时用原始工具名（不带 mcp__ 前缀）
        match self.client.call_tool(&self.tool_def.name, &args).await {
            Ok(result) => Ok(ToolOutput::Sync { result }),
            Err(e) => Ok(ToolOutput::Error { message: e.to_string() }),
        }
    }
}

/// MCP 管理器：管理所有已连接的 MCP server
pub struct McpManager {
    clients: RwLock<HashMap<String, Arc<McpClient>>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
        }
    }

    /// 启动并初始化所有配置的 MCP server，返回注册的工具列表
    /// 设计文档 §8.3.2: 从 config.mcp_servers 加载，支持 stdio + SSE 两种 transport
    pub async fn start_all(&self, servers: &HashMap<String, McpServerConfig>) -> Result<Vec<SharedTool>> {
        let mut tools = Vec::new();
        for (name, config) in servers {
            // 设计文档 §8.3.2: 根据 config 字段选择 transport
            // 有 url → SSE，有 command → stdio
            let client_result = if !config.url.is_empty() {
                McpClient::start_sse(name.clone(), config.clone()).await
            } else if !config.command.is_empty() {
                McpClient::start(name.clone(), config.clone()).await
            } else {
                tracing::warn!("MCP server '{}' config missing both 'command' and 'url', skip", name);
                continue;
            };

            match client_result {
                Ok(client) => {
                    let server_name = name.clone();
                    let mcp_tools = client.list_tools().await;
                    for tool_def in mcp_tools {
                        let wrapper = McpToolWrapper::new(
                            server_name.clone(),
                            tool_def,
                            client.clone(),
                        );
                        tools.push(Arc::new(wrapper) as SharedTool);
                    }
                    self.clients.write().await.insert(name.clone(), client);
                    tracing::info!("MCP server '{}' started", name);
                }
                Err(e) => {
                    tracing::warn!("failed to start MCP server '{}': {}", name, e);
                }
            }
        }
        Ok(tools)
    }

    /// 列出所有已连接 server 及其工具定义
    pub async fn list_all_tools(&self) -> Vec<(String, Vec<McpToolDef>)> {
        let clients = self.clients.read().await;
        let mut result = Vec::new();
        for (name, client) in clients.iter() {
            let tools = client.list_tools().await;
            result.push((name.clone(), tools));
        }
        result
    }

    /// 调用指定 server 上的工具
    pub async fn call_tool(&self, server: &str, tool: &str, args: &Value) -> Result<Value> {
        let client = {
            let clients = self.clients.read().await;
            clients.get(server)
                .cloned()
                .ok_or_else(|| anyhow!("MCP server '{}' not found", server))?
        };
        client.call_tool(tool, args).await
    }

    /// 关闭所有 MCP server
    pub async fn shutdown_all(&self) {
        let clients: Vec<Arc<McpClient>> = self.clients.read().await.values().cloned().collect();
        for client in clients {
            let _ = client.shutdown().await;
        }
    }
}
