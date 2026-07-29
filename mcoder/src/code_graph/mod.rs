// 设计文档 §4.3: find_references/ts_parser 为 forward-looking scaffolding
#![allow(dead_code)]

pub mod schema;
pub mod store;
pub mod symbol_extractor;
pub mod tools;

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::tree_sitter::languages::Language;
use crate::tree_sitter::Parser as TsParser;

pub struct CodeGraph {
    pub store: Arc<store::GraphStore>,
    ts_parser: Arc<TsParser>,
    root: PathBuf,
}

impl CodeGraph {
    pub fn new(db_path: &Path, root: &Path) -> Result<Arc<Self>> {
        let store = Arc::new(store::GraphStore::open(db_path)?);
        let ts_parser = Arc::new(TsParser::new());
        let graph = Arc::new(Self {
            store,
            ts_parser,
            root: root.to_path_buf(),
        });
        Ok(graph)
    }

    /// Index a single file: parse AST, extract symbols + edges, store in SQLite
    /// P2-8: 使用 extract_symbols_and_edges，存储符号关系边
    pub fn index_file(&self, path: &Path) -> Result<()> {
        let lang = Language::from_path(path);
        if lang == Language::Unknown {
            return Ok(());
        }

        let content = std::fs::read_to_string(path)?;
        let mtime = std::fs::metadata(path)?.modified()?;

        // Check if we need to re-index (mtime changed)
        if !self.store.needs_reindex(path, mtime)? {
            return Ok(());
        }

        // Delete old symbols (and edges, via FK cascade in delete_file_symbols) for this file
        self.store.delete_file_symbols(path)?;

        // P2-8: 提取符号 + 关系边
        let extract = symbol_extractor::extract_symbols_and_edges(path, &content, lang)?;

        // P1-1 修复：为每个文件插入一条合成的 <module> 符号，作为顶层边的 source
        // 这样顶层 use/import/#include/require 等边不会因为 source 找不到而丢失
        let module_sym = schema::Symbol {
            id: None,
            file_path: path.to_path_buf(),
            name: "<module>".to_string(),
            kind: schema::SymbolKind::Module,
            language: lang.name().to_string(),
            start_line: 1,
            end_line: content.lines().count() as u32,
            start_col: 0,
            end_col: 0,
            signature: None,
            doc_comment: None,
            parent_id: None,
        };
        let module_id = self.store.insert_symbol(&module_sym)?;

        // 插入符号，并建立 name → symbol_id 映射
        // P1-7 修复：同名符号改为 Vec 收集，按行号区间匹配
        let mut name_to_ids: std::collections::HashMap<String, Vec<(i64, u32, u32)>> = std::collections::HashMap::new();
        for sym in &extract.symbols {
            let id = self.store.insert_symbol(sym)?;
            name_to_ids.entry(sym.name.clone())
                .or_default()
                .push((id, sym.start_line, sym.end_line));
        }
        // <module> 始终可用
        name_to_ids.entry("<module>".to_string())
            .or_default()
            .push((module_id, 1, u32::MAX));

        // P2-8: 插入关系边
        // raw_edges: (source_symbol_name, target_name, edge_kind, line, col)
        for (source_name, target_name, edge_kind, line, col) in &extract.raw_edges {
            // P1-7 修复：按行号区间匹配同名符号，而非取最后一个
            let source_id = name_to_ids.get(source_name)
                .and_then(|candidates| {
                    // 找到包含此边的符号（line 落在 [start_line, end_line] 内）
                    candidates.iter()
                        .find(|(_, start, end)| *line >= *start && *line <= *end)
                        .map(|(id, _, _)| *id)
                        .or_else(|| candidates.last().map(|(id, _, _)| *id))
                });
            if let Some(sid) = source_id {
                let edge = schema::SymbolEdge {
                    id: None,
                    source_symbol_id: sid,
                    target_name: target_name.clone(),
                    edge_type: *edge_kind,
                    file_path: path.to_path_buf(),
                    line: *line,
                    col: *col,
                };
                if let Err(e) = self.store.insert_edge(&edge) {
                    tracing::warn!("insert_edge failed at {}:{}: {}", path.display(), line, e);
                }
            }
        }

        self.store.update_file_meta(path, mtime, &content)?;
        Ok(())
    }

    /// Index an entire directory tree
    pub fn index_dir(&self, dir: &Path) -> Result<IndexStats> {
        let mut stats = IndexStats::default();
        self.walk_and_index(dir, &mut stats)?;
        Ok(stats)
    }

    fn walk_and_index(&self, dir: &Path, stats: &mut IndexStats) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if entry.file_type()?.is_dir() {
                // skip common ignore dirs
                if matches!(name.as_str(), "target" | "node_modules" | ".git" | "dist" | ".mcoder") {
                    continue;
                }
                self.walk_and_index(&path, stats)?;
            } else if entry.file_type()?.is_file() {
                let lang = Language::from_path(&path);
                if lang != Language::Unknown {
                    stats.files_found += 1;
                    match self.index_file(&path) {
                        Ok(()) => stats.files_indexed += 1,
                        Err(e) => {
                            stats.errors += 1;
                            tracing::warn!("index error {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Query symbols by name (substring match)
    pub fn query_symbols(&self, name: &str, limit: u32) -> Result<Vec<schema::Symbol>> {
        self.store.query_symbols(name, limit)
    }

    /// Find all references to a symbol
    pub fn find_references(&self, symbol_id: i64) -> Result<Vec<schema::Reference>> {
        self.store.find_references(symbol_id)
    }

    /// Get symbols in a specific file
    pub fn get_file_symbols(&self, path: &Path) -> Result<Vec<schema::Symbol>> {
        self.store.get_file_symbols(path)
    }

    /// P2-8: 按名称精确查找符号
    pub fn query_symbols_exact(&self, name: &str) -> Result<Vec<schema::Symbol>> {
        self.store.query_symbols_exact(name)
    }

    /// P2-8: 查找调用指定函数的所有调用者
    pub fn find_callers(&self, target_name: &str) -> Result<Vec<(schema::Symbol, schema::SymbolEdge)>> {
        self.store.find_callers(target_name)
    }

    /// P2-8: 查找指定函数调用的所有被调用者
    pub fn find_callees(&self, source_symbol_id: i64) -> Result<Vec<(String, schema::SymbolEdge, Option<schema::Symbol>)>> {
        self.store.find_callees(source_symbol_id)
    }

    /// P2-8: 查找文件中的所有关系边
    pub fn get_file_edges(&self, path: &Path) -> Result<Vec<schema::SymbolEdge>> {
        self.store.get_file_edges(path)
    }

    /// P2-8: 查找所有指向 target_name 的边（任意 edge_type）
    /// 用于 graph_references
    pub fn find_references_by_name(&self, target_name: &str) -> Result<Vec<(schema::Symbol, schema::SymbolEdge)>> {
        self.store.find_references_by_name(target_name)
    }

    /// Get the tree-sitter parser (for line hashing etc.)
    pub fn ts_parser(&self) -> &TsParser {
        &self.ts_parser
    }
}

#[derive(Debug, Default)]
pub struct IndexStats {
    pub files_found: usize,
    pub files_indexed: usize,
    pub errors: usize,
}
