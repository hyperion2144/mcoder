use crate::tools::sandbox::SandboxStore;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;

/// bash 工具：执行单条 shell 命令
/// - 智能判断 async（watch/&/tail -f/serve 等 + timeout>60s 且 build/test/install）
/// - 执行前后做项目快照，diff 变动文件记入 FileJournal
/// - 大输出存 sandbox，返回 summary + handle
pub struct BashTool;

const STDOUT_SUMMARY_LINES: usize = 20;
const STDERR_SUMMARY_LINES: usize = 10;
const OUTPUT_THRESHOLD: usize = 5000;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str { "bash" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "bash".into(),
            description: "Execute a shell command. Smart async detection (watch/&/tail -f/serve/start or long build/test/install). File changes auto-recorded to journal (undoable). Large output stored to sandbox with handle.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "cmd": { "type": "string", "description": "Command to run" },
                    "cwd": { "type": "string", "description": "Working directory, default project root" },
                    "timeout": { "type": "integer", "description": "Timeout in seconds, default 120" },
                    "env": { "type": "object", "description": "Additional env vars" },
                    "async": { "type": "boolean", "description": "Override smart async detection" }
                },
                "required": ["cmd"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let cmd: String = serde_json::from_value(args["cmd"].clone())
            .context("cmd required")?;
        let cwd = args["cwd"].as_str().map(|s| s.to_string());
        let timeout = args["timeout"].as_u64().unwrap_or(120);
        let env: std::collections::HashMap<String, String> = args["env"]
            .as_object()
            .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
            .unwrap_or_default();
        let async_override = args["async"].as_bool();

        let is_async = async_override.unwrap_or_else(|| should_run_async(&cmd, timeout));

        if is_async {
            return Self::run_async(ctx, cmd, cwd, timeout, env).await;
        }
        Self::run_sync(ctx, cmd, cwd, timeout, env).await
    }
}

impl BashTool {
    async fn run_sync(
        ctx: &ToolContext,
        cmd: String,
        cwd: Option<String>,
        timeout: u64,
        env: std::collections::HashMap<String, String>,
    ) -> Result<ToolOutput> {
        let batch_id = ctx.journal.begin_batch(&ctx.project_dir, "bash")
            .context("capturing pre-bash snapshot")?;

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout),
            run_command(&cmd, cwd.as_deref(), &env),
        ).await
            .with_context(|| format!("command timed out after {}s", timeout))??;

        let changed = ctx.journal.end_batch(&batch_id, "bash")
            .context("capturing post-bash diff")?;

        Ok(format_bash_output(&output, &changed, &batch_id, &ctx.project_dir))
    }

    async fn run_async(
        ctx: &ToolContext,
        cmd: String,
        cwd: Option<String>,
        timeout: u64,
        env: std::collections::HashMap<String, String>,
    ) -> Result<ToolOutput> {
        let project_dir = ctx.project_dir.clone();
        let journal = ctx.journal.clone();
        let task_manager = ctx.task_manager.clone();
        let cmd_for_msg = cmd.clone();

        let task_id = task_manager.spawn("bash", async move {
            let batch_id = journal.begin_batch(&project_dir, "bash")
                .map_err(|e| e.to_string())?;

            let output = tokio::time::timeout(
                std::time::Duration::from_secs(timeout),
                run_command(&cmd, cwd.as_deref(), &env),
            ).await
                .map_err(|e| format!("async command timed out after {}s: {}", timeout, e))?
                .map_err(|e| e.to_string())?;

            let changed = journal.end_batch(&batch_id, "bash")
                .map_err(|e| e.to_string())?;

            let exit = output.status.code().unwrap_or(-1);
            Ok(serde_json::json!({
                "exit_code": exit,
                "stdout": tail_lines(&output.stdout, STDOUT_SUMMARY_LINES),
                "stderr": tail_lines(&output.stderr, STDERR_SUMMARY_LINES),
                "files_changed": changed.len(),
                "batch_id": batch_id,
            }).to_string())
        }).await;

        Ok(ToolOutput::AsyncTask {
            task_id: task_id.clone(),
            handle: task_id,
            status_msg: format!("bash running in background: {}", &cmd_for_msg[..cmd_for_msg.len().min(60)]),
        })
    }
}

