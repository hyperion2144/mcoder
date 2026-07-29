// 设计文档 §8.3.4: Skills 系统
// Skill = 可复用的 prompt 模板，注册为工具供 LLM 调用
//
// Skill 文件格式 (YAML):
//   name: tdd
//   description: Test-Driven Development workflow
//   prompt: |
//     Follow TDD: write failing test → implement → verify → refactor
//   allowed_tools:
//     - read
//     - write
//     - edit
//     - bash
//
// 加载路径:
//   全局: ~/.mcoder/skills/*.yaml
//   项目: .mcoder/skills/*.yaml
//
// 调用方式:
//   LLM 调用 skill_use 工具，参数 { name: "tdd", args: { task: "..." } }
//   工具返回展开后的 prompt，LLM 按 prompt 指导后续操作

use crate::tools::{SharedTool, Tool};
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Skill 定义（从 YAML 加载）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDef {
    pub name: String,
    pub description: String,
    /// prompt 模板，支持 {{var}} 变量替换
    pub prompt: String,
    /// 允许使用的工具列表（仅用于提示 LLM，不强制限制）
    #[serde(default)]
    pub allowed_tools: Vec<String>,
}

/// Skill 注册表
pub struct SkillRegistry {
    skills: RwLock<HashMap<String, SkillDef>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: RwLock::new(HashMap::new()),
        }
    }

    /// 从目录加载所有 .yaml / .yml skill 文件
    pub async fn load_from_dir(&self, dir: &PathBuf) -> Result<usize> {
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
                if ext == "yaml" || ext == "yml" {
                    entries.push(path);
                }
            }
        }
        for path in entries {
            match Self::load_skill_file(&path).await {
                Ok(skill) => {
                    tracing::info!("loaded skill '{}' from {}", skill.name, path.display());
                    self.skills.write().await.insert(skill.name.clone(), skill);
                    count += 1;
                }
                Err(e) => {
                    tracing::warn!("failed to load skill from {}: {}", path.display(), e);
                }
            }
        }
        Ok(count)
    }

    async fn load_skill_file(path: &PathBuf) -> Result<SkillDef> {
        let content = tokio::fs::read_to_string(path).await?;
        let skill: SkillDef = serde_yaml::from_str(&content)
            .with_context(|| format!("parsing skill yaml: {}", path.display()))?;
        if skill.name.is_empty() || skill.prompt.is_empty() {
            anyhow::bail!("skill must have name and prompt");
        }
        Ok(skill)
    }

    pub async fn list(&self) -> Vec<SkillDef> {
        self.skills.read().await.values().cloned().collect()
    }

    pub async fn get(&self, name: &str) -> Option<SkillDef> {
        self.skills.read().await.get(name).cloned()
    }

    /// 展开模板变量 {{var}}
    fn render_prompt(template: &str, args: &serde_json::Value) -> String {
        let mut result = template.to_string();
        if let Some(obj) = args.as_object() {
            for (k, v) in obj {
                let placeholder = format!("{{{{{}}}}}", k);
                let value = match v {
                    serde_json::Value::String(s) => s.clone(),
                    _ => v.to_string(),
                };
                result = result.replace(&placeholder, &value);
            }
        }
        result
    }
}

/// skill_use 工具：让 LLM 调用预定义的 skill
/// 参数: { name: "tdd", args: { task: "..." } }
pub struct SkillUseTool {
    pub registry: Arc<SkillRegistry>,
}

#[async_trait]
impl Tool for SkillUseTool {
    fn name(&self) -> &str {
        "skill_use"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "skill_use".into(),
            description: "Invoke a predefined skill (prompt template). Returns the expanded prompt to guide subsequent actions.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Skill name (e.g. 'tdd', 'commit', 'review')"
                    },
                    "args": {
                        "type": "object",
                        "description": "Arguments to substitute into the skill's prompt template ({{var}} placeholders)",
                        "additionalProperties": true
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &crate::tools::ToolContext) -> Result<ToolOutput> {
        let name = args.get("name")
            .and_then(|v| v.as_str())
            .context("missing 'name' field")?;

        let skill = self.registry.get(name).await
            .ok_or_else(|| anyhow!("skill not found: {}", name))?;

        let skill_args = args.get("args").cloned().unwrap_or(serde_json::Value::Null);
        let rendered = SkillRegistry::render_prompt(&skill.prompt, &skill_args);

        let allowed_hint = if skill.allowed_tools.is_empty() {
            String::new()
        } else {
            format!("\n\n[recommended tools: {}]", skill.allowed_tools.join(", "))
        };

        let output = serde_json::json!({
            "skill": name,
            "prompt": rendered + &allowed_hint,
            "description": skill.description,
        });

        Ok(ToolOutput::Sync { result: output })
    }
}

/// skill_list 工具：列出所有可用 skill
pub struct SkillListTool {
    pub registry: Arc<SkillRegistry>,
}

#[async_trait]
impl Tool for SkillListTool {
    fn name(&self) -> &str {
        "skill_list"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "skill_list".into(),
            description: "List all available skills".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        }
    }

    async fn execute(&self, _args: serde_json::Value, _ctx: &crate::tools::ToolContext) -> Result<ToolOutput> {
        let skills = self.registry.list().await;
        let list: Vec<serde_json::Value> = skills.iter().map(|s| serde_json::json!({
            "name": s.name,
            "description": s.description,
            "allowed_tools": s.allowed_tools,
        })).collect();
        Ok(ToolOutput::Sync { result: serde_json::Value::Array(list) })
    }
}

