// 设计文档 §4.4: EditOpResult::Success.affected_lines 为 forward-looking 字段
// 当前 diff_preview 已足够；affected_lines 保留供未来精细 UI 展示
#![allow(dead_code)]

use crate::tools::sandbox::SandboxStore;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn hash_line(line: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(line.as_bytes());
    let result = hasher.finalize();
    result.iter().take(8).map(|b| format!("{:02x}", b)).collect()
}

// ==================== read 工具族 ====================

/// read 工具：读文件，返回带 hash 前缀的行
/// 设计文档 §4.4: 截断规则
/// - 行数 ≤ 500：全返回
/// - 行数 > 500：返回首 100 + 末 100 + 中间摘要 + handle
/// - 单行 > 500 字符：折行显示，全量存 sandbox
pub struct ReadTool;

const READ_FULL_THRESHOLD: usize = 500;
const READ_HEAD_LINES: usize = 100;
const READ_TAIL_LINES: usize = 100;
const READ_LONG_LINE_THRESHOLD: usize = 500;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str { "read" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read".into(),
            description: "Read file with hash-prefixed lines (for use with edit tool). Auto-truncates >500 lines: head 100 + tail 100 + handle. Long lines (>500 chars) wrap and go to sandbox. Set with_hashes=false for raw.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string" },
                    "start": { "type": "integer", "description": "Start line (1-indexed), default 1" },
                    "end": { "type": "integer", "description": "End line (inclusive)" },
                    "with_hashes": { "type": "boolean", "default": true }
                },
                "required": ["file"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path: PathBuf = serde_json::from_value(args["file"].clone())
            .or_else(|_| serde_json::from_value(args["path"].clone()))
            .context("file required")?;
        let start = args["start"].as_u64().or_else(|| args["offset"].as_u64()).unwrap_or(1) as usize;
        let end = args["end"].as_u64().map(|n| n as usize);
        let with_hashes = args["with_hashes"].as_bool().unwrap_or(true);

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;

        let all_lines: Vec<&str> = content.lines().collect();
        let total = all_lines.len();

        let s = start.max(1);
        let e = end.unwrap_or(total).min(total);
        if s > total {
            return Ok(ToolOutput::Sync { result: serde_json::json!({
                "file": path.display().to_string(),
                "content": "",
                "note": "start beyond file end"
            }) });
        }
        let range: &[&str] = &all_lines[s-1..e];

        // 检查长行：任一行 > 500 字符 → 全量存 sandbox，返回折行摘要
        let has_long_line = range.iter().any(|l| l.chars().count() > READ_LONG_LINE_THRESHOLD);
        if has_long_line {
            let full: String = range.iter().enumerate().map(|(i, l)| {
                let ln = s + i;
                if with_hashes {
                    format!("{}│{:>4}│ {}", hash_line(l), ln, l)
                } else {
                    format!("{:>4}│ {}", ln, l)
                }
            }).collect::<Vec<_>>().join("\n");
            let handle = SandboxStore::store(&ctx.project_dir, &full)?;
            // 折行摘要：每行按 100 字符折行显示，保留完整内容（不截断）
            // 但限制摘要总行数避免 token 爆炸
            const WRAP_WIDTH: usize = 100;
            const MAX_SUMMARY_LINES: usize = 200;
            let mut wrapped: Vec<String> = Vec::new();
            let mut total_wrapped_lines = 0;
            for l in range.iter() {
                if total_wrapped_lines >= MAX_SUMMARY_LINES {
                    wrapped.push(format!("... (more lines omitted, see handle)"));
                    break;
                }
                let h = &hash_line(l)[..8];
                let chars: Vec<char> = l.chars().collect();
                if chars.len() <= WRAP_WIDTH {
                    wrapped.push(format!("{}│ {}", h, l));
                    total_wrapped_lines += 1;
                } else {
                    // 折行：第一行带 hash，续行用 ↳ 缩进
                    for (idx, chunk) in chars.chunks(WRAP_WIDTH).enumerate() {
                        if total_wrapped_lines >= MAX_SUMMARY_LINES {
                            wrapped.push(format!("... (more lines omitted, see handle)"));
                            break;
                        }
                        let chunk_str: String = chunk.iter().collect();
                        if idx == 0 {
                            wrapped.push(format!("{}│ {}", h, chunk_str));
                        } else {
                            wrapped.push(format!("    ↳ {}", chunk_str));
                        }
                        total_wrapped_lines += 1;
                    }
                }
            }
            return Ok(ToolOutput::Sync { result: serde_json::json!({
                "file": path.display().to_string(),
                "start_line": s,
                "end_line": e,
                "total_lines": total,
                "content": wrapped.join("\n"),
                "handle": handle,
                "truncated": true,
                "reason": "long_line_wrapped",
                "hint": "Use read_more/read_full with handle for full content."
            }) });
        }

        // 截断规则：>500 行只返回首尾
        if range.len() > READ_FULL_THRESHOLD {
            let head: Vec<String> = range.iter().take(READ_HEAD_LINES).map(format_line_with_hash).collect();
            let tail_start = range.len().saturating_sub(READ_TAIL_LINES);
            let tail: Vec<String> = range[tail_start..].iter().map(format_line_with_hash).collect();
            let middle_count = range.len() - READ_HEAD_LINES - READ_TAIL_LINES;

            let full: String = range.iter().enumerate().map(|(i, l)| {
                let ln = s + i;
                if with_hashes {
                    format!("{}│{:>4}│ {}", hash_line(l), ln, l)
                } else {
                    format!("{:>4}│ {}", ln, l)
                }
            }).collect::<Vec<_>>().join("\n");
            let handle = SandboxStore::store(&ctx.project_dir, &full)?;

            let mut out = format!("{}\n... ({} lines omitted, handle={})\n{}",
                head.join("\n"), middle_count, handle, tail.join("\n"));
            if !with_hashes { out = strip_hashes(&out); }

            return Ok(ToolOutput::Sync { result: serde_json::json!({
                "file": path.display().to_string(),
                "start_line": s,
                "end_line": e,
                "total_lines": total,
                "content": out,
                "handle": handle,
                "truncated": true,
                "hint": "Use read_more or read_full with handle for omitted lines."
            }) });
        }

        // 小范围：全返回
        let out: String = range.iter().enumerate().map(|(i, l)| {
            let ln = s + i;
            if with_hashes {
                format!("{}│{:>4}│ {}", hash_line(l), ln, l)
            } else {
                format!("{:>4}│ {}", ln, l)
            }
        }).collect::<Vec<_>>().join("\n");

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "file": path.display().to_string(),
            "start_line": s,
            "end_line": e,
            "total_lines": total,
            "content": out,
            "truncated": false
        }) })
    }
}

