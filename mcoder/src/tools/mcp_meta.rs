// MCP 元工具：mcp_list / mcp_call
// 不把每个 MCP server 的工具单独注册到 ToolRegistry，
// 而是通过这两个元工具让 LLM 按需发现和调用 MCP 工具。

use crate::tools::{Tool, ToolContext};
use crate::types::{ToolOutput, ToolSchema};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

/// mcp_list：列出所有已连接 MCP server 及其工具
pub struct McpListTool;

#[async_trait]
impl Tool for McpListTool {
    fn name(&self) -> &str {
        "mcp_list"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "mcp_list".into(),
            description: "List all connected MCP servers and their tools. Returns a JSON array of {server, tools: [{name, description, input_schema}]}.".into(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn execute(&self, _args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let manager = match &ctx.mcp_manager {
            Some(m) => m,
            None => {
                return Ok(ToolOutput::Sync {
                    result: json!([]),
                });
            }
        };

        let servers = manager.list_all_tools().await;
        let result: Vec<Value> = servers
            .into_iter()
            .map(|(server, tools)| {
                let tools_json: Vec<Value> = tools
                    .iter()
                    .map(|t| {
                        json!({
                            "name": t.name,
                            "description": t.description,
                            "input_schema": t.input_schema,
                        })
                    })
                    .collect();
                json!({
                    "server": server,
                    "tools": tools_json,
                })
            })
            .collect();

        Ok(ToolOutput::Sync {
            result: Value::Array(result),
        })
    }
}

/// mcp_call：调用指定 MCP server 上的工具
pub struct McpCallTool;

#[async_trait]
impl Tool for McpCallTool {
    fn name(&self) -> &str {
        "mcp_call"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "mcp_call".into(),
            description: "Call a tool on a specific MCP server. Use mcp_list first to discover available servers and tools.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "description": "MCP server name"
                    },
                    "tool": {
                        "type": "string",
                        "description": "Tool name to call on the server"
                    },
                    "args": {
                        "type": "object",
                        "description": "Arguments object to pass to the tool",
                        "default": {}
                    }
                },
                "required": ["server", "tool"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let manager: &Arc<crate::plugin::mcp::McpManager> = match &ctx.mcp_manager {
            Some(m) => m,
            None => {
                return Ok(ToolOutput::Error {
                    message: "No MCP manager available".into(),
                });
            }
        };

        let server = args
            .get("server")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'server' field"))?;
        let tool = args
            .get("tool")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'tool' field"))?;
        let call_args = args.get("args").cloned().unwrap_or(json!({}));

        match manager.call_tool(server, tool, &call_args).await {
            Ok(result) => Ok(ToolOutput::Sync { result }),
            Err(e) => Ok(ToolOutput::Error {
                message: e.to_string(),
            }),
        }
    }
}
