// Agent Skills 开放标准实现（agentskills.io）
//
// Skill = 能力扩展包（文件夹 + SKILL.md + 可选支持文件）
//
// 文件结构：
//   ~/.mcoder/skills/<skill-name>/
//   ├── SKILL.md          # 必需：YAML frontmatter + Markdown 正文
//   ├── reference.md      # 可选：详细参考（按需加载）
//   ├── templates/        # 可选：输出模板
//   ├── scripts/          # 可选：可执行脚本
//   └── assets/           # 可选：素材资源
//
// 渐进式披露（Progressive Disclosure）：
//   Level 1 (Discovery): 会话启动时只加载 name + description 进 system prompt
//   Level 2 (Activation): 用户请求与 description 语义匹配时加载完整正文
//   Level 3 (Execution): 执行时按需读取 scripts/references 等子文件
//
// 触发方式：
//   - 自动检索：agent 扫描 description 做语义匹配（disable-model-invocation: false 时）
//   - 显式调用：用户输入 /skill-name（user-invocable: true 时）

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;

/// Skill 的 YAML frontmatter 元数据
/// Level 1 (Discovery) 只加载这些字段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// skill 名称（小写字母、数字、连字符，≤64 字符）
    pub name: String,
    /// 何用 + 何时用（≤1024 字符）——语义匹配的关键
    pub description: String,
    /// 触发短语/示例请求（可选，增强匹配）
    #[serde(default)]
    pub when_to_use: Option<String>,
    /// true = 禁止模型自动加载，只能用户手动调用
    #[serde(default)]
    pub disable_model_invocation: bool,
    /// false = 禁止用户手动调用，只让模型自动用
    #[serde(default = "default_user_invocable")]
    pub user_invocable: bool,
    /// skill 激活时推荐使用的工具列表
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// 指定运行时模型
    #[serde(default)]
    pub model: Option<String>,
    /// 推理强度：low/medium/high/xhigh/max
    #[serde(default)]
    pub effort: Option<String>,
    /// fork = 在隔离子代理中运行
    #[serde(default)]
    pub context: Option<String>,
    /// 限制激活条件的 glob 模式（文件路径匹配）
    #[serde(default)]
    pub paths: Vec<String>,
}

fn default_user_invocable() -> bool {
    true
}

/// 完整的 Skill 定义（Level 2 激活后加载）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub frontmatter: SkillFrontmatter,
    /// SKILL.md 的 Markdown 正文（去除 frontmatter 后的内容）
    pub body: String,
    /// skill 所在目录（用于执行时读取子文件）
    pub dir: PathBuf,
}

/// Level 1 Discovery 条目（轻量，用于注入 system prompt）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDiscovery {
    pub name: String,
    pub description: String,
    pub user_invocable: bool,
    pub disable_model_invocation: bool,
}

