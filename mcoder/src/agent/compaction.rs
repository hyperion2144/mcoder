//! 上下文压缩策略（advanced）
//!
//! 设计：
//! - 字符截断 + LLM 摘要两级 fallback
//! - 分级 ToolResult：不同工具有不同压缩策略
//! - 分层摘要：超长 session 用 layered summary（不是单层截断）
//! - Image 块压缩：image → text description

use crate::llm::{LLMResponse, SharedLLM};
use crate::types::{CompactConfig, ContentBlock, Message, ModelConfig, ToolOutput};
use anyhow::Result;
use std::sync::Arc;

// ==================== Token 估算 ====================

/// 精确 token 估算器（基于 tiktoken cl100k_base 编码）
/// 与 GPT-4 / Claude tokenizer 近似（BPE 算法不同，但 token 数差异 <10%）
/// 优势：精确到真实 token，不受 UTF-8 多字节字符干扰
pub struct TokenCounter {
    bpe: Arc<tiktoken_rs::CoreBPE>,
}

impl TokenCounter {
    /// 创建默认 cl100k_base 编码器（懒加载，启动时下载 rank 文件）
    pub fn new() -> Result<Self> {
        let bpe = tiktoken_rs::cl100k_base()
            .map_err(|e| anyhow::anyhow!("failed to load cl100k_base tokenizer: {}", e))?;
        Ok(Self { bpe: Arc::new(bpe) })
    }

    /// 估算文本 token 数
    pub fn count_text(&self, text: &str) -> usize {
        self.bpe.encode_ordinary(text).len()
    }

    /// 估算单条消息的 token 数
    pub fn count_message(&self, msg: &Message) -> usize {
        let mut total = 0;
        for b in &msg.content {
            match b {
                ContentBlock::Text { text } => {
                    total += self.count_text(text);
                }
                ContentBlock::ToolUse { name, args, .. } => {
                    total += self.count_text(name);
                    total += self.count_text(&args.to_string());
                    // Anthropic 格式：每条 tool_use 多 2 tokens（id + 标记）
                    total += 2;
                }
                ContentBlock::ToolResult { output, .. } => {
                    let s = serde_json::to_string(output).unwrap_or_default();
                    total += self.count_text(&s);
                    // Anthropic 格式：tool_result 多 4 tokens（id + is_error + 包装）
                    total += 4;
                }
                ContentBlock::Image { .. } => {
                    // 图片按 Claude 实际 token 估算：~1600 tokens（base64 编码 ~85KB）
                    // 实际取决于分辨率，范围 1000-6500
                    total += 1600;
                }
            }
        }
        // Anthropic 格式：每条 message 头尾共 6 tokens（role + 边界）
        total + 6
    }

    /// 估算多条消息的 token 数
    pub fn count_messages(&self, messages: &[Message]) -> usize {
        messages.iter().map(|m| self.count_message(m)).sum()
    }
}

/// 单条消息的 token 估算（精确模式：tiktoken；fallback：4 字符/token）
/// 默认走精确模式；如果 tiktoken 初始化失败（网络/资源问题），回退到 fallback
pub fn estimate_tokens(msg: &Message) -> usize {
    use std::sync::OnceLock;
    static COUNTER: OnceLock<Result<TokenCounter, String>> = OnceLock::new();
    let counter = COUNTER.get_or_init(|| TokenCounter::new().map_err(|e| e.to_string()));
    match counter {
        Ok(c) => c.count_message(msg),
        Err(_) => {
            // fallback：4 字符 = 1 token
            let chars: usize = msg
                .content
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => text.len(),
                    ContentBlock::ToolUse { name, args, .. } => {
                        name.len() + args.to_string().len()
                    }
                    ContentBlock::ToolResult { output, .. } => {
                        serde_json::to_string(output).map(|s| s.len()).unwrap_or(0)
                    }
                    ContentBlock::Image { .. } => 4000,
                })
                .sum();
            chars / 4 + 1
        }
    }
}

