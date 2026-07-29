// 设计文档 §8.4.2: LSP 工具集（注册到 ToolRegistry）
// 6 个工具：lsp_diagnose / lsp_hover / lsp_definition / lsp_references / lsp_rename / lsp_format
// 无状态：所有依赖通过 ToolContext 注入（lsp_manager / journal）
// LspRenameTool / LspFormatTool 通过 ctx.journal 记录文件变更
#![allow(dead_code)]

use crate::lsp::{apply_text_edits, path_to_uri, uri_to_path};
use crate::tools::journal::FileJournal;
use crate::tools::{Tool, ToolContext};
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

/// 构建 LSP 工具集（6 个工具）
/// 设计文档 §8.4.2: 注册到 ToolRegistry
/// lsp_diagnose / lsp_hover / lsp_definition / lsp_references / lsp_rename / lsp_format
/// 无状态：所有依赖通过 ToolContext 注入
pub fn build_lsp_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(LspDiagnoseTool),
        Arc::new(LspHoverTool),
        Arc::new(LspDefinitionTool),
        Arc::new(LspReferencesTool),
        Arc::new(LspRenameTool),
        Arc::new(LspFormatTool),
    ]
}

// ==================== 辅助函数 ====================

/// 解析 file 参数为绝对路径
/// 接受 "file" 或 "path" 字段名
fn parse_file_arg(args: &Value) -> Result<PathBuf> {
    let path_str: String = serde_json::from_value(args["file"].clone())
        .or_else(|_| serde_json::from_value(args["path"].clone()))
        .context("file required")?;
    Ok(PathBuf::from(path_str))
}

/// 解析 line/col 参数（0-based，LSP 规范）
fn parse_position(args: &Value) -> Result<(u32, u32)> {
    let line = args["line"].as_u64().context("line required (0-based)")? as u32;
    let col = args["col"]
        .as_u64()
        .or_else(|| args["character"].as_u64())
        .or_else(|| args["column"].as_u64())
        .context("col required (0-based)")? as u32;
    Ok((line, col))
}

// ==================== lsp_diagnose ====================

/// lsp_diagnose(file) - 获取文件诊断
/// 设计文档 §8.4.2: 拉取文件诊断信息（errors/warnings）
pub struct LspDiagnoseTool;

#[async_trait]
impl Tool for LspDiagnoseTool {
    fn name(&self) -> &str {
        "lsp_diagnose"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "lsp_diagnose".into(),
            description: "Get LSP diagnostics for a file (errors/warnings). Returns array of \
                          {range: {start, end}, severity, message, source}. File must be in a \
                          supported language (Rust/TS/Go/Python).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "File path (absolute or relative to project)" }
                },
                "required": ["file"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = parse_file_arg(&args)?;
        // 确保文件在 LSP server 中已打开
        let client = ctx.lsp_manager.ensure_open(&path).await?;
        let client = match client {
            Some(c) => c,
            None => {
                return Ok(ToolOutput::Error {
                    message: format!(
                        "no LSP server available for file: {} (unsupported language?)",
                        path.display()
                    ),
                });
            }
        };
        // P2-3 修复：去除 sleep hack
        // diagnostics() 内部已实现：优先查 push 缓存，无则 pull（LSP 3.17+）
        // pull diagnostics 是同步请求-响应，无需等待
        let uri = path_to_uri(&path);
        let diags = client.diagnostics(&uri).await?;

        let result = serde_json::json!({
            "file": path.display().to_string(),
            "diagnostics": diags,
            "count": diags.len(),
        });
        Ok(ToolOutput::Sync { result })
    }
}

// ==================== lsp_hover ====================

/// lsp_hover(file, line, col) - 获取 hover 信息
/// 设计文档 §8.4.2: 显示符号的类型、文档等信息
pub struct LspHoverTool;

#[async_trait]
impl Tool for LspHoverTool {
    fn name(&self) -> &str {
        "lsp_hover"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "lsp_hover".into(),
            description: "Get hover info (type/doc) for symbol at position. \
                          line/col are 0-based (LSP standard).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string" },
                    "line": { "type": "integer", "description": "0-based line number" },
                    "col": { "type": "integer", "description": "0-based column (character)" }
                },
                "required": ["file", "line", "col"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = parse_file_arg(&args)?;
        let (line, col) = parse_position(&args)?;

        let client = ctx.lsp_manager.ensure_open(&path).await?;
        let client = match client {
            Some(c) => c,
            None => {
                return Ok(ToolOutput::Error {
                    message: format!(
                        "no LSP server available for file: {}",
                        path.display()
                    ),
                });
            }
        };

        let uri = path_to_uri(&path);
        let hover_result = client.hover(&uri, line, col).await?;

        // Hover 结果可能是 null（无信息）或 { contents, range }
        let result = match &hover_result {
            Value::Null => serde_json::json!({
                "file": path.display().to_string(),
                "line": line,
                "col": col,
                "contents": null,
                "note": "no hover info available at this position"
            }),
            _ => serde_json::json!({
                "file": path.display().to_string(),
                "line": line,
                "col": col,
                "contents": hover_result.get("contents").cloned().unwrap_or(Value::Null),
                "range": hover_result.get("range").cloned(),
            }),
        };
        Ok(ToolOutput::Sync { result })
    }
}

