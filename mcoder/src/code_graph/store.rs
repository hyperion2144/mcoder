// 设计文档 §4.3: find_references 为 forward-looking scaffolding
// P2-8: 新增 symbol_edges 表和 edges 查询方法
#![allow(dead_code)]

use crate::code_graph::schema::{EdgeKind, Reference, Symbol, SymbolEdge, SymbolKind};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct GraphStore {
    conn: Mutex<Connection>,
}

impl GraphStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)
            .with_context(|| format!("opening graph db: {}", db_path.display()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS symbols (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                language TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                start_col INTEGER NOT NULL,
                end_col INTEGER NOT NULL,
                signature TEXT,
                doc_comment TEXT,
                parent_id INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
            CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_path);
            CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);

            CREATE TABLE IF NOT EXISTS symbol_refs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                symbol_id INTEGER NOT NULL,
                file_path TEXT NOT NULL,
                line INTEGER NOT NULL,
                col INTEGER NOT NULL,
                context TEXT,
                FOREIGN KEY (symbol_id) REFERENCES symbols(id)
            );
            CREATE INDEX IF NOT EXISTS idx_refs_symbol ON symbol_refs(symbol_id);

            CREATE TABLE IF NOT EXISTS file_meta (
                file_path TEXT PRIMARY KEY,
                mtime_secs INTEGER NOT NULL,
                mtime_nanos INTEGER NOT NULL,
                line_count INTEGER NOT NULL,
                symbol_count INTEGER NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
                name, file_path, content='symbols', content_rowid='id'
            );

            -- P2-8: 符号关系表
            CREATE TABLE IF NOT EXISTS symbol_edges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                source_symbol_id INTEGER NOT NULL,
                target_name TEXT NOT NULL,
                edge_type TEXT NOT NULL,
                file_path TEXT NOT NULL,
                line INTEGER NOT NULL,
                col INTEGER NOT NULL,
                FOREIGN KEY (source_symbol_id) REFERENCES symbols(id)
            );
            CREATE INDEX IF NOT EXISTS idx_edges_source ON symbol_edges(source_symbol_id);
            CREATE INDEX IF NOT EXISTS idx_edges_target ON symbol_edges(target_name);
            CREATE INDEX IF NOT EXISTS idx_edges_type ON symbol_edges(edge_type);
            CREATE INDEX IF NOT EXISTS idx_edges_file ON symbol_edges(file_path);
            ",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn needs_reindex(&self, path: &Path, mtime: std::time::SystemTime) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let path_str = path.to_string_lossy();
        let dur = mtime.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let secs = dur.as_secs() as i64;
        let nanos = dur.subsec_nanos() as i64;

        let result: Option<(i64, i64)> = conn
            .query_row(
                "SELECT mtime_secs, mtime_nanos FROM file_meta WHERE file_path = ?1",
                rusqlite::params![path_str],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        match result {
            Some((s, n)) => Ok(secs != s || nanos != n),
            None => Ok(true),
        }
    }

    pub fn delete_file_symbols(&self, path: &Path) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let path_str = path.to_string_lossy();
        // P2-8: 同时删除该文件关联的 edges
        conn.execute(
            "DELETE FROM symbol_edges WHERE file_path = ?1",
            rusqlite::params![path_str],
        )?;
        conn.execute(
            "DELETE FROM symbols WHERE file_path = ?1",
            rusqlite::params![path_str],
        )?;
        conn.execute(
            "DELETE FROM symbol_refs WHERE file_path = ?1",
            rusqlite::params![path_str],
        )?;
        Ok(())
    }

    pub fn insert_symbol(&self, sym: &Symbol) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO symbols (file_path, name, kind, language, start_line, end_line, start_col, end_col, signature, doc_comment, parent_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                sym.file_path.to_string_lossy(),
                sym.name,
                sym.kind.as_str(),
                sym.language,
                sym.start_line,
                sym.end_line,
                sym.start_col,
                sym.end_col,
                sym.signature,
                sym.doc_comment,
                sym.parent_id,
            ],
        )?;
        // P2-8: 返回新插入 symbol 的 id，供 edge 关联使用
        Ok(conn.last_insert_rowid())
    }

    pub fn update_file_meta(
        &self,
        path: &Path,
        mtime: std::time::SystemTime,
        content: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let dur = mtime.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let line_count = content.lines().count() as u32;

        let symbol_count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM symbols WHERE file_path = ?1",
                rusqlite::params![path.to_string_lossy()],
                |row| row.get(0),
            )
            .unwrap_or(0);

        conn.execute(
            "INSERT OR REPLACE INTO file_meta (file_path, mtime_secs, mtime_nanos, line_count, symbol_count)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                path.to_string_lossy(),
                dur.as_secs() as i64,
                dur.subsec_nanos() as i64,
                line_count,
                symbol_count,
            ],
        )?;
        Ok(())
    }

    pub fn query_symbols(&self, name: &str, limit: u32) -> Result<Vec<Symbol>> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("%{}%", name);
        let mut stmt = conn.prepare(
            "SELECT id, file_path, name, kind, language, start_line, end_line, start_col, end_col, signature, doc_comment, parent_id
             FROM symbols WHERE name LIKE ?1 LIMIT ?2",
        )?;

        let rows = stmt.query_map(rusqlite::params![pattern, limit], |row| {
            let kind_str: String = row.get(3)?;
            Ok(Symbol {
                id: Some(row.get(0)?),
                file_path: PathBuf::from(row.get::<_, String>(1)?),
                name: row.get(2)?,
                kind: SymbolKind::from_str(&kind_str).unwrap_or(SymbolKind::Variable),
                language: row.get(4)?,
                start_line: row.get(5)?,
                end_line: row.get(6)?,
                start_col: row.get(7)?,
                end_col: row.get(8)?,
                signature: row.get(9)?,
                doc_comment: row.get(10)?,
                parent_id: row.get(11)?,
            })
        })?;

        let mut symbols = Vec::new();
        for row in rows {
            symbols.push(row?);
        }
        Ok(symbols)
    }

    pub fn find_references(&self, symbol_id: i64) -> Result<Vec<Reference>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, symbol_id, file_path, line, col, context
             FROM symbol_refs WHERE symbol_id = ?1",
        )?;

        let rows = stmt.query_map(rusqlite::params![symbol_id], |row| {
            Ok(Reference {
                id: Some(row.get(0)?),
                symbol_id: row.get(1)?,
                file_path: PathBuf::from(row.get::<_, String>(2)?),
                line: row.get(3)?,
                col: row.get(4)?,
                context: row.get(5)?,
            })
        })?;

        let mut refs = Vec::new();
        for row in rows {
            refs.push(row?);
        }
        Ok(refs)
    }

    pub fn get_file_symbols(&self, path: &Path) -> Result<Vec<Symbol>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, file_path, name, kind, language, start_line, end_line, start_col, end_col, signature, doc_comment, parent_id
             FROM symbols WHERE file_path = ?1 ORDER BY start_line",
        )?;

        let rows = stmt.query_map(rusqlite::params![path.to_string_lossy()], |row| {
            let kind_str: String = row.get(3)?;
            Ok(Symbol {
                id: Some(row.get(0)?),
                file_path: PathBuf::from(row.get::<_, String>(1)?),
                name: row.get(2)?,
                kind: SymbolKind::from_str(&kind_str).unwrap_or(SymbolKind::Variable),
                language: row.get(4)?,
                start_line: row.get(5)?,
                end_line: row.get(6)?,
                start_col: row.get(7)?,
                end_col: row.get(8)?,
                signature: row.get(9)?,
                doc_comment: row.get(10)?,
                parent_id: row.get(11)?,
            })
        })?;

        let mut symbols = Vec::new();
        for row in rows {
            symbols.push(row?);
        }
        Ok(symbols)
    }

    // ==================== P2-8: Edge 方法 ====================

    /// 插入一条符号关系边
    pub fn insert_edge(&self, edge: &SymbolEdge) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO symbol_edges (source_symbol_id, target_name, edge_type, file_path, line, col)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                edge.source_symbol_id,
                edge.target_name,
                edge.edge_type.as_str(),
                edge.file_path.to_string_lossy(),
                edge.line,
                edge.col,
            ],
        )?;
        Ok(())
    }

    /// 查找调用指定函数的所有调用者（即 source_symbol 调用了 target_name）
    /// 返回 (caller_symbol, edge) 对
    pub fn find_callers(&self, target_name: &str) -> Result<Vec<(Symbol, SymbolEdge)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.file_path, s.name, s.kind, s.language, s.start_line, s.end_line, s.start_col, s.end_col, s.signature, s.doc_comment, s.parent_id,
                    e.id, e.source_symbol_id, e.target_name, e.edge_type, e.file_path, e.line, e.col
             FROM symbol_edges e
             JOIN symbols s ON e.source_symbol_id = s.id
             WHERE e.target_name = ?1 AND e.edge_type = 'calls'
             ORDER BY e.file_path, e.line",
        )?;

        let rows = stmt.query_map(rusqlite::params![target_name], |row| {
            let kind_str: String = row.get(3)?;
            let edge_type_str: String = row.get(15)?;
            Ok((
                Symbol {
                    id: Some(row.get(0)?),
                    file_path: PathBuf::from(row.get::<_, String>(1)?),
                    name: row.get(2)?,
                    kind: SymbolKind::from_str(&kind_str).unwrap_or(SymbolKind::Variable),
                    language: row.get(4)?,
                    start_line: row.get(5)?,
                    end_line: row.get(6)?,
                    start_col: row.get(7)?,
                    end_col: row.get(8)?,
                    signature: row.get(9)?,
                    doc_comment: row.get(10)?,
                    parent_id: row.get(11)?,
                },
                SymbolEdge {
                    id: Some(row.get(12)?),
                    source_symbol_id: row.get(13)?,
                    target_name: row.get(14)?,
                    edge_type: EdgeKind::from_str(&edge_type_str).unwrap_or(EdgeKind::Calls),
                    file_path: PathBuf::from(row.get::<_, String>(16)?),
                    line: row.get(17)?,
                    col: row.get(18)?,
                },
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// 查找指定函数调用的所有被调用者（即 source_symbol_id 对应的 symbol 调用了哪些 target）
    /// 返回 (callee_name, edge) 对，以及如果 callee 已索引则附带 symbol
    /// P0-1 修复：不再在持锁状态下调用 query_symbols_exact（会死锁），改用 _locked 版本
    /// P1-CG-1 修复：用单条 SQL LEFT JOIN 替代 N+1 查询
    pub fn find_callees(&self, source_symbol_id: i64) -> Result<Vec<(String, SymbolEdge, Option<Symbol>)>> {
        let conn = self.conn.lock().unwrap();
        // P1-CG-1 修复：用 LEFT JOIN 一次性查出 edges + callee symbol
        // 对每个 target_name 取 start_line 最小的同名符号（与原 query_symbols_exact 逻辑一致）
        let mut stmt = conn.prepare(
            "SELECT e.id, e.source_symbol_id, e.target_name, e.edge_type, e.file_path, e.line, e.col,
                    s.id, s.file_path, s.name, s.kind, s.language, s.start_line, s.end_line, s.start_col, s.end_col, s.signature, s.doc_comment, s.parent_id
             FROM symbol_edges e
             LEFT JOIN symbols s ON s.id = (
                 SELECT id FROM symbols WHERE name = e.target_name ORDER BY start_line LIMIT 1
             )
             WHERE e.source_symbol_id = ?1 AND e.edge_type = 'calls'
             ORDER BY e.file_path, e.line",
        )?;

        let rows = stmt.query_map(rusqlite::params![source_symbol_id], |row| {
            let edge_type_str: String = row.get(3)?;
            let edge = SymbolEdge {
                id: Some(row.get(0)?),
                source_symbol_id: row.get(1)?,
                target_name: row.get(2)?,
                edge_type: EdgeKind::from_str(&edge_type_str).unwrap_or(EdgeKind::Calls),
                file_path: PathBuf::from(row.get::<_, String>(4)?),
                line: row.get(5)?,
                col: row.get(6)?,
            };
            // s.id 可能为 NULL（callee 未索引）
            let sym_id: Option<i64> = row.get(7).ok();
            let callee_sym = sym_id.map(|_| {
                let kind_str: String = row.get(10).unwrap_or_default();
                Symbol {
                    id: Some(row.get(7).unwrap_or(0)),
                    file_path: PathBuf::from(row.get::<_, String>(8).unwrap_or_default()),
                    name: row.get(9).unwrap_or_default(),
                    kind: SymbolKind::from_str(&kind_str).unwrap_or(SymbolKind::Variable),
                    language: row.get(11).unwrap_or_default(),
                    start_line: row.get(12).unwrap_or(0),
                    end_line: row.get(13).unwrap_or(0),
                    start_col: row.get(14).unwrap_or(0),
                    end_col: row.get(15).unwrap_or(0),
                    signature: row.get(16).ok(),
                    doc_comment: row.get(17).ok(),
                    parent_id: row.get(18).ok(),
                }
            });
            Ok((edge.target_name.clone(), edge, callee_sym))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// 按名称精确查找符号（非模糊匹配）
    pub fn query_symbols_exact(&self, name: &str) -> Result<Vec<Symbol>> {
        let conn = self.conn.lock().unwrap();
        query_symbols_exact_locked(&conn, name)
    }

    /// 查找文件中的所有 edges
    pub fn get_file_edges(&self, path: &Path) -> Result<Vec<SymbolEdge>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, source_symbol_id, target_name, edge_type, file_path, line, col
             FROM symbol_edges WHERE file_path = ?1 ORDER BY line",
        )?;

        let rows = stmt.query_map(rusqlite::params![path.to_string_lossy()], |row| {
            let edge_type_str: String = row.get(3)?;
            Ok(SymbolEdge {
                id: Some(row.get(0)?),
                source_symbol_id: row.get(1)?,
                target_name: row.get(2)?,
                edge_type: EdgeKind::from_str(&edge_type_str).unwrap_or(EdgeKind::Calls),
                file_path: PathBuf::from(row.get::<_, String>(4)?),
                line: row.get(5)?,
                col: row.get(6)?,
            })
        })?;

        let mut edges = Vec::new();
        for row in rows {
            edges.push(row?);
        }
        Ok(edges)
    }

    /// P2-8: 查找所有指向 target_name 的边（任意 edge_type）
    /// 用于 graph_references：返回 (source_symbol, edge) 对
    pub fn find_references_by_name(&self, target_name: &str) -> Result<Vec<(Symbol, SymbolEdge)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.file_path, s.name, s.kind, s.language, s.start_line, s.end_line, s.start_col, s.end_col, s.signature, s.doc_comment, s.parent_id,
                    e.id, e.source_symbol_id, e.target_name, e.edge_type, e.file_path, e.line, e.col
             FROM symbol_edges e
             JOIN symbols s ON e.source_symbol_id = s.id
             WHERE e.target_name = ?1
             ORDER BY e.file_path, e.line",
        )?;

        let rows = stmt.query_map(rusqlite::params![target_name], |row| {
            let kind_str: String = row.get(3)?;
            let edge_type_str: String = row.get(15)?;
            Ok((
                Symbol {
                    id: Some(row.get(0)?),
                    file_path: PathBuf::from(row.get::<_, String>(1)?),
                    name: row.get(2)?,
                    kind: SymbolKind::from_str(&kind_str).unwrap_or(SymbolKind::Variable),
                    language: row.get(4)?,
                    start_line: row.get(5)?,
                    end_line: row.get(6)?,
                    start_col: row.get(7)?,
                    end_col: row.get(8)?,
                    signature: row.get(9)?,
                    doc_comment: row.get(10)?,
                    parent_id: row.get(11)?,
                },
                SymbolEdge {
                    id: Some(row.get(12)?),
                    source_symbol_id: row.get(13)?,
                    target_name: row.get(14)?,
                    edge_type: EdgeKind::from_str(&edge_type_str).unwrap_or(EdgeKind::Calls),
                    file_path: PathBuf::from(row.get::<_, String>(16)?),
                    line: row.get(17)?,
                    col: row.get(18)?,
                },
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}

/// P0-1 修复：已持有 conn 的查询函数，避免 find_callees 重入死锁
fn query_symbols_exact_locked(conn: &Connection, name: &str) -> Result<Vec<Symbol>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_path, name, kind, language, start_line, end_line, start_col, end_col, signature, doc_comment, parent_id
         FROM symbols WHERE name = ?1 ORDER BY start_line",
    )?;

    let rows = stmt.query_map(rusqlite::params![name], |row| {
        let kind_str: String = row.get(3)?;
        Ok(Symbol {
            id: Some(row.get(0)?),
            file_path: PathBuf::from(row.get::<_, String>(1)?),
            name: row.get(2)?,
            kind: SymbolKind::from_str(&kind_str).unwrap_or(SymbolKind::Variable),
            language: row.get(4)?,
            start_line: row.get(5)?,
            end_line: row.get(6)?,
            start_col: row.get(7)?,
            end_col: row.get(8)?,
            signature: row.get(9)?,
            doc_comment: row.get(10)?,
            parent_id: row.get(11)?,
        })
    })?;

    let mut symbols = Vec::new();
    for row in rows {
        symbols.push(row?);
    }
    Ok(symbols)
}
