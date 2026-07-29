use anyhow::{Context, Result};
use glob::Pattern;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 单条 journal 记录：对应一次文件变更（单文件 edit 或 batch 中的一个文件）
/// 持久化到 SQLite journal_entries 表
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: String,
    pub timestamp: i64,
    pub path: PathBuf,
    pub before_hash: String,
    pub after_hash: String,
    pub before_snapshot: String,
    pub after_snapshot: String,
    pub operation: String,
    /// 关联的操作批次ID（如一次 bash 调用可能改多个文件，共享同一 batch_id）
    pub batch_id: Option<String>,
}

/// 项目文件快照：path -> (mtime, hash)
/// 用于 begin_batch/end_batch 的快速 diff（mtime 过滤优化，设计 §4.9）
#[derive(Clone)]
pub struct ProjectSnapshot {
    pub files: HashMap<PathBuf, (u64, String)>,
}

impl ProjectSnapshot {
    /// 扫描项目目录下所有文件，记录 (mtime, hash)
    /// 跳过 .git, target, node_modules, .mcoder, dist, build 目录
    /// 跳过 .gitignore 匹配的路径
    /// 跳过 >2MB 的大文件
    pub fn capture(project_dir: &Path) -> Result<Self> {
        let skip_dirs = [".git", "target", "node_modules", ".mcoder", "dist", "build"];
        let ignore_patterns = load_gitignore(project_dir);
        let mut files = HashMap::new();
        walk_dir(project_dir, project_dir, &skip_dirs, &ignore_patterns, &mut files)?;
        Ok(Self { files })
    }

    /// 与另一个快照对比，返回变动的文件路径（相对路径）
    /// 优化（设计 §4.9）：先比较 mtime，mtime 未变则跳过 hash 比较
    pub fn diff(&self, current: &Self) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        for (path, (mtime, hash)) in &self.files {
            match current.files.get(path) {
                Some((cur_mtime, cur_hash)) => {
                    // mtime 未变 → 跳过 hash 比较（性能优化）
                    if *mtime != *cur_mtime && hash != cur_hash {
                        changed.push(path.clone());
                    }
                }
                None => {
                    // 文件被删除
                    changed.push(path.clone());
                }
            }
        }
        for path in current.files.keys() {
            if !self.files.contains_key(path) {
                changed.push(path.clone());
            }
        }
        changed
    }
}

/// 递归扫描目录，记录每个文件的 (mtime, hash)
fn walk_dir(
    dir: &Path,
    project_root: &Path,
    skip_dirs: &[&str],
    ignore_patterns: &[Pattern],
    files: &mut HashMap<PathBuf, (u64, String)>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if skip_dirs.contains(&name.as_str()) {
            continue;
        }

        let rel = path
            .strip_prefix(project_root)
            .unwrap_or(&path)
            .to_path_buf();
        if is_ignored(&rel, ignore_patterns) {
            continue;
        }

        if path.is_dir() {
            walk_dir(&path, project_root, skip_dirs, ignore_patterns, files)?;
        } else if path.is_file() {
            if let Ok(meta) = path.metadata() {
                if meta.len() > 2 * 1024 * 1024 {
                    continue; // 跳过 >2MB 文件
                }
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let h = hash_content(&content);
                    files.insert(rel, (mtime, h));
                }
            }
        }
    }
    Ok(())
}

/// 读取项目根目录的 .gitignore，编译为 glob Pattern 列表
fn load_gitignore(project_dir: &Path) -> Vec<Pattern> {
    let gitignore_path = project_dir.join(".gitignore");
    let mut patterns = Vec::new();
    if let Ok(content) = std::fs::read_to_string(&gitignore_path) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            // 去除尾部的 /（gitignore 目录标记），glob Pattern 不支持
            let pat_str = line.trim_end_matches('/');
            if let Ok(pat) = Pattern::new(pat_str) {
                patterns.push(pat);
            }
        }
    }
    patterns
}

/// 检查相对路径是否被 gitignore pattern 匹配
fn is_ignored(rel_path: &Path, patterns: &[Pattern]) -> bool {
    let path_str = rel_path.to_string_lossy();
    for pat in patterns {
        // 匹配完整相对路径
        if pat.matches(&path_str) {
            return true;
        }
        // 对不含 / 的 pattern（如 *.log），检查每个路径分量
        for component in rel_path.iter() {
            if pat.matches(&component.to_string_lossy()) {
                return true;
            }
        }
    }
    false
}

fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    result.iter().take(16).map(|b| format!("{:02x}", b)).collect()
}

/// 批量操作的内存中间状态：begin_batch 时创建，end_batch 时消费
struct BatchState {
    project_dir: PathBuf,
    before: ProjectSnapshot,
    /// before 阶段各文件的原始内容（用于 end_batch 写入 before_snapshot）
    before_contents: HashMap<PathBuf, String>,
}

/// FileJournal：基于 SQLite 的文件变更日志，支持单文件记录与批量 undo
/// 存储路径：<project_dir>/journal/journal.db
pub struct FileJournal {
    /// SQLite 连接（rusqlite Connection 不是 Sync，用 Mutex 保护）
    conn: Mutex<Connection>,
    /// 进行中的批量操作状态（begin_batch 后、end_batch 前的中间快照）
    batch_states: Mutex<HashMap<String, BatchState>>,
}