fn format_line_with_hash(l: &&str) -> String {
    format!("{}│ {}", hash_line(l), l)
}
fn strip_hashes(s: &str) -> String {
    s.lines().map(|l| {
        if let Some(pos) = l.find('│') {
            if let Some(pos2) = l[pos+1..].find('│') {
                return l[pos+1+pos2+1..].to_string();
            }
        }
        l.to_string()
    }).collect::<Vec<_>>().join("\n")
}

/// read_more 工具：按 handle + offset/limit 分页读取
pub struct ReadMoreTool;

#[async_trait]
impl Tool for ReadMoreTool {
    fn name(&self) -> &str { "read_more" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read_more".into(),
            description: "Read more lines from a truncated read result by handle + offset.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "handle": { "type": "string" },
                    "offset": { "type": "integer", "description": "Line offset (0-indexed), default 0" },
                    "limit": { "type": "integer", "description": "Max lines, default 200" }
                },
                "required": ["handle"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let handle: String = serde_json::from_value(args["handle"].clone())?;
        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = args["limit"].as_u64().unwrap_or(200) as usize;
        let lines = SandboxStore::read_range(&ctx.project_dir, &handle, offset, limit)?
            .unwrap_or_default();
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "handle": handle,
            "offset": offset,
            "returned": lines.len(),
            "lines": lines
        }) })
    }
}

/// read_full 工具：返回 handle 对应的完整内容（走 sandbox）
pub struct ReadFullTool;

#[async_trait]
impl Tool for ReadFullTool {
    fn name(&self) -> &str { "read_full" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read_full".into(),
            description: "Read full content by handle from sandbox. Prefer read_more for paging.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "handle": { "type": "string" }
                },
                "required": ["handle"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let handle: String = serde_json::from_value(args["handle"].clone())?;
        let content = SandboxStore::read(&ctx.project_dir, &handle)?.unwrap_or_default();
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "handle": handle,
            "content": content,
            "bytes": content.len()
        }) })
    }
}

/// read_original 工具：获取摘要对应的原文
pub struct ReadOriginalTool;

#[async_trait]
impl Tool for ReadOriginalTool {
    fn name(&self) -> &str { "read_original" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read_original".into(),
            description: "Get original content behind a summary/handle. Use when you need the raw source after a summary.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "handle": { "type": "string" }
                },
                "required": ["handle"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let handle: String = serde_json::from_value(args["handle"].clone())?;
        let content = SandboxStore::read(&ctx.project_dir, &handle)?.unwrap_or_default();
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "handle": handle,
            "original": content,
            "bytes": content.len()
        }) })
    }
}

