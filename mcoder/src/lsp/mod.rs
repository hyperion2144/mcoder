// 设计文档 §8.4.2: LSP 集成模块
// 支持语言：Rust (rust-analyzer) / TS (tsserver) / Go (gopls) / Python (pylsp)
// 架构：LSP client 在 server 端管理多语言服务器进程（stdio transport）
// 与图谱协同：图谱做粗粒度查询（符号索引），LSP 做精粒度操作（hover/定义/引用/重命名/格式化）
#![allow(dead_code)]

pub mod tools;

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, Mutex, RwLock};

// ==================== 类型定义 ====================

/// LSP 支持的语言（按文件后缀识别）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    TypeScript,
    TypeScriptReact,
    Go,
    Python,
    Unknown,
}

impl Language {
    /// 按文件后缀识别语言
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some("rs") => Language::Rust,
            Some("ts") => Language::TypeScript,
            Some("tsx") => Language::TypeScriptReact,
            Some("go") => Language::Go,
            Some("py") => Language::Python,
            _ => Language::Unknown,
        }
    }

    /// 返回该语言的 LSP server 启动命令（stdio transport）
    /// 若 server 未安装或未配置，返回 None
    fn server_command(&self) -> Option<Vec<&'static str>> {
        match self {
            Language::Rust => Some(vec!["rust-analyzer"]),
            Language::TypeScript | Language::TypeScriptReact => {
                Some(vec!["typescript-language-server", "--stdio"])
            }
            Language::Go => Some(vec!["gopls", "serve"]),
            Language::Python => Some(vec!["pylsp"]),
            Language::Unknown => None,
        }
    }

    /// 返回该语言在 LSP 中的 languageId
    fn language_id(&self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::TypeScript => "typescript",
            Language::TypeScriptReact => "typescriptreact",
            Language::Go => "go",
            Language::Python => "python",
            Language::Unknown => "plaintext",
        }
    }
}

/// LSP Position：0-based line/character
/// 注意：LSP 规范中 character 是 UTF-16 code unit 计数
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// LSP Range
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// LSP Location
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

// ==================== URI 工具函数 ====================

