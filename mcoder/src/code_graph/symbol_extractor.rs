use crate::code_graph::schema::{EdgeKind, Symbol, SymbolKind};
use crate::tree_sitter::languages::Language;
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Node, Parser};

/// 提取结果：符号列表 + 边的原始数据（source_name 待后续解析为 symbol_id）
pub struct ExtractResult {
    pub symbols: Vec<Symbol>,
    /// (source_symbol_name, target_name, edge_kind, line, col)
    pub raw_edges: Vec<(String, String, EdgeKind, u32, u32)>,
}

pub fn extract_symbols(
    file_path: &Path,
    content: &str,
    lang: Language,
) -> Result<Vec<Symbol>> {
    let result = extract_symbols_and_edges(file_path, content, lang)?;
    Ok(result.symbols)
}

/// P2-8: 提取符号 + 关系边
pub fn extract_symbols_and_edges(
    file_path: &Path,
    content: &str,
    lang: Language,
) -> Result<ExtractResult> {
    let ts_lang = match lang.tree_sitter_language() {
        Some(l) => l,
        None => return Ok(ExtractResult { symbols: Vec::new(), raw_edges: Vec::new() }),
    };

    let mut parser = Parser::new();
    parser.set_language(&ts_lang)?;

    let tree = parser.parse(content, None)
        .ok_or_else(|| anyhow::anyhow!("failed to parse"))?;
    let root = tree.root_node();

    let mut symbols = Vec::new();
    walk_node(root, content, file_path, lang, &mut symbols);

    // P2-8: 提取关系边
    let raw_edges = extract_edges(root, content, file_path, lang);

    Ok(ExtractResult { symbols, raw_edges })
}

fn walk_node(
    node: Node,
    content: &str,
    file_path: &Path,
    lang: Language,
    symbols: &mut Vec<Symbol>,
) {
    if let Some(sym) = try_extract_symbol(node, content, file_path, lang) {
        symbols.push(sym);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_node(child, content, file_path, lang, symbols);
    }
}

fn try_extract_symbol(
    node: Node,
    content: &str,
    file_path: &Path,
    lang: Language,
) -> Option<Symbol> {
    let kind = match lang {
        Language::Rust => rust_symbol_kind(node)?,
        Language::JavaScript | Language::TypeScript => js_symbol_kind(node)?,
        Language::Python => python_symbol_kind(node)?,
        Language::Go => go_symbol_kind(node)?,
        // P2-7: 新增语言
        Language::C | Language::Cpp => c_symbol_kind(node)?,
        Language::Java => java_symbol_kind(node)?,
        Language::Ruby => ruby_symbol_kind(node)?,
        Language::CSharp => csharp_symbol_kind(node)?,
        Language::Bash => bash_symbol_kind(node)?,
        Language::Json => json_symbol_kind(node)?,
        Language::Css => css_symbol_kind(node)?,
        Language::Html => html_symbol_kind(node)?,
        Language::Unknown => return None,
    };

    let name = node_name(node, content, lang)?;

    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;
    let start_col = node.start_position().column as u32;
    let end_col = node.end_position().column as u32;

    let signature = extract_signature(node, content, lang);
    let doc_comment = extract_doc_comment(node, content);

    Some(Symbol {
        id: None,
        file_path: file_path.to_path_buf(),
        name,
        kind,
        language: lang.name().to_string(),
        start_line,
        end_line,
        start_col,
        end_col,
        signature,
        doc_comment,
        parent_id: None,
    })
}

fn rust_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "function_item" => Some(SymbolKind::Function),
        "function_signature_item" => Some(SymbolKind::Function),
        "struct_item" => Some(SymbolKind::Struct),
        "enum_item" => Some(SymbolKind::Enum),
        "trait_item" => Some(SymbolKind::Trait),
        "impl_item" => Some(SymbolKind::Class),
        "type_item" => Some(SymbolKind::TypeAlias),
        "const_item" => Some(SymbolKind::Constant),
        "static_item" => Some(SymbolKind::Constant),
        "mod_item" => Some(SymbolKind::Module),
        "macro_definition" => Some(SymbolKind::Function),
        _ => None,
    }
}

