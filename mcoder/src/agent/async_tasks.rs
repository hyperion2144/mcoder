// 设计文档 §3.5: get_status 为 forward-looking API（当前通过 drain_completed 注入结果）
//
// Phase 5: 异步任务按 session 持久化到 SQLite（async_tasks 表）。
//
// 行为契约：
// 1. TaskManager 在 spawn 时立刻写 DB（status=running，task_id, session_id,
//    tool_name, args_json, created_at_ms）
// 2. task 完成 / 失败 / 取消 时同步写 DB
// 3. drain_completed 把已完成的 task 从内存中清除（DB 保留终态行）
// 4. mark_session_orphans_interrupted 把 session 内所有 queued/running
//    原子标记为 interrupted（**绝不自动重跑任何工具**）
// 5. SessionManager::attach_session_with_offset 的 snapshot 从 DB 读 per-session
//    tasks（不再全局 best-effort）
// 6. RPC task.list 按 session 隔离；task.cancel 只能取消 caller 所属 session 的 task

use crate::persistence::async_task_store::{AsyncTaskState, AsyncTaskStore};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// in-memory task status. **Phase 5b: now includes `Interrupted` to preserve
/// the terminal-interrupted state without remapping to Failed** (which was
/// masking it from `has_interrupted_tasks` resume policy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    /// 服务重启时被原子标记；agent inspect 后决定是否重跑（绝不自动重跑）
    Interrupted,
}

impl TaskStatus {
    pub fn to_db_state(&self) -> AsyncTaskState {
        match self {
            TaskStatus::Pending => AsyncTaskState::Queued,
            TaskStatus::Running => AsyncTaskState::Running,
            TaskStatus::Completed => AsyncTaskState::Completed,
            TaskStatus::Failed => AsyncTaskState::Failed,
            TaskStatus::Cancelled => AsyncTaskState::Cancelled,
            TaskStatus::Interrupted => AsyncTaskState::Interrupted,
        }
    }