// ==================== lsp_definition ====================

/// lsp_definition(file, line, col) - 跳转定义
/// 设计文档 §8.4.2: 查找符号定义位置
pub struct LspDefinitionTool;

#[async_trait]
impl Tool for LspDefinitionTool {
    fn name(&self) -> &str {
        "lsp_definition"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "lsp_definition".into(),
            description: "Find definition of symbol at position. Returns Location array \
                          [{uri, range: {start, end}}]. line/col are 0-based.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string" },
                    "line": { "type": "integer", "description": "0-based line" },
                    "col": { "type": "integer", "description": "0-based column" }
                },
                "required": ["file", "line", "col"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = parse_file_arg(&args)?;
        let (line, col) = parse_position(&args)?;

        let client = ctx.lsp_manager.ensure_open(&path).await?;
        let client = match client {
            Some(c) => c,
            None => {
                return Ok(ToolOutput::Error {
                    message: format!(
                        "no LSP server available for file: {}",
                        path.display()
                    ),
                });
            }
        };

        let uri = path_to_uri(&path);
        let def_result = client.definition(&uri, line, col).await?;

        // definition 可能返回 Location | Location[] | LocationLink[] | null
        let locations = normalize_locations(&def_result);
        let result = serde_json::json!({
            "file": path.display().to_string(),
            "line": line,
            "col": col,
            "locations": locations,
            "count": locations.len(),
        });
        Ok(ToolOutput::Sync { result })
    }
}

// ==================== lsp_references ====================

/// lsp_references(file, line, col) - 引用查找
/// 设计文档 §8.4.2: 查找符号的所有引用位置
pub struct LspReferencesTool;

#[async_trait]
impl Tool for LspReferencesTool {
    fn name(&self) -> &str {
        "lsp_references"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "lsp_references".into(),
            description: "Find all references to symbol at position. Returns Location array. \
                          line/col are 0-based.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string" },
                    "line": { "type": "integer", "description": "0-based line" },
                    "col": { "type": "integer", "description": "0-based column" }
                },
                "required": ["file", "line", "col"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = parse_file_arg(&args)?;
        let (line, col) = parse_position(&args)?;

        let client = ctx.lsp_manager.ensure_open(&path).await?;
        let client = match client {
            Some(c) => c,
            None => {
                return Ok(ToolOutput::Error {
                    message: format!(
                        "no LSP server available for file: {}",
                        path.display()
                    ),
                });
            }
        };

        let uri = path_to_uri(&path);
        let ref_result = client.references(&uri, line, col).await?;

        let locations = normalize_locations(&ref_result);
        let result = serde_json::json!({
            "file": path.display().to_string(),
            "line": line,
            "col": col,
            "references": locations,
            "count": locations.len(),
        });
        Ok(ToolOutput::Sync { result })
    }
}

// ==================== lsp_rename ====================

/// lsp_rename(file, line, col, new_name) - 重命名符号
/// 设计文档 §8.4.2: 跨文件重命名（基于 journal 记录文件变更）
pub struct LspRenameTool;

#[async_trait]
impl Tool for LspRenameTool {
    fn name(&self) -> &str {
        "lsp_rename"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "lsp_rename".into(),
            description: "Rename symbol at position across the project. Applies WorkspaceEdit \
                          to disk and records changes via journal for undo. line/col are 0-based.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string" },
                    "line": { "type": "integer", "description": "0-based line" },
                    "col": { "type": "integer", "description": "0-based column" },
                    "new_name": { "type": "string", "description": "New symbol name" }
                },
                "required": ["file", "line", "col", "new_name"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = parse_file_arg(&args)?;
        let (line, col) = parse_position(&args)?;
        let new_name: String = serde_json::from_value(args["new_name"].clone())
            .or_else(|_| serde_json::from_value(args["newName"].clone()))
            .context("new_name required")?;

        let client = ctx.lsp_manager.ensure_open(&path).await?;
        let client = match client {
            Some(c) => c,
            None => {
                return Ok(ToolOutput::Error {
                    message: format!(
                        "no LSP server available for file: {}",
                        path.display()
                    ),
                });
            }
        };

        let uri = path_to_uri(&path);
        // 调用 textDocument/rename 获取 WorkspaceEdit
        let edit = client.rename(&uri, line, col, &new_name).await?;

        // 应用 WorkspaceEdit 到磁盘，记录 journal
        let affected = apply_workspace_edit(&edit, &ctx.journal).await?;

        // 应用后通知 LSP server 文件已变更（didChange）
        for file_path in &affected {
            let file_uri = path_to_uri(file_path);
            if let Ok(new_text) = tokio::fs::read_to_string(file_path).await {
                let _ = client.did_change(&file_uri, &new_text).await;
            }
        }

        let result = serde_json::json!({
            "file": path.display().to_string(),
            "line": line,
            "col": col,
            "new_name": new_name,
            "affected_files": affected.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "affected_count": affected.len(),
        });
        Ok(ToolOutput::Sync { result })
    }
}

