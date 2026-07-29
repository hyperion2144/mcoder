// 设计文档 §8.4.3: DAP 调试工具集
// 7 个工具注册到 ToolRegistry，供 agent 自驱调试
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

/// debug_start - 启动调试会话
/// config: lang/program/args/cwd/stop_on_entry
/// 自动选择 adapter：rust→lldb-dap, node→node, python→debugpy, go→dlv
pub struct DebugStartTool;

#[async_trait]
impl Tool for DebugStartTool {
    fn name(&self) -> &str {
        "debug_start"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "debug_start".into(),
            description: "Start a debug session via DAP. Auto-selects adapter by lang: rust→lldb-dap, node→node, python→debugpy, go→dlv. Supports launch and attach (attach_pid).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "lang": {
                        "type": "string",
                        "enum": ["rust", "node", "python", "go"],
                        "description": "Debug language"
                    },
                    "program": {
                        "type": "string",
                        "description": "Executable path (for launch) or symbol-bearing binary (for attach)"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Command-line arguments passed to program"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory"
                    },
                    "stop_on_entry": {
                        "type": "boolean",
                        "description": "Whether to stop at entry point"
                    },
                    "attach_pid": {
                        "type": "integer",
                        "description": "Optional: attach to running process by pid (instead of launch)"
                    }
                },
                "required": ["lang", "program"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
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
                    "hint": "Use debug_set_breakpoint to add breakpoints, then debug_continue or debug_step to control execution."
                }),
            }),
            Err(e) => Ok(ToolOutput::Error {
                message: format!("failed to start debug session: {}", e),
            }),
        }
    }
}

/// debug_set_breakpoint - 设置断点（支持条件断点）
pub struct DebugSetBreakpointTool;

#[async_trait]
impl Tool for DebugSetBreakpointTool {
    fn name(&self) -> &str {
        "debug_set_breakpoint"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "debug_set_breakpoint".into(),
            description: "Set a breakpoint at file:line. Optional condition for conditional breakpoint. Returns breakpoint_id and verified status.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "Source file path"
                    },
                    "line": {
                        "type": "integer",
                        "description": "Line number (1-based)"
                    },
                    "condition": {
                        "type": "string",
                        "description": "Optional: conditional breakpoint expression"
                    }
                },
                "required": ["file", "line"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
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
                    message: "no active debug session; call debug_start first".into(),
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
}

/// debug_continue - 继续执行
pub struct DebugContinueTool;

#[async_trait]
impl Tool for DebugContinueTool {
    fn name(&self) -> &str {
        "debug_continue"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "debug_continue".into(),
            description: "Continue execution of the active debug session. Returns after the next stop (breakpoint/step/exception/termination).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn execute(&self, _args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let session = match ctx.debug_manager.default_session().await {
            Some(s) => s,
            None => {
                return Ok(ToolOutput::Error {
                    message: "no active debug session; call debug_start first".into(),
                });
            }
        };

        // 检查状态：必须处于 stopped
        let state = session.state().await;
        if state == super::SessionState::Terminated {
            return Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "status": "terminated",
                    "hint": "session already terminated; call debug_start to begin a new session"
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
                        "hint": "use debug_get_state to inspect current frame and variables"
                    }),
                })
            }
            Err(e) => Ok(ToolOutput::Error {
                message: format!("debug_continue failed: {}", e),
            }),
        }
    }
}

/// debug_step - 单步执行（granularity: over|in|out）
pub struct DebugStepTool;

#[async_trait]
impl Tool for DebugStepTool {
    fn name(&self) -> &str {
        "debug_step"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "debug_step".into(),
            description: "Step execution: granularity=over (next line, skip function calls), in (step into function), out (step out of current function).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "granularity": {
                        "type": "string",
                        "enum": ["over", "in", "out"],
                        "description": "Step granularity (default: over)"
                    }
                }
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let granularity = args
            .get("granularity")
            .and_then(|v| v.as_str())
            .unwrap_or("over");

        let session = match ctx.debug_manager.default_session().await {
            Some(s) => s,
            None => {
                return Ok(ToolOutput::Error {
                    message: "no active debug session; call debug_start first".into(),
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
                message: format!("debug_step failed: {}", e),
            }),
        }
    }
}

/// debug_eval - 求值表达式
pub struct DebugEvalTool;

#[async_trait]
impl Tool for DebugEvalTool {
    fn name(&self) -> &str {
        "debug_eval"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "debug_eval".into(),
            description: "Evaluate an expression in the current stack frame context. Useful for inspecting variables, calling methods, or testing conditions.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "Expression to evaluate (language-specific syntax)"
                    }
                },
                "required": ["expression"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let expression = args
            .get("expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required field: expression"))?;

        let session = match ctx.debug_manager.default_session().await {
            Some(s) => s,
            None => {
                return Ok(ToolOutput::Error {
                    message: "no active debug session; call debug_start first".into(),
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
                message: format!("debug_eval failed: {}", e),
            }),
        }
    }
}

/// debug_get_state - 获取当前调试状态
/// 返回 {stopped, thread_id, frames, variables}
/// P2-5 修复：输出超过阈值时存入 sandbox 返回 handle + 摘要
pub struct DebugGetStateTool;

/// P2-5 修复：大输出阈值（4KB），超过则走 sandbox
const DEBUG_STATE_SANDBOX_THRESHOLD: usize = 4 * 1024;

#[async_trait]
impl Tool for DebugGetStateTool {
    fn name(&self) -> &str {
        "debug_get_state"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "debug_get_state".into(),
            description: "Get current debug state: stopped flag, thread id, call stack frames (file/line/function), and local variables (name/value/type). Large output is stored in sandbox and a handle is returned for pagination via sandbox_read.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn execute(&self, _args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let session = match ctx.debug_manager.default_session().await {
            Some(s) => s,
            None => {
                return Ok(ToolOutput::Error {
                    message: "no active debug session; call debug_start first".into(),
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
}

/// debug_stop - 停止调试会话
pub struct DebugStopTool;

#[async_trait]
impl Tool for DebugStopTool {
    fn name(&self) -> &str {
        "debug_stop"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "debug_stop".into(),
            description: "Stop and remove the active debug session. Sends disconnect to adapter and kills the adapter process.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Optional: specific session id to stop. If omitted, stops the default session."
                    }
                }
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
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
                message: format!("debug_stop failed: {}", e),
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

/// 构建调试工具集（7 个工具），注册到 ToolRegistry 时使用
/// 无状态：所有依赖通过 ToolContext 注入
pub fn build_debug_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(DebugStartTool),
        Arc::new(DebugSetBreakpointTool),
        Arc::new(DebugContinueTool),
        Arc::new(DebugStepTool),
        Arc::new(DebugEvalTool),
        Arc::new(DebugGetStateTool),
        Arc::new(DebugStopTool),
    ]
}
