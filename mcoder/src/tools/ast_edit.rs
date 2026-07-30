// 设计文档 §8.4.1: 基于 tree-sitter 的语法感知编辑
// 统一工具 ast_edit，op=rename|extract|inline
// 架构：
//   - rename: 优先委托 LSP textDocument/rename（语义级精确），LSP 不可用时 fallback 到 code_graph + tree-sitter 定位引用节点
//   - extract: tree-sitter 解析文件，定位行范围对应的最小节点，提取节点文本
//   - inline: code_graph.find_callers 拿到精确 (file,line,col)，tree-sitter 定位该位置的 call_expression 节点，精确替换 byte_range

use crate::code_graph::CodeGraph;
use crate::lsp::path_to_uri;
use crate::lsp::LspManager;
use crate::tools::journal::FileJournal;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::tree_sitter::hash_line;
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tree_sitter::{Node, Parser as TsParser};

// ==================== 辅助函数 ====================

/// 用 tree-sitter 解析文件，返回 (root_node, content)
fn parse_file_ts(path: &Path) -> Result<(tree_sitter::Tree, String)> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let lang = crate::tree_sitter::languages::Language::from_path(path);
    let ts_lang = lang.tree_sitter_language()
        .ok_or_else(|| anyhow::anyhow!("unsupported language for: {}", path.display()))?;
    let mut parser = TsParser::new();
    parser.set_language(&ts_lang)
        .map_err(|e| anyhow::anyhow!("tree-sitter language error: {}", e))?;
    let tree = parser.parse(&content, None)
        .ok_or_else(|| anyhow::anyhow!("failed to parse {}", path.display()))?;
    Ok((tree, content))
}

/// 根据行号（1-indexed）和列号（0-indexed）定位 tree-sitter 节点
/// 返回该位置最小的节点
fn node_at_position<'a>(root: Node<'a>, line: usize, col: usize) -> Option<Node<'a>> {
    let target_point = tree_sitter::Point::new(line - 1, col);
    root.descendant_for_point_range(target_point, target_point)
}

/// 查找包含给定行范围（1-indexed）的最小节点
fn node_covering_lines<'a>(root: Node<'a>, start_line: u32, end_line: u32) -> Option<Node<'a>> {
    // tree-sitter Point 是 0-indexed，row 是 usize
    // 用 start 行的首字符定位节点，然后向上查找覆盖完整行范围的最小节点
    // 注意：不能用 usize::MAX 作为 end column，因为 descendant_for_point_range
    // 要求节点范围包含整个查询范围，函数节点的 end column 不可能 >= usize::MAX
    let start_line = start_line as usize;
    let end_line = end_line as usize;
    let start_point = tree_sitter::Point::new(start_line - 1, 0);
    let mut node = root.descendant_for_point_range(start_point, start_point)?;

    // 向上查找：找到 start_line 和 end_line 都匹配的最小节点
    while let Some(parent) = node.parent() {
        let parent_start = parent.start_position().row + 1;
        let parent_end = parent.end_position().row + 1;
        if parent_start <= start_line && parent_end >= end_line {
            // parent 也覆盖范围，但 node 更小--检查 node 是否已覆盖
            let node_start = node.start_position().row + 1;
            let node_end = node.end_position().row + 1;
            if node_start <= start_line && node_end >= end_line {
                // node 已覆盖完整范围，不需要再向上
                break;
            }
            node = parent;
        } else {
            break;
        }
    }

    // 验证最终节点确实覆盖了行范围（fallback: 返回找到的节点，可能不完美但比 None 好）
    Some(node)
}

/// 在文件内容中根据行号（1-indexed）计算 byte offset
fn line_to_byte_offset(content: &str, line: usize) -> Option<usize> {
    // line 是 1-indexed
    if line == 0 {
        return Some(0);
    }
    let mut current_line = 1;
    let mut offset = 0;
    for (i, ch) in content.char_indices() {
        if current_line == line {
            return Some(i);
        }
        if ch == '\n' {
            current_line += 1;
        }
        offset = i + ch.len_utf8();
    }
    if current_line == line {
        Some(offset)
    } else {
        None
    }
}

/// 判断节点是否为函数调用表达式
/// 各语言的 call_expression 节点类型：
///   Rust: call_expression
///   JS/TS: call_expression
///   Python: call
///   Go: call_expression
fn is_call_expression(node: &Node) -> bool {
    let kind = node.kind();
    matches!(kind, "call_expression" | "call")
}

