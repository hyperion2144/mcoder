// 设计文档 §8.4.3: DAP 调试子系统
// 实现 Debug Adapter Protocol 客户端，支持 Rust/Node/Python/Go
// forward-looking scaffolding: 部分扩展 API 暂未在工具层使用
#![allow(dead_code)]

pub mod tools;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex, RwLock};
use tokio::task::JoinHandle;

/// DAP 支持的调试语言
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugLang {
    Rust,
    Node,
    Python,
    Go,
}

impl DebugLang {
    /// 从字符串解析语言
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "rust" | "rs" => Ok(Self::Rust),
            "node" | "nodejs" | "javascript" | "js" => Ok(Self::Node),
            "python" | "py" => Ok(Self::Python),
            "go" | "golang" => Ok(Self::Go),
            other => bail!("unsupported debug language: {} (supported: rust/node/python/go)", other),
        }
    }

    /// 选择对应的 adapter 启动命令
    /// rust → lldb-dap
    /// node → node（内置 inspector，但 DAP 通常用 vscode-js-debug 或内置 node --inspect）
    /// 这里 node adapter 用 node 自身启动内置 adapter（需 lldb-dap 风格）
    /// python → python -m debugpy.adapter
    /// go → dlv dap
    fn adapter_command(&self) -> Vec<String> {
        match self {
            Self::Rust => vec!["lldb-dap".to_string()],
            // node 内置支持 inspector；vscode-js-debug 通常通过命令行启动
            // 这里假设 PATH 中存在 node（实际 adapter 通常独立安装）
            Self::Node => vec!["node".to_string()],
            Self::Python => vec![
                "python3".to_string(),
                "-m".to_string(),
                "debugpy.adapter".to_string(),
            ],
            Self::Go => vec!["dlv".to_string(), "dap".to_string()],
        }
    }

    /// adapterID 字段（initialize 请求需要）
    fn adapter_id(&self) -> &'static str {
        match self {
            Self::Rust => "lldb",
            Self::Node => "node",
            Self::Python => "debugpy",
            Self::Go => "go",
        }
    }
}

/// 调试会话启动配置
#[derive(Debug, Clone)]
pub struct DebugConfig {
    pub lang: DebugLang,
    /// 可执行文件路径（launch 模式）
    pub program: String,
    /// 命令行参数
    pub args: Vec<String>,
    /// 工作目录
    pub cwd: Option<String>,
    /// 是否在入口暂停
    pub stop_on_entry: bool,
    /// attach 模式（attach 到已运行进程）；None 表示 launch
    pub attach_pid: Option<u32>,
}

/// 调试会话状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// 已初始化但未启动
    Initialized,
    /// 已启动并运行中
    Running,
    /// 已停止在断点/步进
    Stopped,
    /// 已终止
    Terminated,
}

/// DAP 协议中的断点信息
#[derive(Debug, Clone, Serialize)]
pub struct BreakpointInfo {
    pub id: Option<i64>,
    pub verified: bool,
    pub line: i64,
    pub message: Option<String>,
}

/// 调用栈帧
#[derive(Debug, Clone, Serialize)]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    pub file: Option<String>,
    pub line: i64,
    pub column: i64,
}

/// 变量
#[derive(Debug, Clone, Serialize)]
pub struct VariableInfo {
    pub name: String,
    pub value: String,
    #[serde(rename = "type")]
    pub var_type: Option<String>,
    pub variables_reference: i64,
}

/// 调试会话当前状态摘要
#[derive(Debug, Clone, Serialize)]
pub struct DebugState {
    pub stopped: bool,
    pub thread_id: Option<i64>,
    pub frames: Vec<StackFrame>,
    pub variables: Vec<VariableInfo>,
    pub terminated: bool,
}

/// 一条 DAP 消息的原始 JSON 包装
/// type: "request" | "response" | "event"
#[derive(Debug, Clone)]
pub enum DapMessage {
    Request {
        seq: i64,
        command: String,
        arguments: Option<Value>,
    },
    Response {
        request_seq: i64,
        success: bool,
        message: Option<String>,
        body: Option<Value>,
    },
    Event {
        event: String,
        body: Option<Value>,
    },
}

