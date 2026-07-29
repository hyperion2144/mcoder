// 跨平台 shell 命令执行工具
//
// Unix: sh -c
// Windows: cmd /C
//
// 设计文档 §8.6: 所有需要执行 shell 命令的模块应通过此模块
// 而非直接硬编码 "bash" / "sh"

use std::process::Command;

/// 返回配置好的 shell Command（已注入 -c / /C 参数）
///
/// 用法：
/// ```ignore
/// use crate::utils::shell::shell_command;
/// let output = shell_command().arg("ls -la").output()?;
/// ```
#[cfg(unix)]
pub fn shell_command() -> Command {
    let mut c = Command::new("sh");
    c.arg("-c");
    c
}

#[cfg(windows)]
pub fn shell_command() -> Command {
    let mut c = Command::new("cmd");
    c.arg("/C");
    c
}

/// 异步版本（tokio）
#[cfg(unix)]
pub fn shell_command_tokio() -> tokio::process::Command {
    let mut c = tokio::process::Command::new("sh");
    c.arg("-c");
    c
}

#[cfg(windows)]
pub fn shell_command_tokio() -> tokio::process::Command {
    let mut c = tokio::process::Command::new("cmd");
    c.arg("/C");
    c
}

/// 检测命令是否为后台运行语法
/// Unix: cmd ends with '&'
/// Windows: cmd starts with "start "
pub fn is_background_command(cmd: &str) -> bool {
    let trimmed = cmd.trim_end();
    #[cfg(unix)]
    {
        trimmed.ends_with('&')
    }
    #[cfg(windows)]
    {
        trimmed.to_lowercase().starts_with("start ")
    }
}

/// 获取 home 环境变量名
#[cfg(unix)]
pub const HOME_ENV: &str = "HOME";

#[cfg(windows)]
pub const HOME_ENV: &str = "USERPROFILE";

/// PATH 分隔符
#[cfg(unix)]
pub const PATH_SEP: char = ':';

#[cfg(windows)]
pub const PATH_SEP: char = ';';
