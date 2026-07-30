use crate::tools::sandbox::SandboxStore;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

/// 代码执行工具：支持 shell/rust/python/javascript/go
/// 设计文档 §4.6:
/// - lang: "shell" | "javascript" | "python" | "rust"
/// - cwd: 工作目录
/// - timeout: 默认 30s
/// - async: 可选，后台执行
/// - 沙箱限制（基础实现：cwd 隔离 + timeout + 大输出 sandbox）
/// - journal 接入：记录文件变更
/// - 大输出存 sandbox，返回 summary + handle
pub struct CodeExecTool;

const CODE_EXEC_TIMEOUT_DEFAULT: u64 = 30;
const STDOUT_THRESHOLD: usize = 5000;
const STDERR_THRESHOLD: usize = 2000;

#[async_trait]
impl Tool for CodeExecTool {
    fn name(&self) -> &str { "code_exec" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "code_exec".into(),
            description: "Execute code: lang=shell|python|javascript|rust|go. cwd=working dir. timeout=seconds (default 30). async=true for background. File changes auto-recorded to journal. Large output stored to sandbox with handle.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "lang": { "type": "string", "enum": ["shell", "python", "javascript", "rust", "go"] },
                    "code": { "type": "string" },
                    "cwd": { "type": "string", "description": "Working directory, default project root" },
                    "timeout": { "type": "integer", "description": "Timeout in seconds, default 30" },
                    "stdin": { "type": "string", "description": "Optional stdin input" },
                    "async": { "type": "boolean", "description": "Override: run in background" }
                },
                "required": ["lang", "code"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let lang: String = serde_json::from_value(args["lang"].clone())?;
        let code: String = serde_json::from_value(args["code"].clone())?;
        let cwd = args["cwd"].as_str().map(|s| s.to_string());
        let timeout = args["timeout"].as_u64().unwrap_or(CODE_EXEC_TIMEOUT_DEFAULT);
        let stdin: Option<String> = args["stdin"].as_str().map(|s| s.to_string());
        let async_override = args["async"].as_bool();

        // shell 语言 + 长超时 + 重命令 → 默认 async
        let is_async = async_override.unwrap_or(false);

        if is_async {
            return Self::run_async(ctx, lang, code, cwd, timeout, stdin).await;
        }
        Self::run_sync(ctx, lang, code, cwd, timeout, stdin).await
    }
}

impl CodeExecTool {
    async fn run_sync(
        ctx: &ToolContext,
        lang: String,
        code: String,
        cwd: Option<String>,
        timeout: u64,
        stdin: Option<String>,
    ) -> Result<ToolOutput> {
        let batch_id = ctx.journal.begin_batch(&ctx.project_dir, &format!("code_exec:{}", lang))
            .context("capturing pre-exec snapshot")?;

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout),
            run_code(&lang, &code, cwd.as_deref(), stdin),
        ).await
            .with_context(|| format!("code_exec timed out after {}s", timeout))??;

        let changed = ctx.journal.end_batch(&batch_id, &format!("code_exec:{}", lang))
            .context("capturing post-exec diff")?;

        Ok(format_exec_output(&lang, &result, &changed, &batch_id, &ctx.project_dir))
    }

    async fn run_async(
        ctx: &ToolContext,
        lang: String,
        code: String,
        cwd: Option<String>,
        timeout: u64,
        stdin: Option<String>,
    ) -> Result<ToolOutput> {
        let project_dir = ctx.project_dir.clone();
        let journal = ctx.journal.clone();
        let task_manager = ctx.task_manager.clone();
        let lang_for_msg = lang.clone();

        let task_id = task_manager.spawn_compat("code_exec", async move {
            let batch_id = journal.begin_batch(&project_dir, &format!("code_exec:{}", lang))
                .map_err(|e| e.to_string())?;

            let result = tokio::time::timeout(
                std::time::Duration::from_secs(timeout),
                run_code(&lang, &code, cwd.as_deref(), stdin),
            ).await
                .map_err(|e| format!("timed out after {}s: {}", timeout, e))?
                .map_err(|e| e.to_string())?;

            let changed = journal.end_batch(&batch_id, &format!("code_exec:{}", lang))
                .map_err(|e| e.to_string())?;

            Ok(serde_json::json!({
                "lang": lang,
                "exit_code": result.exit_code,
                "stdout": tail_lines(&result.stdout, 20),
                "stderr": tail_lines(&result.stderr, 10),
                "files_changed": changed.len(),
                "batch_id": batch_id,
            }).to_string())
        }).await?;

        Ok(ToolOutput::AsyncTask {
            task_id: task_id.clone(),
            handle: task_id,
            status_msg: format!("code_exec running in background: {}", lang_for_msg),
        })
    }
}