/// Skill 注册表
/// 管理所有已发现的 skill，支持渐进式披露
pub struct SkillRegistry {
    /// 所有已发现的 skill frontmatter（Level 1，常驻）
    skills: RwLock<HashMap<String, Skill>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: RwLock::new(HashMap::new()),
        }
    }

    /// 从目录加载所有 skill（扫描子目录，每个子目录是一个 skill）
    /// 路径结构：<dir>/<skill-name>/SKILL.md
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
            if path.is_dir() {
                let skill_md = path.join("SKILL.md");
                if skill_md.exists() {
                    entries.push((path, skill_md));
                }
            }
        }
        for (skill_dir, skill_md_path) in entries {
            match Self::parse_skill_file(&skill_md_path, &skill_dir).await {
                Ok(skill) => {
                    tracing::info!(
                        "loaded skill '{}' from {}",
                        skill.frontmatter.name,
                        skill_dir.display()
                    );
                    self.skills
                        .write()
                        .await
                        .insert(skill.frontmatter.name.clone(), skill);
                    count += 1;
                }
                Err(e) => {
                    tracing::warn!(
                        "failed to load skill from {}: {}",
                        skill_md_path.display(),
                        e
                    );
                }
            }
        }
        Ok(count)
    }

    /// 解析 SKILL.md 文件（YAML frontmatter + Markdown 正文）
    async fn parse_skill_file(skill_md_path: &Path, skill_dir: &Path) -> Result<Skill> {
        let content = tokio::fs::read_to_string(skill_md_path).await?;

        // 解析 frontmatter：--- 分隔的 YAML 块
        let (frontmatter_str, body) = parse_frontmatter(&content)
            .with_context(|| format!("parsing frontmatter of {}", skill_md_path.display()))?;

        let frontmatter: SkillFrontmatter = serde_yaml::from_str(frontmatter_str)
            .with_context(|| format!("parsing YAML frontmatter of {}", skill_md_path.display()))?;

        if frontmatter.name.is_empty() {
            anyhow::bail!("skill must have a name in frontmatter");
        }

        Ok(Skill {
            frontmatter,
            body: body.trim().to_string(),
            dir: skill_dir.to_path_buf(),
        })
    }

    /// Level 1: 返回所有 skill 的 Discovery 条目（用于注入 system prompt）
    pub async fn discover(&self) -> Vec<SkillDiscovery> {
        self.skills
            .read()
            .await
            .values()
            .map(|s| SkillDiscovery {
                name: s.frontmatter.name.clone(),
                description: s.frontmatter.description.clone(),
                user_invocable: s.frontmatter.user_invocable,
                disable_model_invocation: s.frontmatter.disable_model_invocation,
            })
            .collect()
    }

    /// Level 1: 生成 system prompt 注入文本（列出所有可用 skill 的 name + description）
    pub async fn discovery_prompt(&self) -> String {
        let skills = self.discover().await;
        if skills.is_empty() {
            return String::new();
        }
        let mut lines = vec![
            "# Available Skills".to_string(),
            "The following skills are available. Use the `skill_use` tool to activate one when the user's request matches its description, or the user can invoke it directly with `/skill-name`.".to_string(),
            String::new(),
        ];
        for s in &skills {
            let auto = if s.disable_model_invocation {
                " [manual only]"
            } else {
                ""
            };
            let manual = if s.user_invocable { "" } else { " [auto only]" };
            lines.push(format!(
                "- **{}**: {}{}{}",
                s.name, s.description, auto, manual
            ));
        }
        lines.join("\n")
    }

    /// Level 2: 获取完整 skill（激活时加载正文）
    pub async fn get(&self, name: &str) -> Option<Skill> {
        self.skills.read().await.get(name).cloned()
    }

    /// 列出所有 skill 元数据
    pub async fn list(&self) -> Vec<SkillFrontmatter> {
        self.skills
            .read()
            .await
            .values()
            .map(|s| s.frontmatter.clone())
            .collect()
    }

    /// Level 2 + 3: 激活 skill 并渲染（变量替换 + shell 命令注入）
    /// 返回展开后的完整提示词
    pub async fn activate(&self, name: &str, args: &str) -> Result<String> {
        let skill = self
            .get(name)
            .await
            .ok_or_else(|| anyhow::anyhow!("skill not found: {}", name))?;

        // 检查 user_invocable
        if !skill.frontmatter.user_invocable && !args.is_empty() {
            // 非用户调用时才允许（这里简化：args 非空视为用户调用）
            anyhow::bail!(
                "skill '{}' is not user-invocable (auto-only)",
                name
            );
        }

        let mut rendered = skill.body.clone();

        // 变量替换
        rendered = render_variables(&rendered, args, &skill.dir);

        // Shell 命令注入：!`command` → 执行结果
        rendered = inject_shell_commands(&rendered).await?;

        // 附加元数据提示
        let mut header = String::new();
        if !skill.frontmatter.allowed_tools.is_empty() {
            header.push_str(&format!(
                "\n\n[Recommended tools: {}]",
                skill.frontmatter.allowed_tools.join(", ")
            ));
        }
        if let Some(model) = &skill.frontmatter.model {
            header.push_str(&format!("\n[Model: {}]", model));
        }

        Ok(format!("# Skill: {}\n\n{}{}", skill.frontmatter.name, rendered, header))
    }

    /// 查找与用户请求语义匹配的 skill（自动检索）
    /// 简单实现：关键词匹配（未来可换为 embedding）
    pub async fn find_matching(&self, request: &str) -> Vec<String> {
        let request_lower = request.to_lowercase();
        let mut matches = Vec::new();
        for skill in self.skills.read().await.values() {
            if skill.frontmatter.disable_model_invocation {
                continue;
            }
            // 简单关键词匹配：description 中的词出现在 request 中
            let desc_lower = skill.frontmatter.description.to_lowercase();
            let name_lower = skill.frontmatter.name.to_lowercase();
            if request_lower.contains(&name_lower)
                || desc_lower
                    .split_whitespace()
                    .any(|word| word.len() > 3 && request_lower.contains(word))
            {
                matches.push(skill.frontmatter.name.clone());
            }
        }
        matches
    }
}