fn js_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "function_declaration" => Some(SymbolKind::Function),
        "method_definition" => Some(SymbolKind::Method),
        "class_declaration" => Some(SymbolKind::Class),
        "variable_declaration" => Some(SymbolKind::Variable),
        "lexical_declaration" => Some(SymbolKind::Variable),
        "export_statement" => None,
        _ => None,
    }
}

fn python_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "function_definition" => Some(SymbolKind::Function),
        "class_definition" => Some(SymbolKind::Class),
        "decorated_definition" => None,
        _ => None,
    }
}

fn go_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "function_declaration" => Some(SymbolKind::Function),
        "method_declaration" => Some(SymbolKind::Method),
        "type_declaration" => Some(SymbolKind::TypeAlias),
        _ => None,
    }
}

// ==================== P2-7: 新增语言的 symbol_kind ====================

/// C/C++: function_definition / struct / enum / class (C++) / namespace (C++)
fn c_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "function_definition" => Some(SymbolKind::Function),
        "function_declaration" => Some(SymbolKind::Function),
        "struct_specifier" => Some(SymbolKind::Struct),
        "class_specifier" => Some(SymbolKind::Class),
        "enum_specifier" => Some(SymbolKind::Enum),
        "union_specifier" => Some(SymbolKind::Struct),
        "namespace_definition" => Some(SymbolKind::Module),
        "declaration" => None,  // 变量声明太常见，跳过避免噪声
        "type_definition" => Some(SymbolKind::TypeAlias),
        "preproc_function_def" => Some(SymbolKind::Function),
        _ => None,
    }
}

/// Java: method_declaration / class_declaration / interface_declaration / enum_declaration
fn java_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "method_declaration" => Some(SymbolKind::Method),
        "constructor_declaration" => Some(SymbolKind::Method),
        "class_declaration" => Some(SymbolKind::Class),
        "interface_declaration" => Some(SymbolKind::Interface),
        "enum_declaration" => Some(SymbolKind::Enum),
        "record_declaration" => Some(SymbolKind::Class),
        "annotation_type_declaration" => Some(SymbolKind::Interface),
        "field_declaration" => None,
        "local_variable_declaration" => None,
        _ => None,
    }
}

/// Ruby: method / class / module
fn ruby_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "method" => Some(SymbolKind::Method),
        "singleton_method" => Some(SymbolKind::Method),
        "class" => Some(SymbolKind::Class),
        "module" => Some(SymbolKind::Module),
        "singleton_class" => Some(SymbolKind::Class),
        _ => None,
    }
}

/// C#: method_declaration / class_declaration / interface_declaration / struct_declaration
fn csharp_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "method_declaration" => Some(SymbolKind::Method),
        "constructor_declaration" => Some(SymbolKind::Method),
        "class_declaration" => Some(SymbolKind::Class),
        "interface_declaration" => Some(SymbolKind::Interface),
        "struct_declaration" => Some(SymbolKind::Struct),
        "enum_declaration" => Some(SymbolKind::Enum),
        "record_declaration" => Some(SymbolKind::Class),
        "namespace_declaration" => Some(SymbolKind::Module),
        "property_declaration" => None,
        "field_declaration" => None,
        _ => None,
    }
}

/// Bash: function_definition
fn bash_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "function_definition" => Some(SymbolKind::Function),
        _ => None,
    }
}

/// JSON: object 键作为 variable（pair 节点的 key 子节点是 string）
/// 简化：只把 top-level 对象的 key 作为符号
fn json_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "pair" => Some(SymbolKind::Variable),
        _ => None,
    }
}

/// CSS: rule_set（选择器规则）
fn css_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "rule_set" => Some(SymbolKind::Variable),
        "at_rule" => Some(SymbolKind::Variable),
        "keyframes_statement" => Some(SymbolKind::Function),
        _ => None,
    }
}

