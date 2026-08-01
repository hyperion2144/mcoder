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
    /// 当前消息树分支末端消息 id（用于分叉/切换；None=空会话或未设置）
    #[serde(default)]
    pub current_head_id: Option<String>,
    /// 父 session id（子代理/handoff 创建时设置；普通 session 为 None）
    #[serde(default)]
    pub parent_session_id: Option<String>,
    /// session 来源
    #[serde(default)]
    pub source: SessionSource,
    /// 子代理 role（仅 source=subagent 时有值）
    #[serde(default)]
    pub subagent_role: Option<String>,
    /// 子代理任务描述（仅 source=subagent/handoff 时有值）
    #[serde(default)]
    pub task_description: Option<String>,
}

/// session 来源类型
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    #[default]
    Normal,
    /// subagent 工具创建
    Subagent,
    /// /handoff 命令创建
    Handoff,
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
            current_head_id: None,
            parent_session_id: None,
            source: SessionSource::default(),
            subagent_role: None,
            task_description: None,
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
    pub fn current_head_id(&self) -> Option<&str> { self.meta.current_head_id.as_deref() }

    /// 更新 current_head_id 并持久化到 meta.json（用于消息树分叉/切换）
    pub fn update_head_id(&mut self, head_id: impl Into<String>) -> Result<()> {
        self.meta.current_head_id = Some(head_id.into());
        let meta_path = self.path.with_extension("meta.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&self.meta)?)?;
        Ok(())
    }

    /// 更新 model 并持久化到 meta.json（用于运行时切换模型）
    pub fn update_model(&mut self, model: impl Into<String>) -> Result<()> {
        self.meta.model = model.into();
        let meta_path = self.path.with_extension("meta.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&self.meta)?)?;
        Ok(())
    }

    /// 设置子代理/handoff 元数据并持久化
    pub fn set_child_meta(
        &mut self,
        parent_session_id: &str,
        source: SessionSource,
        subagent_role: Option<&str>,
        task_description: Option<&str>,
    ) -> Result<()> {
        self.meta.parent_session_id = Some(parent_session_id.to_string());
        self.meta.source = source;
        self.meta.subagent_role = subagent_role.map(String::from);
        self.meta.task_description = task_description.map(String::from);
        let meta_path = self.path.with_extension("meta.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&self.meta)?)?;
        Ok(())
    }

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
