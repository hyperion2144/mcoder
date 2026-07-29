// SQLite CRUD 方法 + 序列编号体系 + 文件系统 artifact 存储
use anyhow::Result;
use rusqlite::params;

use super::types::{ImplStatus, ReviewVerdict, WorkflowPhase, WorkflowProfile};
use super::{ArtifactType, WorkflowStore};

impl WorkflowStore {
    /// 从 counters 表取下一个序列号，返回 "前缀-N" 格式的 ID
    /// 如 next_id("RM") -> "RM-1", next_id("SP") -> "SP-2"
    pub fn next_id(&self, artifact_type: &str) -> Result<String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        // 尝试递增计数器
        let updated = tx.execute(
            "UPDATE counters SET next_seq = next_seq + 1 WHERE artifact_type = ?1",
            params![artifact_type],
        )?;

        let seq = if updated == 0 {
            // 计数器不存在，插入初始值（next_seq=2，本次分配 1）
            tx.execute(
                "INSERT INTO counters (artifact_type, next_seq) VALUES (?1, 2)",
                params![artifact_type],
            )?;
            1
        } else {
            // 已递增，读取当前值（已 +1），本次分配 = 当前值 - 1
            let current: i64 = tx.query_row(
                "SELECT next_seq FROM counters WHERE artifact_type = ?1",
                params![artifact_type],
                |row| row.get(0),
            )?;
            current - 1
        };