impl FileJournal {
    /// 打开（或创建）指定项目数据目录下的 journal.db
    /// project_dir 通常是 <project>/.mcoder，数据库路径为 project_dir/journal/journal.db
    pub fn new(project_dir: &Path) -> Result<Self> {
        let db_path = project_dir.join("journal").join("journal.db");
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&db_path)
            .with_context(|| format!("opening journal db: {}", db_path.display()))?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS journal_entries (
                id TEXT PRIMARY KEY,
                timestamp INTEGER NOT NULL,
                path TEXT NOT NULL,
                before_hash TEXT NOT NULL,
                after_hash TEXT NOT NULL,
                before_snapshot TEXT NOT NULL,
                after_snapshot TEXT NOT NULL,
                operation TEXT NOT NULL,
                batch_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_journal_batch ON journal_entries(batch_id);
            CREATE INDEX IF NOT EXISTS idx_journal_timestamp ON journal_entries(timestamp DESC);
            "#,
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            batch_states: Mutex::new(HashMap::new()),
        })
    }

    /// 记录单次文件编辑（write/edit/ast_edit 用）
    /// 返回 journal entry id
    pub fn record(&self, path: &Path, before: &str, after: &str, operation: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().timestamp_millis();
        let before_hash = hash_content(before);
        let after_hash = hash_content(after);
        if let Err(e) = self.conn.lock().unwrap().execute(
            "INSERT INTO journal_entries (id, timestamp, path, before_hash, after_hash, before_snapshot, after_snapshot, operation, batch_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
            params![id, timestamp, path.to_string_lossy(), before_hash, after_hash, before, after, operation],
        ) {
            tracing::warn!("journal record insert failed: {}", e);
        }
        id
    }

    /// 开始一个批量操作批次：捕获项目快照（begin 时拍快照）
    /// 用于 bash/code_exec 等可能修改多个文件的操作
    pub fn begin_batch(&self, project_dir: &Path, label: &str) -> Result<String> {
        let batch_id = format!(
            "batch_{}_{}",
            chrono::Utc::now().timestamp_millis(),
            uuid::Uuid::new_v4().simple().to_string().chars().take(6).collect::<String>()
        );

        let before = ProjectSnapshot::capture(project_dir)
            .with_context(|| format!("capturing project snapshot for batch {}", batch_id))?;

        // 读取所有快照内文件的原始内容，用于 end_batch 时写入 before_snapshot
        let mut before_contents = HashMap::new();
        for rel_path in before.files.keys() {
            let abs_path = project_dir.join(rel_path);
            if let Ok(content) = std::fs::read_to_string(&abs_path) {
                before_contents.insert(rel_path.clone(), content);
            }
        }

        let file_count = before.files.len();
        self.batch_states.lock().unwrap().insert(
            batch_id.clone(),
            BatchState {
                project_dir: project_dir.to_path_buf(),
                before,
                before_contents,
            },
        );

        tracing::info!(
            "batch {} begun: {} files snapshotted (label={})",
            batch_id, file_count, label
        );
        Ok(batch_id)
    }

    /// 结束批量操作：捕获后置快照，diff 出变动文件，逐个记录到 journal
    /// 返回变动文件列表（相对路径）
    pub fn end_batch(&self, batch_id: &str, label: &str) -> Result<Vec<PathBuf>> {
        let batch = self
            .batch_states
            .lock()
            .unwrap()
            .remove(batch_id)
            .context("batch not found (already ended?)")?;

        let after = ProjectSnapshot::capture(&batch.project_dir)?;
        let changed = batch.before.diff(&after);
        let project_dir = batch.project_dir.clone();
        let timestamp = chrono::Utc::now().timestamp_millis();

        let conn = self.conn.lock().unwrap();
        for rel_path in &changed {
            let abs_path = project_dir.join(rel_path);
            // before 内容从 begin_batch 时缓存的快照取
            let before_content = batch
                .before_contents
                .get(rel_path)
                .cloned()
                .unwrap_or_default();
            // after 内容从当前文件读（已被 bash/code_exec 修改）
            let after_content = std::fs::read_to_string(&abs_path).unwrap_or_default();
            let entry_id = uuid::Uuid::new_v4().to_string();
            let before_hash = batch
                .before
                .files
                .get(rel_path)
                .map(|(_, h)| h.clone())
                .unwrap_or_default();
            let after_hash = hash_content(&after_content);

            if let Err(e) = conn.execute(
                "INSERT INTO journal_entries (id, timestamp, path, before_hash, after_hash, before_snapshot, after_snapshot, operation, batch_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    entry_id,
                    timestamp,
                    abs_path.to_string_lossy(),
                    before_hash,
                    after_hash,
                    before_content,
                    after_content,
                    format!("{}:{}", label, batch_id),
                    batch_id,
                ],
            ) {
                tracing::warn!("journal batch entry insert failed: {}", e);
            }
        }

        tracing::info!("batch {} ended: {} files changed", batch_id, changed.len());
        Ok(changed)
    }

    /// 撤销指定 entry：将 before_snapshot 写回文件
    /// 设计文档 §4.9: undo 也会写一条新 journal entry，保持审计链不断
    pub fn undo(&self, entry_id: &str) -> Result<()> {
        let (path_str, before_snapshot, original_op) = {
            let conn = self.conn.lock().unwrap();
            let result = conn.query_row(
                "SELECT path, before_snapshot, operation FROM journal_entries WHERE id = ?1",
                params![entry_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            );
            match result {
                Ok(v) => v,
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    anyhow::bail!("journal entry not found: {}", entry_id);
                }
                Err(e) => return Err(e.into()),
            }
        };

        let path = PathBuf::from(path_str);
        if before_snapshot.is_empty() {
            return Ok(());
        }

        // 读取当前文件内容作为 undo 操作的 before
        let current_content = std::fs::read_to_string(&path).unwrap_or_default();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &before_snapshot)?;

        // 设计文档 §4.9: 写一条新 journal entry 记录 undo 操作本身
        self.record(&path, &current_content, &before_snapshot, &format!("undo:{}", original_op));

        Ok(())
    }

    /// 撤销整个批次：按 batch_id 恢复该批次所有文件的 before_snapshot
    /// 设计文档 §4.9: undo 也会写一条新 journal entry（每文件一条，共享新 batch_id）
    pub fn undo_batch(&self, batch_id: &str) -> Result<()> {
        let rows: Vec<(String, String, String)> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT path, before_snapshot, operation FROM journal_entries WHERE batch_id = ?1 ORDER BY timestamp DESC",
            )?;
            let rows = stmt
                .query_map(params![batch_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
                })?
                .filter_map(|r| r.ok())
                .collect();
            rows
        };

        if rows.is_empty() {
            anyhow::bail!("no entries found for batch: {}", batch_id);
        }

        // 为 undo 操作生成新的 batch_id
        let undo_batch_id = format!("undo_{}", batch_id);
        let timestamp = chrono::Utc::now().timestamp_millis();

        for (path_str, before_snapshot, original_op) in rows {
            if before_snapshot.is_empty() {
                continue;
            }
            let path = PathBuf::from(&path_str);
            let current_content = std::fs::read_to_string(&path).unwrap_or_default();

            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, &before_snapshot)?;

            // 写新 journal entry 记录 undo
            let entry_id = uuid::Uuid::new_v4().to_string();
            let before_hash = hash_content(&current_content);
            let after_hash = hash_content(&before_snapshot);
            if let Err(e) = self.conn.lock().unwrap().execute(
                "INSERT INTO journal_entries (id, timestamp, path, before_hash, after_hash, before_snapshot, after_snapshot, operation, batch_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    entry_id,
                    timestamp,
                    path_str,
                    before_hash,
                    after_hash,
                    current_content,
                    before_snapshot,
                    format!("undo:{}", original_op),
                    undo_batch_id,
                ],
            ) {
                tracing::warn!("journal undo entry insert failed: {}", e);
            }
        }
        Ok(())
    }

    /// 撤销最近一次操作（单文件或批次）
    /// 跳过 operation 以 "undo:" 开头的 entry（避免 undo 一个 undo = redo 需单独支持）
    pub fn undo_last(&self) -> Result<()> {
        let (id, batch_id) = {
            let conn = self.conn.lock().unwrap();
            let result = conn.query_row(
                "SELECT id, batch_id FROM journal_entries \
                 WHERE operation NOT LIKE 'undo:%' \
                 ORDER BY timestamp DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            );
            match result {
                Ok(v) => v,
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    anyhow::bail!("no journal entries to undo");
                }
                Err(e) => return Err(e.into()),
            }
        };

        // 锁已释放，可安全调用 undo / undo_batch
        if let Some(bid) = batch_id {
            self.undo_batch(&bid)
        } else {
            self.undo(&id)
        }
    }

    /// 按 id 查询单条 entry
    pub fn get_entry(&self, id: &str) -> Option<JournalEntry> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, timestamp, path, before_hash, after_hash, before_snapshot, after_snapshot, operation, batch_id \
             FROM journal_entries WHERE id = ?1",
            params![id],
            row_to_entry,
        )
        .ok()
    }

    /// 查询最近 limit 条 entry（按时间倒序）
    pub fn list(&self, limit: usize) -> Vec<JournalEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, timestamp, path, before_hash, after_hash, before_snapshot, after_snapshot, operation, batch_id \
             FROM journal_entries ORDER BY timestamp DESC LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![limit as i64], row_to_entry)
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }
}

/// rusqlite 行映射为 JournalEntry
fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<JournalEntry> {
    let path_str: String = row.get(2)?;
    Ok(JournalEntry {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        path: PathBuf::from(path_str),
        before_hash: row.get(3)?,
        after_hash: row.get(4)?,
        before_snapshot: row.get(5)?,
        after_snapshot: row.get(6)?,
        operation: row.get(7)?,
        batch_id: row.get(8)?,
    })
}
