// spec-driven workflow 模块
// 5 步循环：propose -> plan -> apply -> review -> archive
// 7+1 类 artifact：RM/MS/CH/PR/DS/SP/T/RV
// 序列编号体系（counters 表自增，非时间戳）
#![allow(dead_code)]

mod orchestrator;
mod phase;
mod store;
mod types;

pub use orchestrator::{SpawnSubagentHint, WorkflowOrchestrator};
pub use types::*;

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

/// 工作流存储：管理 spec-driven 开发的所有 artifact
pub struct WorkflowStore {
    pub(super) conn: Mutex<Connection>,
}

impl WorkflowStore {
    /// 打开/创建 workflow 数据库，兼容旧库自动升级
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;

        // 基础表（兼容旧库）
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS roadmaps (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft',
                profile TEXT NOT NULL DEFAULT 'standard',
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS milestones (
                id TEXT PRIMARY KEY,
                roadmap_id TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft',
                sort_order INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (roadmap_id) REFERENCES roadmaps(id)
            );
            CREATE TABLE IF NOT EXISTS changes (
                id TEXT PRIMARY KEY,
                milestone_id TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft',
                phase TEXT NOT NULL DEFAULT 'propose',
                spec_id TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (milestone_id) REFERENCES milestones(id)
            );
            CREATE TABLE IF NOT EXISTS specs (
                id TEXT PRIMARY KEY,
                change_id TEXT NOT NULL,
                title TEXT NOT NULL,
                content TEXT NOT NULL,
                tdd INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                FOREIGN KEY (change_id) REFERENCES changes(id)
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                change_id TEXT NOT NULL,
                title TEXT NOT NULL,
                description TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'todo',
                sort_order INTEGER NOT NULL DEFAULT 0,
                impl_id TEXT,
                FOREIGN KEY (change_id) REFERENCES changes(id)
            );
            CREATE TABLE IF NOT EXISTS implementations (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL,
                description TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft',
                files_changed TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                FOREIGN KEY (task_id) REFERENCES tasks(id)
            );",
        )?;

        // 新增表（spec-driven 重写）
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS counters (
                artifact_type TEXT PRIMARY KEY,
                next_seq INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE IF NOT EXISTS proposals (
                id TEXT PRIMARY KEY,
                change_id TEXT NOT NULL,
                title TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (change_id) REFERENCES changes(id)
            );
            CREATE TABLE IF NOT EXISTS designs (
                id TEXT PRIMARY KEY,
                change_id TEXT NOT NULL,
                title TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (change_id) REFERENCES changes(id)
            );
            CREATE TABLE IF NOT EXISTS reviews (
                id TEXT PRIMARY KEY,
                change_id TEXT NOT NULL,
                title TEXT NOT NULL,
                content TEXT NOT NULL,
                verdict TEXT NOT NULL DEFAULT 'needs_work',
                created_at TEXT NOT NULL,
                FOREIGN KEY (change_id) REFERENCES changes(id)
            );",
        )?;

        // 兼容旧库：幂等添加新列（失败表示列已存在）
        let _ = conn.execute(
            "ALTER TABLE changes ADD COLUMN phase TEXT NOT NULL DEFAULT 'propose'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE roadmaps ADD COLUMN profile TEXT NOT NULL DEFAULT 'standard'",
            [],
        );
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN impl_status TEXT", []);

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 创建初始 workflow：roadmap + 第一个 milestone + 第一个 change（phase=propose）
    pub fn init_workflow(
        &self,
        roadmap_title: &str,
        roadmap_desc: &str,
        profile: WorkflowProfile,
        first_milestone_title: &str,
        first_change_title: &str,
    ) -> Result<(String, String, String)> {
        let roadmap_id = self.create_roadmap_with_profile(roadmap_title, roadmap_desc, profile)?;
        let milestone_id = self.create_milestone(&roadmap_id, first_milestone_title, "", 0)?;
        let change_id = self.create_change(&milestone_id, first_change_title, "")?;
        Ok((roadmap_id, milestone_id, change_id))
    }
}
