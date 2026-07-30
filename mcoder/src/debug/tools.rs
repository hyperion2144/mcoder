// 设计文档 §8.4.3: DAP 调试工具集
// 合并为单个 debug 工具，通过 action 参数分派
// forward-looking scaffolding
#![allow(dead_code)]

use super::{DebugConfig, DebugLang};
use crate::tools::sandbox::SandboxStore;
use crate::tools::{Tool, ToolContext};
use crate::types::{ToolOutput, ToolSchema};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

/// P2-5 修复：大输出阈值（4KB），超过则走 sandbox
const DEBUG_STATE_SANDBOX_THRESHOLD: usize = 4 * 1024;

/// debug - 统一调试工具，通过 action 参数分派
/// action: "start" | "breakpoint" | "continue" | "step" | "eval" | "state" | "stop"
pub struct DebugTool;

#[async_trait]
impl Tool for DebugTool {
    fn name(&self) -> &str {
        "debug"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "debug".into(),
            description: "Unified debug tool via DAP. Dispatch by 'action': \
                start (launch/attach session; auto-selects adapter by lang: rust->lldb-dap, node->node, python->debugpy, go->dlv), \
                breakpoint (set breakpoint at file:line, optional condition), \
                continue (resume execution until next stop), \
                step (step over/in/out), \
                eval (evaluate expression in current frame), \
                state (get stopped/frames/variables/output), \
                stop (stop and remove session).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["start", "breakpoint", "continue", "step", "eval", "state", "stop"],
                        "description": "Debug action to perform"
                    },
                    "lang": { "type": "string", "enum": ["rust", "node", "python", "go"], "description": "[start] Debug language" },
                    "program": { "type": "string", "description": "[start] Executable path (launch) or symbol-bearing binary (attach)" },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "[start] Command-line arguments passed to program" },
                    "cwd": { "type": "string", "description": "[start] Working directory" },
                    "stop_on_entry": { "type": "boolean", "description": "[start] Whether to stop at entry point" },
                    "attach_pid": { "type": "integer", "description": "[start] Attach to running process by pid (instead of launch)" },
                    "file": { "type": "string", "description": "[breakpoint] Source file path" },
                    "line": { "type": "integer", "description": "[breakpoint] Line number (1-based)" },
                    "condition": { "type": "string", "description": "[breakpoint] Optional conditional breakpoint expression" },
                    "granularity": { "type": "string", "enum": ["over", "in", "out"], "description": "[step] Step granularity (default: over)" },
                    "expression": { "type": "string", "description": "[eval] Expression to evaluate (language-specific syntax)" },
                    "session_id": { "type": "string", "description": "[stop] Optional specific session id to stop; default session if omitted" }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required field: action"))?;
        match action {
            "start" => self.execute_start(args, ctx).await,
            "breakpoint" => self.execute_breakpoint(args, ctx).await,
            "continue" => self.execute_continue(args, ctx).await,
            "step" => self.execute_step(args, ctx).await,
            "eval" => self.execute_eval(args, ctx).await,
            "state" => self.execute_state(args, ctx).await,
            "stop" => self.execute_stop(args, ctx).await,
            other => Ok(ToolOutput::Error {
                message: format!(
                    "unknown action: {} (expected: start|breakpoint|continue|step|eval|state|stop)",
                    other
                ),
            }),
        }
    }
}

impl DebugTool {
    /// action=start - 启动调试会话
    async fn execute_start(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let lang_str = args
            .get("lang")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required field: lang"))?;
        let program = args
            .get("program")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required field: program"))?
            .to_string();

        let lang = match DebugLang::from_str(lang_str) {
            Ok(l) => l,
            Err(e) => {
                return Ok(ToolOutput::Error {
                    message: format!("invalid lang: {}", e),
                });
            }
        };

        let args_list: Vec<String> = args
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let cwd = args
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let stop_on_entry = args
            .get("stop_on_entry")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let attach_pid = args
            .get("attach_pid")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);

