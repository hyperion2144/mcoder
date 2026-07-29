// 设计文档 §3.4: with_system_prompt/with_allowed_tools 为 forward-looking builder API
#![allow(dead_code)]

use crate::types::{ModelConfig, RoleConfig};
use std::collections::HashMap;

/// 设计文档 §3.4: Role 系统
/// 把 mode/subagent/type 统一为 role 概念
#[derive(Debug, Clone)]
pub struct Role {
    pub name: String,
    pub system_prompt: String,
    pub model: Option<ModelConfig>,
    pub allowed_tools: Vec<String>, // 空 = 全部工具
    pub max_tokens: Option<u32>,    // None = 无限
    pub max_iters: Option<u32>,
    pub timeout: Option<u32>,
    pub loop_condition: Option<String>,
}

impl Role {
    pub fn from_config(name: &str, config: &RoleConfig, models: &HashMap<String, ModelConfig>) -> Self {
        Self {
            name: name.to_string(),
            system_prompt: String::new(),
            model: config.model.as_ref().and_then(|m| models.get(m).cloned()),
            allowed_tools: Vec::new(),
            max_tokens: config.max_tokens,
            max_iters: config.max_iters,
            timeout: config.timeout,
            loop_condition: config.loop_condition.clone(),
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = tools;
        self
    }

    /// 检查工具是否允许
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        if self.allowed_tools.is_empty() {
            return true; // 空 = 全部允许
        }
        self.allowed_tools.iter().any(|t| t == tool_name)
    }
}

/// Role 注册表
/// 管理所有可用 role，支持运行时切换
pub struct RoleRegistry {
    roles: HashMap<String, Role>,
}

impl Default for RoleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RoleRegistry {
    pub fn new() -> Self {
        let mut reg = Self { roles: HashMap::new() };
        reg.register_builtins();
        reg
    }