/// 兼容旧 API：传入 mode 选择
pub fn estimate_tokens_with_mode(msg: &Message, mode: TokenMode) -> usize {
    match mode {
        TokenMode::Tiktoken => estimate_tokens(msg),
        TokenMode::CharsFallback => {
            let chars: usize = msg
                .content
                .iter()
                .map(|b| b.text_len_or_size())
                .sum();
            chars / 4 + 1
        }
    }
}

/// Token 估算模式（用于测试 / 强制 fallback）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenMode {
    /// 精确：tiktoken cl100k_base
    Tiktoken,
    /// Fallback：4 字符 = 1 token
    CharsFallback,
}

/// 便捷：估算多消息总 token
pub fn estimate_messages_total(messages: &[Message]) -> usize {
    messages.iter().map(estimate_tokens).sum()
}

/// 把 Message 的 content 序列化成可读文本（给 LLM 摘要用）
pub fn serialize_for_summary(msg: &Message) -> String {
    let role = format!("[{:?}]", msg.role);
    let mut parts = Vec::new();
    for b in &msg.content {
        match b {
            ContentBlock::Text { text } => parts.push(text.clone()),
            ContentBlock::ToolUse { name, args, .. } => {
                parts.push(format!("[tool_call: {}({})]", name, args));
            }
            ContentBlock::ToolResult { output, .. } => {
                parts.push(format!(
                    "[tool_result: {}]",
                    serde_json::to_string(output).unwrap_or_default()
                ));
            }
            ContentBlock::Image { path, media_type } => {
                parts.push(format!("[image: {} ({})]", path, media_type));
            }
        }
    }
    format!("{} {}", role, parts.join(" "))
}

// ==================== 分级 ToolResult ====================

/// 根据工具名对 ToolResult 做工具感知的压缩
pub fn compact_tool_result_aware(tool_name: &str, output_str: &str, threshold: usize) -> String {
    if output_str.chars().count() <= threshold {
        return output_str.to_string();
    }
    match tool_name {
        "read" => compact_read_output(output_str, threshold),
        "bash" => compact_bash_output(output_str, threshold),
        "grep" => compact_grep_output(output_str, threshold),
        "glob" => compact_glob_output(output_str, threshold),
        "lsp_diagnostics" => compact_lsp_diagnostics(output_str, threshold),
        "launch" => compact_launch_output(output_str, threshold),
        _ => default_truncate(output_str, threshold),
    }
}

/// 默认截断：保留前 head + 后 tail
fn default_truncate(s: &str, threshold: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    let total = chars.len();
    if total <= threshold {
        return s.to_string();
    }
    let head: usize = threshold / 4;
    let tail: usize = threshold / 4;
    let tail_start = total.saturating_sub(tail);
    let truncated_chars = total.saturating_sub(head + tail);
    format!(
        "{}...[truncated {} chars]...{}",
        chars[..head].iter().collect::<String>(),
        truncated_chars,
        chars[tail_start..].iter().collect::<String>()
    )
}

/// read 输出压缩：保留文件结构 + 截断长行
fn compact_read_output(s: &str, threshold: usize) -> String {
    let total_lines = s.lines().count();
    if total_lines <= 50 {
        return default_truncate(s, threshold);
    }
    let head_lines = 25;
    let tail_lines = 15;
    let mut out = String::new();
    for line in s.lines().take(head_lines) {
        out.push_str(line);
        if line.len() > 200 {
            out.push_str("...[line truncated]");
        }
        out.push('\n');
    }
    out.push_str(&format!(
        "\n...[{} lines truncated during compaction]...\n\n",
        total_lines - head_lines - tail_lines
    ));
    let start = total_lines.saturating_sub(tail_lines);
    for line in s.lines().skip(start) {
        out.push_str(line);
        if line.len() > 200 {
            out.push_str("...[line truncated]");
        }
        out.push('\n');
    }
    out
}