/// P2-4 修复：对路径的单个分量做 percent-encoding
/// 保留安全字符（A-Za-z0-9-_.~ 和路径分隔符 /），其余按 UTF-8 字节 percent-encode
/// file:// URI 规范见 RFC 8089
fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            // unreserved set (RFC 3986) + '/' (路径分隔符，不编码)
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(byte as char),
            // 其余全部 percent-encode（包括空格、中文、特殊符号等）
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// P2-4 修复：对 percent-encoded 路径做 decode
fn percent_decode_path(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            // 尝试解析两位 hex
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 将路径转为 file:// URI（跨平台）
/// - Unix: file:///absolute/path
/// - Windows: file:///C:/Users/foo/bar.rs
pub fn path_to_uri(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    // 跨平台：用 url crate 自动处理盘符和反斜杠
    // 简化实现：手动构造，确保三个斜杠 + 正斜杠
    let path_str = abs.to_string_lossy();
    #[cfg(unix)]
    {
        // Unix: file:// + /absolute/path = file:///absolute/path
        format!("file://{}", percent_encode_path(&path_str))
    }
    #[cfg(windows)]
    {
        // Windows: file:///C:/Users/foo/bar.rs
        // 把反斜杠转为正斜杠，并确保以 / 开头
        let normalized = path_str.replace('\\', "/");
        let with_slash = if normalized.starts_with('/') {
            normalized
        } else {
            format!("/{}", normalized)
        };
        format!("file://{}", percent_encode_path(&with_slash))
    }
}

/// 将 file:// URI 转回 PathBuf（跨平台）
pub fn uri_to_path(uri: &str) -> PathBuf {
    // 兼容 file:// 和 file:/// 两种前缀
    let rest = if let Some(r) = uri.strip_prefix("file:///") {
        r
    } else if let Some(r) = uri.strip_prefix("file://") {
        r
    } else {
        return PathBuf::from(uri);
    };
    let decoded = percent_decode_path(rest);
    #[cfg(windows)]
    {
        // Windows: 去掉前导 / （/C:/Users -> C:/Users），把正斜杠转反斜杠
        let trimmed = decoded.trim_start_matches('/');
        PathBuf::from(trimmed.replace('/', "\\"))
    }
    #[cfg(unix)]
    {
        PathBuf::from(decoded)
    }
}

// ==================== LspClient ====================

/// 单个 LSP server 进程的客户端
/// 一个 LspClient 对应一个语言服务器进程（stdio transport）
pub struct LspClient {
    /// 子进程句柄（kill_on_drop 保证 Drop 时进程被 kill）
    /// 用 Mutex 包裹以支持显式 shutdown
    process: Mutex<Option<Child>>,
    /// stdin 受 Mutex 保护（异步写入）
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    /// 下一个 JSON-RPC request id（自增）
    next_id: Arc<Mutex<u64>>,
    /// 等待响应的 oneshot channels：request id -> sender
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
    /// server capabilities（initialize 后填充）
    capabilities: Arc<RwLock<Option<Value>>>,
    /// 服务端推送的诊断（按文件 URI 索引）
    /// 注：传统 push 模式（textDocument/publishDiagnostics）
    diagnostics: Arc<RwLock<HashMap<String, Vec<Value>>>>,
    /// 已打开的文档（uri -> 当前文本），用于 didChange/didClose
    open_docs: Arc<Mutex<HashMap<String, String>>>,
    /// P1-4 修复：文档版本计数器（uri -> version）
    /// LSP 规范要求 version 严格单调递增，不能使用文本长度等启发式
    doc_versions: Arc<Mutex<HashMap<String, i32>>>,
    /// 项目根目录 URI（initialize 时传给 server）
    root_uri: String,
}

impl LspClient {
    /// 启动 LSP server 进程并完成 initialize 握手
    pub async fn new(language: Language, root_path: &Path) -> Result<Arc<Self>> {
        let cmd_args = language
            .server_command()
            .ok_or_else(|| anyhow::anyhow!("no LSP server configured for {:?}", language))?;

        let mut cmd = Command::new(&cmd_args[0]);
        if cmd_args.len() > 1 {
            cmd.args(&cmd_args[1..]);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawning LSP server: {}", cmd_args[0]))?;

        let stdin = child.stdin.take().context("failed to take stdin")?;
        let stdout = child.stdout.take().context("failed to take stdout")?;

        let client = Arc::new(Self {
            process: Mutex::new(Some(child)),
            stdin: Arc::new(Mutex::new(Some(stdin))),
            next_id: Arc::new(Mutex::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            capabilities: Arc::new(RwLock::new(None)),
            diagnostics: Arc::new(RwLock::new(HashMap::new())),
            open_docs: Arc::new(Mutex::new(HashMap::new())),
            doc_versions: Arc::new(Mutex::new(HashMap::new())),
            root_uri: path_to_uri(root_path),
        });

        // 启动 stdout reader 协程：解析 LSP 消息并分发
        let client_clone = client.clone();
        tokio::spawn(async move {
            if let Err(e) = client_clone.read_loop(stdout).await {
                tracing::warn!("LSP reader loop exited: {}", e);
            }
        });

        // 执行 initialize/initialized 握手
        client.initialize().await?;

        Ok(client)
    }

    /// stdout 读取循环：解析 Content-Length 头 + JSON-RPC payload，分发响应/通知
    async fn read_loop(self: Arc<Self>, stdout: ChildStdout) -> Result<()> {
        let mut reader = BufReader::new(stdout);
        loop {
            // 1. 读取 headers（每行形如 "Key: Value\r\n"，空行结束）
            let mut content_length: Option<usize> = None;
            loop {
                let mut line = String::new();
                let n = reader.read_line(&mut line).await?;
                if n == 0 {
                    // EOF：server 关闭了 stdout
                    tracing::debug!("LSP stdout EOF");
                    return Ok(());
                }
                let trimmed = line.trim_end_matches(|c| c == '\r' || c == '\n');
                if trimmed.is_empty() {
                    break; // 空行：header 结束，接下来是 body
                }
                if let Some(rest) = trimmed.strip_prefix("Content-Length: ") {
                    content_length = Some(rest.parse::<usize>()?);
                }
                // 忽略其他 header（如 Content-Type）
            }

            // 2. 读取 body（JSON-RPC payload）
            let len = content_length.context("missing Content-Length header")?;
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf).await?;
            let msg: Value = serde_json::from_slice(&buf)
                .with_context(|| "parsing LSP JSON-RPC message".to_string())?;

            // 3. 分发消息
            //    - 有 id + 有 method：server 发来的 request（如 workspace/configuration）
            //    - 有 id + 无 method：response（对应我们发出去的 request）
            //    - 无 id + 有 method：notification（如 textDocument/publishDiagnostics）
            if let Some(id) = msg.get("id") {
                if msg.get("method").is_some() {
                    // server -> client request：暂不处理，回 method not supported
                    // （实际场景：workspace/configuration, workspace/applyEdit 等）
                    tracing::debug!("ignoring server request: {:?}", msg.get("method"));
                } else {
                    // response：根据 id 找到等待中的 oneshot sender
                    let id_num = id.as_u64().or_else(|| {
                        id.as_str().and_then(|s| s.parse::<u64>().ok())
                    });
                    if let Some(id_num) = id_num {
                        let mut pending = self.pending.lock().await;
                        if let Some(tx) = pending.remove(&id_num) {
                            if let Some(err) = msg.get("error") {
                                let _ = tx.send(Err(anyhow::anyhow!(
                                    "LSP error response: {}",
                                    err
                                )));
                            } else {
                                let result = msg.get("result").cloned().unwrap_or(Value::Null);
                                let _ = tx.send(Ok(result));
                            }
                        }
                    }
                }
            } else if let Some(method) = msg.get("method").and_then(|v| v.as_str()) {
                // notification
                match method {
                    "textDocument/publishDiagnostics" => {
                        if let Some(params) = msg.get("params") {
                            let uri = params
                                .get("uri")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let diags = params
                                .get("diagnostics")
                                .cloned()
                                .unwrap_or(Value::Array(vec![]));
                            let diags_arr: Vec<Value> = match diags {
                                Value::Array(a) => a,
                                _ => vec![],
                            };
                            self.diagnostics.write().await.insert(uri, diags_arr);
                        }
                    }
                    _ => {
                        tracing::debug!("ignoring notification: {}", method);
                    }
                }
            }
        }
    }

    /// 分配下一个 request id（自增）
    async fn next_id(&self) -> u64 {
        let mut id = self.next_id.lock().await;
        let v = *id;
        *id += 1;
        v
    }

    /// 发送 JSON-RPC request 并等待 response（30s 超时）
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id().await;
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.send_message(payload).await?;

        let result = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| {
                anyhow::anyhow!("LSP request timeout: method={}, id={}", method, id)
            })??;
        result
    }

    /// 发送 JSON-RPC notification（无 id，无响应）
    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.send_message(payload).await
    }

    /// 底层发送：写 Content-Length 头 + body 到 stdin
    async fn send_message(&self, msg: Value) -> Result<()> {
        let body = serde_json::to_vec(&msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut guard = self.stdin.lock().await;
        let stdin = guard.as_mut().context("LSP stdin closed")?;
        stdin.write_all(header.as_bytes()).await?;
        stdin.write_all(&body).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// 执行 LSP initialize 握手
    /// 1. 发送 initialize request（声明 client capabilities）
    /// 2. 保存 server capabilities
    /// 3. 发送 initialized notification（完成握手）
    async fn initialize(&self) -> Result<()> {
        let params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": self.root_uri,
            "capabilities": {
                "textDocument": {
                    "synchronization": {
                        "didOpen": true,
                        "didChange": true,
                        "didClose": true,
                    },
                    "hover": {
                        "contentFormat": ["markdown", "plaintext"],
                    },
                    "definition": {
                        "linkSupport": false,
                    },
                    "references": {},
                    "rename": {
                        "prepareSupport": false,
                    },
                    "publishDiagnostics": {
                        "relatedInformation": true,
                    },
                    "diagnostic": {
                        "dynamicRegistration": false,
                    },
                    "formatting": {},
                },
                "workspace": {
                    "workspaceFolders": true,
                },
            },
        });
        let result = self.request("initialize", params).await?;
        {
            let mut caps = self.capabilities.write().await;
            *caps = Some(result);
        }
        // 发送 initialized notification 完成握手
        self.notify("initialized", serde_json::json!({})).await?;
        Ok(())
    }

    /// 获取 server capabilities
    pub async fn capabilities(&self) -> Option<Value> {
        self.capabilities.read().await.clone()
    }

    /// 打开文档：textDocument/didOpen
    /// 同时缓存文本到 open_docs，初始化 version 计数器为 1
    pub async fn did_open(
        &self,
        uri: &str,
        language_id: &str,
        text: &str,
    ) -> Result<()> {
        self.open_docs
            .lock()
            .await
            .insert(uri.to_string(), text.to_string());
        // P1-4 修复：version 从 1 开始，didChange 时单调递增
        self.doc_versions
            .lock()
            .await
            .insert(uri.to_string(), 1);
        let params = serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": 1,
                "text": text,
            }
        });
        self.notify("textDocument/didOpen", params).await
    }

    /// 修改文档：textDocument/didChange（全量替换）
    /// P1-4 修复：version 使用独立计数器，严格单调递增
    pub async fn did_change(&self, uri: &str, new_text: &str) -> Result<()> {
        let version = {
            let mut versions = self.doc_versions.lock().await;
            let v = versions.entry(uri.to_string()).or_insert(1);
            *v += 1;
            *v
        };
        self.open_docs
            .lock()
            .await
            .insert(uri.to_string(), new_text.to_string());
        let params = serde_json::json!({
            "textDocument": { "uri": uri, "version": version },
            "contentChanges": [{ "text": new_text }],
        });
        self.notify("textDocument/didChange", params).await
    }

    /// 关闭文档：textDocument/didClose
    pub async fn did_close(&self, uri: &str) -> Result<()> {
        self.open_docs.lock().await.remove(uri);
        self.doc_versions.lock().await.remove(uri);
        let params = serde_json::json!({
            "textDocument": { "uri": uri }
        });
        self.notify("textDocument/didClose", params).await
    }

    /// textDocument/hover
    pub async fn hover(&self, uri: &str, line: u32, character: u32) -> Result<Value> {
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        });
        self.request("textDocument/hover", params).await
    }

    /// textDocument/definition
    pub async fn definition(&self, uri: &str, line: u32, character: u32) -> Result<Value> {
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character }
        });
        self.request("textDocument/definition", params).await
    }

    /// textDocument/references
    pub async fn references(&self, uri: &str, line: u32, character: u32) -> Result<Value> {
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": true }
        });
        self.request("textDocument/references", params).await
    }

    /// textDocument/rename - 返回 WorkspaceEdit
    pub async fn rename(
        &self,
        uri: &str,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> Result<Value> {
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "newName": new_name
        });
        self.request("textDocument/rename", params).await
    }

    /// textDocument/formatting - 返回 TextEdit[]
    pub async fn formatting(&self, uri: &str) -> Result<Value> {
        let params = serde_json::json!({
            "textDocument": { "uri": uri },
            "options": {
                "tabSize": 4,
                "insertSpaces": true,
            }
        });
        self.request("textDocument/formatting", params).await
    }

    /// 获取文件诊断
    /// 优先返回 push 推送的（多数 server 走 textDocument/publishDiagnostics）
    /// 若无，则尝试 pull diagnostics (LSP 3.17+ textDocument/diagnostic)
    pub async fn diagnostics(&self, uri: &str) -> Result<Vec<Value>> {
        // 1. 检查 push 缓存
        let pushed = self.diagnostics.read().await.get(uri).cloned();
        if let Some(d) = pushed {
            return Ok(d);
        }
        // 2. 尝试 pull diagnostics（LSP 3.17+）
        //    返回 Either Diagnostic[] | FullDocumentDiagnosticReport | None
        let params = serde_json::json!({
            "textDocument": { "uri": uri }
        });
        match self.request("textDocument/diagnostic", params).await {
            Ok(result) => {
                // FullDocumentDiagnosticReport: { kind: "full", items: [...] }
                if let Some(arr) = result.get("items").and_then(|v| v.as_array()) {
                    return Ok(arr.clone());
                }
                // RelatedFullDocumentDiagnosticReport: { relatedDocuments: ..., items: [...] }
                // 或者直接是 Diagnostic[]
                if let Some(arr) = result.as_array() {
                    return Ok(arr.clone());
                }
                Ok(vec![])
            }
            Err(e) => {
                // server 不支持 pull diagnostics：返回空数组
                tracing::debug!("pull diagnostics failed for {}: {}", uri, e);
                Ok(vec![])
            }
        }
    }

    /// 优雅关闭：发送 shutdown + exit，然后 kill 进程
    pub async fn shutdown(&self) {
        // shutdown request（spec 要求带 params: null）
        let _ = self.request("shutdown", Value::Null).await;
        // exit notification
        let _ = self.notify("exit", Value::Null).await;
        // 显式 kill（kill_on_drop 也会兜底）
        let mut guard = self.process.lock().await;
        if let Some(mut child) = guard.take() {
            let _ = child.start_kill();
        }
    }
}

