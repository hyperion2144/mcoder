// Slash Command 系统
//
// Slash command 是 skill 的简化形式：
//   - 单个 .md 文件（无支持文件夹）
//   - 用户通过 /name 显式调用
//   - 内容是提示词模板（YAML frontmatter + Markdown 正文）
//
// 与 skill 的关系：
//   - skill 可以被自动检索 + 手动调用
//   - command 只能手动调用
//   - skill 文件夹包含 command 的能力并扩展
//   - 用户输入 /xxx 时，先查 command，再查 skill
//
// 文件位置：
//   全局: ~/.mcoder/commands/<name>.md
//   项目: .mcoder/commands/<name>.md
//
// 内置元命令（/help /mode /sessions 等）由服务端处理，不通过文件加载

use crate::skills::SkillRegistry;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::sync::RwLock;

pub mod workflow_prompts;

/// Slash command 定义（从 .md 文件加载）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandDef {
    pub name: String,
    pub description: String,
    /// 命令正文（提示词模板）
    pub content: String,
    /// 参数提示（用于 /help 显示）
    #[serde(default)]
    pub argument_hint: Option<String>,
}

/// 元命令类型（内置，不走文件加载）
/// 这些命令由服务端直接处理，返回结构化结果而非提示词
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MetaCommandResult {
    /// 切换 role
    Mode { role: String },
    /// 列出/切换模型
    Model { action: String, model: Option<String> },
    /// 会话管理
    Sessions { action: String, session_id: Option<String> },
    /// 撤销
    Undo { id: Option<String> },
    /// 查看差异
    Diff,
    /// 压缩上下文
    Compact,
    /// 取消当前 agent loop
    Cancel,
    /// 任务管理
    Task { action: String, task_id: Option<String> },
    /// 配置管理
    Config { key: String, value: Option<String> },
    /// 配对
    Pair,
    /// 退出
    Exit,
    /// 工作流管理
    ///
    /// `prompt` 是基于 blueprint workflow 模板生成的完整编排步骤提示词，
    /// 注入到 agent loop 中让 LLM 执行。`list` action 的 prompt 为空字符串
    /// （由服务端直接查询并返回结果）。
    Workflow {
        action: String,
        change_id: Option<String>,
        args: Vec<String>,
        prompt: String,
    },
    /// 帮助
    Help,
    /// 消息树视图（分叉/切换）
    Tree,
}

/// 内置元命令名
pub const META_COMMANDS: &[&str] = &[
    "help", "mode", "model", "sessions", "undo", "diff", "compact", "cancel",
    "task", "config", "pair", "exit", "quit", "workflow", "tree",
];

/// 判断是否为元命令
pub fn is_meta_command(name: &str) -> bool {
    META_COMMANDS.contains(&name)
}