    pub fn from_db_state(s: AsyncTaskState) -> Self {
        match s {
            AsyncTaskState::Queued => TaskStatus::Pending,
            AsyncTaskState::Running => TaskStatus::Running,
            AsyncTaskState::Completed => TaskStatus::Completed,
            AsyncTaskState::Failed => TaskStatus::Failed,
            AsyncTaskState::Cancelled => TaskStatus::Cancelled,
            AsyncTaskState::Interrupted => TaskStatus::Interrupted,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub status: TaskStatus,
    pub result: Option<String>,
    pub error: Option<String>,
}

/// Phase 5: TaskManager 现在是 per-session 的（不再全局共享）
pub struct TaskManager {
    session_id: String,
    store: Arc<AsyncTaskStore>,
    tasks: Mutex<HashMap<String, Task>>,
    handles: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl TaskManager {
    /// 创建 per-session TaskManager（Phase 5 主入口）
    pub fn new_for_session(session_id: impl Into<String>, store: Arc<AsyncTaskStore>) -> Arc<Self> {
        Arc::new(Self {
            session_id: session_id.into(),
            store,
            tasks: Mutex::new(HashMap::new()),
            handles: Mutex::new(HashMap::new()),
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn store(&self) -> Arc<AsyncTaskStore> {
        self.store.clone()
    }

    /// TaskStatus → AsyncTaskState 映射（公开用于测试）
    pub fn task_status_to_db_state(s: &TaskStatus) -> AsyncTaskState {
        s.to_db_state()
    }

    /// spawn 一个后台 task
    /// - 立刻把 task 写入 DB（status=running），使用 DB 分配的 task_id
    /// - 完成后更新 DB（status=completed/failed/cancelled）
    pub async fn spawn<F>(
        self: &Arc<Self>,
        name: impl Into<String>,
        args: serde_json::Value,
        f: F,
    ) -> Result<String, sqlx::Error>
    where
        F: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        let name = name.into();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let id = self
            .store
            .create_task(&self.session_id, &name, args, now_ms)
            .await?
            .task_id;

        self.tasks.lock().await.insert(
            id.clone(),
            Task {
                id: id.clone(),
                name: name.clone(),
                status: TaskStatus::Running,
                result: None,
                error: None,
            },
        );

        let manager = self.clone();
        let task_id = id.clone();
        let handle = tokio::spawn(async move {
            let result = f.await;
            let transition = match result {
                Ok(output) => {
                    manager
                        .store
                        .complete_task(
                            &task_id,
                            serde_json::Value::String(output),
                            chrono::Utc::now().timestamp_millis(),
                        )
                        .await
                }
                Err(error) => {
                    manager
                        .store
                        .fail_task(
                            &task_id,
                            &error,
                            chrono::Utc::now().timestamp_millis(),
                        )
                        .await
                }
            };
            if let Err(error) = transition {
                tracing::warn!("task terminal transition failed for {}: {}", task_id, error);
                return;
            }
            manager.sync_task_from_store(&task_id).await;
        });

        self.handles.lock().await.insert(id.clone(), handle);
        Ok(id)
    }

    /// 不带 args 的兼容 spawn（args 用 {"name": name} 兜底）
    /// 保留以兼容旧调用方（bash/code_exec/subagent 内部仍用单参数）
    pub async fn spawn_compat<F>(
        self: &Arc<Self>,
        name: impl Into<String>,
        f: F,
    ) -> Result<String, sqlx::Error>
    where
        F: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        let name = name.into();
        let args = serde_json::json!({"name": name.clone()});
        self.spawn(name.clone(), args, f).await
    }

    async fn sync_task_from_store(&self, id: &str) {
        let Some(record) = self.store.get_task(id).await else {
            return;
        };
        let mut tasks = self.tasks.lock().await;
        let Some(task) = tasks.get_mut(id) else {
            return;
        };
        task.status = TaskStatus::from_db_state(record.status);
        task.result = record.output_json.map(|value| match value {
            serde_json::Value::String(text) => text,
            other => other.to_string(),
        });
        task.error = record.error;
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

    /// 取消 task（DB + 内存）
    pub async fn cancel(&self, id: &str) -> Result<bool, sqlx::Error> {
        let won = self
            .store
            .cancel_task(id, chrono::Utc::now().timestamp_millis())
            .await?;
        if !won {
            self.sync_task_from_store(id).await;
            return Ok(false);
        }

        if let Some(handle) = self.handles.lock().await.remove(id) {
            handle.abort();
        }
        self.sync_task_from_store(id).await;
        Ok(true)
    }

    pub async fn drain_completed(&self) -> Vec<Task> {
        // 保留兼容旧 API：返回已完成 task 列表，但**不**从内存清除。
        // 清除与"已注入"判定由 SessionManager 持久化层负责。
        let tasks = self.tasks.lock().await;
        tasks
            .values()
            .filter(|t| {
                matches!(
                    t.status,
                    TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
                )
            })
            .cloned()
            .collect()
    }

    pub async fn has_running(&self) -> bool {
        let tasks = self.tasks.lock().await;
        tasks.values().any(|t| matches!(t.status, TaskStatus::Pending | TaskStatus::Running))
    }

    /// 重启补投：列出 DB 中已 completed/failed 但未注入的 task（in-memory 重新 hydrate）。
    pub async fn list_undelivered(&self) -> Vec<Task> {
        let Ok(records) = self
            .store
            .list_undelivered_terminal_tasks(&self.session_id)
            .await
        else {
            return Vec::new();
        };
        let mut tasks = self.tasks.lock().await;
        records
            .into_iter()
            .map(|r| {
                let task = Task {
                    id: r.task_id.clone(),
                    name: r.tool_name.clone(),
                    status: TaskStatus::from_db_state(r.status),
                    result: r.output_json.clone().map(|v| match v {
                        serde_json::Value::String(s) => s,
                        other => other.to_string(),
                    }),
                    error: r.error.clone(),
                };
                tasks.insert(task.id.clone(), task.clone());
                task
            })
            .collect()
    }
}