/// 一个活跃的 DAP 调试会话
/// 对应一个 adapter 子进程
pub struct DebugSession {
    /// adapter 子进程（Mutex<Option<Child>> 支持显式 kill）
    child: Mutex<Option<Child>>,
    /// 进程 stdin（用 Mutex 保护并发写入）
    stdin: Mutex<tokio::process::ChildStdin>,
    /// 当前会话状态
    state: RwLock<SessionState>,
    /// 当前停止的 thread id（stopped 事件提供）
    current_thread: RwLock<Option<i64>>,
    /// 启动配置（保留以便重启）
    config: DebugConfig,
    /// 等待响应的 oneshot 表
    /// key = request seq, value = oneshot sender
    pending: Mutex<HashMap<i64, oneshot::Sender<Result<Value>>>>,
    /// 输出事件累积（output 事件的 stdout/stderr）
    output: Mutex<String>,
    /// 全局事件广播（stopped / terminated）
    /// 用 watch channel 保证最新状态可读取
    event_rx: RwLock<Option<tokio::sync::watch::Receiver<String>>>,
    /// seq 生成器
    seq: AtomicU64,
    /// 文件断点累积：file -> Vec<(line, condition)>
    /// DAP setBreakpoints 语义是替换该文件全部断点，为支持 debug_set_breakpoint 追加语义，这里维护已设置的断点列表
    file_breakpoints: RwLock<HashMap<String, Vec<(i64, Option<String>)>>>,
    /// P2-9 修复：reader 任务的 JoinHandle
    /// stop() 中 kill child 后 await 此 handle（带超时），确保 reader 任务退出释放 Arc
    /// 避免 reader 持有 Arc 导致 DebugSession 无法 drop
    reader_handle: Mutex<Option<JoinHandle<()>>>,
}