/// 解析元命令，返回结构化结果
pub fn parse_meta_command(name: &str, args: &[&str]) -> Result<MetaCommandResult> {
    match name {
        "help" => Ok(MetaCommandResult::Help),
        "exit" | "quit" => Ok(MetaCommandResult::Exit),
        "mode" => {
            let role = args
                .first()
                .ok_or_else(|| anyhow::anyhow!("usage: /mode <role>"))?;
            Ok(MetaCommandResult::Mode {
                role: role.to_string(),
            })
        }
        "model" => {
            let action = args.first().copied().unwrap_or("list");
            match action {
                "list" => Ok(MetaCommandResult::Model {
                    action: "list".into(),
                    model: None,
                }),
                "set" => {
                    let model = args
                        .get(1)
                        .ok_or_else(|| anyhow::anyhow!("usage: /model set <name>"))?;
                    Ok(MetaCommandResult::Model {
                        action: "set".into(),
                        model: Some(model.to_string()),
                    })
                }
                _ => anyhow::bail!("usage: /model <list|set <name>>"),
            }
        }
        "sessions" => {
            let action = args.first().copied().unwrap_or("list");
            match action {
                "list" | "new" => Ok(MetaCommandResult::Sessions {
                    action: action.to_string(),
                    session_id: None,
                }),
                "open" | "delete" => {
                    let id = args
                        .get(1)
                        .ok_or_else(|| anyhow::anyhow!("usage: /sessions {} <id>", action))?;
                    Ok(MetaCommandResult::Sessions {
                        action: action.to_string(),
                        session_id: Some(id.to_string()),
                    })
                }
                _ => anyhow::bail!("usage: /sessions <list|new|open <id>|delete <id>>"),
            }
        }
        "undo" => Ok(MetaCommandResult::Undo {
            id: args.first().map(|s| s.to_string()),
        }),
        "diff" => Ok(MetaCommandResult::Diff),
        "compact" => Ok(MetaCommandResult::Compact),
        "cancel" => Ok(MetaCommandResult::Cancel),
        "task" => {
            let action = args.first().copied().unwrap_or("list");
            match action {
                "list" => Ok(MetaCommandResult::Task {
                    action: "list".into(),
                    task_id: None,
                }),
                "cancel" => {
                    let id = args
                        .get(1)
                        .ok_or_else(|| anyhow::anyhow!("usage: /task cancel <id>"))?;
                    Ok(MetaCommandResult::Task {
                        action: "cancel".into(),
                        task_id: Some(id.to_string()),
                    })
                }
                _ => anyhow::bail!("usage: /task <list|cancel <id>>"),
            }
        }
        "config" => {
            let key = args
                .first()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("usage: /config <get|set> <key> [value]"))?;
            match key {
                "get" => {
                    let k = args
                        .get(1)
                        .ok_or_else(|| anyhow::anyhow!("usage: /config get <key>"))?;
                    Ok(MetaCommandResult::Config {
                        key: k.to_string(),
                        value: None,
                    })
                }
                "set" => {
                    let k = args
                        .get(1)
                        .ok_or_else(|| anyhow::anyhow!("usage: /config set <key> <value>"))?;
                    let v = args.get(2).map(|s| s.to_string());
                    Ok(MetaCommandResult::Config {
                        key: k.to_string(),
                        value: v,
                    })
                }
                _ => anyhow::bail!("usage: /config <get|set> <key> [value]"),
            }
        }
        "pair" => Ok(MetaCommandResult::Pair),
        "tree" => Ok(MetaCommandResult::Tree),
        "workflow" => {
            let action = args.first().copied().unwrap_or("list");
            let valid = ["init", "propose", "plan", "apply", "review", "archive", "continue", "loop", "list"];
            if !valid.contains(&action) {
                anyhow::bail!(
                    "usage: /workflow <init|propose|plan|apply|review|archive|continue|loop|list> [change_id]"
                );
            }
            let workflow_args = args.get(1..).unwrap_or(&[]);
            let change_id_index = workflow_args.iter().position(|arg| *arg != "--fix");
            let change_id = change_id_index.map(|index| workflow_args[index].to_string());
            let rest: Vec<String> = workflow_args
                .iter()
                .enumerate()
                .filter(|(index, _)| Some(*index) != change_id_index)
                .map(|(_, arg)| arg.to_string())
                .collect();
            let prompt = workflow_prompt(action, change_id.as_deref());
            Ok(MetaCommandResult::Workflow {
                action: action.to_string(),
                change_id,
                args: rest,
                prompt,
            })
        }
        _ => anyhow::bail!("unknown meta command: {}", name),
    }
}

/// 为 workflow action 生成完整的编排步骤提示词。
///
/// 提示词基于 blueprint（specworkflow）的 workflow 模板，适配 mcoder 的工具名和路径：
/// - `bp/changes/<name>/` -> `.mcoder/workflow/changes/<name>/`
/// - `bp/specs/` -> `.mcoder/workflow/specs/`
/// - `bp map` -> `graph_search` / `graph_file_symbols`
/// - `bp commit` -> `bash git add ... && git commit`
///
/// `list` action 返回空字符串（由服务端直接查询并返回结果）。
fn workflow_prompt(action: &str, change_id: Option<&str>) -> String {
    match action {
        "init" => crate::commands::workflow_prompts::init_prompt(),
        "propose" => crate::commands::workflow_prompts::propose_prompt(change_id.unwrap_or("")),
        "plan" => crate::commands::workflow_prompts::plan_prompt(change_id.unwrap_or(""), false),
        "apply" => crate::commands::workflow_prompts::apply_prompt(change_id.unwrap_or(""), false),
        "review" => crate::commands::workflow_prompts::review_prompt(change_id.unwrap_or(""), false),
        "archive" => crate::commands::workflow_prompts::archive_prompt(change_id.unwrap_or("")),
        "continue" => crate::commands::workflow_prompts::continue_prompt(),
        "loop" => crate::commands::workflow_prompts::loop_prompt(),
        "list" => "Use workflow(action=list) to list all changes.".to_string(),
        other => format!("unknown workflow action: {}", other),
    }
}

