use crate::types::AppConfig;
use anyhow::{Context, Result};
use dirs::home_dir;
use std::path::{Path, PathBuf};
use tracing::warn;

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
///
/// 错误友好化：文件存在但解析失败时，**打印明确错误位置**而非静默吞掉；
/// 调用方应捕获后回退到空配置 + 启动 setup mode。
pub fn load_config(project: Option<&Path>) -> Result<AppConfig> {
    let global_path = global_config_dir().join("config.toml");
    let global_value = load_toml_value_reporting(&global_path, "global")
        .unwrap_or_else(|| toml::Value::Table(toml::value::Table::new()));

    let merged_value = if let Some(proj) = project {
        let proj_path = project_config_dir(proj).join("config.toml");
        if let Some(proj_value) = load_toml_value_reporting(&proj_path, "project") {
            merge_toml_values(global_value, proj_value)
        } else {
            global_value
        }
    } else {
        global_value
    };

    let config: AppConfig = merged_value.try_into()
        .context("failed to deserialize merged config (check that global/project config.toml is valid)")?;

    // S2 修复: 不在此处展开 ${ENV_VAR}；内存中保留原始 ${ENV_VAR} 形式，
    // 由 create_adapter / test_provider 在使用时展开。
    // 这样 save_config 写盘时保留 ${ENV_VAR}，不会泄露明文 key。

    Ok(config)
}

/// 展开 ${ENV_VAR} 格式的环境变量引用
/// 支持: ${MINIMAX_API_KEY} → 环境变量值
/// 不匹配格式则原样返回
pub fn expand_env_var(s: &str) -> String {
    if s.starts_with("${") && s.ends_with("}") && s.len() > 3 {
        let var_name = &s[2..s.len()-1];
        if let Ok(val) = std::env::var(var_name) {
            return val;
        }
    }
    s.to_string()
}

/// 加载 TOML 文件并报告错误（不再静默吞错）
/// - 文件不存在 → 返回 Ok(None)，调用方按空配置处理
/// - IO/解析错误 → warn! 后返回 Ok(None)，调用方继续按空配置启动（setup mode 友好）
fn load_toml_value_reporting(path: &Path, scope: &str) -> Option<toml::Value> {
    if !path.exists() {
        return None;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            warn!("reading {scope} config ({}) failed: {e}", path.display());
            return None;
        }
    };
    match toml::from_str(&content) {
        Ok(v) => Some(v),
        Err(e) => {
            // 设计文档：把 TOML 解析错误明确打出来
            // 旧版 .unwrap_or(empty) 会让用户完全看不到错误位置
            warn!(
                "parsing {scope} config ({}) failed: {e}\n\
                 hint: fix syntax error in TOML, or delete the file to start fresh",
                path.display()
            );
            None
        }
    }
}

/// 原子写 ~/.mcoder/config.toml（tmp + rename），避免半写文件
pub fn save_config(config: &AppConfig) -> Result<()> {
    let path = global_config_dir().join("config.toml");
    let content = toml::to_string_pretty(config)
        .context("serialize config to TOML")?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, content)
        .with_context(|| format!("writing tmp config: {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} → {}", tmp.display(), path.display()))?;
    Ok(())
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