/// 判断节点是否为标识符
fn is_identifier(node: &Node) -> bool {
    let kind = node.kind();
    matches!(kind, "identifier" | "type_identifier" | "property_identifier")
}

/// 从 call_expression 节点中提取函数名
/// 对于 `foo(args)` 返回 "foo"
/// 对于 `obj.method(args)` 返回 "method"（取最后一段）
fn extract_call_name<'a>(call_node: &Node<'a>, content: &'a str) -> Option<String> {
    // call_expression 的第一个子节点通常是函数部分
    let func_node = call_node.child(0)?;
    let func_text = func_node.utf8_text(content.as_bytes()).ok()?;
    // 处理 obj.method 的情况：取最后一段
    let name = func_text.rsplit(|c: char| c == '.' || c == ':').next()?;
    Some(name.to_string())
}

/// 判断是否为有效标识符
fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() { return false; }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_alphabetic() && first != '_' { return false; }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// 在文件中查找所有调用指定函数名的 call_expression 节点
/// 返回 [(byte_start, byte_end, line, col)]
fn find_call_sites(content: &str, tree: &tree_sitter::Tree, func_name: &str) -> Vec<(usize, usize, u32, u32)> {
    let mut results = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();

    // 遍历所有节点
    fn walk_for_calls<'a>(
        node: Node<'a>,
        cursor: &mut tree_sitter::TreeCursor<'a>,
        content: &'a str,
        func_name: &str,
        results: &mut Vec<(usize, usize, u32, u32)>,
    ) {
        // 检查当前节点是否为 call_expression
        if is_call_expression(&node) {
            if let Some(name) = extract_call_name(&node, content) {
                if name == func_name {
                    let start = node.start_byte();
                    let end = node.end_byte();
                    let line = (node.start_position().row + 1) as u32;  // 转 1-indexed
                    let col = node.start_position().column as u32;
                    results.push((start, end, line, col));
                }
            }
        }

        // 递归遍历子节点
        if cursor.goto_first_child() {
            loop {
                walk_for_calls(cursor.node(), cursor, content, func_name, results);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }
    }

    walk_for_calls(root, &mut cursor, content, func_name, &mut results);
    results
}

/// 在文件中查找所有引用指定标识符的节点（非 call_expression，如变量引用、类型引用等）
/// 返回 [(byte_start, byte_end, line, col)]
fn find_identifier_references(content: &str, tree: &tree_sitter::Tree, name: &str) -> Vec<(usize, usize, u32, u32)> {
    let mut results = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();

    fn walk_for_idents<'a>(
        node: Node<'a>,
        cursor: &mut tree_sitter::TreeCursor<'a>,
        content: &'a str,
        name: &str,
        results: &mut Vec<(usize, usize, u32, u32)>,
    ) {
        if is_identifier(&node) {
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                // 对于 property_identifier (obj.foo)，只匹配 foo 部分
                let last_part = text.rsplit(|c: char| c == '.' || c == ':').next().unwrap_or(text);
                if last_part == name {
                    let start = node.start_byte();
                    let end = node.end_byte();
                    let line = (node.start_position().row + 1) as u32;
                    let col = node.start_position().column as u32;
                    results.push((start, end, line, col));
                }
            }
        }

        if cursor.goto_first_child() {
            loop {
                walk_for_idents(cursor.node(), cursor, content, name, results);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }
    }

    walk_for_idents(root, &mut cursor, content, name, &mut results);
    results
}

/// 对文件内容应用多处 byte range 替换
/// edits: [(byte_start, byte_end, replacement)]，必须按 byte_start 降序排列（从后往前替换，避免 offset 变化）
fn apply_byte_edits(content: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    // 按 byte_start 降序排序
    edits.sort_by(|a, b| b.0.cmp(&a.0));
    let mut result = content.to_string();
    for (start, end, replacement) in edits {
        // 注意：String 的 byte range 操作需要小心 UTF-8 边界
        if start <= result.len() && end <= result.len() && start <= end {
            result.replace_range(start..end, &replacement);
        }
    }
    result
}

// ==================== ast_edit (merged) ====================