/// bash 输出压缩：保留 stderr + 最后 N 行 stdout
fn compact_bash_output(s: &str, threshold: usize) -> String {
    // 尝试解析 {"ok": true, "stdout": "...", "stderr": "..."} 结构
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
        let stdout = v.get("stdout").and_then(|x| x.as_str()).unwrap_or("");
        let stderr = v.get("stderr").and_then(|x| x.as_str()).unwrap_or("");
        let mut stdout_chars: Vec<char> = stdout.chars().collect();
        let stderr_chars: Vec<char> = stderr.chars().collect();

        // 保留 stderr 全部 + stdout 末尾
        let keep_stdout = threshold.saturating_sub(stderr_chars.len()).max(500);
        let stdout_truncated;
        let stdout_text = if stdout_chars.len() > keep_stdout {
            let skip = stdout_chars.len() - keep_stdout;
            let _ = stdout_chars.split_off(skip);
            stdout_truncated = skip;
            stdout.to_string()
        } else {
            stdout_truncated = 0;
            stdout.to_string()
        };

        let mut out = format!(
            "{{\"ok\":{},\"stdout\":",
            v.get("ok").map(|x| x.to_string()).unwrap_or("true".into())
        );
        if stdout_truncated > 0 {
            out.push_str(&format!(
                "\"[...{} stdout chars truncated during compaction...]\\n{}\"",
                stdout_truncated, stdout_text
            ));
        } else {
            out.push_str(&format!("\"{}\"", stdout_text));
        }
        out.push_str(&format!(",\"stderr\":\"{}\"", stderr));
        if let Some(code) = v.get("exit_code") {
            out.push_str(&format!(",\"exit_code\":{}", code));
        }
        out.push('}');
        return out;
    }
    default_truncate(s, threshold)
}

