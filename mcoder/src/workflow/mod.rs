// 设计文档 §8.5: workflow 系统的实体类型为 M3 蓝图式开发
// 5 步循环：propose → plan → apply → review → archive
// 3 个内置子代理：planner / executor / reviewer（在 agent/role.rs 中定义）
// profile: lite（顺序执行、TDD 可选、review 任意通过）/ standard（并行、TDD 强制、review 全通过）
#![allow(dead_code)]

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

/// 工作流系统：blueprint 风格的项目变更管理
/// 路线图 → 里程碑 → 变更(Change) → 规格(Spec) → 任务(Task) → 实现(Impl)
/// 每个实体有编号，可关联启动，形成项目变更图谱
pub struct WorkflowStore {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Roadmap {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: WorkflowStatus,
    pub created_at: String,
    pub milestones: Vec<Milestone>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    pub id: String,
    pub roadmap_id: String,
    pub title: String,
    pub description: String,
    pub status: WorkflowStatus,
    pub order: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    pub milestone_id: String,
    pub title: String,
    pub description: String,
    pub status: WorkflowStatus,
    pub phase: WorkflowPhase,
    pub spec_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    pub id: String,
    pub change_id: String,
    pub title: String,
    pub content: String,
    pub tdd: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub change_id: String,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub order: u32,
    pub impl_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implementation {
    pub id: String,
    pub task_id: String,
    pub description: String,
    pub status: WorkflowStatus,
    pub files_changed: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Draft,
    Active,
    InProgress,
    Completed,
    Archived,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Todo,
    InProgress,
    Done,
    Blocked,
}

/// 设计文档 §8.5: workflow 5 步循环阶段
/// propose → plan → apply → review → archive
/// 顺序不可跳转，只能逐级推进（transition_phase 会校验）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    /// 提案阶段：创建 Change，描述要做什么、为什么
    Propose,
    /// 规划阶段：planner 子代理生成 spec/tasks
    Plan,
    /// 执行阶段：executor 子代理按 spec 实现
    Apply,
    /// 审查阶段：reviewer 子代理检查实现是否符合 spec
    Review,
    /// 归档阶段：已审查通过，归档变更
    Archive,
}

impl WorkflowPhase {
    /// 设计文档 §8.5: 5 步循环的合法顺序
    /// 返回下一个阶段（如果已是 Archive 则返回 None）
    pub fn next(self) -> Option<WorkflowPhase> {
        match self {
            WorkflowPhase::Propose => Some(WorkflowPhase::Plan),
            WorkflowPhase::Plan => Some(WorkflowPhase::Apply),
            WorkflowPhase::Apply => Some(WorkflowPhase::Review),
            WorkflowPhase::Review => Some(WorkflowPhase::Archive),
            WorkflowPhase::Archive => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowPhase::Propose => "propose",
            WorkflowPhase::Plan => "plan",
            WorkflowPhase::Apply => "apply",
            WorkflowPhase::Review => "review",
            WorkflowPhase::Archive => "archive",
        }
    }

    pub fn from_str(s: &str) -> Option<WorkflowPhase> {
        match s {
            "propose" => Some(WorkflowPhase::Propose),
            "plan" => Some(WorkflowPhase::Plan),
            "apply" => Some(WorkflowPhase::Apply),
            "review" => Some(WorkflowPhase::Review),
            "archive" => Some(WorkflowPhase::Archive),
            _ => None,
        }
    }
}

/// 设计文档 §8.5: workflow profile
/// lite: 顺序执行、TDD 可选、review 任意通过
/// standard: 并行、TDD 强制、review 全通过
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowProfile {
    Lite,
    Standard,
}

impl Default for WorkflowProfile {
    fn default() -> Self {
        WorkflowProfile::Standard
    }
}

impl WorkflowStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;

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

        // 设计文档 §8.5: 兼容旧库（无 phase 列时添加）
        // ALTER TABLE ADD COLUMN 是幂等的（失败表示列已存在）
        let _ = conn.execute("ALTER TABLE changes ADD COLUMN phase TEXT NOT NULL DEFAULT 'propose'", []);
        let _ = conn.execute("ALTER TABLE roadmaps ADD COLUMN profile TEXT NOT NULL DEFAULT 'standard'", []);

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 设计文档 §8.5: workflow init
    /// 创建一个完整的初始 workflow：roadmap + 第一个 milestone + 第一个 change（phase=propose）
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

    pub fn create_roadmap(&self, title: &str, description: &str) -> Result<String> {
        self.create_roadmap_with_profile(title, description, WorkflowProfile::Standard)
    }