// ==================== write 工具 ====================

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str { "write" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "write".into(),
            description: "Write content to file (overwrite). Creates parent dirs. Use create_only=true to fail if file exists.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string" },
                    "content": { "type": "string" },
                    "create_only": { "type": "boolean", "default": false }
                },
                "required": ["file", "content"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path: PathBuf = serde_json::from_value(args["file"].clone())
            .or_else(|_| serde_json::from_value(args["path"].clone()))
            .context("file required")?;
        let content: String = serde_json::from_value(args["content"].clone())?;
        let create_only = args["create_only"].as_bool().unwrap_or(false);

        if create_only && path.exists() {
            anyhow::bail!("file already exists: {}", path.display());
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let before = std::fs::read_to_string(&path).unwrap_or_default();
        let journal_id = ctx.journal.record(&path, &before, &content, "write");
        std::fs::write(&path, &content)?;

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "ok": true,
            "file": path.display().to_string(),
            "bytes": content.len(),
            "journal_id": journal_id,
            "after_hash": hash_line(&content)[..8].to_string()
        }) })
    }
}

// ==================== edit 工具（单工具，edits 数组，自动推断操作）====================

/// edit 工具：基于 hash 锚点的编辑工具
/// 设计文档 §4.3 + 用户偏好"参数扁平、少嵌套、自动推断"
/// 一次调用可跨多个文件，混合多种操作（replace/insert/delete/sed）
/// 操作类型根据提供的字段自动推断，无需 op 字段:
///   - 有 pattern + replacement       → sed   (需 start + end)
///   - 有 start 无 content 无 pattern  → delete (end 可选)
///   - 有 position                     → insert (需 anchor + content)
///   - 有 anchor + content 无 position → replace (expect 可选)
///
/// 每个 edit 项字段:
///   {file, anchor?, content?, expect?, position?, start?, end?, pattern?, replacement?, flags?}
///
/// 返回值: {ok, files: [{file, ok, new_hashes, diff_preview, journal_id, edits_applied, summaries?}]}
/// 错误处理（per-file per-edit）:
/// - hash 未找到 → 该 file 的 ok=false，error 含 current_hashes 列表 + 行号
/// - expect 不匹配 → 该 file 的 ok=false，error 含 current_hash
/// - file 不存在 → 该 file 的 ok=false，error="file_not_found"，hint 用 write
/// - 一个 file 内某 edit 失败则该 file 不写入，其他 file 继续执行
pub struct EditTool;

/// 根据 edit 项的字段推断操作类型
#[derive(Debug)]
enum EditKind {
    Replace,
    Insert,
    Delete,
    Sed,
}