/// HTML: script/style/tag 作为 module
fn html_symbol_kind(node: Node) -> Option<SymbolKind> {
    match node.kind() {
        "script_element" => Some(SymbolKind::Module),
        "style_element" => Some(SymbolKind::Module),
        "element" => None,  // 太多，跳过
        _ => None,
    }
}

fn node_name(node: Node, content: &str, lang: Language) -> Option<String> {
    // 优先尝试标准的 "name" field（Rust/JS/TS/Python/Go/Java/C#/Ruby 都有）
    if let Some(name_field) = node.child_by_field_name("name") {
        if let Ok(text) = name_field.utf8_text(content.as_bytes()) {
            return Some(text.to_string());
        }
    }

    // P1-2 修复：Rust impl_item 没有 "name" field，但有 "type" field
    // impl Foo 的 type=Foo，impl Trait for Foo 的 type=Foo, trait=Trait
    if lang == Language::Rust && node.kind() == "impl_item" {
        if let Some(type_field) = node.child_by_field_name("type") {
            if let Ok(text) = type_field.utf8_text(content.as_bytes()) {
                return Some(text.to_string());
            }
        }
    }

    // P2-7: 各语言的特殊处理
    match lang {
        Language::C | Language::Cpp => {
            // C/C++ function_definition: 第一个 declarator 子节点包含函数名
            // 简化：取 declarator 节点的第一个 identifier
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "function_declarator" || child.kind() == "identifier" {
                    if let Ok(text) = child.utf8_text(content.as_bytes()) {
                        // 取第一个 identifier
                        let ident = text.split(|c: char| !c.is_alphanumeric() && c != '_')
                            .find(|s| !s.is_empty() && s != &"static" && s != &"inline" && s != &"const")?;
                        return Some(ident.to_string());
                    }
                }
            }
            None
        }
        Language::Bash => {
            // bash function_definition: 第一个子节点是 function name 或 "function" 关键字 + name
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "word" {
                    if let Ok(text) = child.utf8_text(content.as_bytes()) {
                        return Some(text.to_string());
                    }
                }
            }
            None
        }
        Language::Json => {
            // JSON pair: 第一个子节点是 key（string）
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "string" {
                    if let Ok(text) = child.utf8_text(content.as_bytes()) {
                        // 去掉引号
                        let trimmed = text.trim_matches('"');
                        return Some(trimmed.to_string());
                    }
                }
            }
            None
        }
        Language::Css => {
            // CSS rule_set: 第一个子节点是 selectors
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "selectors" || child.kind() == "selector" {
                    if let Ok(text) = child.utf8_text(content.as_bytes()) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
            None
        }
        Language::Html => {
            // P1-3 修复：tree-sitter-html 的 script_element/style_element 没有 "tag" field
            // 需要从 start_tag 子节点中取 tag_name
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "start_tag" {
                    let mut c2 = child.walk();
                    for inner in child.children(&mut c2) {
                        if inner.kind() == "tag_name" {
                            if let Ok(text) = inner.utf8_text(content.as_bytes()) {
                                return Some(format!("<{}>", text));
                            }
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn extract_signature(node: Node, content: &str, _lang: Language) -> Option<String> {
    // Get the first line of the symbol as signature
    let start = node.start_byte();
    let end = content[start..].find('\n').map(|i| start + i).unwrap_or(node.end_byte());
    let sig = &content[start..end];
    // P0-2 修复：按字符数（而非字节数）截断，避免非 ASCII 字节边界 panic
    const MAX_CHARS: usize = 200;
    if sig.chars().count() > MAX_CHARS {
        let truncated: String = sig.chars().take(MAX_CHARS).collect();
        Some(format!("{}...", truncated))
    } else {
        Some(sig.to_string())
    }
}

fn extract_doc_comment(node: Node, content: &str) -> Option<String> {
    let prev = node.prev_sibling()?;
    if prev.kind().contains("comment") {
        let text = prev.utf8_text(content.as_bytes()).ok()?;
        Some(text.to_string())
    } else {
        None
    }
}

// ==================== P2-8: 关系边提取 ====================

/// 提取文件中的关系边（calls / imports / extends / implements）
/// 返回原始边列表：(source_symbol_name, target_name, edge_kind, line, col)
/// source_symbol_name 为包含该边的最内层符号名（函数/方法），用于后续关联 symbol_id
fn extract_edges(
    root: Node,
    content: &str,
    file_path: &Path,
    lang: Language,
) -> Vec<(String, String, EdgeKind, u32, u32)> {
    let mut edges = Vec::new();
    let mut symbol_stack: Vec<String> = Vec::new();
    walk_for_edges(root, content, file_path, lang, &mut symbol_stack, &mut edges);
    edges
}

fn walk_for_edges(
    node: Node,
    content: &str,
    file_path: &Path,
    lang: Language,
    symbol_stack: &mut Vec<String>,
    edges: &mut Vec<(String, String, EdgeKind, u32, u32)>,
) {
    // 检查是否进入新符号作用域
    let entered_symbol = try_get_symbol_name(node, content, lang);
    if let Some(ref name) = entered_symbol {
        symbol_stack.push(name.clone());
    }

    // 提取当前节点的边
    extract_edge_for_node(node, content, file_path, lang, symbol_stack, edges);

    // 递归子节点
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_for_edges(child, content, file_path, lang, symbol_stack, edges);
    }

    // 离开符号作用域
    if entered_symbol.is_some() {
        symbol_stack.pop();
    }
}

/// 尝试获取节点对应的符号名（用于维护 symbol_stack）
fn try_get_symbol_name(node: Node, content: &str, lang: Language) -> Option<String> {
    let is_symbol = match lang {
        Language::Rust => matches!(node.kind(),
            "function_item" | "function_signature_item" | "struct_item" |
            "enum_item" | "trait_item" | "impl_item" | "mod_item" | "macro_definition"
        ),
        Language::JavaScript | Language::TypeScript => matches!(node.kind(),
            "function_declaration" | "method_definition" | "class_declaration"
        ),
        Language::Python => matches!(node.kind(),
            "function_definition" | "class_definition"
        ),
        Language::Go => matches!(node.kind(),
            "function_declaration" | "method_declaration" | "type_declaration"
        ),
        // P2-7: 新增语言
        Language::C | Language::Cpp => matches!(node.kind(),
            "function_definition" | "struct_specifier" | "class_specifier" |
            "enum_specifier" | "namespace_definition"
        ),
        Language::Java => matches!(node.kind(),
            "method_declaration" | "constructor_declaration" |
            "class_declaration" | "interface_declaration" | "enum_declaration"
        ),
        Language::Ruby => matches!(node.kind(),
            "method" | "singleton_method" | "class" | "module"
        ),
        Language::CSharp => matches!(node.kind(),
            "method_declaration" | "class_declaration" | "interface_declaration" |
            "struct_declaration" | "enum_declaration" | "namespace_declaration"
        ),
        Language::Bash => matches!(node.kind(),
            "function_definition"
        ),
        Language::Json => false,  // JSON 无嵌套符号作用域
        Language::Css => false,
        Language::Html => matches!(node.kind(),
            "script_element" | "style_element"
        ),
        Language::Unknown => false,
    };

    if !is_symbol {
        return None;
    }

    node_name(node, content, lang)
}

/// 提取单个节点的关系边
fn extract_edge_for_node(
    node: Node,
    content: &str,
    _file_path: &Path,
    lang: Language,
    symbol_stack: &[String],
    edges: &mut Vec<(String, String, EdgeKind, u32, u32)>,
) {
    let current_symbol = symbol_stack.last().cloned().unwrap_or_else(|| "<module>".to_string());
    let line = node.start_position().row as u32 + 1;
    let col = node.start_position().column as u32;

    match lang {
        Language::Rust => extract_rust_edges(node, content, &current_symbol, line, col, edges),
        Language::JavaScript | Language::TypeScript => extract_js_edges(node, content, &current_symbol, line, col, edges),
        Language::Python => extract_python_edges(node, content, &current_symbol, line, col, edges),
        Language::Go => extract_go_edges(node, content, &current_symbol, line, col, edges),
        // P2-7: 新增语言
        // P1-9 修复：Java 和 C# 拆分为独立提取函数（节点类型不同）
        Language::C | Language::Cpp => extract_c_edges(node, content, &current_symbol, line, col, edges),
        Language::Java => extract_java_edges(node, content, &current_symbol, line, col, edges),
        Language::CSharp => extract_csharp_edges(node, content, &current_symbol, line, col, edges),
        Language::Ruby => extract_ruby_edges(node, content, &current_symbol, line, col, edges),
        // Bash/JSON/CSS/HTML 关系边较少，暂不提取
        Language::Bash | Language::Json | Language::Css | Language::Html => {}
        Language::Unknown => {}
    }
}

/// Rust: 提取 call_expression / use_declaration / impl trait
fn extract_rust_edges(
    node: Node,
    content: &str,
    current_symbol: &str,
    line: u32,
    col: u32,
    edges: &mut Vec<(String, String, EdgeKind, u32, u32)>,
) {
    match node.kind() {
        "call_expression" => {
            // call_expression 的 function 子节点是被调用的函数名
            if let Some(func) = node.child_by_field_name("function") {
                if let Ok(text) = func.utf8_text(content.as_bytes()) {
                    // 提取最后的标识符部分（处理 foo::bar::baz 等路径）
                    let name = extract_last_ident(text);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Calls, line, col));
                    }
                }
            }
        }
        "use_declaration" => {
            // use foo::bar::baz; → target = baz (或整个路径)
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                let text = text.trim();
                // 去掉 "use " 前缀和 ";" 后缀
                let path = text.strip_prefix("use ")
                    .or_else(|| text.strip_prefix("use\t"))
                    .unwrap_or(text)
                    .trim_end_matches(';')
                    .trim();
                // 取路径的最后一段作为 target_name
                let name = path.rsplit("::").next().unwrap_or(path);
                // 去掉通配符
                let name = name.trim_end_matches('*').trim();
                if !name.is_empty() && name != "self" {
                    edges.push((current_symbol.to_string(), name.to_string(), EdgeKind::Imports, line, col));
                }
            }
        }
        "impl_item" => {
            // impl Trait for Type → implements Trait
            // impl_item 有 type/trait 两个 field（trait 仅在 impl Trait for Type 时存在）
            if let Some(trait_node) = node.child_by_field_name("trait") {
                if let Ok(text) = trait_node.utf8_text(content.as_bytes()) {
                    let name = extract_last_ident(text);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Implements, line, col));
                    }
                }
            }
        }
        _ => {}
    }
}