/// Slash command 注册表（自定义命令，从文件加载）
pub struct CommandRegistry {
    commands: RwLock<HashMap<String, CommandDef>>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: RwLock::new(HashMap::new()),
        }
    }

    /// 从目录加载所有 .md 命令文件
    pub async fn load_from_dir(&self, dir: &Path) -> Result<usize> {
        if !dir.exists() {
            return Ok(0);
        }
        let mut count = 0;
        let mut rd = match tokio::fs::read_dir(dir).await {
            Ok(rd) => rd,
            Err(_) => return Ok(0),
        };
        let mut entries = Vec::new();
        while let Some(entry) = rd.next_entry().await? {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "md" {
                    entries.push(path);
                }
            }
        }
        for path in entries {
            match Self::parse_command_file(&path).await {
                Ok(cmd) => {
                    tracing::info!("loaded command '{}' from {}", cmd.name, path.display());
                    self.commands.write().await.insert(cmd.name.clone(), cmd);
                    count += 1;
                }
                Err(e) => {
                    tracing::warn!("failed to load command from {}: {}", path.display(), e);
                }
            }
        }
        Ok(count)
    }

    async fn parse_command_file(path: &Path) -> Result<CommandDef> {
        let content = tokio::fs::read_to_string(path).await?;

        // 解析 frontmatter
        let (fm_str, body) = parse_command_frontmatter(&content)
            .with_context(|| format!("parsing frontmatter of {}", path.display()))?;

        let mut name = String::new();
        let mut description = String::new();
        let mut argument_hint = None;

        if !fm_str.is_empty() {
            let fm: serde_yaml::Value = serde_yaml::from_str(fm_str)?;
            if let Some(m) = fm.get("name").and_then(|v| v.as_str()) {
                name = m.to_string();
            }
            if let Some(d) = fm.get("description").and_then(|v| v.as_str()) {
                description = d.to_string();
            }
            if let Some(a) = fm.get("argument-hint").and_then(|v| v.as_str()) {
                argument_hint = Some(a.to_string());
            }
        }

        // 如果 frontmatter 没有 name，从文件名推导
        if name.is_empty() {
            name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
        }

        Ok(CommandDef {
            name,
            description,
            content: body.trim().to_string(),
            argument_hint,
        })
    }

    /// 获取命令
    pub async fn get(&self, name: &str) -> Option<CommandDef> {
        self.commands.read().await.get(name).cloned()
    }

    /// 列出所有自定义命令
    pub async fn list(&self) -> Vec<CommandDef> {
        self.commands.read().await.values().cloned().collect()
    }

    /// 渲染命令（变量替换）
    pub fn render(&self, cmd: &CommandDef, args: &str) -> String {
        let mut result = cmd.content.clone();
        result = result.replace("$ARGUMENTS", args);

        // $ARGUMENTS[N] 和 $N
        let parts: Vec<&str> = args.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            result = result.replace(&format!("$ARGUMENTS[{}]", i), part);
            result = result.replace(&format!("${}", i), part);
        }

        result
    }
}

/// 解析命令文件的 frontmatter
fn parse_command_frontmatter(content: &str) -> Result<(&str, &str)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Ok(("", content));
    }
    let rest = &trimmed[3..];
    let end = rest
        .find("\n---")
        .ok_or_else(|| anyhow::anyhow!("missing closing '---' in frontmatter"))?;
    Ok((&rest[..end], &rest[end + 4..]))
}

/// 统一的命令处理器：解析 /xxx 输入，决定走元命令、自定义命令还是 skill
pub struct CommandDispatcher {
    pub commands: std::sync::Arc<CommandRegistry>,
    pub skills: std::sync::Arc<SkillRegistry>,
}