fn infer_edit_kind(edit: &Value) -> Result<EditKind> {
    let has_pattern = edit.get("pattern").and_then(|v| v.as_str()).is_some();
    let has_replacement = edit.get("replacement").and_then(|v| v.as_str()).is_some();
    let has_start = edit.get("start").and_then(|v| v.as_str()).is_some();
    let has_anchor = edit.get("anchor").and_then(|v| v.as_str()).is_some();
    let has_content = edit.get("content").and_then(|v| v.as_str()).is_some();
    let has_position = edit.get("position").and_then(|v| v.as_str()).is_some();

    if has_pattern && has_replacement {
        Ok(EditKind::Sed)
    } else if has_start && !has_content && !has_pattern {
        Ok(EditKind::Delete)
    } else if has_position {
        Ok(EditKind::Insert)
    } else if has_anchor && has_content {
        Ok(EditKind::Replace)
    } else {
        anyhow::bail!(
            "cannot infer edit kind from fields: need (anchor+content) for replace, (anchor+content+position) for insert, (start) for delete, or (start+end+pattern+replacement) for sed"
        )
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str { "edit" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "edit".into(),
            description: "Hash-anchored edit tool. Accepts edits array; each edit = {file, ...fields}. Operation auto-inferred: pattern+replacement=sed, start-only=delete, position=insert, anchor+content=replace. One call mixes multiple ops across multiple files atomically per file. Returns {ok, files:[{file, ok, new_hashes, diff_preview, journal_id}]}. On hash miss, error includes current_hashes for self-correction. File must exist (use write for new).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "edits": {
                        "type": "array",
                        "description": "Array of edit operations. Each item: {file, ...fields}. Operation auto-inferred from fields. Multiple ops across multiple files in one call.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "file": { "type": "string", "description": "Target file path" },
                                "anchor": { "type": "string", "description": "replace/insert: 8-char hash of anchor line" },
                                "content": { "type": "string", "description": "replace/insert: new content (multi-line)" },
                                "expect": { "type": "string", "description": "replace: optimistic lock hash" },
                                "position": { "type": "string", "enum": ["before", "after"], "default": "after", "description": "insert: before/after anchor (presence triggers insert mode)" },
                                "start": { "type": "string", "description": "delete/sed: 8-char hash of first line" },
                                "end": { "type": "string", "description": "delete/sed: 8-char hash of last line" },
                                "pattern": { "type": "string", "description": "sed: regex pattern (presence triggers sed mode)" },
                                "replacement": { "type": "string", "description": "sed: replacement" },
                                "flags": { "type": "string", "default": "g", "description": "sed: g=global, i=case-insensitive" }
                            },
                            "required": ["file"]
                        }
                    }
                },
                "required": ["edits"]
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let edits: Vec<Value> = serde_json::from_value(args["edits"].clone())
            .context("edits array required")?;

        // 按 file 分组，每个文件内原子应用所有 edits
        let mut by_file: std::collections::HashMap<PathBuf, Vec<&Value>> = std::collections::HashMap::new();
        let mut file_order: Vec<PathBuf> = Vec::new();
        for edit in edits.iter() {
            let file: PathBuf = serde_json::from_value(edit["file"].clone())
                .context("each edit requires 'file' field")?;
            if !by_file.contains_key(&file) {
                file_order.push(file.clone());
            }
            by_file.entry(file).or_default().push(edit);
        }

        let mut results: Vec<serde_json::Value> = Vec::new();
        // 多文件 edit: 每个文件独立 journal.record（finalize 内完成）
        // 共享一个逻辑 batch_id 便于客户端 undo 分组（仅记录到返回值，不依赖 batch snapshot）
        let batch_id = if file_order.len() > 1 {
            format!("edit_{}", uuid::Uuid::new_v4().simple())
        } else {
            String::new()
        };

        for file in &file_order {
            let edits_for_file = &by_file[file];
            if !file.exists() {
                results.push(serde_json::json!({
                    "file": file.display().to_string(),
                    "ok": false,
                    "error": "file_not_found",
                    "hint": "Use the write tool to create new files."
                }));
                continue;
            }
            let before = std::fs::read_to_string(file)?;
            let mut content = before.clone();
            let mut summaries = Vec::new();
            let mut file_ok = true;
            let mut file_error: Option<serde_json::Value> = None;

            for (i, edit) in edits_for_file.iter().enumerate() {
                // 自动推断操作类型
                let kind = match infer_edit_kind(edit) {
                    Ok(k) => k,
                    Err(e) => {
                        file_ok = false;
                        file_error = Some(serde_json::json!({
                            "edit_index": i,
                            "error": e.to_string()
                        }));
                        break;
                    }
                };

                let result = match kind {
                    EditKind::Replace => {
                        let anchor: String = serde_json::from_value(edit["anchor"].clone())?;
                        let c: String = serde_json::from_value(edit["content"].clone())?;
                        let expect: Option<String> = edit["expect"].as_str().map(|s| s.to_string());
                        apply_replace(&content, &anchor, &c, expect, file)
                    }
                    EditKind::Insert => {
                        let anchor: String = serde_json::from_value(edit["anchor"].clone())?;
                        let c: String = serde_json::from_value(edit["content"].clone())?;
                        let pos = edit["position"].as_str().unwrap_or("after");
                        apply_insert(&content, &anchor, &c, pos, file)
                    }
                    EditKind::Delete => {
                        let start: String = serde_json::from_value(edit["start"].clone())?;
                        let end: Option<String> = edit["end"].as_str().map(|s| s.to_string());
                        apply_delete(&content, &start, end.as_deref(), file)
                    }
                    EditKind::Sed => {
                        // start/end 可选：不传时对全文做替换，传了则限定行范围
                        let start: Option<String> = edit["start"].as_str().map(|s| s.to_string());
                        let end: Option<String> = edit["end"].as_str().map(|s| s.to_string());
                        let pattern: String = serde_json::from_value(edit["pattern"].clone())?;
                        let replacement: String = serde_json::from_value(edit["replacement"].clone())?;
                        let flags = edit["flags"].as_str().unwrap_or("g");
                        apply_sed(&content, start.as_deref(), end.as_deref(), &pattern, &replacement, flags, file)
                    }
                };

                match result {
                    Ok(EditOpResult::Success { new_content, summary, .. }) => {
                        content = new_content;
                        summaries.push(summary);
                    }
                    Ok(EditOpResult::HashNotFound { all_hashes }) => {
                        file_ok = false;
                        file_error = Some(serde_json::json!({
                            "edit_index": i,
                            "error": "anchor_not_found",
                            "current_hashes": all_hashes,
                            "hint": "Use a hash from current_hashes list."
                        }));
                        break;
                    }
                    Ok(EditOpResult::ExpectMismatch { actual_hash }) => {
                        file_ok = false;
                        file_error = Some(serde_json::json!({
                            "edit_index": i,
                            "error": "expect_mismatch",
                            "current_hash": actual_hash,
                            "hint": "File modified since read. Re-read to get fresh hashes."
                        }));
                        break;
                    }
                    Err(e) => {
                        file_ok = false;
                        file_error = Some(serde_json::json!({
                            "edit_index": i,
                            "error": e.to_string()
                        }));
                        break;
                    }
                }
            }

            if file_ok {
                // 保持末尾换行
                if before.ends_with('\n') && !content.ends_with('\n') {
                    content.push('\n');
                }
                let (new_hashes, journal_id, diff) = Self::finalize(&ctx.journal, file, &before, &content, "edit")?;
                results.push(serde_json::json!({
                    "file": file.display().to_string(),
                    "ok": true,
                    "new_hashes": new_hashes,
                    "diff_preview": diff,
                    "journal_id": journal_id,
                    "edits_applied": summaries.len(),
                    "summaries": summaries
                }));
            } else {
                results.push(serde_json::json!({
                    "file": file.display().to_string(),
                    "ok": false,
                    "error": file_error
                }));
            }
        }

        let all_ok = results.iter().all(|r| r["ok"].as_bool().unwrap_or(false));
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "ok": all_ok,
            "files": results,
            "total_files": file_order.len(),
            "batch_id": if batch_id.is_empty() { Value::Null } else { Value::String(batch_id) }
        }) })
    }
}