/// 解析 YAML frontmatter（--- 分隔）
/// 返回 (frontmatter_str, body_str)
fn parse_frontmatter(content: &str) -> Result<(&str, &str)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        // 无 frontmatter，整个内容作为 body，name 从目录名推导
        return Ok(("", content));
    }
    // 找第二个 ---
    let rest = &trimmed[3..]; // 跳过第一个 ---
    let end = rest
        .find("\n---")
        .ok_or_else(|| anyhow::anyhow!("missing closing '---' in frontmatter"))?;
    let frontmatter = &rest[..end];
    let body = &rest[end + 4..]; // 跳过 \n---
    Ok((frontmatter, body))
}

/// 变量替换
/// 支持的变量：
///   $ARGUMENTS - 全部参数
///   $ARGUMENTS[N] / $N - 第 N 个参数（0-indexed）
///   ${SKILL_DIR} - skill 目录绝对路径
fn render_variables(template: &str, args: &str, skill_dir: &Path) -> String {
    let arg_parts: Vec<&str> = args.split_whitespace().collect();
    let mut result = template.to_string();

    // $ARGUMENTS
    result = result.replace("$ARGUMENTS", args);

    // $ARGUMENTS[N] 和 $N
    for (i, part) in arg_parts.iter().enumerate() {
        result = result.replace(&format!("$ARGUMENTS[{}]", i), part);
        result = result.replace(&format!("${}", i), part);
    }

    // ${SKILL_DIR}
    result = result.replace("${SKILL_DIR}", &skill_dir.display().to_string());

    result
}

/// Shell 命令注入：!`command` → 执行结果
async fn inject_shell_commands(content: &str) -> Result<String> {
    // 匹配 !`command` 模式
    let mut result = String::new();
    let mut remaining = content;

    loop {
        if let Some(start) = remaining.find("!`") {
            result.push_str(&remaining[..start]);
            let after_start = &remaining[start + 2..];
            if let Some(end) = after_start.find('`') {
                let cmd = &after_start[..end];
                // 跨平台执行命令：Unix 用 sh -c，Windows 用 cmd /C
                let output = crate::utils::shell::shell_command_tokio()
                    .arg(cmd)
                    .output()
                    .await;
                match output {
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        result.push_str(&stdout);
                    }
                    Err(e) => {
                        result.push_str(&format!("[error: {}]", e));
                    }
                }
                remaining = &after_start[end + 1..];
            } else {
                // 没有闭合的 `，原样保留
                result.push_str("!`");
                remaining = after_start;
            }
        } else {
            result.push_str(remaining);
            break;
        }
    }

    Ok(result)
}

/// 首次启动时写入内置 skills（如果全局 skills 目录为空）
pub async fn ensure_builtin_skills(global_skills_dir: &Path) -> Result<()> {
    if !global_skills_dir.exists() {
        tokio::fs::create_dir_all(global_skills_dir).await?;
    }

    // 检查目录是否已有 skill 子目录
    let mut rd = match tokio::fs::read_dir(global_skills_dir).await {
        Ok(rd) => rd,
        Err(_) => return Ok(()),
    };
    while let Some(entry) = rd.next_entry().await? {
        if entry.path().is_dir() && entry.path().join("SKILL.md").exists() {
            // 已有 skill，不覆盖
            return Ok(());
        }
    }
    drop(rd);

    // 写入内置 skills
    for (name, content) in BUILTIN_SKILLS {
        let skill_dir = global_skills_dir.join(name);
        tokio::fs::create_dir_all(&skill_dir).await?;
        let skill_md = skill_dir.join("SKILL.md");
        tokio::fs::write(&skill_md, content).await?;
        tracing::info!("wrote builtin skill: {}", skill_md.display());
    }

    Ok(())
}

/// 内置 skills（标准 SKILL.md 格式）
const BUILTIN_SKILLS: &[(&str, &str)] = &[
    ("tdd", include_str!("builtin/tdd.md")),
    ("commit", include_str!("builtin/commit.md")),
    ("review", include_str!("builtin/review.md")),
    ("debug", include_str!("builtin/debug.md")),
    ("simplify", include_str!("builtin/simplify.md")),
    ("explain", include_str!("builtin/explain.md")),
    ("workflow", WORKFLOW_SKILL_BODY),
];