/// JS/TS: 提取 call_expression / import_statement / class_heritage (extends)
fn extract_js_edges(
    node: Node,
    content: &str,
    current_symbol: &str,
    line: u32,
    col: u32,
    edges: &mut Vec<(String, String, EdgeKind, u32, u32)>,
) {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                if let Ok(text) = func.utf8_text(content.as_bytes()) {
                    let name = extract_last_ident(text);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Calls, line, col));
                    }
                }
            }
        }
        "import_statement" => {
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                // 简化：提取 from "module" 中的 module 名
                // 或 import { foo } from "bar" → target = bar
                if let Some(pos) = text.find("from") {
                    let after_from = text[pos + 4..].trim();
                    let module = after_from
                        .trim_start_matches('"')
                        .trim_start_matches('\'')
                        .trim_end_matches('"')
                        .trim_end_matches('\'');
                    if !module.is_empty() {
                        let name = module.rsplit('/').next().unwrap_or(module);
                        edges.push((current_symbol.to_string(), name.to_string(), EdgeKind::Imports, line, col));
                    }
                }
            }
        }
        "class_heritage" => {
            // extends ParentClass
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                let text = text.trim();
                if text.starts_with("extends") {
                    let parent = text.strip_prefix("extends").unwrap_or("").trim();
                    let name = extract_last_ident(parent);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Extends, line, col));
                    }
                }
            }
        }
        _ => {}
    }
}

