//! launch 工具：启动/管理后台进程（dev server、watcher、长跑任务等）
//!
//! 设计：
//! - 每个进程 3 个后台 task：stdout reader / stderr reader / wait task
//! - 日志按行写入 LogBuffer（环形缓冲，默认 5000 行）
//! - stop 优雅关闭：SIGTERM → 3s 超时 → SIGKILL
//! - 进程绑 session_id（session 删除时用户决定 kill 或保留）
//!
//! Server 进程退出时不会自动 kill 启动的进程（用户可能想继续 dev server）

use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, RwLock, Mutex};
use futures_util::FutureExt;
use uuid::Uuid;

// ==================== 类型定义 ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessStatus {
    Running,
    Exited,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchedProcess {
    pub id: String,
    pub name: Option<String>,
    pub command: String,
    pub cwd: PathBuf,
    pub pid: Option<u32>,
    pub started_at: i64,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    pub ts: i64,
    pub stream: LogStream,
    pub text: String,
}

/// 环形日志缓冲（最多保留 max_lines 行）
pub struct LogBuffer {
    lines: Mutex<VecDeque<LogLine>>,
    max_lines: usize,
}

impl LogBuffer {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: Mutex::new(VecDeque::with_capacity(max_lines.min(1024))),
            max_lines,
        }
    }

    pub async fn push(&self, line: LogLine) {
        let mut lines = self.lines.lock().await;
        if lines.len() >= self.max_lines {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// 获取最近 tail 行（默认全部）
    pub async fn tail(&self, tail: usize) -> Vec<LogLine> {
        let lines = self.lines.lock().await;
        let start = lines.len().saturating_sub(tail);
        lines.iter().skip(start).cloned().collect()
    }

    /// 获取 since_ts 之后的行
    pub async fn since(&self, since_ts: i64) -> Vec<LogLine> {
        let lines = self.lines.lock().await;
        lines
            .iter()
            .filter(|l| l.ts >= since_ts)
            .cloned()
            .collect()
    }

    pub async fn len(&self) -> usize {
        self.lines.lock().await.len()
    }
}

// ==================== 进程条目（运行时）====================

/// 进程条目：包含静态信息 + 子进程句柄 + 日志缓冲 + 取消信号
pub struct ProcessEntry {
    pub info: RwLock<LaunchedProcess>,
    pub logs: Arc<LogBuffer>,
    pub cancel_tx: broadcast::Sender<()>,
    pub child: Mutex<Option<Child>>,
}

impl ProcessEntry {
    pub async fn snapshot(&self) -> LaunchedProcess {
        self.info.read().await.clone()
    }
}

// ==================== LaunchManager ====================

#[derive(Clone)]
pub struct LaunchManager {
    inner: Arc<LaunchManagerInner>,
}

struct LaunchManagerInner {
    /// process_id -> entry
    processes: RwLock<HashMap<String, Arc<ProcessEntry>>>,
    /// (session_id, name) -> process_id（用于按名字查找）
    by_name: RwLock<HashMap<(String, String), String>>,
    /// 全局 ServerEvent 总线
    event_tx: broadcast::Sender<crate::session_manager::ServerEvent>,
    /// 配置
    config: crate::types::LaunchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessSummary {
    pub id: String,
    pub name: Option<String>,
    pub command: String,
    pub pid: Option<u32>,
    pub status: ProcessStatus,
    pub started_at: i64,
    pub uptime_ms: i64,
    pub log_lines: usize,
}

impl LaunchManager {
    pub fn new(
        event_tx: broadcast::Sender<crate::session_manager::ServerEvent>,
        config: crate::types::LaunchConfig,
    ) -> Self {
        Self {
            inner: Arc::new(LaunchManagerInner {
                processes: RwLock::new(HashMap::new()),
                by_name: RwLock::new(HashMap::new()),
                event_tx,
                config,
            }),
        }
    }

    /// 启动后台进程
    pub async fn start(
        &self,
        session_id: &str,
        command: &str,
        cwd: Option<&PathBuf>,
        env: HashMap<String, String>,
        name: Option<String>,
    ) -> Result<LaunchedProcess> {
        // 1. 解析 cwd（默认 = project_path）
        let cwd: PathBuf = match cwd {
            Some(p) => p.clone(),
            None => std::env::current_dir().context("no cwd")?,
        };

        // 2. 持 write 锁期间检查所有约束并插入（避免并发 start 越过限制）
        // 顺序：name 冲突 → 进程数限制 → insert
        let mut procs = self.inner.processes.write().await;
        if let Some(ref n) = name {
            let by_name = self.inner.by_name.read().await;
            if by_name.contains_key(&(session_id.to_string(), n.clone())) {
                anyhow::bail!("process with name '{}' already exists in this session", n);
            }
        }
        // 进程数限制（持 procs 写锁 + entry.info.read 不能持写锁，安全）
        let count = {
            let mut c = 0;
            for entry in procs.values() {
                let info = entry.info.read().await;
                if info.session_id == session_id {
                    c += 1;
                }
            }
            c
        };
        if count >= self.inner.config.max_processes_per_session {
            anyhow::bail!(
                "max processes per session reached ({}/{}). Stop some first.",
                count,
                self.inner.config.max_processes_per_session
            );
        }
        drop(procs); // 临时释放锁（下面 spawn 进程 + 加锁重新插入）
        let mut procs = self.inner.processes.write().await;

        // 3. 启动子进程（用 shell 解析 command，支持管道/重定向）
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().with_context(|| format!("spawning: {}", command))?;
        let pid = child.id();
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;

        // 4. 创建 entry
        let id = Uuid::new_v4().to_string()[..8].to_string();
        let (cancel_tx, _) = broadcast::channel(1);
        let logs = Arc::new(LogBuffer::new(self.inner.config.max_log_lines_per_process));
        let info = LaunchedProcess {
            id: id.clone(),
            name: name.clone(),
            command: command.to_string(),
            cwd: cwd.clone(),
            pid,
            started_at: Utc::now().timestamp(),
            status: ProcessStatus::Running,
            exit_code: None,
            signal: None,
            session_id: session_id.to_string(),
        };
        let entry = Arc::new(ProcessEntry {
            info: RwLock::new(info.clone()),
            logs: logs.clone(),
            cancel_tx: cancel_tx.clone(),
            child: Mutex::new(Some(child)),
        });

        // 5. 注册（仍在锁内）
        procs.insert(id.clone(), entry.clone());
        drop(procs); // 释放后下面再注册 by_name
        if let Some(ref n) = name {
            let mut by_name = self.inner.by_name.write().await;
            by_name.insert((session_id.to_string(), n.clone()), id.clone());
        }
        // 克隆 entry 供 wait task 使用（避免 std async block 跨 move 问题）
        let entry_for_wait = entry.clone();

        // 7. spawn 后台 task：stdout reader（panic-safe）
        let event_tx = self.inner.event_tx.clone();
        let session_id_owned = session_id.to_string();
        let id_owned = id.clone();
        let name_owned = name.clone();
        let logs_owned = logs.clone();
        let pid_for_log = pid;
        tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(async {
                run_stdout_reader(
                    stdout,
                    &logs_owned,
                    &event_tx,
                    &session_id_owned,
                    &id_owned,
                    name_owned.as_deref(),
                )
                .await;
            })
            .catch_unwind()
            .await;
            if let Err(e) = result {
                tracing::warn!("launch stdout reader panicked (pid={:?}): {:?}", pid_for_log, e);
            }
        });

        // 8. spawn 后台 task：stderr reader（panic-safe）
        let event_tx = self.inner.event_tx.clone();
        let session_id_owned = session_id.to_string();
        let id_owned = id.clone();
        let name_owned = name.clone();
        let logs_owned = logs.clone();
        let pid_for_log = pid;
        tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(async {
                run_stderr_reader(
                    stderr,
                    &logs_owned,
                    &event_tx,
                    &session_id_owned,
                    &id_owned,
                    name_owned.as_deref(),
                )
                .await;
            })
            .catch_unwind()
            .await;
            if let Err(e) = result {
                tracing::warn!("launch stderr reader panicked (pid={:?}): {:?}", pid_for_log, e);
            }
        });

        // 9. spawn wait task：进程退出时更新 status + 发事件（panic-safe）
        // by_name 清理交给 stop() 调用方处理（避免异步块内持 RwLock）
        let event_tx = self.inner.event_tx.clone();
        let session_id_owned = session_id.to_string();
        let id_owned = id.clone();
        let name_owned = name.clone();
        tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(async {
                run_wait_task(
                    entry_for_wait,
                    &event_tx,
                    &session_id_owned,
                    &id_owned,
                    name_owned.as_deref(),
                )
                .await;
            })
            .catch_unwind()
            .await;
            if let Err(e) = result {
                tracing::warn!("launch wait task panicked (id={}): {:?}", id_owned, e);
            }
        });

        tracing::info!(
            "launched process {} (pid {:?}): {}",
            id,
            pid,
            command
        );
        Ok(info)
    }

    /// 停止进程（SIGTERM → 超时 → SIGKILL）
    pub async fn stop(&self, id_or_name: &str, session_id: &str, timeout_ms: u64) -> Result<()> {
        let entry = self.resolve(id_or_name, session_id).await?;
        // 先检查是否需要清理 by_name（无论 stop 结果如何都清理）
        let name_to_cleanup = entry.snapshot().await.name.clone();
        // 标记取消
        let _ = entry.cancel_tx.send(());
        // 优雅关闭
        let pid: Option<u32> = {
            let mut guard = entry.child.lock().await;
            if let Some(child) = guard.as_mut() {
                let pid = child.id();
                #[cfg(unix)]
                {
                    if let Some(pid) = pid {
                        // libc::kill(pid, SIGTERM)
                        unsafe {
                            libc::kill(pid as i32, libc::SIGTERM);
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = child.start_kill();
                }
                pid
            } else {
                None
            }
        };
        tracing::debug!("sent SIGTERM to {} (pid {:?})", id_or_name, pid);

        // 等 timeout
        let start = std::time::Instant::now();
        loop {
            let info = entry.snapshot().await;
            if info.status != ProcessStatus::Running {
                break;
            }
            if start.elapsed() > std::time::Duration::from_millis(timeout_ms) {
                // 超时，强杀
                let mut guard = entry.child.lock().await;
                if let Some(child) = guard.as_mut() {
                    let _ = child.start_kill();
                }
                // 等一小段确保 wait task 收到
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        // 清理 by_name（避免同名重启冲突）
        if let Some(n) = name_to_cleanup {
            let mut map = self.inner.by_name.write().await;
            map.remove(&(session_id.to_string(), n));
        }
        Ok(())
    }

    /// 获取进程状态
    pub async fn status(&self, id_or_name: &str, session_id: &str) -> Result<LaunchedProcess> {
        let entry = self.resolve(id_or_name, session_id).await?;
        Ok(entry.snapshot().await)
    }

    /// 获取日志
    pub async fn logs(
        &self,
        id_or_name: &str,
        session_id: &str,
        tail: Option<usize>,
        since_ts: Option<i64>,
    ) -> Result<Vec<LogLine>> {
        let entry = self.resolve(id_or_name, session_id).await?;
        if let Some(ts) = since_ts {
            Ok(entry.logs.since(ts).await)
        } else {
            Ok(entry.logs.tail(tail.unwrap_or(100)).await)
        }
    }

    /// 列出 session 的所有进程
    pub async fn list(&self, session_id: &str) -> Vec<ProcessSummary> {
        let procs = self.inner.processes.read().await;
        let mut out = Vec::new();
        let now = Utc::now().timestamp_millis();
        for (id, entry) in procs.iter() {
            let info = entry.snapshot().await;
            if info.session_id != session_id {
                continue;
            }
            out.push(ProcessSummary {
                id: id.clone(),
                name: info.name.clone(),
                command: info.command.clone(),
                pid: info.pid,
                status: info.status,
                started_at: info.started_at,
                uptime_ms: now - (info.started_at * 1000),
                log_lines: entry.logs.len().await,
            });
        }
        // 按 started_at 排序（最新在前）
        out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        out
    }

    /// 解析 id 或 name（per-session）
    async fn resolve(
        &self,
        id_or_name: &str,
        session_id: &str,
    ) -> Result<Arc<ProcessEntry>> {
        // 先尝试按 id
        let procs = self.inner.processes.read().await;
        if let Some(entry) = procs.get(id_or_name) {
            let info = entry.snapshot().await;
            if info.session_id == session_id {
                return Ok(entry.clone());
            }
        }
        // 再尝试按 name
        let by_name = self.inner.by_name.read().await;
        if let Some(id) = by_name.get(&(session_id.to_string(), id_or_name.to_string())) {
            if let Some(entry) = procs.get(id) {
                return Ok(entry.clone());
            }
        }
        anyhow::bail!("process '{}' not found in session {}", id_or_name, session_id)
    }

    #[allow(dead_code)]
    async fn count_by_session(&self, session_id: &str) -> usize {
        let procs = self.inner.processes.read().await;
        let mut count = 0;
        for e in procs.values() {
            if e.snapshot().await.session_id == session_id {
                count += 1;
            }
        }
        count
    }

    /// shutdown 时调用：返回所有进程的 (session_id, id, name, optional_stop_timeout)
    /// 用 name 优先（更稳定）
    pub async fn all_processes_snapshot(
        &self,
    ) -> Vec<(String, String, Option<String>, Option<u64>)> {
        let procs = self.inner.processes.read().await;
        let mut out = Vec::new();
        for entry in procs.values() {
            let info = entry.snapshot().await;
            out.push((
                info.session_id.clone(),
                info.id.clone(),
                info.name.clone(),
                None, // 用默认 timeout（在 caller 处填）
            ));
        }
        out
    }
}

// ==================== 后台 task 逻辑（panic-safe wrapper 调用）====================

/// stdout reader：从 stdout pipe 逐行读，写入日志缓冲 + 推送 ServerEvent
async fn run_stdout_reader(
    stdout: tokio::process::ChildStdout,
    logs: &Arc<LogBuffer>,
    event_tx: &tokio::sync::broadcast::Sender<crate::session_manager::ServerEvent>,
    session_id: &str,
    id: &str,
    name: Option<&str>,
) {
    let mut reader = BufReader::new(stdout).lines();
    loop {
        match reader.next_line().await {
            Ok(Some(text)) => {
                let line = LogLine {
                    ts: Utc::now().timestamp_millis(),
                    stream: LogStream::Stdout,
                    text: text.clone(),
                };
                logs.push(line).await;
                let _ = event_tx.send(crate::session_manager::ServerEvent::LaunchOutput {
                    session_id: session_id.to_string(),
                    id: id.to_string(),
                    name: name.map(|s| s.to_string()),
                    stream: "stdout".to_string(),
                    text,
                    ts: Utc::now().timestamp_millis(),
                });
            }
            _ => break, // EOF or error
        }
    }
}

/// stderr reader：与 stdout 相同
async fn run_stderr_reader(
    stderr: tokio::process::ChildStderr,
    logs: &Arc<LogBuffer>,
    event_tx: &tokio::sync::broadcast::Sender<crate::session_manager::ServerEvent>,
    session_id: &str,
    id: &str,
    name: Option<&str>,
) {
    let mut reader = BufReader::new(stderr).lines();
    loop {
        match reader.next_line().await {
            Ok(Some(text)) => {
                let line = LogLine {
                    ts: Utc::now().timestamp_millis(),
                    stream: LogStream::Stderr,
                    text: text.clone(),
                };
                logs.push(line).await;
                let _ = event_tx.send(crate::session_manager::ServerEvent::LaunchOutput {
                    session_id: session_id.to_string(),
                    id: id.to_string(),
                    name: name.map(|s| s.to_string()),
                    stream: "stderr".to_string(),
                    text,
                    ts: Utc::now().timestamp_millis(),
                });
            }
            _ => break,
        }
    }
}

/// wait task：等待进程退出 + 更新 status + 发 LaunchExited
async fn run_wait_task(
    entry: Arc<ProcessEntry>,
    event_tx: &tokio::sync::broadcast::Sender<crate::session_manager::ServerEvent>,
    session_id: &str,
    id: &str,
    name: Option<&str>,
) {
    let wait_result = {
        let mut guard = entry.child.lock().await;
        if let Some(mut child) = guard.take() {
            child.wait().await
        } else {
            return;
        }
    };
    let (status, exit_code, signal) = match wait_result {
        Ok(status) => {
            let code = status.code();
            #[cfg(unix)]
            let sig = {
                use std::os::unix::process::ExitStatusExt;
                status.signal()
            };
            #[cfg(not(unix))]
            let sig = None;
            if sig.is_some() {
                (ProcessStatus::Stopped, code, sig)
            } else if code == Some(0) {
                (ProcessStatus::Exited, code, sig)
            } else {
                (ProcessStatus::Failed, code, sig)
            }
        }
        Err(e) => {
            tracing::warn!("wait() failed for launch {}: {}", id, e);
            (ProcessStatus::Failed, None, None)
        }
    };

    // 更新 info
    {
        let mut info = entry.info.write().await;
        info.status = status;
        info.exit_code = exit_code;
        info.signal = signal;
    }

    // 发事件
    let _ = event_tx.send(crate::session_manager::ServerEvent::LaunchExited {
        session_id: session_id.to_string(),
        id: id.to_string(),
        name: name.map(|s| s.to_string()),
        exit_code,
        signal,
        ts: Utc::now().timestamp_millis(),
    });

    tracing::info!(
        "launch process {} (session={}) exited: status={:?}, code={:?}, signal={:?}",
        id,
        session_id,
        status,
        exit_code,
        signal
    );
}

pub struct LaunchTool;

#[async_trait]
impl crate::tools::Tool for LaunchTool {
    fn name(&self) -> &str {
        "launch"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "launch".into(),
            description: "Manage background processes (dev servers, watchers, long-running tasks). Each process has an id (uuid short) and optional name. Output is streamed into a log buffer (default 5000 lines). Actions: start | stop | status | logs | list.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["start", "stop", "status", "logs", "list"]
                    },
                    "command": {"type": "string", "description": "Shell command (start)"},
                    "cwd": {"type": "string", "description": "Working directory (start)"},
                    "name": {"type": "string", "description": "Semantic name (start/status/stop/logs)"},
                    "env": {
                        "type": "object",
                        "description": "Extra env vars (start)",
                        "additionalProperties": {"type": "string"}
                    },
                    "id": {"type": "string", "description": "Process id (status/stop/logs)"},
                    "tail": {"type": "integer", "description": "Number of log lines (logs)", "default": 100},
                    "since_ts": {"type": "integer", "description": "Filter logs after ts millis (logs)"},
                    "timeout_ms": {"type": "integer", "description": "Stop timeout before SIGKILL (stop)", "default": 3000}
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &crate::tools::ToolContext) -> Result<ToolOutput> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("action required"))?;
        let manager = &ctx.launch_manager;
        let session_id = &ctx.session_id;

        match action {
            "start" => {
                let command = args["command"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("command required for start"))?;
                let cwd = args["cwd"].as_str().map(PathBuf::from);
                let name = args["name"].as_str().map(|s| s.to_string());
                // env 解析：严格要求字符串值，非字符串报错
                let env: HashMap<String, String> = if let Some(obj) = args["env"].as_object() {
                    let mut map = HashMap::new();
                    for (k, v) in obj {
                        let val = v
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("env.{} must be a string, got {}", k, v))?;
                        map.insert(k.clone(), val.to_string());
                    }
                    map
                } else {
                    HashMap::new()
                };
                let info = manager.start(session_id, command, cwd.as_ref(), env, name).await?;
                Ok(ToolOutput::Sync {
                    result: serde_json::json!({
                        "ok": true,
                        "process": info,
                    }),
                })
            }
            "stop" => {
                let id = args["id"]
                    .as_str()
                    .or_else(|| args["name"].as_str())
                    .ok_or_else(|| anyhow::anyhow!("id or name required for stop"))?;
                let timeout_ms = args["timeout_ms"].as_u64().unwrap_or(3000);
                manager.stop(id, session_id, timeout_ms).await?;
                Ok(ToolOutput::Sync {
                    result: serde_json::json!({"ok": true, "id": id}),
                })
            }
            "status" => {
                let id = args["id"]
                    .as_str()
                    .or_else(|| args["name"].as_str())
                    .ok_or_else(|| anyhow::anyhow!("id or name required for status"))?;
                let info = manager.status(id, session_id).await?;
                Ok(ToolOutput::Sync {
                    result: serde_json::json!({
                        "ok": true,
                        "process": info,
                    }),
                })
            }
            "logs" => {
                let id = args["id"]
                    .as_str()
                    .or_else(|| args["name"].as_str())
                    .ok_or_else(|| anyhow::anyhow!("id or name required for logs"))?;
                let tail = args["tail"].as_u64().map(|n| n as usize);
                let since_ts = args["since_ts"].as_i64();
                let lines = manager.logs(id, session_id, tail, since_ts).await?;
                Ok(ToolOutput::Sync {
                    result: serde_json::json!({
                        "ok": true,
                        "id": id,
                        "lines": lines,
                        "count": lines.len(),
                    }),
                })
            }
            "list" => {
                let procs = manager.list(session_id).await;
                Ok(ToolOutput::Sync {
                    result: serde_json::json!({
                        "ok": true,
                        "processes": procs,
                        "count": procs.len(),
                    }),
                })
            }
            other => anyhow::bail!("unknown action: {}", other),
        }
    }
}