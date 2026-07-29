// 设计文档 §3.5: get_status 为 forward-looking API（当前通过 drain_completed 注入结果）
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub status: TaskStatus,
    pub result: Option<String>,
    pub error: Option<String>,
}

pub struct TaskManager {
    tasks: Mutex<HashMap<String, Task>>,
    handles: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            handles: Mutex::new(HashMap::new()),
        }
    }

    pub async fn spawn<F>(self: &Arc<Self>, name: impl Into<String>, f: F) -> String
    where
        F: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        let id = Uuid::new_v4().to_string();
        let task = Task {
            id: id.clone(),
            name: name.into(),
            status: TaskStatus::Running,
            result: None,
            error: None,
        };
        self.tasks.lock().await.insert(id.clone(), task);

        let manager = self.clone();
        let task_id = id.clone();
        let handle = tokio::spawn(async move {
            let result = f.await;
            let mut tasks = manager.tasks.lock().await;
            if let Some(t) = tasks.get_mut(&task_id) {
                match result {
                    Ok(r) => {
                        t.status = TaskStatus::Completed;
                        t.result = Some(r);
                    }
                    Err(e) => {
                        t.status = TaskStatus::Failed;
                        t.error = Some(e);
                    }
                }
            }
        });

        self.handles.lock().await.insert(id.clone(), handle);
        id
    }

    pub async fn get_status(&self, id: &str) -> Option<TaskStatus> {
        self.tasks.lock().await.get(id).map(|t| t.status.clone())
    }

    pub async fn get_task(&self, id: &str) -> Option<Task> {
        self.tasks.lock().await.get(id).cloned()
    }

    pub async fn list(&self) -> Vec<Task> {
        self.tasks.lock().await.values().cloned().collect()
    }

    pub async fn cancel(&self, id: &str) -> bool {
        if let Some(handle) = self.handles.lock().await.remove(id) {
            handle.abort();
            if let Some(t) = self.tasks.lock().await.get_mut(id) {
                t.status = TaskStatus::Cancelled;
            }
            true
        } else {
            false
        }
    }

    /// 取出所有已完成（Completed/Failed/Cancelled）的任务并从列表中移除
    /// 设计文档 §3.5: agent loop 每轮调用，将结果作为新消息注入 session
    /// 设计文档 §3.7: 任务完成后结果作为新消息追加到 session，下一轮 LLM 调用自动看到
    pub async fn drain_completed(&self) -> Vec<Task> {
        let mut tasks = self.tasks.lock().await;
        let mut drained: Vec<Task> = Vec::new();
        let mut to_remove: Vec<String> = Vec::new();
        for (id, t) in tasks.iter() {
            if matches!(t.status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled) {
                drained.push(t.clone());
                to_remove.push(id.clone());
            }
        }
        for id in &to_remove {
            tasks.remove(id);
        }
        // 同步清理已结束的 JoinHandle（防止内存泄漏）
        let mut handles = self.handles.lock().await;
        for id in &to_remove {
            handles.remove(id);
        }
        drained
    }

    /// 是否还有运行中（Pending/Running）的任务
    /// 设计文档 §3.5: 没有工具调用且无 running 后台任务 → 结束
    pub async fn has_running(&self) -> bool {
        let tasks = self.tasks.lock().await;
        tasks.values().any(|t| matches!(t.status, TaskStatus::Pending | TaskStatus::Running))
    }
}