/// Python: 提取 call / import / class 定义中的基类
fn extract_python_edges(
    node: Node,
    content: &str,
    current_symbol: &str,
    line: u32,
    col: u32,
    edges: &mut Vec<(String, String, EdgeKind, u32, u32)>,
) {
    match node.kind() {
        "call" => {
            if let Some(func) = node.child_by_field_name("function") {
                if let Ok(text) = func.utf8_text(content.as_bytes()) {
                    let name = extract_last_ident(text);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Calls, line, col));
                    }
                }
            }
        }
        "import_statement" | "import_from_statement" => {
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                let text = text.trim();
                // from X import Y → target = X
                // import X → target = X
                let module = if let Some(pos) = text.find("from") {
                    text[pos + 4..].split_whitespace().next().unwrap_or("")
                } else if text.starts_with("import") {
                    text.strip_prefix("import").unwrap_or("").split_whitespace().next().unwrap_or("")
                } else {
                    ""
                };
                let name = module.rsplit('.').next().unwrap_or(module);
                if !name.is_empty() {
                    edges.push((current_symbol.to_string(), name.to_string(), EdgeKind::Imports, line, col));
                }
            }
        }
        "class_definition" => {
            // class Foo(Bar): → extends Bar
            // argument_list 子节点包含基类
            if let Some(args) = node.child_by_field_name("superclasses") {
                let mut cursor = args.walk();
                for arg in args.children(&mut cursor) {
                    if arg.kind() == "argument_list" || arg.kind() == "parent" {
                        continue;
                    }
                    if let Ok(text) = arg.utf8_text(content.as_bytes()) {
                        let name = extract_last_ident(text);
                        if !name.is_empty() {
                            edges.push((current_symbol.to_string(), name, EdgeKind::Extends, line, col));
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Go: 提取 call_expression / import_declaration
fn extract_go_edges(
    node: Node,
    content: &str,
    current_symbol: &str,
    line: u32,
    col: u32,
    edges: &mut Vec<(String, String, EdgeKind, u32, u32)>,
) {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                if let Ok(text) = func.utf8_text(content.as_bytes()) {
                    let name = extract_last_ident(text);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Calls, line, col));
                    }
                }
            }
        }
        "import_declaration" => {
            // import "path/to/package" → target = package
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                let text = text.trim();
                // 提取引号内的路径
                if let Some(start) = text.find('"') {
                    if let Some(end) = text[start + 1..].find('"') {
                        let path = &text[start + 1..start + 1 + end];
                        let name = path.rsplit('/').next().unwrap_or(path);
                        if !name.is_empty() {
                            edges.push((current_symbol.to_string(), name.to_string(), EdgeKind::Imports, line, col));
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

// ==================== P2-7: 新增语言的 edges 提取 ====================

/// C/C++: 提取 call_expression (function call) / preproc_include (#include)
fn extract_c_edges(
    node: Node,
    content: &str,
    current_symbol: &str,
    line: u32,
    col: u32,
    edges: &mut Vec<(String, String, EdgeKind, u32, u32)>,
) {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                if let Ok(text) = func.utf8_text(content.as_bytes()) {
                    let name = extract_last_ident(text);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Calls, line, col));
                    }
                }
            }
        }
        "preproc_include" => {
            // #include <foo.h> 或 #include "foo.h"
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                // 取引号或尖括号内的文件名
                let path = if let Some(start) = text.find('"') {
                    text[start + 1..].split('"').next().unwrap_or("")
                } else if let Some(start) = text.find('<') {
                    text[start + 1..].split('>').next().unwrap_or("")
                } else {
                    ""
                };
                let name = path.rsplit('/').next().unwrap_or(path)
                    .trim_end_matches(".h")
                    .trim_end_matches(".hpp");
                if !name.is_empty() {
                    edges.push((current_symbol.to_string(), name.to_string(), EdgeKind::Imports, line, col));
                }
            }
        }
        // C++ class derivation: class Foo : public Bar
        "class_specifier" => {
            // base_class_clause 子节点包含基类
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "base_class_clause" {
                    if let Ok(text) = child.utf8_text(content.as_bytes()) {
                        // 文本形如 ": public Bar, private Baz"
                        for part in text.split(',') {
                            let part = part.trim_start_matches(':').trim();
                            // 去掉访问修饰符
                            let part = part.trim_start_matches("public")
                                .trim_start_matches("private")
                                .trim_start_matches("protected")
                                .trim_start_matches("virtual")
                                .trim();
                            let name = extract_last_ident(part);
                            if !name.is_empty() {
                                edges.push((current_symbol.to_string(), name, EdgeKind::Extends, line, col));
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Java/C#: 提取 method_invocation / import_declaration / super_class / implements
/// Java: 提取 method_invocation / import_declaration / superclass / super_interfaces
/// P1-9 修复：C# 的节点类型不同（using_directive / base_list），拆分到 extract_csharp_edges
fn extract_java_edges(
    node: Node,
    content: &str,
    current_symbol: &str,
    line: u32,
    col: u32,
    edges: &mut Vec<(String, String, EdgeKind, u32, u32)>,
) {
    match node.kind() {
        "method_invocation" => {
            if let Some(func) = node.child_by_field_name("name") {
                if let Ok(text) = func.utf8_text(content.as_bytes()) {
                    let name = extract_last_ident(text);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Calls, line, col));
                    }
                }
            }
        }
        "import_declaration" => {
            // Java: import foo.bar.Baz;
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                let text = text.trim().trim_end_matches(';').trim();
                let path = if let Some(p) = text.strip_prefix("import") {
                    p.trim().trim_start_matches("static ").trim()
                } else {
                    ""
                };
                let name = path.rsplit('.').next().unwrap_or(path);
                if !name.is_empty() && name != "*" {
                    edges.push((current_symbol.to_string(), name.to_string(), EdgeKind::Imports, line, col));
                }
            }
        }
        // Java: class Foo extends Bar implements Baz
        "superclass" => {
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                let name = extract_last_ident(text);
                if !name.is_empty() {
                    edges.push((current_symbol.to_string(), name, EdgeKind::Extends, line, col));
                }
            }
        }
        "super_interfaces" => {
            // implements A, B → 每个接口一条 implements 边
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                for part in text.split(',') {
                    let name = extract_last_ident(part);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Implements, line, col));
                    }
                }
            }
        }
        // P2-6 补充：Java interface extends Bar, Baz
        "extends_interfaces" => {
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                for part in text.split(',') {
                    let name = extract_last_ident(part);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Extends, line, col));
                    }
                }
            }
        }
        _ => {}
    }
}

