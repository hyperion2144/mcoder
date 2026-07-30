// 设计文档 §8.5 M3 协作: 子代理系统
// - 通用 subagent 工具（spawn/status/result/list/ask/done）
// - 通过 role 配置指定模型和工具白名单
// - 后台异步执行，支持超时
// - 双阈值失败检测：连续 N=3 次 + 单轮内同一工具 M=5 次
#![allow(dead_code)]

use crate::agent::role::RoleRegistry;
use crate::llm::{create_adapter, SharedLLM};
use crate::tools::ToolRegistry;
use crate::types::{ContentBlock, Message, ModelConfig, Role, ToolOutput, ToolSchema};
use crate::tools::Tool;
use crate::tools::ToolContext;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 子代理运行时：每次 spawn 时根据 role 构建
/// 持有该子代理专属的 LLM adapter（可能因 role 指定不同 model）和工具过滤配置
pub struct SubagentRuntime {
    pub llm: SharedLLM,
    pub tools: Arc<ToolRegistry>,
    pub model_config: ModelConfig,
    pub allowed_tools: Vec<String>, // 空 = 全部工具
    pub system_prompt: String,
}

/// 子代理依赖：late binding 注入，解决 ToolRegistry → SubagentTool → SubagentRuntime → ToolRegistry 循环依赖
/// 在 registry 构建完成后通过 set_dependencies 注入
pub struct SubagentDeps {
    pub default_llm: SharedLLM,
    pub default_model_config: ModelConfig,
    pub tools: Arc<ToolRegistry>,
    pub role_registry: Arc<RoleRegistry>,
}

/// 子代理工具：让 agent 能启动子代理执行子任务
/// 子代理默认异步执行，通过 task_id 查询状态/获取结果
/// 子代理之间不能调度，只能通过消息互通（ask 模式）
/// 双阈值失败检测：连续 N=3 次 + 单轮内同一工具 M=5 次
pub struct SubagentTool {
    /// late binding 依赖（registry 构建后注入）
    deps: Arc<Mutex<Option<Arc<SubagentDeps>>>>,
    pub tasks: Arc<Mutex<HashMap<String, SubagentTask>>>,
    pub default_max_iters: u32,
    pub max_consecutive_failures: u32,
    pub max_per_iter_tool_failures: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTask {
    pub id: String,
    pub role: String,
    pub task: String,
    pub status: String,
    pub result: Option<String>,
    pub created_at: String,
    pub error: Option<String>,
}

impl SubagentTool {
    pub fn new() -> Self {
        Self {
            deps: Arc::new(Mutex::new(None)),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            default_max_iters: 50,
            max_consecutive_failures: 3,
            max_per_iter_tool_failures: 5,
        }
    }

    /// Late binding: 在 registry 构建完成后注入依赖
    pub async fn set_dependencies(&self, deps: Arc<SubagentDeps>) {
        *self.deps.lock().await = Some(deps);
    }

