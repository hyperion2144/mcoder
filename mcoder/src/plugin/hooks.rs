// 设计文档 §8.3.3: Shell Hook Handler
// 从 config.toml 的 [[hooks]] 加载，支持变量替换和 block 控制
// 示例:
//   [[hooks]]
//   event = "post_tool_use"
//   command = "rustfmt $FILE"
//   block = false
//
// 支持的变量:
//   $SESSION_ID  - 当前会话 ID
//   $TOOL        - 当前工具名（仅 pre/post_tool_use）
//   $FILE        - 当前操作的文件路径（如 edit/write）
//   $ARGS        - 工具参数 JSON

use crate::plugin::{HookContext, HookHandler, HookPoint, HookResult};
use crate::types::HookConfig;
use anyhow::Result;
use async_trait::async_trait;
use tokio::process::Command;

/// Shell 命令 Hook 处理器
pub struct ShellHookHandler {
    pub name: String,
    pub hook_point: HookPoint,
    pub command_template: String,
    pub block: bool,
}

impl ShellHookHandler {
    pub fn from_config(cfg: &HookConfig) -> Result<Self> {
        let hook_point = HookPoint::from_event_str(&cfg.event)?;
        Ok(Self {
            name: format!("shell-{}", cfg.event),
            hook_point,
            command_template: cfg.command.clone(),
            block: cfg.block,
        })
    }

    /// 将 $VAR 替换为 ctx.data 中的值
    fn substitute(&self, ctx: &HookContext) -> String {
        let mut cmd = self.command_template.clone();
        cmd = cmd.replace("$SESSION_ID", &ctx.session_id);

        if let Some(tool) = ctx.get_str("tool") {
            cmd = cmd.replace("$TOOL", tool);
        }
        if let Some(file) = ctx.get_str("file") {
            cmd = cmd.replace("$FILE", file);
        }
        if let Some(args) = ctx.data.get("args") {
            cmd = cmd.replace("$ARGS", &args.to_string());
        }
        cmd
    }
}

#[async_trait]
impl HookHandler for ShellHookHandler {
    fn name(&self) -> &str {
        &self.name
    }

    fn hooks(&self) -> &[HookPoint] {
        std::slice::from_ref(&self.hook_point)
    }

    async fn execute(&self, ctx: &HookContext) -> Result<HookResult> {
        let cmd = self.substitute(ctx);
        tracing::debug!("hook '{}' executing: {}", self.name, cmd);

        let output = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::warn!(
                "hook '{}' failed (exit {:?}): stderr={}, stdout={}",
                self.name,
                output.status.code(),
                stderr,
                stdout
            );
            if self.block {
                return Ok(HookResult {
                    allow: false,
                    modified_data: None,
                    message: Some(format!(
                        "hook '{}' blocked execution: {}",
                        self.name,
                        if stderr.is_empty() { stdout.to_string() } else { stderr.to_string() }
                    )),
                });
            }
        }

        Ok(HookResult::default())
    }
}