impl DebugSession {
    /// 启动 adapter 进程并完成 DAP 握手
    pub async fn start(config: DebugConfig) -> Result<Arc<Self>> {
        // 1. 检查 adapter 可用性
        let cmd_parts = config.lang.adapter_command();
        let adapter_name = cmd_parts[0].clone();
        if which_adapter(&adapter_name).await.is_none() {
            bail!(
                "debug adapter '{}' not found in PATH; please install it first (lang={:?})",
                adapter_name,
                config.lang
            );
        }

        // 2. 启动 adapter 子进程（kill_on_drop 防止泄漏）
        let mut cmd = Command::new(&cmd_parts[0]);
        if cmd_parts.len() > 1 {
            cmd.args(&cmd_parts[1..]);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // P1-3 修复：stderr 重定向到 null，避免 adapter 写 stderr 时管道阻塞
            // （若需要 stderr 日志，应启动 reader 任务消费，而非 take 后丢弃）
            .stderr(Stdio::null())
            .kill_on_drop(true);

        // macOS 下需要继承 PATH 以便 adapter 找到子工具
        cmd.env("PATH", std::env::var("PATH").unwrap_or_default());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn debug adapter: {}", adapter_name))?;

        let stdin = child
            .stdin
            .take()
            .context("adapter stdin not available")?;
        let stdout = child
            .stdout
            .take()
            .context("adapter stdout not available")?;
        // stderr 已重定向到 null，无需 take

        let session = Arc::new(Self {
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(stdin),
            state: RwLock::new(SessionState::Initialized),
            current_thread: RwLock::new(None),
            config: config.clone(),
            pending: Mutex::new(HashMap::new()),
            output: Mutex::new(String::new()),
            event_rx: RwLock::new(None),
            seq: AtomicU64::new(1),
            file_breakpoints: RwLock::new(HashMap::new()),
            reader_handle: Mutex::new(None),
        });

        // 3. 启动 reader 任务（处理 stdout）
        // reader 任务负责解析 DAP 消息并分发到 pending oneshot 或事件处理
        // P2-9 修复：reader 持有 session 的 Arc 克隆，session 不会立即 drop
        // stop() 会显式 kill child 并 close stdin，reader 检测到 EOF 后退出
        // stop() 中 await reader_handle 确保 reader 退出后释放 Arc
        let session_for_reader = session.clone();
        let (event_tx, event_rx) = tokio::sync::watch::channel("init".to_string());
        let handle = tokio::spawn(async move {
            session_for_reader.run_reader(stdout, event_tx).await;
        });
        // P2-9 修复：保存 JoinHandle，stop() 中 await 它
        *session.reader_handle.lock().await = Some(handle);
        // 注册事件 watch receiver
        *session.event_rx.write().await = Some(event_rx);

        // 4. initialize 请求
        let init_args = serde_json::json!({
            "clientID": "mcoder",
            "clientName": "mcoder DAP client",
            "adapterID": config.lang.adapter_id(),
            "locale": "en-US",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "pathFormat": "path",
            "supportsVariableType": true,
            "supportsRunInTerminalRequest": false,
        });
        session.send_request("initialize", Some(init_args)).await?;

        // 5. launch 或 attach
        if let Some(pid) = config.attach_pid {
            let attach_args = serde_json::json!({
                "program": config.program,
                "pid": pid,
                "stopOnEntry": config.stop_on_entry,
                "cwd": config.cwd,
                "args": config.args,
            });
            session.send_request("attach", Some(attach_args)).await?;
        } else {
            let launch_args = serde_json::json!({
                "program": config.program,
                "args": config.args,
                "cwd": config.cwd,
                "stopOnEntry": config.stop_on_entry,
            });
            session.send_request("launch", Some(launch_args)).await?;
        }

        // 6. configurationDone
        let done_args = serde_json::json!({});
        session.send_request("configurationDone", Some(done_args)).await?;

        // 7. 状态：若 stop_on_entry，会收到 stopped 事件，否则 running
        *session.state.write().await = SessionState::Running;

        Ok(session)
    }

    /// reader 任务主循环
    /// 解析 DAP 消息帧并分发到 pending oneshot 或事件处理
    async fn run_reader(
        self: Arc<Self>,
        stdout: tokio::process::ChildStdout,
        event_tx: tokio::sync::watch::Sender<String>,
    ) {
        let mut reader = BufReader::new(stdout);
        let mut header = String::new();

        loop {
            header.clear();
            // 读取 header（Content-Length: N\r\n\r\n）
            loop {
                let n = match reader.read_line(&mut header).await {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::debug!("DAP reader: read_line error: {}", e);
                        return;
                    }
                };
                if n == 0 {
                    // EOF
                    return;
                }
                // 空行表示 header 结束
                if header.ends_with("\r\n\r\n") || header == "\r\n" {
                    break;
                }
                // 防止恶意 header
                if header.len() > 8192 {
                    tracing::warn!("DAP reader: header too long, aborting");
                    return;
                }
            }

            // 解析 Content-Length
            let mut content_len: Option<usize> = None;
            for line in header.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("Content-Length:") {
                    if let Ok(n) = rest.trim().parse::<usize>() {
                        content_len = Some(n);
                    }
                }
            }
            let content_len = match content_len {
                Some(n) => n,
                None => {
                    tracing::warn!("DAP reader: missing Content-Length header: {:?}", header);
                    continue;
                }
            };

            // 读取消息体
            let mut buf = vec![0u8; content_len];
            if let Err(e) = reader.read_exact(&mut buf).await {
                tracing::debug!("DAP reader: read_exact error: {}", e);
                return;
            }
            let body_str = match std::str::from_utf8(&buf) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let msg: Value = match serde_json::from_str(body_str) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("DAP reader: invalid JSON: {}", e);
                    continue;
                }
            };

            // 分发消息
            self.handle_message(msg, &event_tx).await;
        }
    }

    /// 处理一条 DAP 消息
    async fn handle_message(&self, msg: Value, event_tx: &tokio::sync::watch::Sender<String>) {
        let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match msg_type {
            "response" => {
                let request_seq = msg
                    .get("request_seq")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(-1);
                let success = msg
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let message = msg
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let body = msg.get("body").cloned();

                let mut pending = self.pending.lock().await;
                if let Some(tx) = pending.remove(&request_seq) {
                    let result = if success {
                        Ok(body.unwrap_or(Value::Null))
                    } else {
                        Err(anyhow!(
                            "DAP request failed: {}",
                            message.unwrap_or_else(|| "unknown error".to_string())
                        ))
                    };
                    // oneshot send 失败说明 receiver 已被 drop，忽略
                    let _ = tx.send(result);
                }
            }
            "event" => {
                let event = msg
                    .get("event")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let body = msg.get("body").cloned();
                self.handle_event(&event, body, event_tx).await;
            }
            _ => {
                tracing::debug!("DAP reader: ignoring unknown message type: {}", msg_type);
            }
        }
    }

    /// 处理 DAP 事件
    async fn handle_event(
        &self,
        event: &str,
        body: Option<Value>,
        event_tx: &tokio::sync::watch::Sender<String>,
    ) {
        match event {
            "stopped" => {
                let thread_id = body
                    .as_ref()
                    .and_then(|b| b.get("threadId"))
                    .and_then(|v| v.as_i64());
                let reason = body
                    .as_ref()
                    .and_then(|b| b.get("reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                *self.current_thread.write().await = thread_id;
                *self.state.write().await = SessionState::Stopped;
                let _ = event_tx.send(format!("stopped:{}", reason));
                tracing::debug!("DAP stopped: reason={} thread={:?}", reason, thread_id);
            }
            "terminated" | "exited" => {
                *self.state.write().await = SessionState::Terminated;
                *self.current_thread.write().await = None;
                let _ = event_tx.send("terminated".to_string());
                tracing::debug!("DAP session terminated");
            }
            "output" => {
                // 累积 output 事件（stdout/stderr）
                if let Some(b) = &body {
                    let category = b
                        .get("category")
                        .and_then(|v| v.as_str())
                        .unwrap_or("console");
                    let text = b.get("output").and_then(|v| v.as_str()).unwrap_or("");
                    if !text.is_empty() {
                        let mut out = self.output.lock().await;
                        // 限制累积大小，避免无限增长
                        if out.len() < 1024 * 1024 {
                            out.push_str(&format!("[{}] {}", category, text));
                        }
                    }
                }
            }
            "thread" | "breakpoint" | "module" | "loadedSource" => {
                // 这些事件目前不处理，仅记录 debug 日志
                tracing::trace!("DAP event {} (ignored): {:?}", event, body);
            }
            _ => {
                tracing::trace!("DAP unknown event: {} body={:?}", event, body);
            }
        }
    }

    /// 发送 DAP 请求，等待响应
    pub async fn send_request(
        &self,
        command: &str,
        arguments: Option<Value>,
    ) -> Result<Value> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) as i64;
        let mut req = serde_json::json!({
            "seq": seq,
            "type": "request",
            "command": command,
        });
        if let Some(args) = arguments {
            req["arguments"] = args;
        }

        let json = serde_json::to_string(&req)
            .with_context(|| format!("serializing DAP request: {}", command))?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);

        // 注册 pending oneshot
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(seq, tx);
        }

        // 写入 stdin
        {
            let mut stdin = self.stdin.lock().await;
            if let Err(e) = stdin.write_all(frame.as_bytes()).await {
                // 写入失败：清理 pending
                let mut pending = self.pending.lock().await;
                pending.remove(&seq);
                bail!("DAP write failed (command={}): {}", command, e);
            }
            let _ = stdin.flush().await;
        }

        // 等待响应
        match tokio::time::timeout(
            std::time::Duration::from_secs(60),
            rx,
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                // sender 被 drop（reader 任务退出）
                let mut pending = self.pending.lock().await;
                pending.remove(&seq);
                bail!("DAP response channel closed (command={})", command)
            }
            Err(_) => {
                // 超时：清理 pending
                let mut pending = self.pending.lock().await;
                pending.remove(&seq);
                bail!("DAP request timeout (command={}, seq={})", command, seq)
            }
        }
    }

    /// 设置断点（追加语义）
    /// 设计文档 §8.4.3: debug_set_breakpoint 工具是单数语义（追加一个断点）
    /// 但 DAP setBreakpoints 是替换语义，这里通过 file_breakpoints 维护已设置断点列表
    /// 每次调用将新断点追加到列表，然后全量发送给 adapter
    /// 返回新设置断点的 id 和 verified 状态
    pub async fn set_breakpoints(
        &self,
        file: &str,
        breakpoints: Vec<(i64, Option<String>)>,
    ) -> Result<Vec<BreakpointInfo>> {
        // P2-2 修复：追加到已存在断点列表
        let all_bps = {
            let mut bps_map = self.file_breakpoints.write().await;
            let existing = bps_map.entry(file.to_string()).or_default();
            for bp in &breakpoints {
                // 避免重复添加同行的断点
                if !existing.iter().any(|(l, _)| l == &bp.0) {
                    existing.push(bp.clone());
                }
            }
            existing.clone()
        };

        let bp_json: Vec<Value> = all_bps
            .iter()
            .map(|(line, cond)| {
                let mut b = serde_json::json!({ "line": line });
                if let Some(c) = cond {
                    b["condition"] = serde_json::Value::String(c.clone());
                }
                b
            })
            .collect();

        let args = serde_json::json!({
            "source": { "path": file },
            "breakpoints": bp_json,
            "lines": all_bps.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
        });

        let resp = self.send_request("setBreakpoints", Some(args)).await?;
        let body = resp.get("breakpoints").cloned().unwrap_or(Value::Array(vec![]));
        let arr = body.as_array().cloned().unwrap_or_default();

        let mut result = Vec::new();
        for (i, bp) in arr.iter().enumerate() {
            let id = bp.get("id").and_then(|v| v.as_i64());
            let verified = bp.get("verified").and_then(|v| v.as_bool()).unwrap_or(false);
            let line = bp
                .get("line")
                .and_then(|v| v.as_i64())
                .unwrap_or_else(|| all_bps.get(i).map(|(l, _)| *l).unwrap_or(0));
            let message = bp
                .get("message")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            result.push(BreakpointInfo {
                id,
                verified,
                line,
                message,
            });
        }
        // 只返回本次新增断点对应的结果
        let new_count = breakpoints.len();
        let start = result.len().saturating_sub(new_count);
        Ok(result[start..].to_vec())
    }

    /// 清除文件的所有断点
    pub async fn clear_breakpoints(&self, file: &str) -> Result<()> {
        self.file_breakpoints.write().await.remove(file);
        // 发送空断点列表给 adapter
        let args = serde_json::json!({
            "source": { "path": file },
            "breakpoints": [],
            "lines": [],
        });
        let _ = self.send_request("setBreakpoints", Some(args)).await;
        Ok(())
    }

    /// continue - 继续执行
    pub async fn continue_exec(&self) -> Result<()> {
        let thread_id = self
            .current_thread()
            .await
            .ok_or_else(|| anyhow!("no current thread to continue"))?;
        let args = serde_json::json!({ "threadId": thread_id });
        self.send_request("continue", Some(args)).await?;
        *self.state.write().await = SessionState::Running;
        Ok(())
    }

    /// 单步执行（granularity: over/in/out）
    pub async fn step(&self, granularity: &str) -> Result<()> {
        let thread_id = self
            .current_thread()
            .await
            .ok_or_else(|| anyhow!("no current thread to step"))?;
        let command = match granularity {
            "in" => "stepIn",
            "out" => "stepOut",
            _ => "next",
        };
        let args = serde_json::json!({
            "threadId": thread_id,
            "granularity": "statement",
        });
        self.send_request(command, Some(args)).await?;
        *self.state.write().await = SessionState::Running;
        Ok(())
    }

    /// 求值表达式
    /// frame_id: 调用栈帧 id（来自 stackTrace）
    pub async fn evaluate(&self, expression: &str, frame_id: Option<i64>) -> Result<String> {
        let mut args = serde_json::json!({
            "expression": expression,
            "context": "watch",
        });
        if let Some(fid) = frame_id {
            args["frameId"] = serde_json::Value::Number(fid.into());
        }
        let resp = self.send_request("evaluate", Some(args)).await?;
        let result = resp
            .get("result")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(result)
    }

    /// 获取调用栈
    pub async fn stack_trace(&self) -> Result<Vec<StackFrame>> {
        let thread_id = self
            .current_thread()
            .await
            .ok_or_else(|| anyhow!("no current thread (not stopped)"))?;
        let args = serde_json::json!({
            "threadId": thread_id,
            "startFrame": 0,
            "levels": 20,
        });
        let resp = self.send_request("stackTrace", Some(args)).await?;
        let frames_json = resp
            .get("stackFrames")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        let arr = frames_json.as_array().cloned().unwrap_or_default();

        let mut frames = Vec::new();
        for f in arr {
            let id = f.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let name = f
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let line = f.get("line").and_then(|v| v.as_i64()).unwrap_or(0);
            let column = f.get("column").and_then(|v| v.as_i64()).unwrap_or(0);
            let file = f
                .get("source")
                .and_then(|s| s.get("path"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            frames.push(StackFrame {
                id,
                name,
                file,
                line,
                column,
            });
        }
        Ok(frames)
    }

    /// 获取当前栈帧的变量（先取 scopes，再取第一个 scope 的 variables）
    pub async fn get_variables(&self, frame_id: Option<i64>) -> Result<Vec<VariableInfo>> {
        // 若未提供 frame_id，自动取栈顶
        let frame_id = if let Some(fid) = frame_id {
            fid
        } else {
            let frames = self.stack_trace().await?;
            frames
                .first()
                .map(|f| f.id)
                .ok_or_else(|| anyhow!("no stack frame available"))?
        };

        // scopes
        let scope_args = serde_json::json!({ "frameId": frame_id });
        let scope_resp = self.send_request("scopes", Some(scope_args)).await?;
        let scopes = scope_resp
            .get("scopes")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        let scopes_arr = scopes.as_array().cloned().unwrap_or_default();

        // 取第一个非 expensive scope 的 variables
        let mut all_vars = Vec::new();
        for scope in &scopes_arr {
            let var_ref = scope
                .get("variablesReference")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if var_ref == 0 {
                continue;
            }
            let var_args = serde_json::json!({ "variablesReference": var_ref });
            let var_resp = self.send_request("variables", Some(var_args)).await?;
            let vars_json = var_resp
                .get("variables")
                .cloned()
                .unwrap_or(Value::Array(vec![]));
            let vars_arr = vars_json.as_array().cloned().unwrap_or_default();
            for v in vars_arr {
                let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let value = v.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let var_type = v
                    .get("type")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                let variables_reference = v
                    .get("variablesReference")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0);
                all_vars.push(VariableInfo {
                    name,
                    value,
                    var_type,
                    variables_reference,
                });
            }
            // 通常只取第一个 scope（Locals）即可
            if !all_vars.is_empty() {
                break;
            }
        }
        Ok(all_vars)
    }

    /// 获取当前调试状态摘要
    pub async fn get_state(&self) -> DebugState {
        let state = self.state.read().await.clone();
        let thread_id = self.current_thread.read().await.clone();

        let (stopped, terminated) = match state {
            SessionState::Stopped => (true, false),
            SessionState::Terminated => (false, true),
            _ => (false, false),
        };

        // 仅在 stopped 时尝试获取栈帧和变量
        let (frames, variables) = if stopped {
            let frames = self.stack_trace().await.unwrap_or_default();
            let frame_id = frames.first().map(|f| f.id);
            let vars = self.get_variables(frame_id).await.unwrap_or_default();
            (frames, vars)
        } else {
            (vec![], vec![])
        };

        DebugState {
            stopped,
            thread_id,
            frames,
            variables,
            terminated,
        }
    }

    /// 取累积的 output 文本
    pub async fn get_output(&self) -> String {
        self.output.lock().await.clone()
    }

    /// 当前 thread id
    pub async fn current_thread(&self) -> Option<i64> {
        self.current_thread.read().await.clone()
    }

    /// 当前状态
    pub async fn state(&self) -> SessionState {
        self.state.read().await.clone()
    }

    /// 等待下一个 stopped/terminated 事件
    /// 用 watch channel 的 changed 等待
    pub async fn wait_next_event(&self) -> Result<String> {
        let rx_guard = self.event_rx.read().await;
        let mut rx = match rx_guard.as_ref() {
            Some(r) => r.clone(),
            None => bail!("event channel not initialized"),
        };
        drop(rx_guard);
        match rx.changed().await {
            Ok(_) => Ok(rx.borrow().clone()),
            Err(_) => bail!("event channel closed"),
        }
    }

    /// 停止调试会话：发送 disconnect 请求并强制 kill adapter
    /// P1-1 修复：显式 kill child 进程，不依赖 kill_on_drop（因 reader 持有 Arc 克隆）
    /// P2-9 修复：kill child 后 await reader JoinHandle，确保 reader 任务退出释放 Arc
    pub async fn stop(&self) -> Result<()> {
        // 标记为 terminated，阻止后续请求
        *self.state.write().await = SessionState::Terminated;

        // 发送 disconnect 请求（忽略响应，adapter 可能已退出）
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.send_request("disconnect", Some(serde_json::json!({}))),
        ).await;

        // P1-1 修复：显式 kill child 进程
        // 不依赖 kill_on_drop，因为 reader 任务持有 Arc 克隆，session 不会立即 drop
        let mut child_guard = self.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            // 先尝试优雅 kill（SIGTERM）
            let _ = child.start_kill();
            // 等待最多 2 秒，超时则强制 kill
            match tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await {
                Ok(_) => {},
                Err(_) => {
                    // 超时，强制 kill（kill_on_drop 也会兜底）
                    let _ = child.kill().await;
                }
            }
        }
        drop(child_guard);

        // 清空 pending，让等待中的请求立即失败
        let pending: HashMap<i64, oneshot::Sender<Result<Value>>> =
            std::mem::take(&mut *self.pending.lock().await);
        for (_, tx) in pending {
            let _ = tx.send(Err(anyhow!("debug session stopped")));
        }

        // P2-9 修复：await reader 任务退出
        // child 被 kill 后 stdout 关闭，reader 检测到 EOF (read_line 返回 0) 后自然退出
        // 用 3 秒超时兜底，防止 reader 卡住导致 Arc 泄漏
        let reader_handle = self.reader_handle.lock().await.take();
        if let Some(handle) = reader_handle {
            match tokio::time::timeout(std::time::Duration::from_secs(3), handle).await {
                Ok(_) => tracing::debug!("DAP reader task joined cleanly"),
                Err(_) => tracing::warn!("DAP reader task did not exit within 3s after child kill; aborting"),
            }
        }

        Ok(())
    }
}