// ==================== LspManager ====================

/// LSP 客户端管理器：按语言维护多个 LSP server 进程
/// 设计文档 §8.4.2：server 端管理多语言服务器进程
pub struct LspManager {
    /// 按 Language 索引的 LspClient（懒启动，首次使用时创建）
    clients: Mutex<HashMap<Language, Arc<LspClient>>>,
    /// 项目根目录（root_uri 传给每个 LSP server）
    root: PathBuf,
}

impl LspManager {
    pub fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            clients: Mutex::new(HashMap::new()),
            root,
        })
    }

    /// 获取（或按需启动）某文件对应的 LspClient
    /// 若文件后缀不支持，返回 Ok(None)
    /// 若 server 启动失败，返回 Err
    pub async fn client_for(&self, path: &Path) -> Result<Option<Arc<LspClient>>> {
        let lang = Language::from_path(path);
        if lang == Language::Unknown {
            return Ok(None);
        }
        // 先查缓存
        {
            let clients = self.clients.lock().await;
            if let Some(c) = clients.get(&lang) {
                return Ok(Some(c.clone()));
            }
        }
        // 启动新 client（持锁期间启动，避免重复启动）
        let mut clients = self.clients.lock().await;
        // double-check：可能在等锁期间已被其他任务启动
        if let Some(c) = clients.get(&lang) {
            return Ok(Some(c.clone()));
        }
        match LspClient::new(lang, &self.root).await {
            Ok(c) => {
                tracing::info!("LSP server started for {:?}", lang);
                clients.insert(lang, c.clone());
                Ok(Some(c))
            }
            Err(e) => {
                tracing::warn!("failed to start LSP server for {:?}: {}", lang, e);
                Err(e)
            }
        }
    }

    /// 便捷方法：确保文件已在 LSP server 中 didOpen
    /// 返回对应的 LspClient（若语言不支持则返回 None）
    /// P2-1 修复：使用 open_docs 的 lock 作为文件级锁，避免并发 didOpen
    pub async fn ensure_open(&self, path: &Path) -> Result<Option<Arc<LspClient>>> {
        let client = self.client_for(path).await?;
        if let Some(c) = &client {
            let uri = path_to_uri(path);
            let lang = Language::from_path(path);
            // P2-1 修复：持锁期间检查 + didOpen，避免竞态
            // did_open 内部会获取 open_docs lock，为避免死锁，这里先检查再释放锁
            let need_open = {
                let docs = c.open_docs.lock().await;
                !docs.contains_key(&uri)
            };
            if need_open {
                let text = tokio::fs::read_to_string(path)
                    .await
                    .with_context(|| format!("reading {}", path.display()))?;
                // 再次检查（可能在读文件期间已被其他任务打开）
                let need_open2 = {
                    let docs = c.open_docs.lock().await;
                    !docs.contains_key(&uri)
                };
                if need_open2 {
                    c.did_open(&uri, lang.language_id(), &text).await?;
                }
            }
        }
        Ok(client)
    }

    /// 关闭所有已启动的 LSP client（优雅关闭）
    pub async fn shutdown_all(&self) {
        let mut clients = self.clients.lock().await;
        for (lang, c) in clients.drain() {
            tracing::info!("shutting down LSP server for {:?}", lang);
            c.shutdown().await;
        }
    }

    /// Returns the list of active (started) LSP server language names
    pub async fn active_languages(&self) -> Vec<String> {
        let clients = self.clients.lock().await;
        clients.keys().map(|lang| lang.language_id().to_string()).collect()
    }
}