// ==================== lsp_format ====================

/// lsp_format(file) - 格式化文件
/// 设计文档 §8.4.2: 调用 LSP server 的 formatting 能力
pub struct LspFormatTool;

#[async_trait]
impl Tool for LspFormatTool {
    fn name(&self) -> &str {
        "lsp_format"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "lsp_format".into(),
            description: "Format a file using LSP server's formatting capability. \
                          Applies TextEdit[] to disk and records via journal for undo.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string" }
                },
                "required": ["file"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = parse_file_arg(&args)?;

        let client = ctx.lsp_manager.ensure_open(&path).await?;
        let client = match client {
            Some(c) => c,
            None => {
                return Ok(ToolOutput::Error {
                    message: format!(
                        "no LSP server available for file: {}",
                        path.display()
                    ),
                });
            }
        };

        let uri = path_to_uri(&path);
        // textDocument/formatting 返回 TextEdit[] | null
        let edits = client.formatting(&uri).await?;

        // 无 edits：文件已格式化好
        if edits.is_null() || edits.as_array().map(|a| a.is_empty()).unwrap_or(true) {
            return Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "file": path.display().to_string(),
                    "applied": false,
                    "note": "no formatting changes needed"
                }),
            });
        }

        // 读取原文本
        let before = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("reading {}", path.display()))?;

        // 应用 TextEdit[]
        let after = apply_text_edits(&before, &edits);

        // 写回磁盘
        tokio::fs::write(&path, &after)
            .await
            .with_context(|| format!("writing {}", path.display()))?;

        // 记录到 journal（支持 undo）
        ctx.journal.record(&path, &before, &after, "lsp_format");

        // 通知 LSP server 文件已变更
        let _ = client.did_change(&uri, &after).await;

        let result = serde_json::json!({
            "file": path.display().to_string(),
            "applied": true,
            "edits_count": edits.as_array().map(|a| a.len()).unwrap_or(0),
        });
        Ok(ToolOutput::Sync { result })
    }
}

// ==================== 内部辅助函数 ====================

/// 将 LSP definition/references 返回值归一化为 Location 数组
/// 处理多种返回形式：
/// - null
/// - Location (单个对象)
/// - Location[] (数组)
/// - LocationLink[] (含 targetUri/targetRange)
fn normalize_locations(result: &Value) -> Vec<Value> {
    match result {
        Value::Null => vec![],
        Value::Array(arr) => {
            arr.iter()
                .map(|item| {
                    // LocationLink 形式：{ targetUri, targetRange, ... }
                    if let Some(target_uri) = item.get("targetUri") {
                        serde_json::json!({
                            "uri": target_uri,
                            "range": item.get("targetRange").cloned().unwrap_or(Value::Null),
                        })
                    } else {
                        // Location 形式：{ uri, range }
                        item.clone()
                    }
                })
                .collect()
        }
        // 单个 Location 对象
        obj if obj.is_object() => vec![obj.clone()],
        _ => vec![],
    }
}

/// 应用 WorkspaceEdit 到磁盘
/// 支持 changes: Map<uri, TextEdit[]> 和 documentChanges: TextDocumentEdit[]
/// 每个文件变更记录到 journal（用于 undo）
async fn apply_workspace_edit(
    edit: &Value,
    journal: &Arc<FileJournal>,
) -> Result<Vec<PathBuf>> {
    let mut affected: Vec<PathBuf> = Vec::new();

    // 1. 处理 changes: Map<uri, TextEdit[]>
    if let Some(changes) = edit.get("changes").and_then(|v| v.as_object()) {
        for (uri, edits) in changes {
            let path = uri_to_path(uri);
            let before = tokio::fs::read_to_string(&path)
                .await
                .unwrap_or_default();
            let after = apply_text_edits(&before, edits);
            tokio::fs::write(&path, &after)
                .await
                .with_context(|| format!("writing {}", path.display()))?;
            journal.record(&path, &before, &after, "lsp_rename");
            affected.push(path);
        }
    }

    // 2. 处理 documentChanges: TextDocumentEdit[] / CreateFile / RenameFile / DeleteFile
    if let Some(doc_changes) = edit.get("documentChanges").and_then(|v| v.as_array()) {
        for change in doc_changes {
            // TextDocumentEdit: { textDocument: { uri, version }, edits: TextEdit[] }
            if let Some(uri) = change
                .get("textDocument")
                .and_then(|td| td.get("uri"))
                .and_then(|v| v.as_str())
            {
                let path = uri_to_path(uri);
                let before = tokio::fs::read_to_string(&path)
                    .await
                    .unwrap_or_default();
                let edits = change.get("edits").cloned().unwrap_or(Value::Array(vec![]));
                let after = apply_text_edits(&before, &edits);
                tokio::fs::write(&path, &after)
                    .await
                    .with_context(|| format!("writing {}", path.display()))?;
                journal.record(&path, &before, &after, "lsp_rename");
                affected.push(path);
            }
            // 其他类型（CreateFile/RenameFile/DeleteFile）暂不处理
        }
    }

    Ok(affected)
}