    /// 根据 role 名构建子代理 runtime
    /// - role 有自定义 model → 动态创建 LLM adapter
    /// - role 有 allowed_tools → 传递给 runtime 做过滤
    /// - role 无自定义 model → 复用 default_llm
    fn build_runtime(deps: &SubagentDeps, role_name: &str) -> Result<SubagentRuntime> {
        let role = deps.role_registry.get(role_name);
        let (model_config, llm, allowed_tools, system_prompt) = match role {
            Some(r) => {
                // role 有自定义 model 且与默认不同 → 创建新 adapter
                let (mc, llm) = if let Some(ref role_model) = r.model {
                    if role_model.name == deps.default_model_config.name {
                        (deps.default_model_config.clone(), deps.default_llm.clone())
                    } else {
                        let adapter = create_adapter(role_model)
                            .with_context(|| format!("creating LLM adapter for role '{}' model '{}'", role_name, role_model.name))?;
                        (role_model.clone(), adapter)
                    }
                } else {
                    (deps.default_model_config.clone(), deps.default_llm.clone())
                };
                (mc, llm, r.allowed_tools.clone(), r.system_prompt.clone())
            }
            None => {
                // role 不存在，用默认配置
                (deps.default_model_config.clone(), deps.default_llm.clone(), Vec::new(), String::new())
            }
        };

        Ok(SubagentRuntime {
            llm,
            tools: deps.tools.clone(),
            model_config,
            allowed_tools,
            system_prompt,
        })
    }
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str { "subagent" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "subagent".into(),
            description: "Spawn or manage sub-agents. op=spawn|status|result|list|ask|done. \
                Sub-agents run async by default. \
                spawn: role selects model+tools whitelist (e.g. planner/executor/reviewer), \
                max_iters/timeout optional (defaults from role config). \
                ask = send message to another sub-agent (no scheduling). \
                Sub-agent returns its final result by calling subagent with op=done. \
                Failure detection: consecutive N=3 failures OR same-tool M=5 failures in one iter.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["spawn", "status", "result", "list", "ask", "done"] },
                    "role": { "type": "string", "description": "spawn: role name (e.g. coder, reviewer, planner), default 'subagent'" },
                    "task": { "type": "string", "description": "spawn: task description for the sub-agent" },
                    "max_iters": { "type": "integer", "description": "spawn: override max iterations (default from role or 50)" },
                    "timeout": { "type": "integer", "description": "spawn: timeout in seconds (default from role or none)" },
                    "id": { "type": "string", "description": "status/result/ask/done: target sub-agent id" },
                    "message": { "type": "string", "description": "ask: message to send to another sub-agent" },
                    "result": { "type": "string", "description": "done: final result from sub-agent" }
                },
                "required": ["op"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let op: String = serde_json::from_value(args["op"].clone())?;

        match op.as_str() {
            "spawn" => {
                let role = args["role"].as_str().unwrap_or("subagent").to_string();
                let task: String = serde_json::from_value(args["task"].clone())
                    .context("task required for spawn")?;
                let max_iters_override: Option<u32> = args["max_iters"].as_u64().map(|n| n as u32);
                let timeout_override: Option<u32> = args["timeout"].as_u64().map(|n| n as u32);
                let id = format!("sa-{}", chrono::Utc::now().timestamp_millis());

                // 获取 deps（late binding）
                let deps = {
                    let guard = self.deps.lock().await;
                    guard.clone()
                        .ok_or_else(|| anyhow::anyhow!("subagent deps not initialized (set_dependencies not called)"))?
                };

                // 根据 role 构建 runtime（含 model 选择和工具白名单）
                let runtime = Self::build_runtime(&deps, &role)
                    .context("building subagent runtime")?;

                // 从 role 取 max_iters 和 timeout（可被 spawn 参数覆盖）
                let role_config = deps.role_registry.get(&role);
                let max_iters = max_iters_override
                    .or_else(|| role_config.and_then(|r| r.max_iters))
                    .unwrap_or(self.default_max_iters);
                let timeout_secs = timeout_override
                    .or_else(|| role_config.and_then(|r| r.timeout));

                let entry = SubagentTask {
                    id: id.clone(),
                    role: role.clone(),
                    task: task.clone(),
                    status: "running".into(),
                    result: None,
                    created_at: chrono::Utc::now().to_rfc3339(),
                    error: None,
                };
                self.tasks.lock().await.insert(id.clone(), entry);

                let tasks_map = self.tasks.clone();
                let task_manager = ctx.task_manager.clone();
                let max_failures = self.max_consecutive_failures;
                let max_per_iter = self.max_per_iter_tool_failures;
                let sa_id = id.clone();
                let sa_task = task.clone();
                let sa_system_prompt = runtime.system_prompt.clone();
                let allowed_tools = runtime.allowed_tools.clone();
                let sa_ctx = ctx.clone();

                // 通过 TaskManager spawn 注册子代理任务
                let task_id = task_manager.spawn_compat("subagent", async move {
                    let run_fut = run_subagent(
                        Arc::new(runtime),
                        tasks_map.clone(),
                        sa_id.clone(),
                        sa_task,
                        sa_system_prompt,
                        allowed_tools,
                        max_iters,
                        max_failures,
                        max_per_iter,
                        sa_ctx,
                    );

                    // 设计文档 §8.5: 超时支持
                    if let Some(secs) = timeout_secs {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(secs as u64),
                            run_fut,
                        ).await {
                            Ok(_) => {}
                            Err(_) => {
                                let mut tasks_lock = tasks_map.lock().await;
                                if let Some(t) = tasks_lock.get_mut(&sa_id) {
                                    if t.status != "done" && t.status != "failed" {
                                        t.status = "timeout".into();
                                        t.error = Some(format!("subagent timed out after {}s", secs));
                                    }
                                }
                            }
                        }
                    } else {
                        run_fut.await;
                    }

                    // 从本地 tasks map 读取最终状态与结果
                    let tasks_lock = tasks_map.lock().await;
                    if let Some(t) = tasks_lock.get(&sa_id) {
                        match t.status.as_str() {
                            "done" => Ok(t.result.clone().unwrap_or_else(|| "(no result)".into())),
                            "failed" | "timeout" => Err(t.error.clone()
                                .unwrap_or_else(|| format!("subagent ended with status={}", t.status))),
                            other => {
                                if let Some(r) = &t.result {
                                    Ok(r.clone())
                                } else {
                                    Err(format!("subagent ended with unexpected status={}", other))
                                }
                            }
                        }
                    } else {
                        Err("subagent task disappeared".into())
                    }
                }).await?;

                Ok(ToolOutput::AsyncTask {
                    task_id,
                    handle: id.clone(),
                    status_msg: format!("subagent spawned (id={}, role={}, task={})", id, role, &task[..task.len().min(80)]),
                })
            }
            "status" => {
                let id: String = serde_json::from_value(args["id"].clone())?;
                let tasks = self.tasks.lock().await;
                let task = tasks.get(&id)
                    .context("subagent task not found")?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "id": task.id,
                    "status": task.status,
                    "role": task.role,
                    "error": task.error
                }) })
            }
            "result" => {
                let id: String = serde_json::from_value(args["id"].clone())?;
                let tasks = self.tasks.lock().await;
                let task = tasks.get(&id)
                    .context("subagent task not found")?;
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "id": task.id,
                    "status": task.status,
                    "result": task.result,
                    "task": task.task,
                    "error": task.error
                }) })
            }
            "list" => {
                let tasks = self.tasks.lock().await;
                let list: Vec<_> = tasks.values().map(|t| serde_json::json!({
                    "id": t.id,
                    "role": t.role,
                    "status": t.status,
                    "task": t.task
                })).collect();
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "tasks": list,
                    "total": list.len()
                }) })
            }
            "ask" => {
                // 子代理之间互通消息（不调度，只询问）
                let id: String = serde_json::from_value(args["id"].clone())?;
                let message: String = serde_json::from_value(args["message"].clone())
                    .context("message required for ask")?;
                let mut tasks = self.tasks.lock().await;
                let task = tasks.get_mut(&id)
                    .context("target subagent not found")?;
                // 将消息追加到任务结果中，子代理下次 LLM 调用时会看到
                let existing = task.result.clone().unwrap_or_default();
                let new_result = if existing.is_empty() {
                    format!("[ask] {}", message)
                } else {
                    format!("{}\n[ask] {}", existing, message)
                };
                task.result = Some(new_result);
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "delivered": true,
                    "to": id,
                    "message": message
                }) })
            }
            "done" => {
                // 子代理主动调用此 op 返回最终结果
                let id: String = serde_json::from_value(args["id"].clone())?;
                let result: String = serde_json::from_value(args["result"].clone())
                    .context("result required for done")?;
                let mut tasks = self.tasks.lock().await;
                let task = tasks.get_mut(&id)
                    .context("subagent not found (already done?)")?;
                if task.status == "done" || task.status == "failed" {
                    anyhow::bail!("subagent already finished with status={}", task.status);
                }
                task.status = "done".into();
                task.result = Some(result.clone());
                Ok(ToolOutput::Sync { result: serde_json::json!({
                    "done": true,
                    "id": id,
                    "result": result
                }) })
            }
            other => anyhow::bail!("unknown op: {} (use spawn|status|result|list|ask|done)", other),
        }
    }
}

