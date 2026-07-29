use crate::types::AppConfig;
use anyhow::{Context, Result};
use dirs::home_dir;
use std::path::{Path, PathBuf};

pub fn global_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MCODER_HOME") {
        return PathBuf::from(dir);
    }
    home_dir()
        .map(|h| h.join(".mcoder"))
        .unwrap_or_else(|| PathBuf::from(".mcoder"))
}

pub fn project_config_dir(project: &Path) -> PathBuf {
    project.join(".mcoder")
}

/// 设计文档 §2.1: 全局经验沉淀数据库路径
/// ~/.mcoder/experiences/sqlite.db - 跨项目共享
pub fn global_experiences_db_path() -> PathBuf {
    global_config_dir().join("experiences").join("sqlite.db")
}

/// 设计文档 §7.1/§7.2: 配置加载与合并
/// 全局配置 (~/.mcoder/config.toml) + 项目级配置 (<project>/.mcoder/config.toml) 深度合并
/// 合并策略：基于 toml::Value 的字段级深度合并
///   - table: 递归合并（项目级字段覆盖全局，未设置的保留全局）
///   - array: 追加（如 hooks、mcp_servers）
///   - scalar: 项目级覆盖全局
///   - 显式设置为默认值也会覆盖（解决了启发式判断的问题）
pub fn load_config(project: Option<&Path>) -> Result<AppConfig> {
    let global_path = global_config_dir().join("config.toml");
    let global_value = load_toml_value(&global_path).unwrap_or(toml::Value::Table(toml::value::Table::new()));

    let merged_value = if let Some(proj) = project {
        let proj_path = project_config_dir(proj).join("config.toml");
        if let Some(proj_value) = load_toml_value(&proj_path) {
            merge_toml_values(global_value, proj_value)
        } else {
            global_value
        }
    } else {
        global_value
    };

    let mut config: AppConfig = merged_value.try_into()
        .context("failed to deserialize merged config")?;

    // 设计文档 §7.1: api_key 支持 ${ENV_VAR} 语法，从环境变量解析
    for model in config.models.values_mut() {
        model.api_key = expand_env_var(&model.api_key);
    }

    Ok(config)
}

/// 展开 ${ENV_VAR} 格式的环境变量引用
/// 支持: ${MINIMAX_API_KEY} → 环境变量值
/// 不匹配格式则原样返回
fn expand_env_var(s: &str) -> String {
    if s.starts_with("${") && s.ends_with("}") && s.len() > 3 {
        let var_name = &s[2..s.len()-1];
        if let Ok(val) = std::env::var(var_name) {
            return val;
        }
    }
    s.to_string()
}

fn load_toml_value(path: &Path) -> Option<toml::Value> {
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

/// 设计文档 §7.2: 深度合并两个 toml::Value
/// - 都是 table: 递归合并，overlay 字段覆盖 base
/// - array: base 追加 overlay（hooks/mcp_servers 等追加语义）
/// - 其他: overlay 覆盖 base
fn merge_toml_values(mut base: toml::Value, overlay: toml::Value) -> toml::Value {
    use toml::Value;
    match (&mut base, overlay) {
        (Value::Table(base_tbl), Value::Table(overlay_tbl)) => {
            for (k, v) in overlay_tbl {
                match base_tbl.remove(&k) {
                    Some(base_v) => {
                        // 递归合并
                        let merged = merge_toml_values(base_v, v);
                        base_tbl.insert(k, merged);
                    }
                    None => {
                        // base 没有该字段，直接插入
                        base_tbl.insert(k, v);
                    }
                }
            }
            Value::Table(std::mem::take(base_tbl))
        }
        (Value::Array(base_arr), Value::Array(overlay_arr)) => {
            // 设计文档 §8.3: hooks/mcp_servers 等 array 追加（不覆盖）
            base_arr.extend(overlay_arr);
            Value::Array(std::mem::take(base_arr))
        }
        // scalar 或类型不匹配: overlay 覆盖 base
        (_, overlay) => overlay,
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            default_model: "gpt-4o".into(),
            models: std::collections::HashMap::new(),
            roles: std::collections::HashMap::new(),
            loop_max_iters: 50,
            compact: crate::types::CompactConfig {
                strategy: "auto".into(),
                threshold: 0.8,
                keep_recent: 5,
                keep_first: 2,
                tool_results: "summarize".into(),
            },
            tui: crate::types::TuiConfig {
                compact: false,
                theme: "default".into(),
            },
            server: crate::types::ServerConfig {
                host: "127.0.0.1".into(),
                port: 7654,
            },
            hooks: Vec::new(),
            mcp_servers: std::collections::HashMap::new(),
            memory: crate::types::MemoryConfig::default(),
            tools: crate::types::ToolsConfig::default(),
        }
    }
}

pub fn ensure_dirs(project: Option<&Path>) -> Result<()> {
    let global = global_config_dir();
    std::fs::create_dir_all(&global)
        .with_context(|| format!("creating global dir: {}", global.display()))?;
    std::fs::create_dir_all(global.join("sessions"))?;
    std::fs::create_dir_all(global.join("experiences"))?;
    // 设计文档 §8.3.4: 全局 skills 目录
    std::fs::create_dir_all(global.join("skills"))?;

    if let Some(proj) = project {
        let proj_dir = project_config_dir(proj);
        std::fs::create_dir_all(&proj_dir)?;
        std::fs::create_dir_all(proj_dir.join("sandbox"))?;
        std::fs::create_dir_all(proj_dir.join("plans"))?;
        std::fs::create_dir_all(proj_dir.join("journal"))?;
        std::fs::create_dir_all(proj_dir.join("tree-sitter-cache"))?;
        // 设计文档 §8.3.4: 项目级 skills 目录
        std::fs::create_dir_all(proj_dir.join("skills"))?;

        let gitignore = proj.join(".gitignore");
        if !gitignore.exists() {
            std::fs::write(&gitignore, ".mcoder/\n")?;
        } else {
            let content = std::fs::read_to_string(&gitignore)?;
            if !content.contains(".mcoder") {
                std::fs::write(&gitignore, content + ".mcoder/\n")?;
            }
        }
    }

    Ok(())
}
