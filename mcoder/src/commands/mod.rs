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
}

/// 内置元命令名
pub const META_COMMANDS: &[&str] = &[
    "help", "mode", "model", "sessions", "undo", "diff", "compact", "cancel",
    "task", "config", "pair", "exit", "quit", "workflow",
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
        "workflow" => {
            let action = args.first().copied().unwrap_or("list");
            let valid = ["init", "propose", "plan", "apply", "review", "archive", "continue", "list"];
            if !valid.contains(&action) {
                anyhow::bail!(
                    "usage: /workflow <init|propose|plan|apply|review|archive|continue|list> [change_id]"
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
/// - `bp map` -> `graph_query` / `graph_file_symbols`
/// - `bp commit` -> `bash git add ... && git commit`
///
/// `list` action 返回空字符串（由服务端直接查询并返回结果）。
fn workflow_prompt(action: &str, change_id: Option<&str>) -> String {
    let name = change_id.unwrap_or("<name>");
    match action {
        "init" => r#"You are the orchestrator. Initialize the workflow project structure.

### Steps
1. Create directory structure: .mcoder/workflow/{specs,changes,conventions}
2. Write .mcoder/workflow/config.yaml with profile and tech stack
3. Write .mcoder/workflow/conventions/coding.md with coding conventions
4. Suggest: /workflow propose <change-name>
"#.to_string(),

        "propose" => format!(r#"You are the orchestrator. Create a change proposal.

### Input
- Change name: {name} (kebab-case)

### Steps
1. Risk assessment: Trivial/Light/Standard/Critical
   - Auto-assess based on the user's described scope.
   - If Trivial or Light: skip Step 2 (grill), go directly to Step 3 with a minimal proposal.
   - If Standard or Critical: continue to Step 2.
2. Grill the user on requirements (skip if Trivial/Light) (RELENTLESS - do NOT skip)
   - Ask ONE question at a time. Wait for the answer. Do not batch.
   - Always provide a recommended answer when one exists.
   - Walk every branch: Problem, Scope, Deliverables, Approach, Edge cases, Dependencies, Constraints.
   - Do NOT proceed until you can describe every deliverable without guessing.
3. Technical research (skip if Trivial/Light)
   - Read relevant source files referenced in discussion.
   - Use graph_query / graph_file_symbols to analyze existing patterns and call-sites.
   - Use web_search for anything unresolved.
4. Create .mcoder/workflow/changes/{name}/proposal.md with:
   - ## Intent (problem, why now)
   - ## Scope (In/Out)
   - ## Deliverables (PR-N with behavior, rationale, acceptance)
   - ## Approach
5. Commit: bash git add .mcoder/workflow/changes/{name}/ && git commit -m "docs(proposal): {name}"
6. Suggest: /workflow plan {name}
"#),

        "plan" => format!(r#"You are the orchestrator - dispatch sub-agents; do not do their work yourself.

### Input
- Change name: {name}
- Change directory: .mcoder/workflow/changes/{name}/

### Prerequisites
- proposal.md exists in change directory and is not a template

### Steps
1. Resolve change name and paths
   - If change name is empty: list .mcoder/workflow/changes/ for active changes (exclude archive/).
2. Classify change (lightweight vs full)
   - Lightweight: ALL deliverables are config/docs/refactor/scaffolding (no new behavior)
   - Full: any deliverable introduces new behavior
3. If FULL: dispatch planner sub-agent via subagent tool (op=spawn, role=planner)
   - Planner reads: proposal.md, .mcoder/workflow/specs/<domain>/spec.md, .mcoder/workflow/conventions/coding.md
   - Planner queries codebase via graph_query / graph_file_symbols for module structure and dependencies
   - Planner performs impact analysis and writes ## Impact Analysis section in design.md
   - Planner produces: design.md (DS-N), tasks.md (T-N), delta specs (specs/<domain>/spec.md), context.jsonl
   If LIGHTWEIGHT: fill design.md and tasks.md (1 wave) directly, no delta specs needed.
4. Review planner output across 5 dimensions:
   - Implementability (can executor build it without guessing?)
   - Design Correctness (architecture internally consistent?)
   - Decision Completeness (all real technical choices recorded?)
   - Impact Completeness (all downstream effects found?)
   - File Manifest Consistency (every file traces to a component?)
   If ANY fails: re-dispatch planner with structured feedback. Repeat until all pass.
5. Verify traceability: PR->DS->T->spec (no orphans)
   - Every PR-N in proposal.md referenced by at least one DS-N in design.md
   - Every DS-N referenced by at least one T-N in tasks.md
   - Every type:behavior task has spec_ref pointing to delta spec
6. Commit: bash git add .mcoder/workflow/changes/{name}/ && git commit -m "docs(plan): design + tasks + delta specs"
7. Suggest: /workflow apply {name}
"#),

        "apply" => format!(r#"You are the orchestrator - dispatch sub-agents; do not do their work yourself.

### Input
- Change name: {name}
- Change directory: .mcoder/workflow/changes/{name}/

### Prerequisites
- design.md exists and is not a template
- tasks.md exists, has at least 1 wave, checkboxes are unchecked (normal mode)
- Delta specs exist for each affected domain

### Steps
1. Resolve change name and paths
   - If change name is empty: use the most recently planned change.
2. Classify change (lightweight vs full)
   - Lightweight: ALL tasks are type:config|docs|refactor|scaffolding (no type:behavior)
   - Full: any type:behavior task
3. Wave analysis (Full mode)
   - Parse tasks.md into waves (## Wave N sections), keep wave order.
   - Build inter-wave dependency graph from depends_on fields.
   - File manifest overlap check: waves modifying the SAME file cannot run concurrently.
4. For each wave: dispatch executor sub-agent via subagent tool (op=spawn, role=executor)
   - Executor reads: tasks.md, design.md, delta specs, .mcoder/workflow/conventions/coding.md
   - Executor implements TDD: RED (failing test) -> GREEN (minimal impl) -> REFACTOR
   - Executor commits each task atomically
   - Do NOT inject file contents into the dispatch prompt - executor has read access.
   If LIGHTWEIGHT: implement tasks yourself one by one, commit each, mark [x] with commit hash.
5. After each wave: verify git log, git diff, tests pass
   - Check git log --oneline for new commits
   - Check tasks.md: tasks marked [x] with commit hash annotation
   - Run wave's tests: confirm pass
   - If any task missing commit: re-dispatch with specific feedback.
6. After all waves: run full build + test suite
7. Suggest: /workflow review {name}
"#),

        "review" => format!(r#"You are the orchestrator - dispatch sub-agents; do not do their work yourself.

### Input
- Change name: {name}
- Change directory: .mcoder/workflow/changes/{name}/

### Prerequisites
- Code is implemented (tasks.md has [x] entries with commit hashes)
- Build check and test suite pass

### Steps
1. Resolve change name and paths
   - If change name is empty: use the most recently applied change.
2. Pre-review: run build + test suite (must pass before review)
   - If build or tests fail: do NOT dispatch reviewer. Suggest /workflow apply --fix {name}.
3. Classify change (lightweight vs full)
   - Lightweight (all non-behavior tasks, no delta specs): orchestrator does a quick review directly.
   - Full (any behavior task, has delta specs): dispatch reviewer sub-agent.
4. If FULL: dispatch reviewer sub-agent via subagent tool (op=spawn, role=reviewer)
   - Reviewer reads: proposal.md, design.md, tasks.md, delta specs, .mcoder/workflow/specs/<domain>/spec.md, source code
   - Reviewer performs triple review: spec compliance + code quality + goal achievement
   - Reviewer writes review.md with issues (R-N/Q-N/G-N/D-N) and verdict
5. Read review.md and route:
   - PASS (zero issues) -> suggest /workflow archive {name}
   - FAIL (D-issues) -> suggest /workflow plan --fix {name}
   - NEEDS_REVISION (R/Q/G issues) -> suggest /workflow apply --fix {name}
"#),

        "archive" => format!(r#"You are the orchestrator. Archive a completed change.

### Input
- Change name: {name}
- Change directory: .mcoder/workflow/changes/{name}/

### Prerequisites
- review.md exists and Overall Verdict is PASS
- No unresolved issues in review.md ## Issues section

### Steps
1. Verify review verdict is PASS
   - If not PASS: suggest /workflow apply --fix {name}
2. Merge delta specs into global specs:
   - For each domain in changes/{name}/specs/: merge ADDED/MODIFIED/REMOVED into .mcoder/workflow/specs/<domain>/spec.md
   - If merge conflict: resolve in the delta spec and re-merge.
3. Move change directory to archive: .mcoder/workflow/changes/archive/<date>-{name}/
4. Update .mcoder/workflow/roadmap.md if proposal has ## Roadmap Reference
5. Commit: bash git add .mcoder/workflow/ && git commit -m "archive: {name} - specs merged"
6. Suggest: /workflow continue
"#),

        "continue" => r#"You are the orchestrator. Auto-detect current progress and suggest next step.

### Steps
1. List active changes in .mcoder/workflow/changes/ (exclude archive/)
   - If multiple exist, ask the user which one.
2. For each change, check artifact existence:
   - No proposal.md -> suggest /workflow propose <name>
   - proposal.md exists, no design.md -> suggest /workflow plan <name>
   - design.md exists, tasks not all [x] -> suggest /workflow apply <name>
   - All tasks [x], no review.md -> suggest /workflow review <name>
   - review.md PASS -> suggest /workflow archive <name>
3. If no active changes -> suggest /workflow propose <new-name>
"#.to_string(),

        // list: server-side execution, no prompt needed
        "list" => String::new(),

        _ => String::new(),
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