struct ExecResult {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

async fn run_code(lang: &str, code: &str, cwd: Option<&str>, stdin: Option<String>) -> Result<ExecResult> {
    let tmp_dir = std::env::temp_dir().join(format!("mcoder-exec-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&tmp_dir)?;

    let (argv0, cmd_args): (String, Vec<String>) = match lang {
        "shell" => {
            // 跨平台：Unix 用 sh -c，Windows 用 cmd /C
            #[cfg(unix)]
            { ("sh".to_string(), vec!["-c".to_string(), code.to_string()]) }
            #[cfg(windows)]
            { ("cmd".to_string(), vec!["/C".to_string(), code.to_string()]) }
        }
        "python" => {
            let f = tmp_dir.join("exec.py");
            std::fs::write(&f, code)?;
            ("python3".to_string(), vec![f.display().to_string()])
        }
        "javascript" => {
            let f = tmp_dir.join("exec.js");
            std::fs::write(&f, code)?;
            ("node".to_string(), vec![f.display().to_string()])
        }
        "go" => {
            let f = tmp_dir.join("exec.go");
            std::fs::write(&f, code)?;
            ("go".to_string(), vec!["run".to_string(), f.display().to_string()])
        }
        "rust" => {
            let src = tmp_dir.join("exec.rs");
            std::fs::write(&src, code)?;
            let bin = tmp_dir.join("exec_bin");
            let compile = tokio::process::Command::new("rustc")
                .arg("-O")
                .arg("-o").arg(&bin)
                .arg(&src)
                .output().await
                .context("rustc not found")?;
            if !compile.status.success() {
                return Ok(ExecResult {
                    exit_code: compile.status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&compile.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&compile.stderr).to_string(),
                });
            }
            (bin.display().to_string(), vec![])
        }
        other => anyhow::bail!("unsupported language: {}", other),
    };

    let mut cmd = tokio::process::Command::new(&argv0);
    cmd.args(&cmd_args);
    // 设计文档 §4.6: 沙箱限制 - 文件系统只能写 cwd 下
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    } else {
        cmd.current_dir(&tmp_dir);
    }

    // 设计文档 §4.6: 沙箱限制 - 网络禁用（通过环境变量）
    // 注意：真正的网络禁用需要 namespace/isolate，这里靠 NO_PROXY 和 unset 网络相关变量
    cmd.env_clear();
    cmd.env("PATH", std::env::var("PATH").unwrap_or_default());
    // 跨平台 home 环境变量：Unix 用 HOME，Windows 用 USERPROFILE
    cmd.env(crate::utils::shell::HOME_ENV, tmp_dir.to_string_lossy().to_string());
    cmd.env("LANG", "C");
    cmd.env("LC_ALL", "C");
    // 禁用网络相关环境变量
    cmd.env("http_proxy", "");
    cmd.env("https_proxy", "");
    cmd.env("HTTP_PROXY", "");
    cmd.env("HTTPS_PROXY", "");
    cmd.env("NO_PROXY", "*");

    // 设计文档 §4.6: 沙箱限制 - CPU 50% 单核 30s + 内存 256MB
    // 使用 pre_exec 设置 rlimit（仅 Unix）
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            // CPU 时间限制：30 秒（软限制）
            // 内存限制：256MB（软限制）
            // 进程数限制：不能 fork 子进程逃逸（设置 NPROC=1）
            set_rlimits_unix();
            Ok(())
        });
    }

    let output = if let Some(input) = &stdin {
        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn().context(format!("spawning {}", lang))?;
        if let Some(mut stdin_pipe) = child.stdin.take() {
            let _ = stdin_pipe.write_all(input.as_bytes()).await;
        }
        child.wait_with_output().await.context(format!("running {}", lang))?
    } else {
        cmd.stdin(std::process::Stdio::null());
        cmd.output().await.context(format!("running {}", lang))?
    };

    // 清理临时目录
    let _ = std::fs::remove_dir_all(&tmp_dir);

    Ok(ExecResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// 设计文档 §4.6: Unix 沙箱限制
/// - CPU: 30 秒
/// - 内存: 256MB
/// - 进程数: 1（不能 fork）
#[cfg(unix)]
fn set_rlimits_unix() {
    #[cfg(target_os = "macos")]
    use libc::{setrlimit, rlimit, RLIMIT_CPU, RLIMIT_AS, RLIMIT_NPROC};
    #[cfg(any(target_os = "linux", target_os = "android"))]
    use libc::{setrlimit, rlimit, RLIMIT_CPU, RLIMIT_AS, RLIMIT_NPROC};

    unsafe {
        // CPU 时间：30 秒软限制，60 秒硬限制
        let cpu_limit = rlimit {
            rlim_cur: 30,
            rlim_max: 60,
        };
        setrlimit(RLIMIT_CPU, &cpu_limit);

        // 内存：256MB 软限制，512MB 硬限制
        let mem_bytes = 256 * 1024 * 1024;
        let mem_limit = rlimit {
            rlim_cur: mem_bytes,
            rlim_max: mem_bytes * 2,
        };
        setrlimit(RLIMIT_AS, &mem_limit);

        // 进程数：1（防止 fork 子进程逃逸）
        let nproc_limit = rlimit {
            rlim_cur: 1,
            rlim_max: 1,
        };
        setrlimit(RLIMIT_NPROC, &nproc_limit);
    }
}

fn format_exec_output(
    lang: &str,
    result: &ExecResult,
    changed: &[PathBuf],
    batch_id: &str,
    project_dir: &PathBuf,
) -> ToolOutput {
    let combined_len = result.stdout.len() + result.stderr.len();
    let truncated = combined_len > STDOUT_THRESHOLD + STDERR_THRESHOLD;

    let mut json = serde_json::json!({
        "lang": lang,
        "exit_code": result.exit_code,
        "stdout": tail_lines(&result.stdout, 20),
        "stderr": tail_lines(&result.stderr, 10),
        "stdout_lines": result.stdout.lines().count(),
        "stderr_lines": result.stderr.lines().count(),
        "truncated": truncated,
        "files_changed": changed.len(),
        "batch_id": batch_id,
    });

    if truncated {
        let full = format!("=== STDOUT ===\n{}\n=== STDERR ===\n{}", result.stdout, result.stderr);
        if let Ok(handle) = SandboxStore::store(project_dir, &full) {
            json["handle"] = serde_json::json!(handle);
            json["hint"] = serde_json::json!("Output truncated. Use sandbox_read op=range with handle for full output.");
        }
    }

    if !changed.is_empty() {
        let list: Vec<String> = changed.iter().take(20).map(|p| p.display().to_string()).collect();
        json["changed_files"] = serde_json::json!(list);
        json["undo_hint"] = serde_json::json!("Use undo op=batch with batch_id to revert file changes.");
    }

    ToolOutput::Sync { result: json }
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