/// 检查命令是否在 PATH 中可用的简单封装
/// 不用 which crate，直接用 PATH 查找
async fn which_adapter(name: &str) -> Option<PathBuf> {
    // 绝对路径直接检查
    if name.contains('/') || name.contains('\\') {
        let p = Path::new(name);
        if p.exists() {
            return Some(p.to_path_buf());
        }
        return None;
    }
    let path = std::env::var("PATH").ok()?;
    // 跨平台 PATH 分隔符：Unix 用 ':'，Windows 用 ';'
    for dir in path.split(crate::utils::shell::PATH_SEP) {
        let candidate = Path::new(dir).join(name);
        if candidate.exists() {
            return Some(candidate);
        }
        // macOS/Linux 通常无 .exe 后缀
        #[cfg(windows)]
        {
            let with_exe = Path::new(dir).join(format!("{}.exe", name));
            if with_exe.exists() {
                return Some(with_exe);
            }
        }
    }
    None
}

/// DebugManager - 管理多个调试会话
/// 通常只用一个会话，但支持多会话以便扩展
pub struct DebugManager {
    /// 活跃会话表：session_id → DebugSession
    sessions: RwLock<HashMap<String, Arc<DebugSession>>>,
    /// 默认会话 id（最近启动的）
    default_session: RwLock<Option<String>>,
}