/// P1-9 修复：C# 独立的 edge 提取（节点类型与 Java 不同）
/// C#: invocation_expression / using_directive / base_list
fn extract_csharp_edges(
    node: Node,
    content: &str,
    current_symbol: &str,
    line: u32,
    col: u32,
    edges: &mut Vec<(String, String, EdgeKind, u32, u32)>,
) {
    match node.kind() {
        "invocation_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                if let Ok(text) = func.utf8_text(content.as_bytes()) {
                    let name = extract_last_ident(text);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Calls, line, col));
                    }
                }
            }
        }
        "using_directive" => {
            // C#: using foo.bar.Baz;
            if let Ok(text) = node.utf8_text(content.as_bytes()) {
                let text = text.trim().trim_end_matches(';').trim();
                let path = if let Some(p) = text.strip_prefix("using") {
                    p.trim().trim_start_matches("static ").trim()
                } else {
                    ""
                };
                let name = path.rsplit('.').next().unwrap_or(path);
                if !name.is_empty() && name != "*" {
                    edges.push((current_symbol.to_string(), name.to_string(), EdgeKind::Imports, line, col));
                }
            }
        }
        // C#: class Foo : Bar, IBaz（基类和接口都在 base_list 中）
        "base_list" => {
            // base_list 的子节点是 type_name 列表，第一个是基类（extends），其余是接口（implements）
            // 但语法上无法区分，简化处理：全部作为 Extends
            // 更精确的做法需要查符号表判断是 class 还是 interface
            let mut cursor = node.walk();
            let mut is_first = true;
            for child in node.children(&mut cursor) {
                if child.kind() == "type_name" || child.kind() == "identifier" || child.kind() == "qualified_name" {
                    if let Ok(text) = child.utf8_text(content.as_bytes()) {
                        let name = extract_last_ident(text);
                        if !name.is_empty() {
                            // 简化：第一个视为 Extends，其余视为 Implements
                            // 注意：这不完全准确，C# 语法中基类必须在列表第一位
                            let kind = if is_first { EdgeKind::Extends } else { EdgeKind::Implements };
                            edges.push((current_symbol.to_string(), name, kind, line, col));
                            is_first = false;
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Ruby: 提取 call / require / class 父类
fn extract_ruby_edges(
    node: Node,
    content: &str,
    current_symbol: &str,
    line: u32,
    col: u32,
    edges: &mut Vec<(String, String, EdgeKind, u32, u32)>,
) {
    match node.kind() {
        "call" => {
            // Ruby call: receiver.method 或 method(args)
            if let Some(method) = node.child_by_field_name("method") {
                if let Ok(text) = method.utf8_text(content.as_bytes()) {
                    let name = extract_last_ident(text);
                    if !name.is_empty() {
                        edges.push((current_symbol.to_string(), name, EdgeKind::Calls, line, col));
                    }
                }
            }
        }
        "command" => {
            // require / require_relative / include 等
            if let Some(name_node) = node.child_by_field_name("method") {
                if let Ok(text) = name_node.utf8_text(content.as_bytes()) {
                    if text == "require" || text == "require_relative" || text == "load" {
                        // 第一个 argument 是文件名
                        let mut cursor = node.walk();
                        for child in node.children(&mut cursor) {
                            if child.kind() == "argument_list" || child.kind() == "string" {
                                if let Ok(arg) = child.utf8_text(content.as_bytes()) {
                                    let arg = arg.trim_matches(|c: char| c == '"' || c == '\'' || c == '(' || c == ')');
                                    let name = arg.rsplit('/').next().unwrap_or(arg);
                                    if !name.is_empty() {
                                        edges.push((current_symbol.to_string(), name.to_string(), EdgeKind::Imports, line, col));
                                    }
                                }
                                break;
                            }
                        }
                    } else {
                        // 普通命令调用
                        let name = extract_last_ident(text);
                        if !name.is_empty() {
                            edges.push((current_symbol.to_string(), name, EdgeKind::Calls, line, col));
                        }
                    }
                }
            }
        }
        "class" => {
            // class Foo < Bar → extends Bar
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "superclass" {
                    if let Ok(text) = child.utf8_text(content.as_bytes()) {
                        let name = extract_last_ident(text);
                        if !name.is_empty() {
                            edges.push((current_symbol.to_string(), name, EdgeKind::Extends, line, col));
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// 从可能包含路径分隔符的文本中提取最后一个标识符
/// 例: "foo::bar::baz" → "baz", "obj.method" → "method", "foo" → "foo"
fn extract_last_ident(s: &str) -> String {
    let s = s.trim();
    // 按 :: / . / / 分割，取最后一段
    let last = s
        .rsplit(|c| c == ':' || c == '.' || c == '/' || c == '\\')
        .next()
        .unwrap_or(s);
    // 去掉括号等
    last.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
        .to_string()
}