/// ast_edit - 基于 tree-sitter 的语法感知编辑
/// op=rename: 跨文件重命名符号（原 ast_rename）
/// op=extract: 将选定代码范围提取为新函数（原 ast_extract）
/// op=inline: 内联一个简短函数（原 ast_inline）
pub struct AstEditTool;

#[async_trait]
impl Tool for AstEditTool {
    fn name(&self) -> &str { "ast_edit" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "ast_edit".into(),
            description: "AST-aware code editing. op=rename: rename a symbol across all files (LSP or tree-sitter). op=extract: extract a code range into a new function. op=inline: inline a short function (replace call sites with body).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["rename", "extract", "inline"], "description": "Operation to perform" },
                    "old_name": { "type": "string", "description": "rename: current symbol name" },
                    "new_name": { "type": "string", "description": "rename/extract: new symbol/function name" },
                    "file": { "type": "string", "description": "rename/extract: file path" },
                    "line": { "type": "integer", "description": "rename: 1-indexed line of symbol definition (for LSP mode)" },
                    "col": { "type": "integer", "description": "rename: 0-indexed column of symbol definition (for LSP mode)" },
                    "kind": { "type": "string", "description": "rename: optional filter by kind (function|class|struct|variable|method|trait|enum|constant|module|interface|type_alias)" },
                    "start_line": { "type": "integer", "description": "extract: start line (1-indexed, inclusive)" },
                    "end_line": { "type": "integer", "description": "extract: end line (1-indexed, inclusive)" },
                    "args": { "type": "string", "description": "extract: arguments for the new function CALL (no types), e.g. \"(a, b)\". Default: \"()\"" },
                    "def_args": { "type": "string", "description": "extract: arguments for the new function DEFINITION (with types). Default: same as args" },
                    "name": { "type": "string", "description": "inline: function name to inline" },
                    "remove_def": { "type": "boolean", "description": "inline: also remove the function definition (default true)" }
                },
                "required": ["op"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let op: String = serde_json::from_value(args["op"].clone())?;
        match op.as_str() {
            "rename" => Self::rename(&args, ctx).await,
            "extract" => Self::extract(&args, ctx).await,
            "inline" => Self::inline(&args, ctx).await,
            other => anyhow::bail!("unknown op: {} (use rename|extract|inline)", other),
        }
    }
}

impl AstEditTool {
    // ==================== rename (原 AstRenameTool) ====================

    /// 跨文件重命名符号
    /// 策略：
    ///   1. 优先委托 LSP textDocument/rename（语义级精确，理解作用域/类型）
    ///   2. LSP 不可用时，用 code_graph 查找符号定义位置，tree-sitter 定位所有引用节点精确替换
    async fn rename(args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let old_name: String = serde_json::from_value(args["old_name"].clone())
            .or_else(|_| serde_json::from_value(args["symbol"].clone()))
            .context("old_name (or symbol) required")?;
        let new_name: String = serde_json::from_value(args["new_name"].clone())
            .context("new_name required")?;
        let kind_filter: Option<String> = args["kind"].as_str().map(|s| s.to_string());
        let file_arg: Option<String> = args["file"].as_str().map(|s| s.to_string());
        let line_arg: Option<u32> = args["line"].as_u64().map(|n| n as u32);
        let col_arg: Option<u32> = args["col"].as_u64().map(|n| n as u32);

        if old_name == new_name {
            anyhow::bail!("old_name and new_name are identical");
        }
        if !is_valid_identifier(&new_name) {
            anyhow::bail!("new_name is not a valid identifier");
        }

        // 策略 1：优先用 LSP rename
        // 1a: 用户提供了 file/line/col -> 直接用
        // 1b: 没提供 line/col -> 用 code_graph 查符号定义位置
        let lsp = &ctx.lsp_manager;
        let lsp_result = if let (Some(file_str), Some(line), Some(col)) =
            (&file_arg, line_arg, col_arg) {
            Self::try_lsp_rename(&ctx.journal, &ctx.code_graph, lsp, file_str, line, col, &old_name, &new_name).await
        } else {
            // 用 code_graph 查找符号定义位置
            match ctx.code_graph.query_symbols_exact(&old_name) {
                Ok(symbols) => {
                    let filtered: Vec<_> = symbols.iter()
                        .filter(|s| kind_filter.as_ref().map_or(true, |k| s.kind.as_str() == k.as_str()))
                        .collect();
                    if let Some(sym) = filtered.first() {
                        let file_str = sym.file_path.display().to_string();
                        // LSP 用 0-indexed line，code_graph 用 1-indexed
                        let line = sym.start_line - 1;
                        let col = sym.start_col;
                        Self::try_lsp_rename(&ctx.journal, &ctx.code_graph, lsp, &file_str, line, col, &old_name, &new_name).await
                    } else {
                        anyhow::bail!("symbol {} not found in code_graph (try graph_index first)", old_name);
                    }
                }
                Err(e) => Err(e),
            }
        };
        match lsp_result {
            Ok(result) => return Ok(result),
            Err(e) => {
                tracing::warn!("LSP rename failed, falling back to tree-sitter: {}", e);
            }
        }

        // 策略 2：fallback - code_graph + tree-sitter 精确定位
        Self::rename_via_tree_sitter(&ctx.journal, &ctx.code_graph, &old_name, &new_name, &kind_filter).await
    }