// ==================== TextEdit 应用工具 ====================

/// 将 LSP TextEdit[] 应用到文本上，返回新文本
/// 处理 changes: Map<uri, TextEdit[]> 形式的 WorkspaceEdit.changes
/// 算法：将 range 转为字节偏移，倒序应用（避免位置偏移）
pub fn apply_text_edits(text: &str, edits: &Value) -> String {
    // 预计算每行起始字节偏移
    let mut line_starts: Vec<usize> = vec![0];
    for (i, c) in text.char_indices() {
        if c == '\n' {
            line_starts.push(i + 1);
        }
    }

    // 收集 (start_byte, end_byte, new_text)
    let mut byte_edits: Vec<(usize, usize, String)> = Vec::new();
    if let Some(arr) = edits.as_array() {
        for e in arr {
            let range = match e.get("range") {
                Some(r) => r,
                None => continue,
            };
            let new_text = e
                .get("newText")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sl = range["start"]["line"].as_u64().unwrap_or(0) as usize;
            let sc = range["start"]["character"].as_u64().unwrap_or(0) as usize;
            let el = range["end"]["line"].as_u64().unwrap_or(0) as usize;
            let ec = range["end"]["character"].as_u64().unwrap_or(0) as usize;

            // LSP character 是 UTF-16 code unit 计数
            // 这里转成 UTF-8 字节偏移
            let start_byte = line_start_byte(&line_starts, sl, text)
                + utf16_char_to_byte(text, sl, sc);
            let end_byte = line_start_byte(&line_starts, el, text)
                + utf16_char_to_byte(text, el, ec);
            byte_edits.push((start_byte, end_byte, new_text));
        }
    }

    // 倒序应用（从后往前，避免前面的 edit 影响后面的字节偏移）
    byte_edits.sort_by(|a, b| b.0.cmp(&a.0));
    let mut result = text.to_string();
    for (start, end, new_text) in byte_edits {
        // 边界保护
        let s = start.min(result.len());
        let e = end.min(result.len()).max(s);
        result.replace_range(s..e, &new_text);
    }
    result
}