    /// 注册内置 role（设计文档 §3.4 表格）
    fn register_builtins(&mut self) {
        // default: 普通对话，全部工具
        self.register(Role {
            name: "default".into(),
            system_prompt: "You are mcoder, a self-hosted coding agent. Help the user with coding tasks using available tools.".into(),
            model: None,
            allowed_tools: vec![], // 空 = 全部
            max_tokens: None,
            max_iters: Some(50),
            timeout: None,
            loop_condition: None,
        });

        // plan: 规划阶段，只能读和创建 plan
        self.register(Role {
            name: "plan".into(),
            system_prompt: "You are in PLAN mode. Read code, explore the codebase via graph, and create a structured plan with plan_create. Do NOT modify files. After plan_create, the user will approve/reject.".into(),
            model: None,
            allowed_tools: vec![
                "read".into(), "read_more".into(), "read_full".into(), "read_original".into(),
                "ls".into(), "grep".into(),
                "graph_query".into(), "graph_file_symbols".into(), "graph_index".into(),
                "plan_create".into(), "plan_update".into(),
                "memory_search".into(), "memory_list".into(),
            ],
            max_tokens: None,
            max_iters: Some(5),
            timeout: None,
            loop_condition: Some("plan_created".into()),
        });

        // execute: plan 执行阶段，全部工具
        self.register(Role {
            name: "execute".into(),
            system_prompt: "You are in EXECUTE mode. Execute the approved plan step by step. Use plan_update to track progress. Use todo for sub-tasks.".into(),
            model: None,
            allowed_tools: vec![], // 全部
            max_tokens: None,
            max_iters: Some(50),
            timeout: None,
            loop_condition: Some("plan_all_done".into()),
        });

        // review: 代码审查阶段，只读
        // 设计文档 §3.4: review 不允许任何修改类工具（含 bash，因 bash 可写文件）
        // 如需运行只读 shell 命令（git log/diff），应切换到 default role
        self.register(Role {
            name: "review".into(),
            system_prompt: "You are in REVIEW mode. Read code and review changes using read/grep/graph tools. Do NOT modify files or run shell commands. Switch to default mode if you need to run read-only shell commands like git log.".into(),
            model: None,
            allowed_tools: vec![
                "read".into(), "read_more".into(), "read_full".into(), "read_original".into(),
                "ls".into(), "grep".into(),
                "graph_query".into(), "graph_file_symbols".into(), "graph_index".into(),
                "memory_search".into(), "memory_list".into(),
            ],
            max_tokens: None,
            max_iters: Some(10),
            timeout: None,
            loop_condition: None,
        });

        // goal: goal 模式，全部工具 + todo
        self.register(Role {
            name: "goal".into(),
            system_prompt: "You are in GOAL mode. Pursue the user's goal autonomously. Use todo to track tasks. Loop until goal is achieved or blocked.".into(),
            model: None,
            allowed_tools: vec![], // 全部
            max_tokens: None,
            max_iters: Some(100),
            timeout: None,
            loop_condition: Some("goal_achieved".into()),
        });

        // loop: loop 模式，全部工具
        // 设计文档 §3.4: max_iters=0 表示无限循环，由 timeout 控制
        // agent/mod.rs::max_iters_for_current_role 将 Some(0) 映射为 u32::MAX
        self.register(Role {
            name: "loop".into(),
            system_prompt: "You are in LOOP mode. Continuously work on the task. Use todo to track progress. Loop until explicitly stopped or task complete.".into(),
            model: None,
            allowed_tools: vec![], // 全部
            max_tokens: None,
            max_iters: Some(0), // 0 = 无限（由 max_iters_for_current_role 映射为 u32::MAX，实际由 timeout=3600s 控制）
            timeout: Some(3600),
            loop_condition: Some("task_complete".into()),
        });

        // subagent: 通用子代理
        self.register(Role {
            name: "subagent".into(),
            system_prompt: "You are a sub-agent. Complete the assigned task using available tools. Call subagent op=done with your final result when finished.".into(),
            model: None,
            allowed_tools: vec![], // 调用时配置
            max_tokens: None,
            max_iters: Some(50),
            timeout: Some(600),
            loop_condition: None,
        });

        // 设计文档 §8.5: 3 个内置子代理角色（workflow 专用）
        // planner: 规划阶段子代理，只读 + 创建 spec/plan
        self.register(Role {
            name: "planner".into(),
            system_prompt: crate::workflow::prompts::PLANNER_PROMPT.into(),
            model: None,
            allowed_tools: vec![
                "read".into(), "read_more".into(), "read_full".into(), "read_original".into(),
                "ls".into(), "grep".into(),
                "graph_query".into(), "graph_file_symbols".into(), "graph_index".into(),
                "workflow_create".into(), "workflow_query".into(), "workflow_update".into(),
                "plan_create".into(), "plan_update".into(),
                "memory_search".into(), "memory_list".into(),
                "subagent".into(), // 用于 op=done
            ],
            max_tokens: None,
            max_iters: Some(20),
            timeout: Some(300),
            loop_condition: None,
        });

        // executor: 执行阶段子代理，可读写 + 运行代码
        self.register(Role {
            name: "executor".into(),
            system_prompt: crate::workflow::prompts::EXECUTOR_PROMPT.into(),
            model: None,
            allowed_tools: vec![], // 全部工具
            max_tokens: None,
            max_iters: Some(100),
            timeout: Some(1800),
            loop_condition: None,
        });

        // reviewer: 审查阶段子代理，只读 + 生成 review artifact
        self.register(Role {
            name: "reviewer".into(),
            system_prompt: crate::workflow::prompts::REVIEWER_PROMPT.into(),
            model: None,
            allowed_tools: vec![
                "read".into(), "read_more".into(), "read_full".into(), "read_original".into(),
                "ls".into(), "grep".into(),
                "graph_query".into(), "graph_file_symbols".into(), "graph_index".into(),
                "workflow_create".into(), "workflow_query".into(), "workflow_update".into(),
                "memory_search".into(), "memory_list".into(),
                "subagent".into(),
            ],
            max_tokens: None,
            max_iters: Some(20),
            timeout: Some(300),
            loop_condition: None,
        });
    }

    pub fn register(&mut self, role: Role) {
        self.roles.insert(role.name.clone(), role);
    }

    pub fn get(&self, name: &str) -> Option<&Role> {
        self.roles.get(name)
    }

    pub fn list(&self) -> Vec<&Role> {
        self.roles.values().collect()
    }

    /// 从配置加载 role（覆盖内置 role 的参数）
    pub fn merge_config(&mut self, configs: &HashMap<String, RoleConfig>, models: &HashMap<String, ModelConfig>) {
        for (name, cfg) in configs {
            let role = Role::from_config(name, cfg, models);
            // 保留内置 system_prompt 和 allowed_tools，只覆盖参数
            if let Some(existing) = self.roles.get_mut(name) {
                existing.model = role.model;
                existing.max_tokens = role.max_tokens;
                existing.max_iters = role.max_iters;
                existing.timeout = role.timeout;
                existing.loop_condition = role.loop_condition;
            } else {
                // 新 role，用默认值
                self.register(Role {
                    name: role.name,
                    system_prompt: role.system_prompt,
                    model: role.model,
                    allowed_tools: vec![],
                    max_tokens: role.max_tokens,
                    max_iters: role.max_iters,
                    timeout: role.timeout,
                    loop_condition: role.loop_condition,
                });
            }
        }
    }
}

/// 内置 role 名列表（设计文档 §3.4 + §8.5）
/// 包含 7 个核心 role + 3 个 workflow 子代理 role
pub fn builtin_role_names() -> Vec<&'static str> {
    vec!["default", "plan", "execute", "review", "goal", "loop", "subagent", "planner", "executor", "reviewer"]
}