        let config = DebugConfig {
            lang,
            program,
            args: args_list,
            cwd,
            stop_on_entry,
            attach_pid,
        };

        match ctx.debug_manager.start_session(config).await {
            Ok(session_id) => Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "session_id": session_id,
                    "status": "started",
                    "hint": "Use debug action=breakpoint to add breakpoints, then action=continue or action=step to control execution."
                }),
            }),
            Err(e) => Ok(ToolOutput::Error {
                message: format!("failed to start debug session: {}", e),
            }),
        }
    }

    /// action=breakpoint - 设置断点（支持条件断点）
    async fn execute_breakpoint(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let file = args
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required field: file"))?;
        let line = args
            .get("line")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow::anyhow!("missing required field: line"))?;
        let condition = args
            .get("condition")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let session = match ctx.debug_manager.default_session().await {
            Some(s) => s,
            None => {
                return Ok(ToolOutput::Error {
                    message: "no active debug session; call debug action=start first".into(),
                });
            }
        };

        let bps = vec![(line, condition.clone())];
        match session.set_breakpoints(file, bps).await {
            Ok(infos) => {
                let info = infos.into_iter().next().unwrap_or(super::BreakpointInfo {
                    id: None,
                    verified: false,
                    line,
                    message: Some("no response from adapter".into()),
                });
                Ok(ToolOutput::Sync {
                    result: serde_json::json!({
                        "breakpoint_id": info.id,
                        "verified": info.verified,
                        "line": info.line,
                        "message": info.message,
                        "file": file,
                        "condition": condition,
                    }),
                })
            }
            Err(e) => Ok(ToolOutput::Error {
                message: format!("set_breakpoint failed: {}", e),
            }),
        }
    }

    /// action=continue - 继续执行
    async fn execute_continue(&self, _args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let session = match ctx.debug_manager.default_session().await {
            Some(s) => s,
            None => {
                return Ok(ToolOutput::Error {
                    message: "no active debug session; call debug action=start first".into(),
                });
            }
        };

        // 检查状态：必须处于 stopped
        let state = session.state().await;
        if state == super::SessionState::Terminated {
            return Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "status": "terminated",
                    "hint": "session already terminated; call debug action=start to begin a new session"
                }),
            });
        }

        match session.continue_exec().await {
            Ok(()) => {
                // continue 之后等待下一个 stopped/terminated 事件
                let event = session.wait_next_event().await.ok();
                let state = session.state().await;
                Ok(ToolOutput::Sync {
                    result: serde_json::json!({
                        "status": format!("{:?}", state).to_lowercase(),
                        "event": event,
                        "hint": "use debug action=state to inspect current frame and variables"
                    }),
                })
            }
            Err(e) => Ok(ToolOutput::Error {
                message: format!("debug continue failed: {}", e),
            }),
        }
    }

    /// action=step - 单步执行（granularity: over|in|out）
    async fn execute_step(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let granularity = args
            .get("granularity")
            .and_then(|v| v.as_str())
            .unwrap_or("over");

        let session = match ctx.debug_manager.default_session().await {
            Some(s) => s,
            None => {
                return Ok(ToolOutput::Error {
                    message: "no active debug session; call debug action=start first".into(),
                });
            }
        };

        let state = session.state().await;
        if state == super::SessionState::Terminated {
            return Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "status": "terminated",
                    "hint": "session already terminated"
                }),
            });
        }

        match session.step(granularity).await {
            Ok(()) => {
                // 等待下一个 stopped 事件
                let event = session.wait_next_event().await.ok();
                let state = session.state().await;
                Ok(ToolOutput::Sync {
                    result: serde_json::json!({
                        "status": format!("{:?}", state).to_lowercase(),
                        "granularity": granularity,
                        "event": event,
                    }),
                })
            }
            Err(e) => Ok(ToolOutput::Error {
                message: format!("debug step failed: {}", e),
            }),
        }
    }

    /// action=eval - 求值表达式
    async fn execute_eval(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let expression = args
            .get("expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required field: expression"))?;

        let session = match ctx.debug_manager.default_session().await {
            Some(s) => s,
            None => {
                return Ok(ToolOutput::Error {
                    message: "no active debug session; call debug action=start first".into(),
                });
            }
        };

        // 取栈顶 frame id 作为求值上下文
        let frame_id = match session.stack_trace().await {
            Ok(frames) => frames.first().map(|f| f.id),
            Err(_) => None,
        };

        match session.evaluate(expression, frame_id).await {
            Ok(result) => Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "expression": expression,
                    "result": result,
                }),
            }),
            Err(e) => Ok(ToolOutput::Error {
                message: format!("debug eval failed: {}", e),
            }),
        }
    }

    /// action=state - 获取当前调试状态
    /// 返回 {stopped, thread_id, frames, variables}
    /// P2-5 修复：输出超过阈值时存入 sandbox 返回 handle + 摘要
    async fn execute_state(&self, _args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let session = match ctx.debug_manager.default_session().await {
            Some(s) => s,
            None => {
                return Ok(ToolOutput::Error {
                    message: "no active debug session; call debug action=start first".into(),
                });
            }
        };

        let state = session.get_state().await;
        let output = session.get_output().await;
        let full_result = serde_json::json!({
            "stopped": state.stopped,
            "terminated": state.terminated,
            "thread_id": state.thread_id,
            "frames": state.frames,
            "variables": state.variables,
            "output_tail": tail_output(&output, 2000),
        });

        // P2-5 修复：检查序列化后大小，超过阈值走 sandbox
        let serialized = serde_json::to_string(&full_result)?;
        if serialized.len() > DEBUG_STATE_SANDBOX_THRESHOLD {
            let handle = SandboxStore::store(&ctx.project_dir, &serialized)?;
            // 返回摘要 + handle，agent 可用 sandbox_read 分页读取完整内容
            let summary = serde_json::json!({
                "stopped": state.stopped,
                "terminated": state.terminated,
                "thread_id": state.thread_id,
                "frame_count": state.frames.len(),
                "variable_count": state.variables.len(),
                "output_bytes": output.len(),
                "handle": handle,
                "hint": "full state stored in sandbox; use sandbox_read with handle to paginate",
            });
            Ok(ToolOutput::Sync { result: summary })
        } else {
            Ok(ToolOutput::Sync { result: full_result })
        }
    }

    /// action=stop - 停止调试会话
    async fn execute_stop(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let session_id = args
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // 若未指定 session_id，取默认会话
        let sid = if let Some(id) = session_id {
            id
        } else {
            match ctx.debug_manager.default_session().await {
                Some(_) => match ctx.debug_manager.list_sessions().await.first().cloned() {
                    Some(id) => id,
                    None => {
                        return Ok(ToolOutput::Sync {
                            result: serde_json::json!({
                                "status": "no_active_session",
                                "hint": "no debug session to stop"
                            }),
                        });
                    }
                },
                None => {
                    return Ok(ToolOutput::Sync {
                        result: serde_json::json!({
                            "status": "no_active_session",
                            "hint": "no debug session to stop"
                        }),
                    });
                }
            }
        };

        match ctx.debug_manager.stop_session(&sid).await {
            Ok(()) => Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "status": "stopped",
                    "session_id": sid
                }),
            }),
            Err(e) => Ok(ToolOutput::Error {
                message: format!("debug stop failed: {}", e),
            }),
        }
    }
}

/// 截取 output 的最后 N 字符（避免返回过大）
fn tail_output(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        let start = s.len() - max_chars;
        format!("... ({} chars above)\n{}", s.len() - max_chars, &s[start..])
    }
}

/// 构建调试工具集（单个合并工具），注册到 ToolRegistry 时使用
/// 无状态：所有依赖通过 ToolContext 注入
pub fn build_debug_tools() -> Arc<DebugTool> {
    Arc::new(DebugTool)
}