/// 获取第 line 行的起始字节偏移（line 超出时返回 text.len()）
fn line_start_byte(line_starts: &[usize], line: usize, text: &str) -> usize {
    line_starts.get(line).copied().unwrap_or_else(|| text.len() + 1)
}

/// 将 LSP (line, character) 中的 character（UTF-16 code unit 计数）转为该行的字节偏移
/// 简化处理：对 BMP 字符（绝大多数代码）UTF-16 == UTF-8 char count
fn utf16_char_to_byte(text: &str, line: usize, character: usize) -> usize {
    // 找到第 line 行的字符串切片
    let mut line_starts: Vec<usize> = vec![0];
    for (i, c) in text.char_indices() {
        if c == '\n' {
            line_starts.push(i + 1);
        }
    }
    let line_start = line_starts.get(line).copied().unwrap_or(text.len());
    let line_end = line_starts
        .get(line + 1)
        .copied()
        .unwrap_or(text.len() + 1)
        .saturating_sub(1)
        .min(text.len());
    let line_text = &text[line_start..line_end.min(text.len())];

    // 遍历字符，累计 UTF-16 code units，到达 character 时返回对应字节偏移
    let mut utf16_count = 0usize;
    let mut byte_offset = 0usize;
    for c in line_text.chars() {
        if utf16_count >= character {
            break;
        }
        utf16_count += c.len_utf16();
        byte_offset += c.len_utf8();
    }
    byte_offset
}
