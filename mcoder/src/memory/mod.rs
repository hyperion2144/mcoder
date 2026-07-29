// 设计文档 §2.1: memory update/delete API 为 forward-looking scaffolding
// 当前仅 store/search/list 被工具使用；update/delete 保留供未来管理 UI
#![allow(dead_code)]

pub mod tools;

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

/// 项目记忆：同一个项目跨会话保留决策、约定、教训
/// 经验沉淀：跨所有项目共享的经验，可在相关内容中召回
pub struct MemoryStore {
    conn: Mutex<Connection>,
}

impl MemoryStore {
    /// 设计文档 §2.1: 安全获取锁，忽略 poison（防止其他线程 panic 导致连锁失败）
    /// poison 后仍可读取数据，只是数据可能不一致，对 memory 来说可接受
    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: Option<i64>,
    pub scope: MemoryScope,
    pub key: String,
    pub content: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub project_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// 项目级记忆，仅当前项目可见
    Project,
    /// 全局经验，所有项目共享
    Experience,
}

impl MemoryScope {
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryScope::Project => "project",
            MemoryScope::Experience => "experience",
        }
    }
}

impl MemoryStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                scope TEXT NOT NULL,
                key TEXT NOT NULL,
                content TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                project_hash TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_mem_scope ON memories(scope);
            CREATE INDEX IF NOT EXISTS idx_mem_key ON memories(key);
            CREATE INDEX IF NOT EXISTS idx_mem_project ON memories(project_hash);

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                key, content, tags, content='memories', content_rowid='id'
            );

            CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, key, content, tags)
                VALUES (new.id, new.key, new.content, new.tags);
            END;
            CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, key, content, tags)
                VALUES ('delete', old.id, old.key, old.content, old.tags);
            END;
            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, key, content, tags)
                VALUES ('delete', old.id, old.key, old.content, old.tags);
                INSERT INTO memories_fts(rowid, key, content, tags)
                VALUES (new.id, new.key, new.content, new.tags);
            END;",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn store(&self, entry: &MemoryEntry) -> Result<i64> {
        let conn = self.conn();
        let now = chrono::Utc::now().to_rfc3339();
        let tags = entry.tags.join(",");

        conn.execute(
            "INSERT INTO memories (scope, key, content, tags, created_at, updated_at, project_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                entry.scope.as_str(),
                entry.key,
                entry.content,
                tags,
                &now,
                &now,
                entry.project_hash,
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    pub fn update(&self, id: i64, content: &str) -> Result<()> {
        let conn = self.conn();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE memories SET content = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![content, &now, id],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM memories WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    /// 按关键词搜索（FTS5 全文检索）
    pub fn search(
        &self,
        query: &str,
        scope: Option<MemoryScope>,
        project_hash: Option<&str>,
        limit: u32,
    ) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn();

        let mut sql = String::from(
            "SELECT m.id, m.scope, m.key, m.content, m.tags, m.created_at, m.updated_at, m.project_hash
             FROM memories_fts f
             JOIN memories m ON m.id = f.rowid
             WHERE memories_fts MATCH ?1",
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(query.to_string())];
        let mut param_idx = 2;

        if let Some(s) = scope {
            sql.push_str(&format!(" AND m.scope = ?{}", param_idx));
            params.push(Box::new(s.as_str().to_string()));
            param_idx += 1;
        }

        if let Some(ph) = project_hash {
            sql.push_str(&format!(" AND (m.project_hash = ?{} OR m.scope = 'experience')", param_idx));
            params.push(Box::new(ph.to_string()));
            param_idx += 1;
        }

        sql.push_str(&format!(" ORDER BY rank LIMIT ?{}", param_idx));
        params.push(Box::new(limit));

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params.iter().map(|p| p.as_ref()).collect::<Vec<_>>().as_slice(),
            |row| {
                let scope_str: String = row.get(1)?;
                let tags_str: String = row.get::<_, Option<String>>(4)?.unwrap_or_default();
                Ok(MemoryEntry {
                    id: Some(row.get(0)?),
                    scope: if scope_str == "experience" {
                        MemoryScope::Experience
                    } else {
                        MemoryScope::Project
                    },
                    key: row.get(2)?,
                    content: row.get(3)?,
                    tags: tags_str.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                    project_hash: row.get(7)?,
                })
            },
        )?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// 列出项目记忆
    pub fn list_project(&self, project_hash: &str) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, scope, key, content, tags, created_at, updated_at, project_hash
             FROM memories WHERE project_hash = ?1 AND scope = 'project'
             ORDER BY updated_at DESC",
        )?;

        let rows = stmt.query_map(rusqlite::params![project_hash], |row| {
            let tags_str: String = row.get::<_, Option<String>>(4)?.unwrap_or_default();
            Ok(MemoryEntry {
                id: Some(row.get(0)?),
                scope: MemoryScope::Project,
                key: row.get(2)?,
                content: row.get(3)?,
                tags: tags_str.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                project_hash: row.get(7)?,
            })
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// 列出所有经验
    pub fn list_experiences(&self, limit: u32) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, scope, key, content, tags, created_at, updated_at, project_hash
             FROM memories WHERE scope = 'experience'
             ORDER BY updated_at DESC LIMIT ?1",
        )?;

        let rows = stmt.query_map(rusqlite::params![limit], |row| {
            let tags_str: String = row.get::<_, Option<String>>(4)?.unwrap_or_default();
            Ok(MemoryEntry {
                id: Some(row.get(0)?),
                scope: MemoryScope::Experience,
                key: row.get(2)?,
                content: row.get(3)?,
                tags: tags_str.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                project_hash: row.get(7)?,
            })
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }
}

pub fn project_hash(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    let result = hasher.finalize();
    result.iter().take(16).map(|b| format!("{:02x}", b)).collect()
}
