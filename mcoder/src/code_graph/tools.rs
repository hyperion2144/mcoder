use crate::tools::{Tool, ToolContext};
use crate::types::{ToolOutput, ToolSchema};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

/// graph_search - 按名称搜索符号
/// action=symbol: 子串匹配（原 graph_query）
/// action=file: 精确匹配 + kind 过滤（原 graph_find）
pub struct GraphSearchTool;

#[async_trait]
impl Tool for GraphSearchTool {
    fn name(&self) -> &str { "graph_search" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "graph_search".into(),
            description: "Search code graph symbols by name. action=symbol: substring match (returns functions, classes, structs etc. with file locations). action=file: exact match with optional kind filter (more precise).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["symbol", "file"], "description": "symbol=substring match, file=exact match with kind filter" },
                    "pattern": { "type": "string", "description": "Symbol name to search" },
                    "path": { "type": "string", "description": "Optional: filter results by file path prefix" },
                    "kind": { "type": "string", "description": "Optional: filter by kind (function|class|struct|variable|method|trait|enum|constant|module|interface|type_alias|import). Used with action=file." },
                    "limit": { "type": "integer", "default": 20 }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let action: String = serde_json::from_value(args["action"].clone())?;

        match action.as_str() {
            // 原 graph_query: 子串匹配
            "symbol" => {
                let pattern: String = serde_json::from_value(args["pattern"].clone())?;
                let limit = args["limit"].as_u64().unwrap_or(20) as u32;

                let symbols = ctx.code_graph.query_symbols(&pattern, limit)?;
                let result: Vec<Value> = symbols.iter().map(|s| serde_json::json!({
                    "name": s.name,
                    "kind": s.kind.as_str(),
                    "file": s.file_path.display().to_string(),
                    "line": s.start_line,
                    "end_line": s.end_line,
                    "signature": s.signature,
                })).collect();

                Ok(ToolOutput::Sync { result: serde_json::Value::Array(result) })
            }
            // 原 graph_find: 精确匹配 + kind 过滤
            "file" => {
                let pattern: String = serde_json::from_value(args["pattern"].clone())?;
                let kind_filter: Option<String> = args["kind"].as_str().map(|s| s.to_string());
                let limit = args["limit"].as_u64().unwrap_or(20) as usize;

                let mut symbols = ctx.code_graph.query_symbols_exact(&pattern)?;
                if let Some(k) = &kind_filter {
                    symbols.retain(|s| s.kind.as_str() == k.as_str());
                }
                symbols.truncate(limit);

                let result: Vec<Value> = symbols.iter().map(|s| serde_json::json!({
                    "id": s.id,
                    "name": s.name,
                    "kind": s.kind.as_str(),
                    "file": s.file_path.display().to_string(),
                    "line": s.start_line,
                    "end_line": s.end_line,
                    "signature": s.signature,
                })).collect();

                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "name": pattern,
                    "kind_filter": kind_filter,
                    "matches": result.len(),
                    "symbols": result
                }) })
            }
            other => anyhow::bail!("unknown action: {} (use symbol|file)", other),
        }
    }
}

pub struct GraphFileSymbolsTool;

#[async_trait]
impl Tool for GraphFileSymbolsTool {
    fn name(&self) -> &str { "graph_file_symbols" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "graph_file_symbols".into(),
            description: "List all symbols in a file from the code graph".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path: String = serde_json::from_value(args["path"].clone())?;
        let symbols = ctx.code_graph.get_file_symbols(std::path::Path::new(&path))?;
        let result: Vec<Value> = symbols.iter().map(|s| serde_json::json!({
            "name": s.name,
            "kind": s.kind.as_str(),
            "line": s.start_line,
            "end_line": s.end_line,
        })).collect();

        Ok(ToolOutput::Sync { result: serde_json::Value::Array(result) })
    }
}

pub struct GraphIndexTool;

#[async_trait]
impl Tool for GraphIndexTool {
    fn name(&self) -> &str { "graph_index" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "graph_index".into(),
            description: "Index a file or directory into the code graph. Use after file changes to update the graph.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File or directory to index" }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path: String = serde_json::from_value(args["path"].clone())?;
        let p = std::path::Path::new(&path);

        if p.is_dir() {
            let stats = ctx.code_graph.index_dir(p)?;
            Ok(ToolOutput::Sync { result: serde_json::json!({
                "indexed": stats.files_indexed,
                "found": stats.files_found,
                "errors": stats.errors
            })})
        } else {
            ctx.code_graph.index_file(p)?;
            Ok(ToolOutput::Sync { result: serde_json::json!({
                "indexed": 1,
                "path": path
            })})
        }
    }
}

/// graph_relations - 查找符号的调用关系
/// direction=callers: 谁调用了此函数（原 graph_callers）
/// direction=callees: 此函数调用了哪些函数（原 graph_callees）
/// direction=references: 符号的所有引用（原 graph_references）
pub struct GraphRelationsTool;