        tx.commit()?;
        Ok(format!("{}-{}", artifact_type, seq))
    }

    // ============ Roadmap (RM-N) ============

    pub fn create_roadmap(&self, title: &str, description: &str) -> Result<String> {
        self.create_roadmap_with_profile(title, description, WorkflowProfile::Standard)
    }

    pub fn create_roadmap_with_profile(
        &self,
        title: &str,
        description: &str,
        profile: WorkflowProfile,
    ) -> Result<String> {
        let id = self.next_id("RM")?;
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO roadmaps (id, title, description, status, profile, created_at) VALUES (?1, ?2, ?3, 'draft', ?4, ?5)",
            params![id, title, description, profile.as_str(), now],
        )?;
        Ok(id)
    }

    // ============ Milestone (MS-N) ============

    pub fn create_milestone(
        &self,
        roadmap_id: &str,
        title: &str,
        description: &str,
        order: u32,
    ) -> Result<String> {
        let id = self.next_id("MS")?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO milestones (id, roadmap_id, title, description, status, sort_order) VALUES (?1, ?2, ?3, ?4, 'draft', ?5)",
            params![id, roadmap_id, title, description, order],
        )?;
        Ok(id)
    }

    // ============ Change (CH-N) ============

    pub fn create_change(
        &self,
        milestone_id: &str,
        title: &str,
        description: &str,
    ) -> Result<String> {
        let id = self.next_id("CH")?;
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO changes (id, milestone_id, title, description, status, phase, created_at) VALUES (?1, ?2, ?3, ?4, 'draft', 'propose', ?5)",
            params![id, milestone_id, title, description, now],
        )?;
        Ok(id)
    }

    // ============ Proposal (PR-N) - 新增 ============

    pub fn create_proposal(&self, change_id: &str, title: &str, content: &str) -> Result<String> {
        let id = self.next_id("PR")?;
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO proposals (id, change_id, title, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, change_id, title, content, now],
        )?;
        Ok(id)
    }

    pub fn get_proposals(&self, change_id: &str) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, content FROM proposals WHERE change_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![change_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ============ Design (DS-N) - 新增 ============

    pub fn create_design(&self, change_id: &str, title: &str, content: &str) -> Result<String> {
        let id = self.next_id("DS")?;
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO designs (id, change_id, title, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, change_id, title, content, now],
        )?;
        Ok(id)
    }

    pub fn get_designs(&self, change_id: &str) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, content FROM designs WHERE change_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![change_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ============ Spec (SP-N) ============

    pub fn create_spec(
        &self,
        change_id: &str,
        title: &str,
        content: &str,
        tdd: bool,
    ) -> Result<String> {
        let id = self.next_id("SP")?;
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO specs (id, change_id, title, content, tdd, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, change_id, title, content, tdd as i32, now],
        )?;
        // 关联 spec 到 change
        conn.execute(
            "UPDATE changes SET spec_id = ?1 WHERE id = ?2",
            params![id, change_id],
        )?;
        Ok(id)
    }

    // ============ Task (T-N) ============

    pub fn create_task(
        &self,
        change_id: &str,
        title: &str,
        description: &str,
        order: u32,
    ) -> Result<String> {
        let id = self.next_id("T")?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tasks (id, change_id, title, description, status, sort_order) VALUES (?1, ?2, ?3, ?4, 'todo', ?5)",
            params![id, change_id, title, description, order],
        )?;
        Ok(id)
    }

    pub fn update_task_status(&self, task_id: &str, status: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET status = ?1 WHERE id = ?2",
            params![status, task_id],
        )?;
        Ok(())
    }

    /// 更新 task 的实现状态（替代原 implementations 表）
    pub fn update_task_impl_status(&self, task_id: &str, status: ImplStatus) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET impl_status = ?1 WHERE id = ?2",
            params![status.as_str(), task_id],
        )?;
        Ok(())
    }

    /// 兼容方法：原 create_implementation，内部改为更新 task 的 impl_status
    pub fn create_implementation(
        &self,
        task_id: &str,
        _title: &str,
        _description: &str,
    ) -> Result<String> {
        let id = self.next_id("IM")?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET impl_id = ?1, impl_status = 'in_progress' WHERE id = ?2",
            params![id, task_id],
        )?;
        Ok(id)
    }

    // ============ Review (RV-N) - 新增 ============

    pub fn create_review(
        &self,
        change_id: &str,
        title: &str,
        content: &str,
        verdict: ReviewVerdict,
    ) -> Result<String> {
        let id = self.next_id("RV")?;
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO reviews (id, change_id, title, content, verdict, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, change_id, title, content, verdict.as_str(), now],
        )?;
        Ok(id)
    }

    pub fn get_reviews(&self, change_id: &str) -> Result<Vec<(String, String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, content, verdict FROM reviews WHERE change_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![change_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ============ Phase 状态机 ============

    /// 5 步循环推进：propose->plan->apply->review->archive
    pub fn transition_phase(&self, change_id: &str) -> Result<WorkflowPhase> {
        let current = self.get_change_phase(change_id)?;
        let next = current.next().ok_or_else(|| {
            anyhow::anyhow!("change {} already in archive phase (terminal)", change_id)
        })?;

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE changes SET phase = ?1 WHERE id = ?2",
            params![next.as_str(), change_id],
        )?;
        // 进入 archive 时同步将 status 置为 completed
        if next == WorkflowPhase::Archive {
            conn.execute(
                "UPDATE changes SET status = 'completed' WHERE id = ?1",
                params![change_id],
            )?;
        }
        Ok(next)
    }

    /// 显式设置 change 的 phase（用于回退或跳转）
    pub fn set_phase(&self, change_id: &str, phase: WorkflowPhase) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE changes SET phase = ?1 WHERE id = ?2",
            params![phase.as_str(), change_id],
        )?;
        Ok(())
    }

    pub fn get_change_phase(&self, change_id: &str) -> Result<WorkflowPhase> {
        let conn = self.conn.lock().unwrap();
        let phase_str: String = conn
            .query_row(
                "SELECT phase FROM changes WHERE id = ?1",
                params![change_id],
                |row| row.get(0),
            )
            .map_err(|e| anyhow::anyhow!("change {} not found: {}", change_id, e))?;
        WorkflowPhase::from_str(&phase_str)
            .ok_or_else(|| anyhow::anyhow!("invalid phase value in db: {}", phase_str))
    }

    // ============ 查询方法 ============

    pub fn list_roadmaps(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, title, status FROM roadmaps ORDER BY created_at DESC")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn get_milestones(&self, roadmap_id: &str) -> Result<Vec<(String, String, String, u32)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, status, sort_order FROM milestones WHERE roadmap_id = ?1 ORDER BY sort_order",
        )?;
        let rows = stmt.query_map(params![roadmap_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn get_changes(&self, milestone_id: &str) -> Result<Vec<(String, String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, status, phase FROM changes WHERE milestone_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![milestone_id], |row| {
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
        let mut stmt = conn.prepare(
            "SELECT id, title, status, sort_order FROM tasks WHERE change_id = ?1 ORDER BY sort_order",
        )?;
        let rows = stmt.query_map(params![change_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// 查询 tasks（含 impl_status 字段），返回五元组
    /// 用于 tasks_full 查询 op，补全 impl_status 读取路径
    pub fn get_tasks_with_impl(
        &self,
        change_id: &str,
    ) -> Result<Vec<(String, String, String, u32, Option<String>)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, status, sort_order, impl_status FROM tasks WHERE change_id = ?1 ORDER BY sort_order",
        )?;
        let rows = stmt.query_map(params![change_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ============ P2: 补全查询和更新方法 ============

    /// 查询 change 关联的所有 spec，返回 (id, title, content, tdd)
    pub fn get_specs(&self, change_id: &str) -> Result<Vec<(String, String, String, bool)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, content, tdd FROM specs WHERE change_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![change_id], |row| {
            let tdd: i32 = row.get(3)?;
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?, tdd != 0))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// 查询 change 关联的最新 spec，返回 (id, title, content, tdd)
    pub fn get_spec_for_change(
        &self,
        change_id: &str,
    ) -> Result<Option<(String, String, String, bool)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, content, tdd FROM specs WHERE change_id = ?1 ORDER BY created_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![change_id], |row| {
            let tdd: i32 = row.get(3)?;
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?, tdd != 0))
        })?;
        if let Some(row) = rows.next() {
            Ok(Some(row?))
        } else {
            Ok(None)
        }
    }

    pub fn update_roadmap_status(&self, roadmap_id: &str, status: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE roadmaps SET status = ?1 WHERE id = ?2",
            params![status, roadmap_id],
        )?;
        Ok(())
    }

    pub fn update_milestone_status(&self, milestone_id: &str, status: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE milestones SET status = ?1 WHERE id = ?2",
            params![status, milestone_id],
        )?;
        Ok(())
    }

    pub fn update_change_status(&self, change_id: &str, status: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE changes SET status = ?1 WHERE id = ?2",
            params![status, change_id],
        )?;
        Ok(())
    }

    pub fn update_spec_content(&self, spec_id: &str, content: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE specs SET content = ?1 WHERE id = ?2",
            params![content, spec_id],
        )?;
        Ok(())
    }

    // ============ 文件系统存储方法 ============
    // artifact content 走文件系统（<project>/.mcoder/workflow/），SQLite 只存元数据

    /// workflow 文件系统根目录
    fn workflow_dir(&self) -> std::path::PathBuf {
        self.project_dir.join(".mcoder").join("workflow")
    }

    /// change 目录路径
    fn change_dir(&self, change_name: &str) -> std::path::PathBuf {
        self.workflow_dir().join("changes").join(change_name)
    }

    /// 读取 artifact 文件内容
    pub fn read_artifact(&self, change_name: &str, artifact_type: &ArtifactType) -> Result<Option<String>> {
        let path = self.change_dir(change_name).join(artifact_type.relative_path());
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(Some(content))
    }

    /// 写入 artifact 文件内容
    pub fn write_artifact(
        &self,
        change_name: &str,
        artifact_type: &ArtifactType,
        content: &str,
    ) -> Result<()> {
        let path = self.change_dir(change_name).join(artifact_type.relative_path());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// 列出活跃变更（changes/ 下的目录名，排除 archive）
    pub fn list_changes(&self) -> Result<Vec<String>> {
        let changes_dir = self.workflow_dir().join("changes");
        if !changes_dir.exists() {
            return Ok(Vec::new());
        }
        let mut changes = Vec::new();
        for entry in std::fs::read_dir(&changes_dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name != "archive" {
                    changes.push(name);
                }
            }
        }
        changes.sort();
        Ok(changes)
    }

    /// 归档变更：移动到 changes/archive/<date>-<name>/
    pub fn archive_change(&self, change_name: &str) -> Result<()> {
        let src = self.change_dir(change_name);
        if !src.exists() {
            return Err(anyhow::anyhow!("change directory not found: {}", change_name));
        }
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let dest = self
            .workflow_dir()
            .join("changes")
            .join("archive")
            .join(format!("{}-{}", date, change_name));
        std::fs::rename(&src, &dest)?;
        Ok(())
    }

    /// 读取全局 spec（specs/<domain>/spec.md）
    pub fn read_global_spec(&self, domain: &str) -> Result<Option<String>> {
        let path = self.workflow_dir().join("specs").join(domain).join("spec.md");
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(Some(content))
    }

    /// 写入全局 spec（specs/<domain>/spec.md）
    pub fn write_global_spec(&self, domain: &str, content: &str) -> Result<()> {
        let path = self.workflow_dir().join("specs").join(domain).join("spec.md");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// delta spec merge：将 change 的 delta spec 合并到全局 spec
    pub fn merge_delta_spec(&self, change_name: &str, domain: &str) -> Result<()> {
        let delta_spec = self
            .read_artifact(change_name, &ArtifactType::DeltaSpec(domain.to_string()))?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "delta spec not found for change {} domain {}",
                    change_name,
                    domain
                )
            })?;
        let global_spec = self.read_global_spec(domain)?.unwrap_or_default();
        let merged = super::delta_merge::merge_delta_spec(&delta_spec, &global_spec)?;
        self.write_global_spec(domain, &merged)?;
        Ok(())
    }
}