/// 命令分发结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum DispatchResult {
    /// 元命令——返回结构化指令，由调用方执行对应 RPC
    Meta { result: MetaCommandResult },
    /// 自定义命令——返回展开后的提示词，注入对话
    CustomCommand { name: String, prompt: String },
    /// Skill——返回展开后的提示词，注入对话
    Skill { name: String, prompt: String },
    /// 未知命令
    Unknown { name: String },
}

impl CommandDispatcher {
    pub fn new(
        commands: std::sync::Arc<CommandRegistry>,
        skills: std::sync::Arc<SkillRegistry>,
    ) -> Self {
        Self { commands, skills }
    }

    /// 分发 /xxx 输入
    /// 输入格式："/name arg1 arg2 ..."（不含前导 /）
    pub async fn dispatch(&self, input: &str) -> Result<DispatchResult> {
        let input = input.trim();
        let parts: Vec<&str> = input.splitn(2, char::is_whitespace).collect();
        let name = parts.first().unwrap_or(&"");
        let args = parts.get(1).unwrap_or(&"");

        // 1. 元命令优先
        if is_meta_command(name) {
            let result = parse_meta_command(name, &args.split_whitespace().collect::<Vec<_>>())?;
            return Ok(DispatchResult::Meta { result });
        }

        // 2. 自定义命令
        if let Some(cmd) = self.commands.get(name).await {
            let prompt = self.commands.render(&cmd, args);
            return Ok(DispatchResult::CustomCommand {
                name: cmd.name,
                prompt,
            });
        }

        // 3. Skill（用户通过 /skill-name 调用）
        if let Some(skill) = self.skills.get(name).await {
            if skill.frontmatter.user_invocable {
                let prompt = self.skills.activate(name, args).await?;
                return Ok(DispatchResult::Skill {
                    name: name.to_string(),
                    prompt,
                });
            }
        }

        // 4. 未知命令
        Ok(DispatchResult::Unknown {
            name: name.to_string(),
        })
    }

    /// 列出所有可调用的命令（元命令 + 自定义命令 + user-invocable skills）
    pub async fn list_all(&self) -> Vec<serde_json::Value> {
        let mut result = Vec::new();

        // 元命令
        for &name in META_COMMANDS {
            result.push(serde_json::json!({
                "name": name,
                "type": "meta",
                "description": meta_command_description(name),
            }));
        }

        // 自定义命令
        for cmd in self.commands.list().await {
            result.push(serde_json::json!({
                "name": cmd.name,
                "type": "command",
                "description": cmd.description,
                "argument_hint": cmd.argument_hint,
            }));
        }

        // user-invocable skills
        for skill in self.skills.list().await {
            if skill.user_invocable {
                result.push(serde_json::json!({
                    "name": skill.name,
                    "type": "skill",
                    "description": skill.description,
                }));
            }
        }

        result
    }
}

fn meta_command_description(name: &str) -> &'static str {
    match name {
        "help" => "show available commands",
        "mode" => "switch role (normal|plan|goal|loop|execute|review)",
        "model" => "model management (list|set)",
        "sessions" => "session management (list|new|open|delete)",
        "undo" => "undo file changes",
        "diff" => "view git diff",
        "compact" => "compact context",
        "cancel" => "cancel current agent loop",
        "task" => "background task management",
        "config" => "config management (get|set)",
        "pair" => "show pairing info",
        "exit" | "quit" => "exit",
        "workflow" => "spec-driven workflow orchestration (init|propose|plan|apply|review|archive|continue|list) - returns orchestration prompt injected into the agent loop",
        "tree" => "view message tree (fork/switch branches)",
        _ => "",
    }
}

/// 构建命令注册表并加载自定义命令
pub async fn build_registry(
    global_commands_dir: &Path,
    project_commands_dir: &Path,
) -> Result<std::sync::Arc<CommandRegistry>> {
    let registry = std::sync::Arc::new(CommandRegistry::new());
    registry.load_from_dir(global_commands_dir).await?;
    registry.load_from_dir(project_commands_dir).await?;
    Ok(registry)
}