/// bash_batch 工具：批量执行多条命令
/// - stop_on_error 默认 true
/// - parallel 默认 false
/// - 整个批次共享一个 journal batch
pub struct BashBatchTool;

#[async_trait]
impl Tool for BashBatchTool {
    fn name(&self) -> &str { "bash_batch" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "bash_batch".into(),
            description: "Execute multiple shell commands as a batch. Saves tokens vs multiple bash calls. stop_on_error=true by default. parallel=false by default.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "cmds": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "cmd": { "type": "string" },
                                "cwd": { "type": "string" },
                                "timeout": { "type": "integer" }
                            },
                            "required": ["cmd"]
                        }
                    },
                    "stop_on_error": { "type": "boolean", "default": true },
                    "parallel": { "type": "boolean", "default": false }
                },
                "required": ["cmds"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let cmds: Vec<Value> = serde_json::from_value(args["cmds"].clone())
            .context("cmds array required")?;
        let stop_on_error = args["stop_on_error"].as_bool().unwrap_or(true);
        let parallel = args["parallel"].as_bool().unwrap_or(false);

        let batch_id = ctx.journal.begin_batch(&ctx.project_dir, "bash_batch")?;

        let mut results: Vec<serde_json::Value> = Vec::new();
        let mut had_error = false;

        if parallel {
            let mut tasks = Vec::new();
            for cmd_val in &cmds {
                let cmd: String = serde_json::from_value(cmd_val["cmd"].clone())?;
                let cwd = cmd_val["cwd"].as_str().map(|s| s.to_string());
                let timeout = cmd_val["timeout"].as_u64().unwrap_or(120);
                let env = std::collections::HashMap::new();
                tasks.push(tokio::spawn(async move {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(timeout),
                        run_command(&cmd, cwd.as_deref(), &env),
                    ).await
                }));
            }
            for (i, t) in tasks.into_iter().enumerate() {
                match t.await {
                    Ok(Ok(Ok(output))) => {
                        let exit = output.status.code().unwrap_or(-1);
                        if exit != 0 { had_error = true; }
                        results.push(serde_json::json!({
                            "index": i,
                            "exit_code": exit,
                            "stdout_summary": tail_lines(&output.stdout, STDOUT_SUMMARY_LINES),
                            "stderr_summary": tail_lines(&output.stderr, STDERR_SUMMARY_LINES),
                        }));
                    }
                    Ok(Ok(Err(e))) => {
                        had_error = true;
                        results.push(serde_json::json!({ "index": i, "error": e.to_string() }));
                    }
                    Ok(Err(_)) => {
                        had_error = true;
                        results.push(serde_json::json!({ "index": i, "error": "timeout" }));
                    }
                    Err(e) => {
                        had_error = true;
                        results.push(serde_json::json!({ "index": i, "error": format!("join: {}", e) }));
                    }
                }
                if had_error && stop_on_error { break; }
            }
        } else {
            for (i, cmd_val) in cmds.iter().enumerate() {
                let cmd: String = serde_json::from_value(cmd_val["cmd"].clone())?;
                let cwd = cmd_val["cwd"].as_str().map(|s| s.to_string());
                let timeout = cmd_val["timeout"].as_u64().unwrap_or(120);
                let env = std::collections::HashMap::new();

                let output = match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout),
                    run_command(&cmd, cwd.as_deref(), &env),
                ).await {
                    Ok(Ok(o)) => o,
                    Ok(Err(e)) => {
                        had_error = true;
                        results.push(serde_json::json!({ "index": i, "error": e.to_string() }));
                        if stop_on_error { break; }
                        continue;
                    }
                    Err(_) => {
                        had_error = true;
                        results.push(serde_json::json!({ "index": i, "error": "timeout" }));
                        if stop_on_error { break; }
                        continue;
                    }
                };

                let exit = output.status.code().unwrap_or(-1);
                if exit != 0 { had_error = true; }

                // 第一条命令的完整输出存 sandbox，后续只存 summary
                let (stdout_field, handle) = if i == 0 {
                    if output.stdout.len() > OUTPUT_THRESHOLD {
                        let h = SandboxStore::store(&ctx.project_dir, &output.stdout)?;
                        (tail_lines(&output.stdout, STDOUT_SUMMARY_LINES), Some(h))
                    } else {
                        (output.stdout.clone(), None)
                    }
                } else {
                    (tail_lines(&output.stdout, STDOUT_SUMMARY_LINES), None)
                };

                results.push(serde_json::json!({
                    "index": i,
                    "exit_code": exit,
                    "stdout_summary": stdout_field,
                    "stderr_summary": tail_lines(&output.stderr, STDERR_SUMMARY_LINES),
                    "handle": handle,
                }));

                if had_error && stop_on_error { break; }
            }
        }

        let changed = ctx.journal.end_batch(&batch_id, "bash_batch")?;

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "results": results,
            "total": cmds.len(),
            "executed": results.len(),
            "had_error": had_error,
            "files_changed": changed.len(),
            "batch_id": batch_id,
            "changed_files": changed.iter().take(20).map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "undo_hint": if !changed.is_empty() { "Use undo op=batch with batch_id to revert." } else { "" }
        }) })
    }
}