/// workflow skill 内容：方法论 + 工具使用指引（AI 自主激活后按此推进）
const WORKFLOW_SKILL_BODY: &str = r#"---
name: workflow
description: Spec-driven change management for large features and refactors. Use when the user describes a multi-step change, wants design before implementation, or needs structured project management with proposals, design docs, task tracking, and review gates.
when_to_use: "implement a feature", "refactor a module", "build a system", "large change", "workflow", "proposal", "design review"
allowed_tools:
  - read
  - write
  - edit
  - bash
  - grep
  - graph_search
  - graph_relations
  - graph_file_symbols
  - graph_index
  - workflow
  - subagent
  - skill_use
  - todo
  - ask_user
---

You are now in spec-driven workflow mode.

## Lifecycle

propose -> plan -> apply -> review -> archive

| Step | What | Who | Artifacts |
|------|------|-----|-----------|
| propose | Grill user on requirements, write proposal | You + user | proposal.md |
| plan | Design, decompose tasks, write delta specs | Planner sub-agent | design.md, tasks.md, specs/ |
| apply | Implement tasks via TDD | Executor sub-agents | Code + commits |
| review | Triple review: spec, quality, goal | Reviewer sub-agent | review.md |
| archive | Merge specs, update roadmap | You via workflow(action=finalize) | Specs merged |

## How to proceed

1. Call `workflow(action=continue)` to detect current state. It returns the next step name, the change name, AND the full step-by-step instructions for that step. Follow those instructions directly.
2. If you already know which step to do (e.g. user said "do the review"), call `workflow(action=step, name=<step>, change=<change>)` to get that step's full instructions.
3. After completing a step, call `workflow(action=continue, change=<change>)` again to get the next step's instructions.
4. Repeat until the change is archived.

## Tools

- `workflow(action=continue, change=<name>)` -- detect state, return next step + full instructions
- `workflow(action=step, name=propose|plan|apply|review|archive, change=<name>, fix=true|false)` -- get full instructions for a specific step
- `workflow(action=template, type=proposal|design|tasks|spec|review)` -- get document template
- `workflow(action=finalize, name=<change>)` -- execute archive (merge specs, move to archive)
- `workflow(action=state, change=<name>)` -- query workflow state
- `workflow(action=list)` -- list active changes (use when multiple changes exist)

## Sub-agent dispatch

- Planner: `subagent(op=spawn, role=planner)` -- produces design + tasks + delta specs
- Executor: `subagent(op=spawn, role=executor)` -- implements tasks via TDD
- Reviewer: `subagent(op=spawn, role=reviewer)` -- triple review

## Code graph

- `graph_search(action=symbol, pattern="")` -- module overview
- `graph_search(action=symbol, pattern="<name>")` -- find symbol
- `graph_relations(direction=callers, symbol="<module>")` -- impact analysis
- `graph_relations(direction=callees, symbol="<module>")` -- dependencies
- `graph_index(path=".")` -- rebuild graph

## Review routing

| Verdict | Issues | Next step |
|---------|--------|-----------|
| PASS | None | archive |
| FAIL | D (design) | plan (fix=true) |
| NEEDS_REVISION | R/Q/G (implementation) | apply (fix=true) |

## Parallel changes

When multiple changes are active, always pass `change=<name>` to workflow actions. Use `workflow(action=list)` to see all active changes.

## Guardrails

- Never skip review.
- Never archive without PASS.
- Dispatch sub-agents, don't do their work yourself.
- Fetch templates before writing artifacts.
- Use `graph_relations(direction=callers)` for impact analysis before designing.
"#;

/// 构建 SkillRegistry 并加载所有 skills
pub async fn build_registry(
    global_skills_dir: &Path,
    project_skills_dir: &Path,
) -> Result<std::sync::Arc<SkillRegistry>> {
    // 首次启动写入内置 skills
    ensure_builtin_skills(global_skills_dir).await?;

    let registry = std::sync::Arc::new(SkillRegistry::new());
    registry.load_from_dir(global_skills_dir).await?;
    registry.load_from_dir(project_skills_dir).await?;

    Ok(registry)
}
