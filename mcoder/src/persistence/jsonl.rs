// 设计文档 §5.4: read_tail 为 forward-looking API（当前通过完整重放加载历史）
#![allow(dead_code)]

use crate::config::global_config_dir;
use crate::types::Message;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub project_path: PathBuf,
    pub title: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub model: String,
}

pub struct JsonlSession {
    path: PathBuf,
    meta: SessionMeta,
}

impl JsonlSession {
    pub fn create(project: &Path, title: impl Into<String>, model: impl Into<String>) -> Result<Self> {
        let session_id = format!(
            "{}-{}",
            chrono::Utc::now().format("%Y%m%d-%H%M%S"),
            uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect::<String>()
        );

        let sessions_dir = crate::config::global_config_dir()
            .join("sessions")
            .join(escape_project_path(project));
        std::fs::create_dir_all(&sessions_dir)?;

        let path = sessions_dir.join(format!("{}.jsonl", session_id));
        let meta_path = sessions_dir.join(format!("{}.meta.json", session_id));

        let meta = SessionMeta {
            session_id: session_id.clone(),
            project_path: project.to_path_buf(),
            title: title.into(),
            created_at: chrono::Utc::now(),
            model: model.into(),
        };

        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
        std::fs::File::create(&path)?;

        Ok(Self { path, meta })
    }

    /// 按 session_id 加载会话，自动从 meta.json 读取 project_path 定位文件
    /// 设计文档 §5.4: 不再依赖外部传入 project，跨项目加载只需 session_id
    pub fn load(session_id: &str) -> Result<Self> {
        // 遍历所有项目目录查找 session_id
        let base = crate::config::global_config_dir().join("sessions");
        if base.exists() {
            for proj_entry in std::fs::read_dir(&base)? {
                let proj_entry = proj_entry?;
                let meta_path = proj_entry.path().join(format!("{}.meta.json", session_id));
                if meta_path.exists() {
                    let meta: SessionMeta = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
                    let path = proj_entry.path().join(format!("{}.jsonl", session_id));
                    return Ok(Self { path, meta });
                }
            }
        }
        anyhow::bail!("session not found: {}", session_id)
    }

    /// 按项目路径过滤列出会话，传 None 返回所有项目的会话
    pub fn list(project: Option<&Path>) -> Result<Vec<SessionMeta>> {
        let base = crate::config::global_config_dir().join("sessions");
        let mut metas = Vec::new();

        if !base.exists() {
            return Ok(metas);
        }

        let filter = project.map(escape_project_path);

        for entry in std::fs::read_dir(&base)? {
            let entry = entry?;
            if let Some(ref proj) = filter {
                if entry.file_name().to_string_lossy() != *proj {
                    continue;
                }
            }
            // 兼容：可能目录名是旧的 hash 格式，尝试读取里面的 meta
            for meta_entry in std::fs::read_dir(entry.path())? {
                let meta_entry = meta_entry?;
                let name = meta_entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".meta.json") {
                    if let Ok(content) = std::fs::read_to_string(meta_entry.path()) {
                        if let Ok(meta) = serde_json::from_str::<SessionMeta>(&content) {
                            metas.push(meta);
                        }
                    }
                }
            }
        }

        metas.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(metas)
    }

    pub fn id(&self) -> &str { &self.meta.session_id }
    pub fn meta(&self) -> &SessionMeta { &self.meta }
    pub fn project_path(&self) -> &Path { &self.meta.project_path }

    pub fn append(&self, msg: &Message) -> Result<()> {
        let line = serde_json::to_string(msg)?;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .with_context(|| format!("appending to {}", self.path.display()))?;
        use std::io::Write;
        writeln!(file, "{}", line)?;
        Ok(())
    }

    pub fn read_all(&self) -> Result<Vec<Message>> {
        let content = std::fs::read_to_string(&self.path)?;
        let mut msgs = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() { continue; }
            msgs.push(serde_json::from_str(line)?);
        }
        Ok(msgs)
    }

    pub fn read_tail(&self, count: usize) -> Result<Vec<Message>> {
        let all = self.read_all()?;
        let start = all.len().saturating_sub(count);
        Ok(all.into_iter().skip(start).collect())
    }

    /// 设计文档 §5.4: session.delete - 按 session_id 删除会话文件
    /// 自动从 meta.json 读取 project_path 定位文件，无需外部传入 project
    pub fn delete(session_id: &str) -> Result<()> {
        let base = global_config_dir().join("sessions");
        if base.exists() {
            for proj_entry in std::fs::read_dir(&base)? {
                let proj_entry = proj_entry?;
                let jsonl_path = proj_entry.path().join(format!("{}.jsonl", session_id));
                let meta_path = proj_entry.path().join(format!("{}.meta.json", session_id));
                if jsonl_path.exists() {
                    std::fs::remove_file(&jsonl_path)?;
                }
                if meta_path.exists() {
                    std::fs::remove_file(&meta_path)?;
                }
                if jsonl_path.exists() || meta_path.exists() {
                    return Ok(());
                }
            }
        }
        anyhow::bail!("session not found: {}", session_id)
    }
}

/// 将项目路径转义为合法目录名（/ → _，可读且可逆）
/// 例: /Users/mutou/projA → _Users_mutou_projA
pub fn escape_project_path(project: &Path) -> String {
    project.to_string_lossy().replace('/', "_")
}
