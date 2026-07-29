use crate::tools::{Tool, ToolContext};
use crate::types::{ToolOutput, ToolSchema};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

pub struct GraphQueryTool;

#[async_trait]
impl Tool for GraphQueryTool {
    fn name(&self) -> &str { "graph_query" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "graph_query".into(),
            description: "Query code graph symbols by name. Returns functions, classes, structs etc. with file locations.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Symbol name (substring match)" },
                    "limit": { "type": "integer", "default": 20 }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let name: String = serde_json::from_value(args["name"].clone())?;
        let limit = args["limit"].as_u64().unwrap_or(20) as u32;

        let symbols = ctx.code_graph.query_symbols(&name, limit)?;
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

// ==================== P2-8: graph_find / graph_callers / graph_callees ====================

/// graph_find - 按名称 + kind 查找符号
/// 设计文档 §8.4.1: graph_find(symbol, type)
pub struct GraphFindTool;

#[async_trait]
impl Tool for GraphFindTool {
    fn name(&self) -> &str { "graph_find" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "graph_find".into(),
            description: "Find symbols by name (exact match) and optional kind filter. Returns symbol locations with signatures. More precise than graph_query (which does substring match).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Exact symbol name" },
                    "kind": { "type": "string", "description": "Optional: filter by kind (function|class|struct|variable|method|trait|enum|constant|module|interface|type_alias|import)" },
                    "limit": { "type": "integer", "default": 20 }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let name: String = serde_json::from_value(args["name"].clone())?;
        let kind_filter: Option<String> = args["kind"].as_str().map(|s| s.to_string());
        let limit = args["limit"].as_u64().unwrap_or(20) as usize;

        let mut symbols = ctx.code_graph.query_symbols_exact(&name)?;
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
            "name": name,
            "kind_filter": kind_filter,
            "matches": result.len(),
            "symbols": result
        }) })
    }
}

/// graph_callers - 查找谁调用了指定函数
/// 设计文档 §8.4.1: graph_callers(fn)
pub struct GraphCallersTool;

#[async_trait]
impl Tool for GraphCallersTool {
    fn name(&self) -> &str { "graph_callers" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "graph_callers".into(),
            description: "Find all callers of a function (who calls this function). Returns caller symbol + call site locations. Requires code graph to be indexed.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Function name (callee)" },
                    "limit": { "type": "integer", "default": 50 }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let name: String = serde_json::from_value(args["name"].clone())?;
        let limit = args["limit"].as_u64().unwrap_or(50) as usize;

        let callers = ctx.code_graph.find_callers(&name)?;
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
            "function": name,
            "caller_count": result.len(),
            "callers": result,
            "hint": if result.is_empty() { "no callers found; try graph_index first" } else { "" }
        }) })
    }
}

/// graph_callees - 查找指定函数调用了哪些函数
/// 设计文档 §8.4.1: graph_callees(fn)
pub struct GraphCalleesTool;

#[async_trait]
impl Tool for GraphCalleesTool {
    fn name(&self) -> &str { "graph_callees" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "graph_callees".into(),
            description: "Find all functions called by a given function (what does this function call). Returns callee names + call sites + resolved symbols if indexed.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Function name (caller)" },
                    "limit": { "type": "integer", "default": 50 }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let name: String = serde_json::from_value(args["name"].clone())?;
        let limit = args["limit"].as_u64().unwrap_or(50) as usize;

        // 先按名称查找符号，拿到 symbol_id
        let symbols = ctx.code_graph.query_symbols_exact(&name)?;
        if symbols.is_empty() {
            return Ok(ToolOutput::Sync { result: serde_json::json!({
                "function": name,
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
            "function": name,
            "returned": result.len(),
            "total": total,
            "ambiguous": ambiguous,
            "matched_symbols": func_symbols.len(),
            "callees": result,
            "hint": if ambiguous { "multiple functions with same name found; aggregated callees from all matches" } else { "" }
        }) })
    }
}

/// graph_references - 查找符号的所有引用（导入此符号的位置）
/// 设计文档 §8.4.1: graph_references(symbol)
pub struct GraphReferencesTool;

#[async_trait]
impl Tool for GraphReferencesTool {
    fn name(&self) -> &str { "graph_references" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "graph_references".into(),
            description: "Find all references to a symbol across the codebase via code graph edges (calls/imports/extends/implements). Note: only graph edges are queried; fine-grained variable/type references are not yet indexed.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Symbol name" },
                    "limit": { "type": "integer", "default": 50 }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let name: String = serde_json::from_value(args["name"].clone())?;
        let limit = args["limit"].as_u64().unwrap_or(50) as usize;

        // P2-8: 查找所有指向 name 的边（calls/imports/extends/implements）
        let refs_edges = ctx.code_graph.find_references_by_name(&name)?;
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
            "symbol": name,
            "reference_count": refs.len(),
            "references": refs,
            "hint": if refs.is_empty() { "no references found; try graph_index first" } else { "" }
        }) })
    }
}