impl EditTool {
    /// 写入文件 + journal + 生成 new_hashes/diff_preview
    fn finalize(journal: &Arc<crate::tools::journal::FileJournal>, path: &Path, before: &str, new_content: &str, op: &str) -> Result<(Vec<String>, String, String)> {
        let journal_id = journal.record(path, before, new_content, op);
        std::fs::write(path, new_content)?;
        // new_hashes: 修改后文件前 5 行的 hash
        let new_hashes: Vec<String> = new_content.lines().take(5).map(|l| hash_line(l)[..8].to_string()).collect();
        // diff_preview: unified diff 格式
        let diff = unified_diff_preview(before, new_content);
        Ok((new_hashes, journal_id, diff))
    }
}

// ==================== Edit 操作实现 ====================

/// 单个 edit 操作的结果
enum EditOpResult {
    /// 成功：返回新内容和摘要
    Success {
        new_content: String,
        summary: String,
        /// 影响的行范围（1-indexed, inclusive）
        affected_lines: Option<(usize, usize)>,
    },
    /// anchor/start/end hash 未找到
    HashNotFound {
        /// 当前文件所有行的 hash 列表（前 8 字符）+ 行号
        all_hashes: Vec<serde_json::Value>,
    },
    /// expect 不匹配（乐观锁失败）
    ExpectMismatch {
        actual_hash: String,
    },
}

/// 替换 anchor 行为 content（content 可多行）
fn apply_replace(
    content: &str,
    anchor: &str,
    new_content: &str,
    expect: Option<String>,
    _file: &Path,
) -> Result<EditOpResult> {
    let lines: Vec<&str> = content.lines().collect();
    // 查找 anchor 行
    let mut found_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if hash_line(line).starts_with(anchor) {
            found_idx = Some(i);
            break;
        }
    }
    let idx = match found_idx {
        Some(i) => i,
        None => return Ok(EditOpResult::HashNotFound { all_hashes: collect_hashes(&lines) }),
    };

    // 乐观锁检查
    if let Some(exp) = &expect {
        let actual = hash_line(lines[idx])[..8].to_string();
        if &actual != exp {
            return Ok(EditOpResult::ExpectMismatch { actual_hash: actual });
        }
    }

    let new_lines: Vec<String> = new_content.lines().map(|s| s.to_string()).collect();
    let mut result: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    result.splice(idx..idx+1, new_lines);

    let summary = format!("replaced line {} ({} → {} lines)", idx + 1, 1, result.len() - lines.len() + 1);
    let new_content_joined = join_lines(&result, content.ends_with('\n'));
    Ok(EditOpResult::Success {
        new_content: new_content_joined,
        summary,
        affected_lines: Some((idx + 1, idx + 1)),
    })
}

/// 在 anchor 行前/后插入 content
fn apply_insert(
    content: &str,
    anchor: &str,
    new_content: &str,
    position: &str,
    _file: &Path,
) -> Result<EditOpResult> {
    let lines: Vec<&str> = content.lines().collect();
    let mut found_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if hash_line(line).starts_with(anchor) {
            found_idx = Some(i);
            break;
        }
    }
    let idx = match found_idx {
        Some(i) => i,
        None => return Ok(EditOpResult::HashNotFound { all_hashes: collect_hashes(&lines) }),
    };

    let insert_lines: Vec<String> = new_content.lines().map(|s| s.to_string()).collect();
    let mut result: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    let insert_at = if position == "before" { idx } else { idx + 1 };
    let inserted_count = insert_lines.len();
    result.splice(insert_at..insert_at, insert_lines);

    let summary = format!("inserted {} line(s) {} line {}", inserted_count, position, idx + 1);
    let new_content_joined = join_lines(&result, content.ends_with('\n'));
    Ok(EditOpResult::Success {
        new_content: new_content_joined,
        summary,
        affected_lines: Some((insert_at + 1, insert_at + inserted_count)),
    })
}