    pub fn create_roadmap_with_profile(
        &self,
        title: &str,
        description: &str,
        profile: WorkflowProfile,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = format!("RM-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"));
        let now = chrono::Utc::now().to_rfc3339();
        let profile_str = match profile {
            WorkflowProfile::Lite => "lite",
            WorkflowProfile::Standard => "standard",
        };
        conn.execute(
            "INSERT INTO roadmaps (id, title, description, status, profile, created_at) VALUES (?1, ?2, ?3, 'draft', ?4, ?5)",
            rusqlite::params![id, title, description, profile_str, now],
        )?;
        Ok(id)
    }

    pub fn create_milestone(&self, roadmap_id: &str, title: &str, description: &str, order: u32) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = format!("MS-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"));
        conn.execute(
            "INSERT INTO milestones (id, roadmap_id, title, description, status, sort_order) VALUES (?1, ?2, ?3, ?4, 'draft', ?5)",
            rusqlite::params![id, roadmap_id, title, description, order],
        )?;
        Ok(id)
    }

    pub fn create_change(&self, milestone_id: &str, title: &str, description: &str) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = format!("CH-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"));
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO changes (id, milestone_id, title, description, status, phase, created_at) VALUES (?1, ?2, ?3, ?4, 'draft', 'propose', ?5)",
            rusqlite::params![id, milestone_id, title, description, now],
        )?;
        Ok(id)
    }

    pub fn create_spec(&self, change_id: &str, title: &str, content: &str, tdd: bool) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = format!("SP-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"));
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO specs (id, change_id, title, content, tdd, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![id, change_id, title, content, tdd as i32, now],
        )?;
        // Link spec to change
        conn.execute(
            "UPDATE changes SET spec_id = ?1 WHERE id = ?2",
            rusqlite::params![id, change_id],
        )?;
        Ok(id)
    }

    pub fn create_task(&self, change_id: &str, title: &str, description: &str, order: u32) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = format!("TK-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"));
        conn.execute(
            "INSERT INTO tasks (id, change_id, title, description, status, sort_order) VALUES (?1, ?2, ?3, ?4, 'todo', ?5)",
            rusqlite::params![id, change_id, title, description, order],
        )?;
        Ok(id)
    }

    pub fn update_task_status(&self, task_id: &str, status: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET status = ?1 WHERE id = ?2",
            rusqlite::params![status, task_id],
        )?;
        Ok(())
    }

    pub fn create_implementation(&self, task_id: &str, _title: &str, description: &str) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = format!("IM-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"));
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO implementations (id, task_id, description, status, files_changed, created_at) VALUES (?1, ?2, ?3, 'draft', '', ?4)",
            rusqlite::params![id, task_id, description, now],
        )?;
        // Link implementation to task
        conn.execute(
            "UPDATE tasks SET impl_id = ?1 WHERE id = ?2",
            rusqlite::params![id, task_id],
        )?;
        Ok(id)
    }

    /// 设计文档 §8.5: 5 步循环推进
    /// 将 change 从当前 phase 推进到下一个 phase（propose→plan→apply→review→archive）
    /// 返回新的 phase；如果已在 archive 阶段则返回错误
    pub fn transition_phase(&self, change_id: &str) -> Result<WorkflowPhase> {
        let current = self.get_change_phase(change_id)?;
        let next = current.next()
            .ok_or_else(|| anyhow::anyhow!("change {} already in archive phase (terminal)", change_id))?;

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE changes SET phase = ?1 WHERE id = ?2",
            rusqlite::params![next.as_str(), change_id],
        )?;
        // 进入 archive 时同步将 status 置为 completed
        if next == WorkflowPhase::Archive {
            conn.execute(
                "UPDATE changes SET status = 'completed' WHERE id = ?1",
                rusqlite::params![change_id],
            )?;
        }
        Ok(next)
    }

    /// 设计文档 §8.5: 显式设置 change 的 phase（用于回退或跳转，需调用方自行保证合法性）
    pub fn set_phase(&self, change_id: &str, phase: WorkflowPhase) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE changes SET phase = ?1 WHERE id = ?2",
            rusqlite::params![phase.as_str(), change_id],
        )?;
        Ok(())
    }

    pub fn get_change_phase(&self, change_id: &str) -> Result<WorkflowPhase> {
        let conn = self.conn.lock().unwrap();
        let phase_str: String = conn.query_row(
            "SELECT phase FROM changes WHERE id = ?1",
            rusqlite::params![change_id],
            |row| row.get(0),
        ).map_err(|e| anyhow::anyhow!("change {} not found: {}", change_id, e))?;
        WorkflowPhase::from_str(&phase_str)
            .ok_or_else(|| anyhow::anyhow!("invalid phase value in db: {}", phase_str))
    }

    pub fn list_roadmaps(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, title, status FROM roadmaps ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn get_milestones(&self, roadmap_id: &str) -> Result<Vec<(String, String, String, u32)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, title, status, sort_order FROM milestones WHERE roadmap_id = ?1 ORDER BY sort_order")?;
        let rows = stmt.query_map(rusqlite::params![roadmap_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// 设计文档 §8.5: 查询 change（含 phase 字段）
    pub fn get_changes(&self, milestone_id: &str) -> Result<Vec<(String, String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, title, status, phase FROM changes WHERE milestone_id = ?1 ORDER BY created_at")?;
        let rows = stmt.query_map(rusqlite::params![milestone_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn get_tasks(&self, change_id: &str) -> Result<Vec<(String, String, String, u32)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, title, status, sort_order FROM tasks WHERE change_id = ?1 ORDER BY sort_order")?;
        let rows = stmt.query_map(rusqlite::params![change_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }
}