struct CmdOutput {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

async fn run_command(cmd: &str, cwd: Option<&str>, env: &std::collections::HashMap<String, String>) -> Result<CmdOutput> {
    // 跨平台：Unix 用 sh -c，Windows 用 cmd /C
    let mut c = crate::utils::shell::shell_command_tokio();
    c.arg(cmd);
    if let Some(cwd) = cwd {
        c.current_dir(cwd);
    }
    for (k, v) in env {
        c.env(k, v);
    }
    c.stdin(std::process::Stdio::null());
    let output = c.output().await.context("running command")?;
    Ok(CmdOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// 智能判断是否异步执行
fn should_run_async(cmd: &str, timeout: u64) -> bool {
    let lower = cmd.to_lowercase();
    // 长时间运行的命令
    let async_keywords = ["watch", "tail -f", "dev", "serve", "start", "--watch"];
    if async_keywords.iter().any(|k| lower.contains(k)) {
        return true;
    }
    // 长超时 + 重命令
    if timeout > 60 {
        let heavy_keywords = ["build", "test", "install", "compile", "cargo", "npm", "yarn"];
        if heavy_keywords.iter().any(|k| lower.contains(k)) {
            return true;
        }
    }
    // 后台进程（跨平台检测）
    if crate::utils::shell::is_background_command(cmd) {
        return true;
    }
    false
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= n {
        s.to_string()
    } else {
        let start = lines.len() - n;
        format!("... ({} lines above)\n{}", lines.len() - n, lines[start..].join("\n"))
    }
}

fn format_bash_output(output: &CmdOutput, changed: &[PathBuf], batch_id: &str, project_dir: &PathBuf) -> ToolOutput {
    let combined_len = output.stdout.len() + output.stderr.len();
    let truncated = combined_len > OUTPUT_THRESHOLD;

    let mut result = serde_json::json!({
        "exit_code": output.status.code().unwrap_or(-1),
        "stdout_summary": tail_lines(&output.stdout, STDOUT_SUMMARY_LINES),
        "stderr_summary": tail_lines(&output.stderr, STDERR_SUMMARY_LINES),
        "stdout_lines": output.stdout.lines().count(),
        "stderr_lines": output.stderr.lines().count(),
        "truncated": truncated,
        "files_changed": changed.len(),
        "batch_id": batch_id,
    });

    if truncated {
        let full = format!("=== STDOUT ===\n{}\n=== STDERR ===\n{}", output.stdout, output.stderr);
        if let Ok(handle) = SandboxStore::store(project_dir, &full) {
            result["handle"] = serde_json::json!(handle);
            result["hint"] = serde_json::json!("Output truncated. Use sandbox_read op=range with handle for full output.");
        }
    }

    if !changed.is_empty() {
        let list: Vec<String> = changed.iter().take(20).map(|p| p.display().to_string()).collect();
        result["changed_files"] = serde_json::json!(list);
        if changed.len() > 20 {
            result["changed_files_truncated"] = serde_json::json!(true);
        }
        result["undo_hint"] = serde_json::json!("Use undo op=batch with batch_id to revert file changes.");
    }

    ToolOutput::Sync { result }
}