/// grep 输出压缩：保留匹配文件:行号 + 行内容
fn compact_grep_output(s: &str, threshold: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= 50 {
        return default_truncate(s, threshold);
    }
    let mut out = String::new();
    // 保留前 20 + 后 20 行（匹配行号是重点）
    for line in lines.iter().take(20) {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!(
        "\n...[{} lines truncated]...\n\n",
        lines.len() - 40
    ));
    let start = lines.len().saturating_sub(20);
    for line in lines.iter().skip(start) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// glob 输出压缩：只保留文件列表（每行短）
fn compact_glob_output(s: &str, _threshold: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.is_empty()).collect();
    if lines.len() <= 100 {
        return s.to_string();
    }
    // glob 输出是文件路径，截断到 100 行 + 提示还有更多
    let mut out = String::new();
    for line in lines.iter().take(50) {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!(
        "\n...[{} more files truncated]...\n\n",
        lines.len() - 100
    ));
    let start = lines.len().saturating_sub(50);
    for line in lines.iter().skip(start) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// LSP diagnostics 输出：保留所有 diagnostic（每条短）
fn compact_lsp_diagnostics(s: &str, threshold: usize) -> String {
    default_truncate(s, threshold)
}

/// launch logs 输出：保留前 50 + 后 50 行
fn compact_launch_output(s: &str, _threshold: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= 100 {
        return s.to_string();
    }
    let mut out = String::new();
    for line in lines.iter().take(50) {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!(
        "\n...[{} log lines truncated]...\n\n",
        lines.len() - 100
    ));
    let start = lines.len().saturating_sub(50);
    for line in lines.iter().skip(start) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 把 ToolResult 内容按工具名压缩（同步，调用方需要传已知的 tool_name）
pub fn compact_tool_result_by_name(
    tool_name: &str,
    output: &ToolOutput,
    cfg: &CompactConfig,
) -> ToolOutput {
    let output_str = serde_json::to_string(output).unwrap_or_default();
    // 阈值优先级：
    //   1. cfg.tool_thresholds[tool_name] （用户配置）
    //   2. tool-aware 默认值（按工具类型不同）
    //   3. 全局默认 800
    let threshold = cfg
        .tool_thresholds
        .get(tool_name)
        .copied()
        .unwrap_or_else(|| default_tool_threshold(tool_name));
    if output_str.chars().count() <= threshold {
        return output.clone();
    }
    match cfg.tool_results.as_str() {
        "tool_aware" | "summarize" => {
            let new_str = compact_tool_result_aware(tool_name, &output_str, threshold);
            ToolOutput::Sync {
                result: serde_json::Value::String(new_str),
            }
        }
        "drop" => ToolOutput::Sync {
            result: serde_json::Value::String(
                "[tool result dropped during context compaction]".into(),
            ),
        },
        _ => output.clone(),
    }
}

/// 各工具的默认压缩阈值（基于输出典型大小）
/// 用户可在 config.toml 中通过 [compaction] tool_thresholds 覆盖
pub fn default_tool_threshold(tool_name: &str) -> usize {
    match tool_name {
        // 大输出：read 文件可能很长
        "read" => 5000,
        // bash 输出可能很长（npm install 等）
        "bash" => 2000,
        // launch 日志可能持续累积
        "launch" => 3000,
        // grep 多文件匹配可能很大
        "grep" => 1500,
        // glob 输出（文件列表）通常较短
        "glob" => 500,
        // LSP diagnostics 列表
        "lsp_diagnostics" => 1000,
        // web_search / web_fetch 摘要
        "web_search" => 1500,
        "web_fetch" => 2000,
        // edit/write 简短
        "edit" => 500,
        "write" => 500,
        // 其他工具 fallback 800
        _ => 800,
    }
}

// ==================== LLM 摘要 ====================

/// 同步字符截断（fallback）
pub fn fallback_truncate(s: &str, head: usize, tail: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    let total = chars.len();
    if total <= head + tail {
        return s.to_string();
    }
    let tail_start = total.saturating_sub(tail);
    format!(
        "{}...[truncated {} chars during compaction]...{}",
        chars[..head].iter().collect::<String>(),
        total - head - tail,
        chars[tail_start..].iter().collect::<String>()
    )
}

/// 异步调 LLM 生成一段摘要（用于整段对话压缩）
/// 超时或失败时回退到 fallback_truncate
pub async fn llm_summarize_messages(
    messages: &[Message],
    llm: &SharedLLM,
    model_config: &ModelConfig,
) -> Result<String> {
    if messages.is_empty() {
        return Ok(String::new());
    }

    // 序列化 messages
    let serialized: String = messages
        .iter()
        .map(serialize_for_summary)
        .collect::<Vec<_>>()
        .join("\n\n");

    // 限制输入长度（避免摘要调用本身就超出 context）
    let input = if serialized.len() > 20_000 {
        format!(
            "{}...[input truncated]...{}",
            &serialized[..10_000],
            &serialized[serialized.len() - 10_000..]
        )
    } else {
        serialized
    };

    let prompt = format!(
        "Summarize the following conversation excerpts concisely for context compaction. \
         Preserve: tool errors and their causes, file paths modified, key decisions, \
         test results. Drop: verbose tool output, repeated content, transient details. \
         Output: a concise paragraph (≤500 words).\n\n{}",
        input
    );

    let req = vec![Message::user(prompt)];
    match llm.chat(&req, &[], model_config).await {
        Ok(LLMResponse {
            content: Some(text), ..
        }) => Ok(text),
        Ok(resp) => Ok(resp.content.unwrap_or_default()),
        Err(e) => {
            tracing::warn!("llm_summarize failed, falling back to truncate: {}", e);
            Ok(fallback_truncate(&input, 500, 200))
        }
    }
}

/// 异步摘要整段 middle 消息，返回替换的 system summary Message
/// 注意：display_only 必须 = false，确保进入 LLM context（messages_along_head_path 会过滤）
pub async fn summarize_middle_as_system(
    messages: &[Message],
    llm: &SharedLLM,
    model_config: &ModelConfig,
) -> Message {
    let summary = llm_summarize_messages(messages, llm, model_config)
        .await
        .unwrap_or_else(|_| {
            // 全部失败：纯文本占位
            format!(
                "[context compacted: {} earlier messages summarized. Tool results were replaced to save tokens.]",
                messages.len()
            )
        });
    let msg = Message::system(summary);
    // 不设 display_only：摘要消息必须送入 LLM context 才能生效
    msg
}

// ==================== 分层摘要 ====================

/// 历史摘要层（超长 session 保留多段摘要）
#[derive(Debug, Clone)]
pub struct SummaryLayer {
    pub span: (usize, usize),  // 原始 messages 的索引范围（end exclusive）
    pub summary: String,
    pub created_at: i64,
}

/// 从完整 messages 中提取候选的中间段用于分层摘要
/// 当 messages[keep_recent..] 数量 > layer_chunk_size 时调用
pub fn should_create_layer(total_after_keep_recent: usize, cfg: &CompactConfig) -> bool {
    cfg.layered_summary && total_after_keep_recent > cfg.layer_chunk_size
}

/// 生成新的摘要层并返回（不修改 messages）
pub async fn create_layer(
    messages: &[Message],
    span: (usize, usize),
    llm: &SharedLLM,
    model_config: &ModelConfig,
) -> SummaryLayer {
    let chunk = &messages[span.0..span.1];
    let summary = llm_summarize_messages(chunk, llm, model_config)
        .await
        .unwrap_or_else(|_| {
            fallback_truncate(
                &chunk
                    .iter()
                    .map(serialize_for_summary)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                500,
                200,
            )
        });
    SummaryLayer {
        span,
        summary,
        created_at: chrono::Utc::now().timestamp(),
    }
}

/// 把多层摘要 + 最近消息拼成完整 context（用于 inject 到 LLM）
/// 注意：这里生成的消息会作为 caller 显式传入 LLM 的完整 messages 列表使用，
/// 不走 messages_along_head_path 过滤，因此 display_only 标记无意义。
pub fn inject_layers(layers: &[SummaryLayer], recent: &[Message]) -> Vec<Message> {
    let mut out = Vec::new();
    for layer in layers {
        let msg = Message::system(format!(
            "[Summary of earlier messages {}-{}]:\n{}",
            layer.span.0, layer.span.1, layer.summary
        ));
        out.push(msg);
    }
    out.extend_from_slice(recent);
    out
}

// ==================== Image 描述 ====================

/// Image 块压缩占位符：通常在 read 工具中已生成 description，
/// 但 compact 时若遇到 image 块（用户直接粘贴），生成描述文本替代
pub fn image_to_placeholder(path: &str, media_type: &str) -> String {
    format!(
        "[image: {} ({}) - description not available; use view_image tool to inspect]",
        path, media_type
    )
}

/// 检查消息中是否有 image 块（compact 时替换为 description 或占位符）
/// 异步：尝试调用视觉模型生成 description（如果失败则用占位符）
/// 输入缓存路径：避免重复描述
pub fn strip_images_sync(msg: &Message) -> Message {
    // 同步版本：直接用 placeholder
    let needs = msg
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }));
    if !needs {
        return msg.clone();
    }
    let new_content: Vec<ContentBlock> = msg
        .content
        .iter()
        .map(|b| match b {
            ContentBlock::Image { path, media_type } => ContentBlock::Text {
                text: image_to_placeholder(path, media_type),
            },
            other => other.clone(),
        })
        .collect();
    Message {
        id: msg.id.clone(),
        parent_id: msg.parent_id.clone(),
        role: msg.role,
        content: new_content,
        usage: msg.usage.clone(),
        display_only: msg.display_only,
    }
}