impl DebugManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: RwLock::new(HashMap::new()),
            default_session: RwLock::new(None),
        })
    }

    /// 启动新的调试会话
    /// 返回 session_id
    pub async fn start_session(&self, config: DebugConfig) -> Result<String> {
        let session = DebugSession::start(config).await?;
        let session_id = uuid::Uuid::new_v4().to_string();
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), session);
        }
        *self.default_session.write().await = Some(session_id.clone());
        tracing::info!("debug session started: {}", session_id);
        Ok(session_id)
    }

    /// 获取默认会话（最近启动的）
    pub async fn default_session(&self) -> Option<Arc<DebugSession>> {
        let id = self.default_session.read().await.clone()?;
        self.sessions.read().await.get(&id).cloned()
    }

    /// 按 id 获取会话
    pub async fn get_session(&self, session_id: &str) -> Option<Arc<DebugSession>> {
        self.sessions.read().await.get(session_id).cloned()
    }

    /// 停止并移除会话
    pub async fn stop_session(&self, session_id: &str) -> Result<()> {
        let session = {
            let mut sessions = self.sessions.write().await;
            sessions
                .remove(session_id)
                .ok_or_else(|| anyhow!("debug session not found: {}", session_id))?
        };
        session.stop().await?;
        // 若移除的是默认会话，清空或切换到其他
        let mut default = self.default_session.write().await;
        if default.as_deref() == Some(session_id) {
            *default = self
                .sessions
                .read()
                .await
                .keys()
                .next()
                .cloned();
        }
        tracing::info!("debug session stopped: {}", session_id);
        Ok(())
    }

    /// 列出所有会话 id
    pub async fn list_sessions(&self) -> Vec<String> {
        self.sessions.read().await.keys().cloned().collect()
    }

    /// 设计文档 §8.4.3 / P1-2: 关闭所有调试会话
    /// 在 server shutdown 时调用，确保所有 adapter 进程被清理
    pub async fn shutdown_all(&self) {
        let sessions: Vec<(String, Arc<DebugSession>)> = {
            let mut sessions = self.sessions.write().await;
            sessions.drain().collect()
        };
        *self.default_session.write().await = None;
        for (id, session) in sessions {
            tracing::info!("shutting down debug session: {}", id);
            let _ = session.stop().await;
        }
    }
}

// 为 DebugSession 实现 Drop：确保进程被 kill
// 注意：child 已经设置了 kill_on_drop，这里无需手动 kill
// 但若 session 仍持有 stdin，Drop 会自然关闭

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lang_from_str() {
        assert!(matches!(DebugLang::from_str("rust").unwrap(), DebugLang::Rust));
        assert!(matches!(DebugLang::from_str("RUST").unwrap(), DebugLang::Rust));
        assert!(matches!(DebugLang::from_str("node").unwrap(), DebugLang::Node));
        assert!(matches!(DebugLang::from_str("python").unwrap(), DebugLang::Python));
        assert!(matches!(DebugLang::from_str("go").unwrap(), DebugLang::Go));
        assert!(DebugLang::from_str("java").is_err());
    }

    #[test]
    fn test_adapter_id() {
        assert_eq!(DebugLang::Rust.adapter_id(), "lldb");
        assert_eq!(DebugLang::Node.adapter_id(), "node");
        assert_eq!(DebugLang::Python.adapter_id(), "debugpy");
        assert_eq!(DebugLang::Go.adapter_id(), "go");
    }
}