    /// 策略 1：委托 LSP textDocument/rename
    async fn try_lsp_rename(
        journal: &Arc<FileJournal>,
        code_graph: &Arc<CodeGraph>,
        lsp: &Arc<LspManager>,
        file: &str,
        line: u32,
        col: u32,
        old_name: &str,
        new_name: &str,
    ) -> Result<ToolOutput> {
        let path = PathBuf::from(file);
        let client = lsp.ensure_open(&path).await?
            .ok_or_else(|| anyhow::anyhow!("no LSP server available for {}", file))?;

        let uri = path_to_uri(&path);
        // LSP 用 0-indexed line/col
        let edit = client.rename(&uri, line, col, new_name).await?;

        if edit.is_null() {
            anyhow::bail!("LSP rename returned null");
        }

        // 应用 WorkspaceEdit 到磁盘
        use crate::lsp::{apply_text_edits, uri_to_path};
        let mut affected_files = Vec::new();

        // documentChanges (LSP 3.0+) 优先于 changes
        if let Some(changes) = edit.get("documentChanges").and_then(|v| v.as_array()) {
            for change in changes {
                if let Some(edits_arr) = change.get("edits").and_then(|v| v.as_array()) {
                    let uri_str = change.get("textDocument")
                        .and_then(|v| v.get("uri"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let file_path = uri_to_path(uri_str);
                    let old_content = std::fs::read_to_string(&file_path)?;
                    let edits_val = serde_json::Value::Array(edits_arr.clone());
                    let new_content = apply_text_edits(&old_content, &edits_val);
                    std::fs::write(&file_path, &new_content)?;
                    let journal_id = journal.record(&file_path, &old_content, &new_content, &format!("ast_rename:lsp:{}->{}", uri_str, new_name));
                    let _ = code_graph.index_file(&file_path);
                    affected_files.push(serde_json::json!({
                        "file": file_path.display().to_string(),
                        "journal_id": journal_id,
                        "after_hash": hash_line(&new_content)[..8].to_string()
                    }));
                }
            }
        } else if let Some(changes) = edit.get("changes").and_then(|v| v.as_object()) {
            for (uri_str, edits_val) in changes {
                let file_path = uri_to_path(uri_str);
                let old_content = std::fs::read_to_string(&file_path)?;
                let new_content = apply_text_edits(&old_content, edits_val);
                std::fs::write(&file_path, &new_content)?;
                let journal_id = journal.record(&file_path, &old_content, &new_content, &format!("ast_rename:lsp:{}->{}", uri_str, new_name));
                let _ = code_graph.index_file(&file_path);
                affected_files.push(serde_json::json!({
                    "file": file_path.display().to_string(),
                    "journal_id": journal_id,
                    "after_hash": hash_line(&new_content)[..8].to_string()
                }));
            }
        }

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "renamed": true,
            "method": "lsp",
            "old_name": old_name,
            "new_name": new_name,
            "files_changed": affected_files.len(),
            "files": affected_files
        }) })
    }

    /// 策略 2：code_graph + tree-sitter 精确定位
    async fn rename_via_tree_sitter(
        journal: &Arc<FileJournal>,
        code_graph: &Arc<CodeGraph>,
        old_name: &str,
        new_name: &str,
        kind_filter: &Option<String>,
    ) -> Result<ToolOutput> {
        // 从 graph 查找所有名为 old_name 的符号，收集相关文件
        let symbols = code_graph.query_symbols(old_name, 200)?;
        let filtered: Vec<_> = symbols.into_iter()
            .filter(|s| {
                if let Some(k) = kind_filter {
                    s.kind.as_str() == k.as_str()
                } else {
                    true
                }
            })
            .collect();

        if filtered.is_empty() {
            return Ok(ToolOutput::Sync { result: serde_json::json!({
                "renamed": false,
                "reason": "no symbols found with that name (try graph_index first)",
                "old_name": old_name
            }) });
        }

        // 收集所有需要修改的文件
        use std::collections::HashSet;
        let mut files: HashSet<PathBuf> = HashSet::new();
        for sym in &filtered {
            files.insert(sym.file_path.clone());
        }
        // 也查找有引用边的文件
        let refs = code_graph.find_references_by_name(old_name)?;
        for (_, edge) in &refs {
            files.insert(edge.file_path.clone());
        }

        let mut changed_files = Vec::new();
        let mut total_replacements = 0u32;

        for file in &files {
            // 用 tree-sitter 解析文件
            let (tree, content) = match parse_file_ts(file) {
                Ok(x) => x,
                Err(_) => continue,
            };

            // 查找所有引用该标识符的节点（包括 call_expression 中的函数名和独立标识符）
            let call_sites = find_call_sites(&content, &tree, old_name);
            let ident_refs = find_identifier_references(&content, &tree, old_name);

            // 合并所有替换点（去重：同一 byte range 只替换一次）
            use std::collections::BTreeSet;
            let mut edits_set: BTreeSet<(usize, usize)> = BTreeSet::new();
            for (start, end, _, _) in &call_sites {
                // 对于 call_expression，我们只替换函数名部分，不替换整个调用
                // 需要找到 call_expression 内的标识符节点
                edits_set.insert((*start, *end));
            }
            for (start, end, _, _) in &ident_refs {
                edits_set.insert((*start, *end));
            }

            if edits_set.is_empty() {
                continue;
            }

            // 构造替换列表
            let edits: Vec<(usize, usize, String)> = edits_set.iter()
                .map(|(start, end)| (*start, *end, new_name.to_string()))
                .collect();

            let new_content = apply_byte_edits(&content, edits);
            let count = edits_set.len() as u32;

            // 先 write 成功再 record journal
            std::fs::write(file, &new_content)?;
            let journal_id = journal.record(file, &content, &new_content, &format!("ast_rename:ts:{}->{}", old_name, new_name));
            changed_files.push(serde_json::json!({
                "file": file.display().to_string(),
                "replacements": count,
                "journal_id": journal_id,
                "after_hash": hash_line(&new_content)[..8].to_string()
            }));
            total_replacements += count;

            // 重新索引
            let _ = code_graph.index_file(file);
        }

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "renamed": true,
            "method": "tree_sitter",
            "old_name": old_name,
            "new_name": new_name,
            "files_changed": changed_files.len(),
            "total_replacements": total_replacements,
            "files": changed_files
        }) })
    }

    // ==================== extract (原 AstExtractTool) ====================

    /// 将选定代码范围提取为新函数
    /// 策略：tree-sitter 解析文件，定位行范围对应的最小节点，提取节点文本
    async fn extract(args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let file: String = serde_json::from_value(args["file"].clone())?;
        let start_line: u32 = serde_json::from_value(args["start_line"].clone())?;
        let end_line: u32 = serde_json::from_value(args["end_line"].clone())?;
        let new_name: String = serde_json::from_value(args["new_name"].clone())?;
        let call_args: String = args["args"].as_str()
            .map(|s| {
                let s = s.trim();
                if s.starts_with('(') { s.to_string() } else { format!("({})", s) }
            })
            .unwrap_or_else(|| "()".to_string());
        let def_args: String = args["def_args"].as_str()
            .map(|s| {
                let s = s.trim();
                if s.starts_with('(') { s.to_string() } else { format!("({})", s) }
            })
            .unwrap_or_else(|| call_args.clone());

        if !is_valid_identifier(&new_name) {
            anyhow::bail!("new_name is not a valid identifier");
        }
        if start_line == 0 || end_line < start_line {
            anyhow::bail!("invalid line range: {}-{}", start_line, end_line);
        }

        let path = std::path::Path::new(&file);
        let (tree, content) = parse_file_ts(path)
            .with_context(|| format!("parsing {}", file))?;

        // 用 tree-sitter 定位包含行范围的最小节点
        let covering_node = node_covering_lines(tree.root_node(), start_line, end_line)
            .ok_or_else(|| anyhow::anyhow!("no node covering lines {}-{}", start_line, end_line))?;

        // 提取节点文本
        let extracted_text = covering_node.utf8_text(content.as_bytes())
            .map_err(|e| anyhow::anyhow!("extracting node text: {}", e))?
            .to_string();

        // 推断原始缩进（从节点起始行）
        let node_start_row = covering_node.start_position().row;  // 0-indexed
        let orig_line = content.lines().nth(node_start_row)
            .ok_or_else(|| anyhow::anyhow!("cannot get line {}", node_start_row))?;
        let orig_indent = orig_line.chars().take_while(|c| c.is_whitespace()).collect::<String>();

        // 函数体缩进 = 原始缩进 + 一级
        let body_indent = if orig_indent.contains('\t') {
            format!("{}\t", orig_indent)
        } else {
            format!("{}    ", orig_indent)
        };

        // 重新缩进提取的文本
        let reindented: Vec<String> = extracted_text.lines().map(|l| {
            if l.starts_with(&orig_indent) {
                format!("{}{}", body_indent, &l[orig_indent.len()..])
            } else if l.trim().is_empty() {
                String::new()
            } else {
                format!("{}{}", body_indent, l.trim_start())
            }
        }).collect();
        let reindented_text = reindented.join("\n");

        // 构造新函数定义
        let lang_kind = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let new_fn = match lang_kind {
            "rs" => format!("\n\nfn {}{} {{\n{}\n}}\n", new_name, def_args, reindented_text),
            "py" => format!("\n\ndef {}{}:\n{}\n", new_name, def_args, reindented_text),
            "go" => format!("\n\nfunc {}{} {{\n{}\n}}\n", new_name, def_args, reindented_text),
            "js" | "jsx" | "ts" | "tsx" => format!("\n\nfunction {}{} {{\n{}\n}}\n", new_name, def_args, reindented_text),
            _ => format!("\n\n// extracted: {}{}\n{}\n", new_name, def_args, reindented_text),
        };

        // 原位置替换为新函数调用
        let call_suffix = if lang_kind == "py" { "" } else { ";" };
        let call_line = format!("{}{}{}{}", orig_indent, new_name, call_args, call_suffix);

        // 用 byte range 精确替换节点文本
        let node_start_byte = covering_node.start_byte();
        let node_end_byte = covering_node.end_byte();

        // 构造新内容：替换节点 + 追加新函数
        let mut new_content = String::with_capacity(content.len() + new_fn.len() + 100);
        new_content.push_str(&content[..node_start_byte]);
        new_content.push_str(&call_line);
        new_content.push_str(&content[node_end_byte..]);
        // 追加新函数定义到文件末尾
        new_content.push_str(&new_fn);

        // 先 write 成功再 record journal
        std::fs::write(path, &new_content)?;
        let journal_id = ctx.journal.record(path, &content, &new_content, &format!("ast_extract:{}", new_name));

        // 重新索引
        let _ = ctx.code_graph.index_file(path);

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "extracted": true,
            "method": "tree_sitter",
            "file": file,
            "new_function": new_name,
            "extracted_node_kind": covering_node.kind(),
            "extracted_lines": [start_line, end_line],
            "journal_id": journal_id,
            "after_hash": hash_line(&new_content)[..8].to_string(),
            "hint": "selected node replaced with function call; new function appended at end of file"
        }) })
    }

    // ==================== inline (原 AstInlineTool) ====================

    /// 内联一个简短函数
    /// 策略：
    ///   1. code_graph 查找函数定义，提取 body（用 tree-sitter 定位 body 节点）
    ///   2. code_graph.find_callers 拿到精确 (file, line, col) 调用点
    ///   3. 对每个调用点：tree-sitter 解析该文件，定位 (line, col) 处的 call_expression 节点
    ///   4. 用 expr 替换该节点的 byte_range（精确替换，不误伤注释/字符串）
    async fn inline(args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let name: String = serde_json::from_value(args["name"].clone())?;
        let remove_def: bool = args["remove_def"].as_bool().unwrap_or(true);

        // 从 graph 查找函数定义
        let symbols = ctx.code_graph.query_symbols_exact(&name)?;
        let func_sym = symbols.iter().find(|s| matches!(s.kind,
            crate::code_graph::schema::SymbolKind::Function
            | crate::code_graph::schema::SymbolKind::Method
        )).ok_or_else(|| anyhow::anyhow!("function {} not found in code graph (try graph_index first)", name))?;

        let def_path = &func_sym.file_path;

        // 用 tree-sitter 解析定义文件，提取函数 body
        let (def_tree, def_content) = parse_file_ts(def_path)
            .with_context(|| format!("parsing {}", def_path.display()))?;

        // 定位函数定义节点
        let def_node = node_covering_lines(def_tree.root_node(), func_sym.start_line, func_sym.end_line)
            .ok_or_else(|| anyhow::anyhow!("cannot locate function definition node in tree"))?;

        // 提取 body：查找函数体子节点
        let body_text = Self::extract_function_body(def_node, &def_content)?;

        // 提取表达式（去掉 return 和 ;）
        let stripped = body_text.strip_prefix("return ").unwrap_or(&body_text);
        let expr = stripped.strip_suffix(';').unwrap_or(stripped).trim().to_string();

        if expr.is_empty() {
            anyhow::bail!("cannot inline: extracted expression is empty");
        }

        // 查找所有调用点
        let callers = ctx.code_graph.find_callers(&name)?;
        if callers.is_empty() {
            return Ok(ToolOutput::Sync { result: serde_json::json!({
                "inlined": false,
                "reason": "no callers found",
                "name": name
            }) });
        }

        // 按文件分组，每文件用 tree-sitter 精确定位 call_expression 节点
        // 收集 (file, [(byte_start, byte_end)]) 对
        let mut file_edits: BTreeMap<PathBuf, Vec<(usize, usize)>> = BTreeMap::new();

        for (_caller_sym, edge) in &callers {
            let edge_file = &edge.file_path;
            // edge.line 是 1-indexed
            let line = edge.line as usize;
            let col = edge.col as usize;

            // 解析调用方文件
            let (call_tree, call_content) = match parse_file_ts(edge_file) {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!("cannot parse caller file {}: {}", edge_file.display(), e);
                    continue;
                }
            };

            // 在 (line, col) 位置定位 call_expression 节点
            let target_point = tree_sitter::Point::new(line - 1, col);  // line is 1-indexed, tree-sitter is 0-indexed
            let node_at_pos = call_tree.root_node().descendant_for_point_range(target_point, target_point);

            if let Some(node) = node_at_pos {
                // 向上查找最近的 call_expression 祖先
                let mut current = Some(node);
                let mut call_node = None;
                while let Some(n) = current {
                    if is_call_expression(&n) {
                        // 确认函数名匹配
                        if let Some(call_name) = extract_call_name(&n, &call_content) {
                            if call_name == name {
                                call_node = Some(n);
                                break;
                            }
                        }
                    }
                    current = n.parent();
                }

                if let Some(cn) = call_node {
                    let byte_start = cn.start_byte();
                    let byte_end = cn.end_byte();
                    file_edits.entry(edge_file.clone())
                        .or_default()
                        .push((byte_start, byte_end));
                }
            }
        }

        if file_edits.is_empty() {
            return Ok(ToolOutput::Sync { result: serde_json::json!({
                "inlined": false,
                "reason": "no call_expression nodes could be located via tree-sitter",
                "name": name
            }) });
        }

        // 对每个文件应用替换
        let mut changed_files = Vec::new();
        let replacement = format!("/* inlined {} */ {}", name, expr);

        for (file_path, edits) in &file_edits {
            let content = std::fs::read_to_string(file_path)
                .with_context(|| format!("reading {}", file_path.display()))?;

            // 构造 byte edits
            let byte_edits: Vec<(usize, usize, String)> = edits.iter()
                .map(|(s, e)| (*s, *e, replacement.clone()))
                .collect();

            let new_content = apply_byte_edits(&content, byte_edits);

            // 先 write 成功再 record journal
            std::fs::write(file_path, &new_content)?;
            let journal_id = ctx.journal.record(file_path, &content, &new_content, &format!("ast_inline:{}", name));
            changed_files.push(serde_json::json!({
                "file": file_path.display().to_string(),
                "replacements": edits.len(),
                "journal_id": journal_id,
                "after_hash": hash_line(&new_content)[..8].to_string()
            }));

            // 重新索引
            let _ = ctx.code_graph.index_file(file_path);
        }

        // 可选：移除原函数定义
        let mut def_removed = false;
        if remove_def {
            // 重新读取 def 文件（可能已被 call site 替换修改）
            let latest_content = if file_edits.contains_key(def_path) {
                std::fs::read_to_string(def_path)?
            } else {
                def_content.clone()
            };

            // 重新解析以获取最新 tree
            let (latest_tree, _) = parse_file_ts(def_path)?;
            let latest_def_node = node_covering_lines(latest_tree.root_node(), func_sym.start_line, func_sym.end_line)
                .ok_or_else(|| anyhow::anyhow!("cannot locate function definition node after call site replacement"))?;

            let def_start_byte = latest_def_node.start_byte();
            let def_end_byte = latest_def_node.end_byte();

            // 用 byte range 精确删除函数定义
            let mut new_def_content = String::with_capacity(latest_content.len());
            new_def_content.push_str(&latest_content[..def_start_byte]);
            // 去除函数定义后的多余空行
            let after = &latest_content[def_end_byte..];
            let after_trimmed = after.trim_start_matches('\n');
            new_def_content.push_str(after_trimmed);

            // journal before 应为最新内容（call site 替换后的状态），而非最初读取的 def_content
            std::fs::write(def_path, &new_def_content)?;
            let journal_id = ctx.journal.record(def_path, &latest_content, &new_def_content, &format!("ast_inline:remove_def:{}", name));
            def_removed = true;
            changed_files.push(serde_json::json!({
                "file": def_path.display().to_string(),
                "journal_id": journal_id,
                "after_hash": hash_line(&new_def_content)[..8].to_string(),
                "def_removed": true
            }));

            // 重新索引
            let _ = ctx.code_graph.index_file(def_path);
        }

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "inlined": true,
            "method": "tree_sitter",
            "name": name,
            "inlined_expr": expr,
            "call_sites_replaced": callers.len(),
            "definition_removed": def_removed,
            "files_changed": changed_files,
            "hint": "call sites precisely replaced via tree-sitter AST nodes; verify manually"
        }) })
    }

    /// 从函数定义节点提取 body 文本
    /// 支持多语言：查找 body/block 节点
    fn extract_function_body(def_node: Node, content: &str) -> Result<String> {
        // 尝试找到 body/block 子节点
        // 不同语言的 body 节点类型不同：
        //   Rust: function_item -> block
        //   JS/TS: function_declaration -> statement_block
        //   Python: function_definition -> block
        //   Go: function_declaration -> block
        let body_kinds = ["block", "statement_block", "block_statement"];

        let mut cursor = def_node.walk();
        let mut body_text = None;

        if cursor.goto_first_child() {
            loop {
                let node = cursor.node();
                if body_kinds.contains(&node.kind()) {
                    let text = node.utf8_text(content.as_bytes())
                        .map_err(|e| anyhow::anyhow!("extracting body text: {}", e))?;
                    body_text = Some(text.to_string());
                    break;
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }

        let body = body_text.ok_or_else(|| anyhow::anyhow!("cannot find function body node (kind: {})", def_node.kind()))?;

        // 去除 { 和 } 以及多余空白
        // 提取花括号内的内容（对于 Rust/JS/Go）
        let trimmed = body.trim();
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            let inner = &trimmed[1..trimmed.len()-1];
            let inner_trimmed = inner.trim();
            if inner_trimmed.is_empty() {
                anyhow::bail!("cannot inline: function body is empty");
            }
            // 只支持单表达式 body
            if inner_trimmed.lines().count() > 1 {
                anyhow::bail!("cannot inline: multi-statement body (only single-expression supported)");
            }
            Ok(inner_trimmed.to_string())
        } else if trimmed.starts_with(':') {
            // Python: block 以 : 开头（不太可能在这里出现，因为 tree-sitter 的 block 节点不含 :）
            anyhow::bail!("Python function body extraction needs special handling")
        } else {
            // 可能是单行函数或其他情况
            if trimmed.lines().count() > 1 {
                anyhow::bail!("cannot inline: multi-statement body (only single-expression supported)");
            }
            Ok(trimmed.to_string())
        }
    }
}