/// 设计文档 §8.3.4: 内置 skill 定义
/// 首次启动时写入 ~/.mcoder/skills/ 供用户参考和修改
const BUILTIN_SKILLS: &[(&str, &str)] = &[
    ("tdd.yaml", r#"name: tdd
description: Test-Driven Development workflow - write failing test first, then implement
prompt: |
  You are following the TDD (Test-Driven Development) workflow for task: {{task}}

  Steps:
  1. Understand the requirement and write a failing test that captures the expected behavior
  2. Run the test to confirm it fails for the right reason
  3. Write the minimal implementation to make the test pass
  4. Run the test to confirm it passes
  5. Refactor the code while keeping the test green
  6. Consider edge cases and add more tests if needed

  Rules:
  - Always write the test BEFORE the implementation
  - Commit after each green test
  - Don't over-engineer; write the minimum code to pass the test
allowed_tools:
  - read
  - write
  - edit
  - bash
  - grep
"#),
    ("commit.yaml", r#"name: commit
description: Generate a well-structured git commit with conventional commit format
prompt: |
  You are creating a git commit for the current changes. Task context: {{task}}

  Steps:
  1. Run `git status` and `git diff --staged` to review changes
  2. If nothing is staged, stage relevant files with `git add <file>` (avoid `git add -A`)
  3. Analyze the changes and determine the commit type:
     - feat: new feature
     - fix: bug fix
     - refactor: code restructuring without behavior change
     - docs: documentation only
     - test: adding/updating tests
     - chore: build/tooling changes
  4. Write a concise commit message:
     - Subject line: <type>(<scope>): <description> (max 50 chars)
     - Body: explain WHY (not what), wrap at 72 chars
  5. Commit with `git commit -m "<message>"`
  6. Show the final commit hash and summary

  Rules:
  - NEVER commit secrets (.env, credentials, API keys)
  - NEVER use --no-verify or --amend unless explicitly asked
  - Stage specific files, not `git add -A`
allowed_tools:
  - bash
  - read
"#),
    ("review.yaml", r#"name: review
description: Review code changes for quality, bugs, and best practices
prompt: |
  You are reviewing code. Review target: {{target}}

  Steps:
  1. Identify the changes to review:
     - If {{target}} is a file path, read that file
     - If {{target}} is "staged" or empty, run `git diff --staged`
     - If {{target}} is a commit hash, run `git show <hash>`
     - If {{target}} is a branch, run `git diff main...<branch>`
  2. Analyze the code for:
     - Correctness: logic errors, edge cases, off-by-one, null handling
     - Security: injection, path traversal, secret leakage
     - Performance: O(n²) loops, unnecessary allocations, missing indexes
     - Style: naming, dead code, missing error handling
     - Tests: are the changes tested? are edge cases covered?
  3. Categorize findings:
     - [BLOCKER] Must fix before merge
     - [WARNING] Should fix, but not blocking
     - [NIT] Minor style/preference
  4. Output a structured review with file:line references

  Rules:
  - Be specific: reference file paths and line numbers
  - Suggest fixes, don't just point out problems
  - Praise good patterns when seen
  - Don't suggest changes outside the diff scope
allowed_tools:
  - read
  - bash
  - grep
"#),
];

/// 设计文档 §8.3.4: 首次启动时写入内置 skills
/// 如果全局 skills 目录为空，写入 tdd/commit/review 三个内置 skill
pub async fn ensure_builtin_skills(global_skills_dir: &PathBuf) -> Result<()> {
    if !global_skills_dir.exists() {
        tokio::fs::create_dir_all(global_skills_dir).await?;
    }
    // 检查目录是否已有 .yaml 文件
    let mut rd = match tokio::fs::read_dir(global_skills_dir).await {
        Ok(rd) => rd,
        Err(_) => return Ok(()),
    };
    while let Some(entry) = rd.next_entry().await? {
        if let Some(ext) = entry.path().extension() {
            if ext == "yaml" || ext == "yml" {
                // 目录非空，不覆盖用户已有 skills
                return Ok(());
            }
        }
    }
    drop(rd);

    // 写入内置 skills
    for (filename, content) in BUILTIN_SKILLS {
        let path = global_skills_dir.join(filename);
        tokio::fs::write(&path, content).await?;
        tracing::info!("wrote builtin skill: {}", path.display());
    }
    Ok(())
}

/// 设计文档 §8.3.4: 加载 skills 并返回工具列表
/// 调用方将工具注册到 ToolRegistry
pub async fn build_skill_tools(
    global_skills_dir: &PathBuf,
    project_skills_dir: &PathBuf,
) -> Result<(Arc<SkillRegistry>, Vec<SharedTool>)> {
    // 首次启动写入内置 skills
    ensure_builtin_skills(global_skills_dir).await?;

    let registry = Arc::new(SkillRegistry::new());
    registry.load_from_dir(global_skills_dir).await?;
    registry.load_from_dir(project_skills_dir).await?;

    let mut tools: Vec<SharedTool> = Vec::new();
    tools.push(Arc::new(SkillUseTool { registry: registry.clone() }));
    tools.push(Arc::new(SkillListTool { registry: registry.clone() }));

    Ok((registry, tools))
}
