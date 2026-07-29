// 设计文档 §4.3: Reference/FileMeta 为 find_references 工具的 forward-looking scaffolding
// P2-8: 新增 SymbolEdge 类型，支持 calls/imports/extends/implements 关系
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub id: Option<i64>,
    pub file_path: PathBuf,
    pub name: String,
    pub kind: SymbolKind,
    pub language: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_col: u32,
    pub end_col: u32,
    pub signature: Option<String>,
    pub doc_comment: Option<String>,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Interface,
    Trait,
    Module,
    Variable,
    Constant,
    TypeAlias,
    Import,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Interface => "interface",
            SymbolKind::Trait => "trait",
            SymbolKind::Module => "module",
            SymbolKind::Variable => "variable",
            SymbolKind::Constant => "constant",
            SymbolKind::TypeAlias => "type_alias",
            SymbolKind::Import => "import",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "function" => Some(Self::Function),
            "method" => Some(Self::Method),
            "class" => Some(Self::Class),
            "struct" => Some(Self::Struct),
            "enum" => Some(Self::Enum),
            "interface" => Some(Self::Interface),
            "trait" => Some(Self::Trait),
            "module" => Some(Self::Module),
            "variable" => Some(Self::Variable),
            "constant" => Some(Self::Constant),
            "type_alias" => Some(Self::TypeAlias),
            "import" => Some(Self::Import),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    pub id: Option<i64>,
    pub symbol_id: i64,
    pub file_path: PathBuf,
    pub line: u32,
    pub col: u32,
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    pub path: PathBuf,
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub line_count: u32,
    pub symbol_count: u32,
}

// ==================== P2-8: 符号关系（edges）====================

/// 符号间的关系类型
/// 设计文档 §8.4.1: defines / calls / imports / extends / implements / references
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// source 调用 target（函数调用）
    Calls,
    /// source 导入 target（use/import 语句）
    Imports,
    /// source 继承 target（class extends）
    Extends,
    /// source 实现 target（impl trait / implements interface）
    Implements,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Calls => "calls",
            EdgeKind::Imports => "imports",
            EdgeKind::Extends => "extends",
            EdgeKind::Implements => "implements",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "calls" => Some(Self::Calls),
            "imports" => Some(Self::Imports),
            "extends" => Some(Self::Extends),
            "implements" => Some(Self::Implements),
            _ => None,
        }
    }
}

/// 一条符号关系边
/// source_symbol_id → target_name（按名称关联，不要求 target 已索引）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolEdge {
    pub id: Option<i64>,
    /// 发起边的符号 id（如调用者函数的 symbol id）
    pub source_symbol_id: i64,
    /// 目标符号名称（如被调用的函数名）
    pub target_name: String,
    /// 关系类型
    pub edge_type: EdgeKind,
    /// 边所在文件
    pub file_path: PathBuf,
    /// 边所在行号
    pub line: u32,
    pub col: u32,
}