/// 后台执行子代理的 agent loop
/// - 用给定的 task 作为 user message
/// - 运行 LLM 循环，模型可以调用工具
/// - 子代理通过调用 subagent op=done 来主动返回结果
/// - 双阈值失败检测：连续 max_failures 次 + 单轮内同一工具 max_per_iter 次
/// - 超过 max_iters 则标记为 timeout
/// - 执行工具前检查 allowed_tools 白名单
async fn run_subagent(
    runtime: Arc<SubagentRuntime>,
    tasks: Arc<Mutex<HashMap<String, SubagentTask>>>,
    id: String,
    task: String,
    system_prompt: String,
    allowed_tools: Vec<String>,
    max_iters: u32,
    max_failures: u32,
    max_per_iter: u32,
    ctx: ToolContext,
) {
    // 设计文档 §8.5: 子代理有独立 context window
    let prompt = if system_prompt.is_empty() {
        format!(
            "You are a sub-agent (id={}). You have been given a specific task to complete.\n\
            Use the available tools to accomplish your task.\n\
            When you are done, call the `subagent` tool with op=done, id={}, and result=<your final answer>.\n\
            If you cannot complete the task, also call op=done with an explanation of what went wrong.\n\
            Be concise and efficient.",
            id, id
        )
    } else {
        format!("{}\n\n[sub-agent id={}]", system_prompt, id)
    };

    let mut messages = vec![
        Message::system(prompt),
        Message::user(&task),
    ];

    let schemas = runtime.tools.list_schemas();
    let mut consecutive_failures = 0u32;

    for iter in 0..max_iters {
        // 调用 LLM
        let resp = match runtime.llm.chat(&messages, &schemas, &runtime.model_config).await {
            Ok(r) => r,
            Err(e) => {
                let mut tasks_lock = tasks.lock().await;
                if let Some(t) = tasks_lock.get_mut(&id) {
                    t.status = "failed".into();
                    t.error = Some(format!("LLM error at iter {}: {}", iter, e));
                }
                return;
            }
        };

        // 构建 assistant 消息
        let mut blocks: Vec<ContentBlock> = Vec::new();
        if let Some(text) = &resp.content {
            if !text.is_empty() {
                blocks.push(ContentBlock::Text { text: text.clone() });
            }
        }
        for tc in &resp.tool_calls {
            blocks.push(ContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                args: tc.args.clone(),
            });
        }
        if blocks.is_empty() {
            // 模型没返回任何内容，结束
            let mut tasks_lock = tasks.lock().await;
            if let Some(t) = tasks_lock.get_mut(&id) {
                t.status = "done".into();
                t.result = Some("(subagent returned no content)".into());
            }
            return;
        }

        let assistant_msg = Message { role: Role::Assistant, content: blocks };
        messages.push(assistant_msg);

        // 如果没有工具调用，子代理结束
        if resp.tool_calls.is_empty() {
            let mut tasks_lock = tasks.lock().await;
            if let Some(t) = tasks_lock.get_mut(&id) {
                if t.status != "done" && t.status != "failed" {
                    t.status = "done".into();
                    t.result = resp.content.clone().or(Some("(no output)".into()));
                }
            }
            return;
        }

        // 设计文档 §8.5: 单轮内同一工具失败计数（M 次也停，防死循环）
        let mut per_iter_failures: HashMap<String, u32> = HashMap::new();

        // 执行工具调用
        for tc in &resp.tool_calls {
            // 检查子代理是否已经 done（可能是上一个工具调用触发了 op=done）
            {
                let tasks_lock = tasks.lock().await;
                if let Some(t) = tasks_lock.get(&id) {
                    if t.status == "done" || t.status == "failed" {
                        return; // 子代理已结束
                    }
                }
            }

            // 设计文档 §8.5: 工具白名单检查（role.allowed_tools）
            // 空 allowed_tools = 全部允许
            let allowed = allowed_tools.is_empty() || allowed_tools.iter().any(|t| t == &tc.name);
            let result = if !allowed {
                consecutive_failures += 1;
                ToolOutput::Error {
                    message: format!("tool '{}' is not allowed for this sub-agent role", tc.name),
                }
            } else {
                match runtime.tools.execute(tc, &ctx).await {
                    Ok(out) => {
                        consecutive_failures = 0;
                        out
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        *per_iter_failures.entry(tc.name.clone()).or_insert(0) += 1;
                        ToolOutput::Error { message: e.to_string() }
                    }
                }
            };

            // 检查是否是 op=done 的返回
            if let ToolOutput::Sync { result: ref val } = result {
                if val.get("done") == Some(&serde_json::Value::Bool(true)) {
                    // 子代理已主动返回结果，status 已在 execute 中设为 done
                    return;
                }
            }

            let tool_msg = Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    id: tc.id.clone(),
                    output: result,
                }],
            };
            messages.push(tool_msg);

            // 设计文档 §8.5: 连续失败检测（N=3）
            if consecutive_failures >= max_failures {
                let mut tasks_lock = tasks.lock().await;
                if let Some(t) = tasks_lock.get_mut(&id) {
                    t.status = "failed".into();
                    t.error = Some(format!(
                        "subagent failed after {} consecutive tool call failures",
                        consecutive_failures
                    ));
                }
                return;
            }

            // 设计文档 §8.5: 单轮内同一工具失败检测（M=5，防死循环）
            let cur_failures = *per_iter_failures.get(&tc.name).unwrap_or(&0);
            if cur_failures >= max_per_iter {
                let mut tasks_lock = tasks.lock().await;
                if let Some(t) = tasks_lock.get_mut(&id) {
                    t.status = "failed".into();
                    t.error = Some(format!(
                        "subagent failed: tool '{}' failed {} times in iter {} (loop detected)",
                        tc.name, cur_failures, iter
                    ));
                }
                return;
            }
        }
    }

    // 超过 max_iters
    let mut tasks_lock = tasks.lock().await;
    if let Some(t) = tasks_lock.get_mut(&id) {
        if t.status != "done" && t.status != "failed" {
            t.status = "timeout".into();
            t.error = Some(format!("subagent exceeded max_iters={}", max_iters));
        }
    }
}