/// 删除从 start 到 end 的行（含两端）。end 可选，缺省只删 start 一行
fn apply_delete(
    content: &str,
    start: &str,
    end: Option<&str>,
    _file: &Path,
) -> Result<EditOpResult> {
    let lines: Vec<&str> = content.lines().collect();
    let mut start_idx: Option<usize> = None;
    let mut end_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        let h = &hash_line(line)[..8];
        if start_idx.is_none() && h.starts_with(start) {
            start_idx = Some(i);
        }
        if let Some(end_hash) = end {
            if h.starts_with(end_hash) {
                end_idx = Some(i);
            }
        }
    }

    let s = match start_idx {
        Some(i) => i,
        None => return Ok(EditOpResult::HashNotFound { all_hashes: collect_hashes(&lines) }),
    };
    let e = match (end, end_idx) {
        (Some(_), Some(ei)) => ei,
        (Some(_), None) => {
            anyhow::bail!("end hash not found for delete operation");
        }
        (None, _) => s,
    };
    if e < s {
        anyhow::bail!("end line ({}) is before start line ({})", e + 1, s + 1);
    }

    let mut result: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    let deleted_count = e - s + 1;
    result.drain(s..=e);

    let summary = format!("deleted {} line(s) ({}-{})", deleted_count, s + 1, e + 1);
    let new_content_joined = join_lines(&result, content.ends_with('\n'));
    Ok(EditOpResult::Success {
        new_content: new_content_joined,
        summary,
        affected_lines: Some((s + 1, e + 1)),
    })
}

/// sed 模式：在 start..end 行范围内，按 pattern + replacement 替换
fn apply_sed(
    content: &str,
    start: Option<&str>,
    end: Option<&str>,
    pattern: &str,
    replacement: &str,
    flags: &str,
    _file: &Path,
) -> Result<EditOpResult> {
    let lines: Vec<&str> = content.lines().collect();
    // start/end 为 None 时对全文做替换
    let (s, e) = match (start, end) {
        (Some(start_hash), Some(end_hash)) => {
            let mut start_idx: Option<usize> = None;
            let mut end_idx: Option<usize> = None;
            for (i, line) in lines.iter().enumerate() {
                let h = &hash_line(line)[..8];
                if start_idx.is_none() && h.starts_with(start_hash) {
                    start_idx = Some(i);
                }
                if h.starts_with(end_hash) {
                    end_idx = Some(i);
                }
            }
            let s = match start_idx {
                Some(i) => i,
                None => return Ok(EditOpResult::HashNotFound { all_hashes: collect_hashes(&lines) }),
            };
            let e = match end_idx {
                Some(i) => i,
                None => anyhow::bail!("end hash not found for sed operation"),
            };
            if e < s {
                anyhow::bail!("end line ({}) is before start line ({})", e + 1, s + 1);
            }
            (s, e)
        }
        _ => (0usize, lines.len().saturating_sub(1)),
    };

    let case_insensitive = flags.contains('i');
    let global = flags.contains('g');
    let re = if case_insensitive {
        Regex::new(&format!("(?i){}", pattern))?
    } else {
        Regex::new(pattern)?
    };

    let mut result: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    let mut total_replacements = 0;
    for i in s..=e {
        let line = &result[i];
        let new_line = if global {
            let after = re.replace_all(line, replacement);
            let count = re.find_iter(line).count();
            total_replacements += count;
            after.to_string()
        } else {
            let count = re.find_iter(line).count();
            if count > 0 {
                total_replacements += 1;
            }
            re.replace(line, replacement).to_string()
        };
        result[i] = new_line;
    }

    let summary = format!("sed replaced {} occurrence(s) in lines {}-{}", total_replacements, s + 1, e + 1);
    let new_content_joined = join_lines(&result, content.ends_with('\n'));
    Ok(EditOpResult::Success {
        new_content: new_content_joined,
        summary,
        affected_lines: Some((s + 1, e + 1)),
    })
}

/// 收集所有行 hash（前 8 字符）+ 行号，供 LLM 自纠错
fn collect_hashes(lines: &[&str]) -> Vec<serde_json::Value> {
    lines.iter().enumerate().map(|(i, l)| {
        serde_json::json!({
            "line": i + 1,
            "hash": hash_line(l)[..8].to_string(),
            "preview": l.chars().take(60).collect::<String>()
        })
    }).collect()
}

