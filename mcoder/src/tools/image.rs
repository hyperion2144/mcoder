// tools/image.rs - 图片工具集
//
// ViewImageTool: 用视觉模型按需求描述图片（供非视觉模型 agent 调用）
// SendImageTool: agent 调用后在会话消息中展示图片

use crate::llm::create_adapter;
use crate::types::{ContentBlock, Message as McoderMessage, ModelConfig, Role, ToolOutput, ToolSchema};
use crate::tools::Tool;
use anyhow::{Context, Result};
use async_trait::async_trait;

/// 根据扩展名推断 media_type
fn infer_media_type(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".bmp") {
        "image/bmp"
    } else {
        "image/png"
    }
}

/// 从 app_config 中查找一个支持图片输入的模型
/// 优先查找 vision role 配置的 model，其次查找任意 input 含 "image" 的模型
/// m13: 提升为 pub，供 read 工具 (file.rs) 复用
pub fn find_vision_model(app_config: &crate::types::AppConfig) -> Option<ModelConfig> {
    // 1. 先看 vision role 是否配置了 model
    if let Some(role_cfg) = app_config.roles.get("vision") {
        if let Some(ref model_name) = role_cfg.model {
            if let Some(mc) = app_config.models.get(model_name) {
                if mc.supports_image() {
                    return Some(mc.clone());
                }
            }
        }
    }
    // 2. 遍历所有模型找第一个支持图片输入的
    for (_, mc) in &app_config.models {
        if mc.supports_image() {
            return Some(mc.clone());
        }
    }
    None
}

// ==================== ImageTool（合并 view_image/send_image）===================

/// image 工具：统一图片操作入口，通过 action 分发
/// - action="view": 用视觉模型按需求描述图片（原 ViewImageTool）
/// - action="send": 在会话消息中展示图片（原 SendImageTool）
pub struct ImageTool;

#[async_trait]
impl Tool for ImageTool {
    fn name(&self) -> &str {
        "image"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "image".into(),
            description: "Image tool. action='view': analyze an image using a vision-capable model (requires image_path + prompt). action='send': display an image in the chat (requires image_path, optional caption).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["view", "send"],
                        "description": "Operation mode"
                    },
                    "image_path": {
                        "type": "string",
                        "description": "Path to the image file"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "[view] What you want to know about the image"
                    },
                    "caption": {
                        "type": "string",
                        "description": "[send] Optional caption text to show alongside the image"
                    }
                },
                "required": ["action", "image_path"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &crate::tools::ToolContext) -> Result<ToolOutput> {
        let action = args.get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("view");

        match action {
            "view" => Self::view_image(args, ctx).await,
            "send" => Self::send_image(args),
            other => Ok(ToolOutput::Error {
                message: format!("unknown action '{}': expected view|send", other),
            }),
        }
    }
}

impl ImageTool {
    /// action="view": 用视觉模型分析图片（原 ViewImageTool 逻辑）
    async fn view_image(args: serde_json::Value, ctx: &crate::tools::ToolContext) -> Result<ToolOutput> {
        let image_path = args.get("image_path")
            .and_then(|v| v.as_str())
            .context("missing 'image_path'")?;
        let prompt = args.get("prompt")
            .and_then(|v| v.as_str())
            .context("missing 'prompt'")?;

        // 验证文件存在
        let path = std::path::Path::new(image_path);
        if !path.exists() {
            return Ok(ToolOutput::Error {
                message: format!("image file not found: {}", image_path),
            });
        }

        // 查找视觉模型
        let vision_model = find_vision_model(&ctx.app_config)
            .ok_or_else(|| anyhow::anyhow!(
                "no vision-capable model found. Configure a model with input=[\"text\",\"image\"] in config.toml, or set model for vision role."
            ))?;

        // 创建 LLM adapter
        let llm = create_adapter(&vision_model)
            .context("failed to create LLM adapter for vision model")?;

        // 构建消息：system prompt + user message (text + image)
        let media_type = infer_media_type(image_path);
        let system_msg = McoderMessage::system(
            "You are a vision assistant. Analyze the image according to the user's request. Be precise and thorough."
        );
        let user_msg = McoderMessage::new(Role::User, vec![
            ContentBlock::Text { text: prompt.to_string() },
            ContentBlock::Image {
                path: image_path.to_string(),
                media_type: media_type.to_string(),
            },
        ]);

        let messages = vec![system_msg, user_msg];
        let resp = llm.chat(&messages, &[], &vision_model)
            .await
            .context("vision model LLM call failed")?;

        let description = resp.content.unwrap_or_default();

        Ok(ToolOutput::Sync {
            result: serde_json::json!({
                "image_path": image_path,
                "description": description,
                "model": vision_model.name,
            }),
        })
    }

    /// action="send": 在会话消息中展示图片（原 SendImageTool 逻辑）
    fn send_image(args: serde_json::Value) -> Result<ToolOutput> {
        let image_path = args.get("image_path")
            .and_then(|v| v.as_str())
            .context("missing 'image_path'")?;
        let caption = args.get("caption")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // 验证文件存在
        let path = std::path::Path::new(image_path);
        if !path.exists() {
            return Ok(ToolOutput::Error {
                message: format!("image file not found: {}", image_path),
            });
        }

        let media_type = infer_media_type(image_path);

        // 返回特殊标记：agent loop 检测到 image action=send 调用后，
        // 会追加一条含 ContentBlock::Image 的 assistant 消息到会话
        Ok(ToolOutput::Sync {
            result: serde_json::json!({
                "type": "image_sent",
                "image_path": image_path,
                "media_type": media_type,
                "caption": caption,
            }),
        })
    }
}