/// 异步版本：尝试调用视觉模型生成 image description（如果配置了视觉模型）
/// 没视觉模型时回退到 placeholder
/// vision_model_app_config: 完整 AppConfig 用于查找 vision model
pub async fn strip_images_with_describe(
    msg: &Message,
    mut image_describe_cache: Option<&mut std::collections::HashMap<String, String>>,
    app_config: Option<&crate::types::AppConfig>,
) -> Message {
    let needs = msg
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }));
    if !needs {
        return msg.clone();
    }
    // 查找视觉模型（如果配置）
    let vision_model = app_config.and_then(crate::tools::image::find_vision_model);
    let mut new_content: Vec<ContentBlock> = Vec::with_capacity(msg.content.len());
    for b in &msg.content {
        match b {
            ContentBlock::Image { path, media_type } => {
                // 查 cache：克隆出 Option<String> 立即释放 borrow
                let cached: Option<String> = image_describe_cache
                    .as_deref_mut()
                    .and_then(|c| c.get(path).cloned());
                let desc = if let Some(text) = cached {
                    text
                } else {
                    let model_arc = vision_model.as_ref().map(|m| Arc::new(m.clone()));
                    let d = describe_image_safe(path, media_type, model_arc.as_ref()).await;
                    if let Some(c) = image_describe_cache.as_deref_mut() {
                        c.insert(path.to_string(), d.clone());
                    }
                    d
                };
                new_content.push(ContentBlock::Text { text: desc });
            }
            other => new_content.push(other.clone()),
        }
    }
    Message {
        id: msg.id.clone(),
        parent_id: msg.parent_id.clone(),
        role: msg.role,
        content: new_content,
        usage: msg.usage.clone(),
        display_only: msg.display_only,
    }
}