/// 拼接行，保持末尾换行
fn join_lines(lines: &[String], trailing_newline: bool) -> String {
    let mut s = lines.join("\n");
    if trailing_newline {
        s.push('\n');
    }
    s
}

/// 生成 unified diff 预览
/// 格式：@@ -10,3 +10,4 @@ ... 行级 diff（仅前若干 hunk）
fn unified_diff_preview(before: &str, after: &str) -> String {
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();

    // 简易 LCS 行级 diff
    let diffs: Vec<(char, String)> = Vec::new();
    let n = before_lines.len();
    let m = after_lines.len();

    // 简单实现：用 LCS 算法
    let lcs = lcs_table(&before_lines, &after_lines);
    let mut i = n;
    let mut j = m;
    let mut ops: Vec<(char, String)> = Vec::new();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && before_lines[i-1] == after_lines[j-1] {
            ops.push((' ', before_lines[i-1].to_string()));
            i -= 1; j -= 1;
        } else if j > 0 && (i == 0 || lcs[i][j-1] >= lcs[i-1][j]) {
            ops.push(('+', after_lines[j-1].to_string()));
            j -= 1;
        } else if i > 0 {
            ops.push(('-', before_lines[i-1].to_string()));
            i -= 1;
        }
    }
    ops.reverse();
    let _ = diffs;

    // 分组为 hunks（连续变化 + 上下文 3 行）
    let hunks = build_hunks(&ops, 3);
    if hunks.is_empty() {
        return String::new();
    }

    // 限制输出：最多 5 个 hunk
    let mut out = String::new();
    for hunk in hunks.iter().take(5) {
        out.push_str(&format!("{}", hunk));
    }
    if hunks.len() > 5 {
        out.push_str(&format!("... ({} more hunks omitted)\n", hunks.len() - 5));
    }
    out
}

/// LCS 表
fn lcs_table(a: &[&str], b: &[&str]) -> Vec<Vec<usize>> {
    let n = a.len();
    let m = b.len();
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            if a[i-1] == b[j-1] {
                dp[i][j] = dp[i-1][j-1] + 1;
            } else {
                dp[i][j] = dp[i-1][j].max(dp[i][j-1]);
            }
        }
    }
    dp
}

/// 构建 unified diff hunks
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<(char, String)>,
}

impl std::fmt::Display for Hunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "@@ -{},{} +{},{} @@",
            self.old_start, self.old_count, self.new_start, self.new_count)?;
        for (op, line) in &self.lines {
            writeln!(f, "{} {}", op, line)?;
        }
        Ok(())
    }
}

fn build_hunks(ops: &[(char, String)], context: usize) -> Vec<Hunk> {
    // 找出所有变化点
    let change_indices: Vec<usize> = ops.iter().enumerate()
        .filter(|(_, (op, _))| *op == '+' || *op == '-')
        .map(|(i, _)| i)
        .collect();
    if change_indices.is_empty() {
        return Vec::new();
    }

    // 分组：相邻变化点距离 <= 2*context+1 合并
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut cur_start = change_indices[0];
    let mut cur_end = change_indices[0];
    for &idx in &change_indices[1..] {
        if idx - cur_end <= 2 * context + 1 {
            cur_end = idx;
        } else {
            groups.push((cur_start, cur_end));
            cur_start = idx;
            cur_end = idx;
        }
    }
    groups.push((cur_start, cur_end));

    // 为每组扩展 context 行并构建 hunk
    let mut hunks = Vec::new();
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut idx = 0usize;

    for (g_start, g_end) in groups {
        let hunk_start = g_start.saturating_sub(context);
        let hunk_end = (g_end + context).min(ops.len() - 1);

        // 推进 old/new 行号到 hunk_start
        while idx < hunk_start {
            match ops[idx].0 {
                ' ' => { old_line += 1; new_line += 1; }
                '-' => { old_line += 1; }
                '+' => { new_line += 1; }
                _ => {}
            }
            idx += 1;
        }

        let hunk_old_start = old_line;
        let hunk_new_start = new_line;
        let mut old_count = 0;
        let mut new_count = 0;
        let mut hunk_lines = Vec::new();

        while idx <= hunk_end && idx < ops.len() {
            let (op, line) = &ops[idx];
            hunk_lines.push((*op, line.clone()));
            match op {
                ' ' => { old_count += 1; new_count += 1; old_line += 1; new_line += 1; }
                '-' => { old_count += 1; old_line += 1; }
                '+' => { new_count += 1; new_line += 1; }
                _ => {}
            }
            idx += 1;
        }

        hunks.push(Hunk {
            old_start: hunk_old_start,
            old_count,
            new_start: hunk_new_start,
            new_count,
            lines: hunk_lines,
        });
    }

    hunks
}

// ==================== Ls / Grep ====================