#[async_trait]
impl Tool for GraphRelationsTool {
    fn name(&self) -> &str { "graph_relations" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "graph_relations".into(),
            description: "Find symbol relations via code graph edges. direction=callers: who calls this function. direction=callees: what this function calls. direction=references: all references to a symbol (calls/imports/extends/implements).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "direction": { "type": "string", "enum": ["callers", "callees", "references"], "description": "Type of relation to query" },
                    "symbol": { "type": "string", "description": "Symbol name to query relations for" },
                    "limit": { "type": "integer", "default": 50 }
                },
                "required": ["direction"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let direction: String = serde_json::from_value(args["direction"].clone())?;
        let symbol: String = serde_json::from_value(args["symbol"].clone())?;
        let limit = args["limit"].as_u64().unwrap_or(50) as usize;

        match direction.as_str() {
            // 原 graph_callers: 查找谁调用了指定函数
            "callers" => {
                let callers = ctx.code_graph.find_callers(&symbol)?;
                let mut result: Vec<Value> = Vec::new();
                for (caller_sym, edge) in callers.iter().take(limit) {
                    result.push(serde_json::json!({
                        "caller": {
                            "name": caller_sym.name,
                            "kind": caller_sym.kind.as_str(),
                            "file": caller_sym.file_path.display().to_string(),
                            "line": caller_sym.start_line,
                        },
                        "call_site": {
                            "file": edge.file_path.display().to_string(),
                            "line": edge.line,
                            "col": edge.col,
                        }
                    }));
                }

                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "function": symbol,
                    "caller_count": result.len(),
                    "callers": result,
                    "hint": if result.is_empty() { "no callers found; try graph_index first" } else { "" }
                }) })
            }
            // 原 graph_callees: 查找指定函数调用了哪些函数
            "callees" => {
                // 先按名称查找符号，拿到 symbol_id
                let symbols = ctx.code_graph.query_symbols_exact(&symbol)?;
                if symbols.is_empty() {
                    return Ok(ToolOutput::Sync { result: serde_json::json!({
                        "function": symbol,
                        "callee_count": 0,
                        "callees": [],
                        "hint": "function not found in code graph; try graph_index first"
                    }) });
                }

                // P1-CG-2 修复：聚合所有同名 Function/Method 符号的 callees，而非只取第一个
                let func_symbols: Vec<_> = symbols.iter()
                    .filter(|s| matches!(s.kind,
                        crate::code_graph::schema::SymbolKind::Function
                        | crate::code_graph::schema::SymbolKind::Method
                    ))
                    .collect();

                let ambiguous = func_symbols.len() > 1;

                let mut all_callees: Vec<(String, crate::code_graph::schema::SymbolEdge, Option<crate::code_graph::schema::Symbol>)> = Vec::new();
                for sym in &func_symbols {
                    if let Some(id) = sym.id {
                        let mut callees = ctx.code_graph.find_callees(id)?;
                        all_callees.append(&mut callees);
                    }
                }

                // 按文件+行号排序
                all_callees.sort_by_key(|(_, e, _)| (e.file_path.clone(), e.line));

                let total = all_callees.len();
                let mut result: Vec<Value> = Vec::new();
                for (callee_name, edge, callee_sym) in all_callees.iter().take(limit) {
                    let entry = serde_json::json!({
                        "callee": callee_name,
                        "resolved": callee_sym.as_ref().map(|s| serde_json::json!({
                            "name": s.name,
                            "kind": s.kind.as_str(),
                            "file": s.file_path.display().to_string(),
                            "line": s.start_line,
                        })),
                        "call_site": {
                            "file": edge.file_path.display().to_string(),
                            "line": edge.line,
                            "col": edge.col,
                        }
                    });
                    result.push(entry);
                }

                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "function": symbol,
                    "returned": result.len(),
                    "total": total,
                    "ambiguous": ambiguous,
                    "matched_symbols": func_symbols.len(),
                    "callees": result,
                    "hint": if ambiguous { "multiple functions with same name found; aggregated callees from all matches" } else { "" }
                }) })
            }
            // 原 graph_references: 查找符号的所有引用
            "references" => {
                // 查找所有指向 symbol 的边（calls/imports/extends/implements）
                let refs_edges = ctx.code_graph.find_references_by_name(&symbol)?;
                let mut refs: Vec<Value> = Vec::new();

                for (sym, edge) in refs_edges.iter().take(limit) {
                    refs.push(serde_json::json!({
                        "symbol": {
                            "name": sym.name,
                            "kind": sym.kind.as_str(),
                            "file": sym.file_path.display().to_string(),
                            "line": sym.start_line,
                        },
                        "reference_site": {
                            "file": edge.file_path.display().to_string(),
                            "line": edge.line,
                            "col": edge.col,
                        },
                        "edge_type": edge.edge_type.as_str()
                    }));
                }

                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "symbol": symbol,
                    "reference_count": refs.len(),
                    "references": refs,
                    "hint": if refs.is_empty() { "no references found; try graph_index first" } else { "" }
                }) })
            }
            other => anyhow::bail!("unknown direction: {} (use callers|callees|references)", other),
        }
    }
}