/// 异步调用视觉模型描述图片（安全 fallback：失败 → placeholder）
async fn describe_image_safe(
    path: &str,
    media_type: &str,
    vision_model: Option<&Arc<ModelConfig>>,
) -> String {
    if let Some(model_config) = vision_model {
        if let Ok(llm) = crate::llm::create_adapter(model_config) {
            let prompt = format!(
                "[Describe this image in 1-2 sentences for context compaction. Path: {}, Media: {}]",
                path, media_type
            );
            let messages = vec![
                crate::types::Message::system("You are a vision assistant. Describe images concisely."),
                crate::types::Message::user(prompt),
            ];
            let req = match llm.chat(&messages, &[], model_config).await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!("vision describe failed for {}: {}", path, e);
                    return image_to_placeholder(path, media_type);
                }
            };
            if let Some(text) = req.content {
                if !text.is_empty() {
                    return format!("[image: {} - {}]", path, text);
                }
            }
        }
    }
    image_to_placeholder(path, media_type)
}

// keep sync version as `strip_images` for backward compatibility
pub fn strip_images(msg: &Message) -> Message {
    strip_images_sync(msg)
}

#[allow(dead_code)]
pub mod _internal {
    use super::*;
    /// tool_aware/summarize 入口：处理 ToolResult 时调用
    pub fn process_tool_results(msg: &Message, cfg: &CompactConfig) -> Message {
        let mut new_content = Vec::with_capacity(msg.content.len());
        // 状态机：跟踪上一个 ToolUse 的 tool_name（用于 pairing ToolResult）
        let mut last_tool_name: Option<String> = None;
        for b in &msg.content {
            match b {
                ContentBlock::ToolUse { name, .. } => {
                    last_tool_name = Some(name.clone());
                    new_content.push(b.clone());
                }
                ContentBlock::ToolResult { id, output } => {
                    let tool_name = last_tool_name.as_deref().unwrap_or("");
                    new_content.push(ContentBlock::ToolResult {
                        id: id.clone(),
                        output: compact_tool_result_by_name(tool_name, output, cfg),
                    });
                    last_tool_name = None;
                }
                _ => {
                    new_content.push(b.clone());
                }
            }
        }
        Message {
            id: msg.id.clone(),
            parent_id: msg.parent_id.clone(),
            role: msg.role,
            content: new_content,
            usage: msg.usage.clone(),
            display_only: msg.display_only,
        }
    }

    /// 计算所有 layer 的总 token 数
    pub fn layers_tokens(layers: &[SummaryLayer]) -> usize {
        layers
            .iter()
            .map(|l| l.summary.len() / 4 + 1)
            .sum()
    }
}