pub struct LsTool;

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &str { "ls" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "ls".into(),
            description: "List directory entries. Returns name, type (file/dir/symlink), size, and modified time. Respects project_dir scope.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory to list (default project root)" },
                    "all": { "type": "boolean", "default": false, "description": "Include hidden entries (starting with .)" },
                    "max": { "type": "integer", "default": 200, "description": "Max entries to return" }
                }
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let path: PathBuf = args["path"].as_str()
            .map(|s| PathBuf::from(s))
            .unwrap_or_else(|| PathBuf::from("."));
        let include_hidden = args["all"].as_bool().unwrap_or(false);
        let max = args["max"].as_u64().unwrap_or(200) as usize;

        let mut entries: Vec<serde_json::Value> = Vec::new();
        let read = std::fs::read_dir(&path)
            .with_context(|| format!("listing {}", path.display()))?;

        for entry in read {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !include_hidden && name.starts_with('.') {
                continue;
            }
            let meta = entry.metadata()?;
            let kind = if meta.is_dir() { "dir" }
                else if meta.is_symlink() { "symlink" }
                else { "file" };
            entries.push(serde_json::json!({
                "name": name,
                "type": kind,
                "size": meta.len(),
                "modified": meta.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            }));
            if entries.len() >= max {
                break;
            }
        }

        entries.sort_by(|a, b| {
            let at = a["type"].as_str().unwrap_or("");
            let bt = b["type"].as_str().unwrap_or("");
            let an = a["name"].as_str().unwrap_or("");
            let bn = b["name"].as_str().unwrap_or("");
            match (at, bt) {
                ("dir", "file") => std::cmp::Ordering::Less,
                ("file", "dir") => std::cmp::Ordering::Greater,
                _ => an.cmp(bn),
            }
        });

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "path": path.display().to_string(),
            "entries": entries,
            "count": entries.len(),
            "truncated": entries.len() >= max
        }) })
    }
}

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str { "grep" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "grep".into(),
            description: "Recursively search file contents with regex. Returns matches with file, line number, and line content. Use glob to filter files.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern" },
                    "path": { "type": "string", "description": "Directory to search (default project root)" },
                    "glob": { "type": "string", "description": "File name glob filter (e.g. *.rs)" },
                    "case_insensitive": { "type": "boolean", "default": false },
                    "max_matches": { "type": "integer", "default": 100 }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let pattern: String = serde_json::from_value(args["pattern"].clone())?;
        let path: PathBuf = args["path"].as_str()
            .map(|s| PathBuf::from(s))
            .unwrap_or_else(|| PathBuf::from("."));
        let glob_filter: Option<String> = args["glob"].as_str().map(|s| s.to_string());
        let case_insensitive = args["case_insensitive"].as_bool().unwrap_or(false);
        let max_matches = args["max_matches"].as_u64().unwrap_or(100) as usize;

        let re = if case_insensitive {
            Regex::new(&format!("(?i){}", pattern))?
        } else {
            Regex::new(&pattern)?
        };

        let glob_pattern = glob_filter.as_deref().map(|g| {
            glob::Pattern::new(g).ok()
        }).flatten();

        let skip_dirs = [".git", "target", "node_modules", ".mcoder", "dist", "build"];
        let mut matches: Vec<serde_json::Value> = Vec::new();
        let mut files_searched = 0usize;

        fn walk(
            dir: &Path,
            re: &Regex,
            glob_pattern: &Option<glob::Pattern>,
            skip_dirs: &[&str],
            matches: &mut Vec<serde_json::Value>,
            max: usize,
            files_searched: &mut usize,
        ) -> Result<()> {
            if matches.len() >= max { return Ok(()); }
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if skip_dirs.contains(&name.as_str()) { continue; }
                if path.is_dir() {
                    walk(&path, re, glob_pattern, skip_dirs, matches, max, files_searched)?;
                } else if path.is_file() {
                    if let Some(gp) = glob_pattern {
                        if !gp.matches(&name) { continue; }
                    }
                    *files_searched += 1;
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        for (i, line) in content.lines().enumerate() {
                            if re.is_match(line) {
                                matches.push(serde_json::json!({
                                    "file": path.display().to_string(),
                                    "line": i + 1,
                                    "content": line.chars().take(500).collect::<String>()
                                }));
                                if matches.len() >= max { return Ok(()); }
                            }
                        }
                    }
                }
            }
            Ok(())
        }

        walk(&path, &re, &glob_pattern, &skip_dirs, &mut matches, max_matches, &mut files_searched)?;

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "pattern": pattern,
            "path": path.display().to_string(),
            "matches": matches,
            "count": matches.len(),
            "files_searched": files_searched,
            "truncated": matches.len() >= max_matches
        }) })
    }
}
