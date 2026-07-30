// 设计文档 §4.4: EditOpResult::Success.affected_lines 为 forward-looking 字段
// 当前 diff_preview 已足够；affected_lines 保留供未来精细 UI 展示
#![allow(dead_code)]

use crate::tools::sandbox::SandboxStore;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use calamine::{DataType as _, Reader as _};
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn hash_line(line: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(line.as_bytes());
    let result = hasher.finalize();
    result.iter().take(8).map(|b| format!("{:02x}", b)).collect()
}

// ==================== read 工具（通用内容读取：文本/URL/目录/压缩包/Excel/Word/PPT/PDF/HTML/图片）===================

/// read 工具：通用读取入口，自动检测内容类型并分发；通过 action 选择 sandbox 模式
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
            description: "Universal read tool. Auto-detects content type (text/url/directory/archive/excel/word/ppt/pdf/html/image/gzipped). Use action='more'/'full'/'original' with a handle for paged access to large content. Use 'format' to force a specific format.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path, directory path, or URL (http/https)" },
                    "action": {
                        "type": "string",
                        "enum": ["default", "more", "full", "original"],
                        "description": "Sandbox mode. When omitted, auto-detect content type. 'more'/'full'/'original' require a handle."
                    },
                    "handle": { "type": "string", "description": "[more/full/original] Sandbox handle" },
                    "offset": { "type": "integer", "description": "[more] Line offset (0-indexed), default 0" },
                    "limit": { "type": "integer", "description": "[more] Max lines. Also limits Excel rows per sheet (default 100)" },
                    "start": { "type": "integer", "description": "[default] Start line (1-indexed), default 1" },
                    "end": { "type": "integer", "description": "[default] End line (inclusive)" },
                    "with_hashes": { "type": "boolean", "default": true, "description": "Include line hashes" },
                    "entry": { "type": "string", "description": "Path inside archive (for zip/tar)" },
                    "depth": { "type": "integer", "default": 2, "description": "Directory recursion depth" },
                    "format": {
                        "type": "string",
                        "enum": ["text", "url", "excel", "word", "ppt", "pdf", "image", "html", "archive", "directory"],
                        "description": "Force a specific format if auto-detection fails"
                    },
                    "prompt": { "type": "string", "description": "[image] Custom question to ask the vision model; defaults to 'Describe this image concisely.'" }
                }
            }),
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args.get("action").and_then(|v| v.as_str());
        match action {
            Some("more") => Self::read_more(args, ctx),
            Some("full") => Self::read_full(args, ctx),
            Some("original") => Self::read_original(args, ctx),
            Some("default") | None => Self::read_auto(args, ctx).await,
            Some(other) => anyhow::bail!("unknown action '{}': expected default|more|full|original", other),
        }
    }
}

impl ReadTool {
    /// action omitted or "default": 自动检测内容类型并读取
    async fn read_auto(args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path_str: String = serde_json::from_value(args["path"].clone())
            .or_else(|_| serde_json::from_value(args["file"].clone()))
            .context("path required")?;
        let start = args["start"].as_u64().or_else(|| args["offset"].as_u64()).unwrap_or(1) as usize;
        let end = args["end"].as_u64().map(|n| n as usize);
        let with_hashes = args["with_hashes"].as_bool().unwrap_or(true);
        let limit = args["limit"].as_u64().map(|n| n as usize);
        let depth = args["depth"].as_u64().unwrap_or(2) as usize;
        let entry: Option<String> = args["entry"].as_str().map(|s| s.to_string());
        let format: Option<String> = args["format"].as_str().map(|s| s.to_string());
        let prompt: Option<String> = args["prompt"].as_str().map(|s| s.to_string());

        // 1. URL
        if path_str.starts_with("http://") || path_str.starts_with("https://") {
            return Self::read_url(&path_str, start, end, with_hashes, ctx).await;
        }

        let path = PathBuf::from(&path_str);

        // 2. Directory
        if path.is_dir() {
            return Self::read_directory(&path, depth, ctx);
        }
        if !path.exists() {
            anyhow::bail!("path does not exist: {}", path.display());
        }

        // 3. 强制 format 或自动检测
        let fmt = format.unwrap_or_else(|| detect_format(&path).to_string());

        match fmt.as_str() {
            "text" => Self::read_text_file(&path, &path_str, start, end, with_hashes, ctx),
            "url" => Self::read_url(&path_str, start, end, with_hashes, ctx).await,
            "excel" => Self::read_excel(&path, &path_str, limit, start, end, with_hashes, ctx),
            "word" => Self::read_word(&path, &path_str, start, end, with_hashes, ctx),
            "ppt" => Self::read_ppt(&path, &path_str, start, end, with_hashes, ctx),
            "pdf" => Self::read_pdf(&path, &path_str, start, end, with_hashes, ctx),
            "image" => Self::read_image(&path, prompt.as_deref(), ctx).await,
            "html" => Self::read_html_file(&path, &path_str, start, end, with_hashes, ctx),
            "archive" => Self::read_archive(&path, &path_str, entry.as_deref(), start, end, with_hashes, ctx),
            "directory" => Self::read_directory(&path, depth, ctx),
            "gzipped" => Self::read_gzipped(&path, &path_str, start, end, with_hashes, ctx),
            other => anyhow::bail!("unknown format: {}", other),
        }
    }

    /// 通用分页：对已读取的文本内容应用 hash 前缀 + 截断逻辑（原 read_default 核心）
    fn paginate_content(
        content: &str,
        path_str: &str,
        start: usize,
        end: Option<usize>,
        with_hashes: bool,
        project_dir: &PathBuf,
    ) -> Result<ToolOutput> {
        let all_lines: Vec<&str> = content.lines().collect();
        let total = all_lines.len();

        let s = start.max(1);
        let e = end.unwrap_or(total).min(total);
        if s > total {
            return Ok(ToolOutput::Sync { result: serde_json::json!({
                "file": path_str,
                "content": "",
                "note": "start beyond file end"
            }) });
        }
        let range: &[&str] = &all_lines[s-1..e];

        // 检查长行：任一行 > 500 字符 -> 全量存 sandbox，返回折行摘要
        let has_long_line = range.iter().any(|l| l.chars().count() > READ_LONG_LINE_THRESHOLD);
        if has_long_line {
            let full: String = range.iter().enumerate().map(|(i, l)| {
                let ln = s + i;
                if with_hashes {
                    format!("{}|{:>4}| {}", hash_line(l), ln, l)
                } else {
                    format!("{:>4}| {}", ln, l)
                }
            }).collect::<Vec<_>>().join("\n");
            let handle = SandboxStore::store(project_dir, &full)?;
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
                    wrapped.push(format!("{}| {}", h, l));
                    total_wrapped_lines += 1;
                } else {
                    for (idx, chunk) in chars.chunks(WRAP_WIDTH).enumerate() {
                        if total_wrapped_lines >= MAX_SUMMARY_LINES {
                            wrapped.push(format!("... (more lines omitted, see handle)"));
                            break;
                        }
                        let chunk_str: String = chunk.iter().collect();
                        if idx == 0 {
                            wrapped.push(format!("{}| {}", h, chunk_str));
                        } else {
                            wrapped.push(format!("    > {}", chunk_str));
                        }
                        total_wrapped_lines += 1;
                    }
                }
            }
            return Ok(ToolOutput::Sync { result: serde_json::json!({
                "file": path_str,
                "start_line": s,
                "end_line": e,
                "total_lines": total,
                "content": wrapped.join("\n"),
                "handle": handle,
                "truncated": true,
                "reason": "long_line_wrapped",
                "hint": "Use read action=more/full with handle for full content."
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
                    format!("{}|{:>4}| {}", hash_line(l), ln, l)
                } else {
                    format!("{:>4}| {}", ln, l)
                }
            }).collect::<Vec<_>>().join("\n");
            let handle = SandboxStore::store(project_dir, &full)?;

            let mut out = format!("{}\n... ({} lines omitted, handle={})\n{}",
                head.join("\n"), middle_count, handle, tail.join("\n"));
            if !with_hashes { out = strip_hashes(&out); }

            return Ok(ToolOutput::Sync { result: serde_json::json!({
                "file": path_str,
                "start_line": s,
                "end_line": e,
                "total_lines": total,
                "content": out,
                "handle": handle,
                "truncated": true,
                "hint": "Use read action=more or action=full with handle for omitted lines."
            }) });
        }

        // 小范围：全返回
        let out: String = range.iter().enumerate().map(|(i, l)| {
            let ln = s + i;
            if with_hashes {
                format!("{}|{:>4}| {}", hash_line(l), ln, l)
            } else {
                format!("{:>4}| {}", ln, l)
            }
        }).collect::<Vec<_>>().join("\n");

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "file": path_str,
            "start_line": s,
            "end_line": e,
            "total_lines": total,
            "content": out,
            "truncated": false
        }) })
    }

    /// 读取纯文本文件（含二进制检查）
    fn read_text_file(
        path: &Path,
        path_str: &str,
        start: usize,
        end: Option<usize>,
        with_hashes: bool,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        // 二进制检查：读前 1KB，含 NULL 字节则视为二进制
        if let Ok(mut f) = std::fs::File::open(path) {
            let mut buf = [0u8; 1024];
            let n = f.read(&mut buf).unwrap_or(0);
            if buf[..n].contains(&0u8) {
                anyhow::bail!("binary file, cannot read: {}", path.display());
            }
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::paginate_content(&content, path_str, start, end, with_hashes, &ctx.project_dir)
    }

    /// 读取 URL：抓取 HTML 并转为文本
    async fn read_url(
        url: &str,
        start: usize,
        end: Option<usize>,
        with_hashes: bool,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let resp = reqwest::Client::new()
            .get(url)
            .send()
            .await
            .with_context(|| format!("fetching {}", url))?;
        let html = resp.text().await
            .with_context(|| format!("reading body from {}", url))?;
        // 提取 <title>
        let title = Regex::new(r"(?is)<title[^>]*>(.*?)</title>")
            .ok()
            .and_then(|re| re.captures(&html))
            .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
            .unwrap_or_default();
        // HTML -> 文本（html2text 会忽略 script/style 内容）
        let content = html2text::from_read(html.as_bytes(), 120)
            .map_err(|e| anyhow::anyhow!("html2text: {}", e))?;
        let paginated = Self::paginate_content(&content, url, start, end, with_hashes, &ctx.project_dir)?;
        if let ToolOutput::Sync { result } = paginated {
            let mut obj = result.as_object().cloned().unwrap_or_default();
            obj.insert("source".into(), serde_json::json!(url));
            obj.insert("title".into(), serde_json::json!(title));
            return Ok(ToolOutput::Sync { result: serde_json::Value::Object(obj) });
        }
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "source": url,
            "title": title,
            "content": content
        }) })
    }

    /// 读取目录：递归列出树形结构
    fn read_directory(path: &Path, depth: usize, _ctx: &ToolContext) -> Result<ToolOutput> {
        let mut tree = String::new();
        let count = Self::build_dir_tree(path, &mut tree, "", 0, depth)?;
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "path": path.display().to_string(),
            "type": "directory",
            "content": tree,
            "file_count": count
        }) })
    }

    /// 递归构建目录树
    fn build_dir_tree(
        dir: &Path,
        out: &mut String,
        prefix: &str,
        level: usize,
        max_depth: usize,
    ) -> Result<usize> {
        let mut entries: Vec<(String, std::fs::Metadata)> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') { continue; }
            let meta = entry.metadata()?;
            entries.push((name, meta));
        }
        // 排序：目录在前，文件在后，字母序
        entries.sort_by(|a, b| {
            let ad = a.1.is_dir();
            let bd = b.1.is_dir();
            match (ad, bd) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.0.cmp(&b.0),
            }
        });

        let mut count = 0usize;
        let total = entries.len();
        for (i, (name, meta)) in entries.iter().enumerate() {
            let is_dir = meta.is_dir();
            let last = i == total - 1;
            let connector = if last { "`-- " } else { "|-- " };
            let size_str = if is_dir {
                String::new()
            } else {
                format!(" ({})", format_size(meta.len()))
            };
            out.push_str(&format!("{}{}{}{}\n", prefix, connector, name, size_str));
            count += 1;
            if is_dir && level + 1 < max_depth {
                let new_prefix = if last {
                    format!("{}    ", prefix)
                } else {
                    format!("{}|   ", prefix)
                };
                count += Self::build_dir_tree(
                    &dir.join(name),
                    out,
                    &new_prefix,
                    level + 1,
                    max_depth,
                )?;
            }
        }
        Ok(count)
    }

    /// 读取压缩包：列出内容或提取指定 entry
    fn read_archive(
        path: &Path,
        path_str: &str,
        entry: Option<&str>,
        start: usize,
        end: Option<usize>,
        with_hashes: bool,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let lower = path_str.to_lowercase();
        let is_tar = lower.ends_with(".tar") || lower.ends_with(".tar.gz") || lower.ends_with(".tgz");

        if let Some(entry_path) = entry {
            let content = if is_tar {
                Self::extract_tar_entry(path, entry_path, path_str)?
            } else {
                Self::extract_zip_entry(path, entry_path)?
            };
            if content.as_bytes().contains(&0u8) {
                anyhow::bail!("binary entry, cannot read: {}", entry_path);
            }
            Self::paginate_content(&content, entry_path, start, end, with_hashes, &ctx.project_dir)
        } else {
            let listing = if is_tar {
                Self::list_tar(path, path_str)?
            } else {
                Self::list_zip(path)?
            };
            Ok(ToolOutput::Sync { result: serde_json::json!({
                "path": path_str,
                "type": "archive",
                "content": listing
            }) })
        }
    }

    fn list_zip(path: &Path) -> Result<String> {
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let mut out = String::new();
        for i in 0..archive.len() {
            let entry = archive.by_index(i)?;
            let name = entry.name().to_string();
            let size = entry.size();
            let marker = if name.ends_with('/') { "/" } else { "" };
            out.push_str(&format!("{}{} ({})\n", name, marker, format_size(size)));
        }
        Ok(out)
    }

    fn list_tar(path: &Path, path_str: &str) -> Result<String> {
        let file = std::fs::File::open(path)?;
        let lower = path_str.to_lowercase();
        let reader: Box<dyn Read> = if lower.ends_with(".gz") || lower.ends_with(".tgz") {
            Box::new(flate2::read::GzDecoder::new(file))
        } else {
            Box::new(file)
        };
        let mut tar = tar::Archive::new(reader);
        let mut out = String::new();
        for entry in tar.entries()? {
            let entry = entry?;
            let name = entry.path()?.display().to_string();
            let size = entry.size();
            let is_dir = entry.header().entry_type().is_dir();
            let marker = if is_dir { "/" } else { "" };
            out.push_str(&format!("{}{} ({})\n", name, marker, format_size(size)));
        }
        Ok(out)
    }

    fn extract_zip_entry(path: &Path, entry: &str) -> Result<String> {
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let mut e = archive.by_name(entry)
            .with_context(|| format!("entry not found in archive: {}", entry))?;
        let mut buf = String::new();
        e.read_to_string(&mut buf)?;
        Ok(buf)
    }

    fn extract_tar_entry(path: &Path, entry: &str, path_str: &str) -> Result<String> {
        let file = std::fs::File::open(path)?;
        let lower = path_str.to_lowercase();
        let reader: Box<dyn Read> = if lower.ends_with(".gz") || lower.ends_with(".tgz") {
            Box::new(flate2::read::GzDecoder::new(file))
        } else {
            Box::new(file)
        };
        let mut tar = tar::Archive::new(reader);
        for e in tar.entries()? {
            let mut e = e?;
            if e.path()?.display().to_string() == entry {
                let mut buf = String::new();
                e.read_to_string(&mut buf)?;
                return Ok(buf);
            }
        }
        anyhow::bail!("entry not found in archive: {}", entry)
    }

    /// 读取 Excel：每个 sheet 输出前 N 行 Markdown 表格
    fn read_excel(
        path: &Path,
        path_str: &str,
        limit: Option<usize>,
        start: usize,
        end: Option<usize>,
        with_hashes: bool,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let max_rows = limit.unwrap_or(100);
        let mut workbook = calamine::open_workbook_auto(path)
            .with_context(|| format!("opening excel {}", path.display()))?;
        let sheets = workbook.sheet_names().to_vec();
        let mut out = String::new();
        for sheet in &sheets {
            out.push_str(&format!("## {}\n\n", sheet));
            if let Ok(range) = workbook.worksheet_range(sheet) {
                let rows: Vec<_> = range.rows().collect();
                for (ri, row) in rows.iter().enumerate() {
                    if ri == 0 {
                        out.push_str("| ");
                        out.push_str(&row.iter().map(|c| cell_to_str(c)).collect::<Vec<_>>().join(" | "));
                        out.push_str(" |\n");
                        out.push_str(&format!("|{}|\n", "|".repeat(row.len().max(1))));
                    } else {
                        if ri > max_rows {
                            out.push_str(&format!("\n... ({} more rows omitted)\n", rows.len() - 1 - max_rows));
                            break;
                        }
                        out.push_str("| ");
                        out.push_str(&row.iter().map(|c| cell_to_str(c)).collect::<Vec<_>>().join(" | "));
                        out.push_str(" |\n");
                    }
                }
            }
            out.push('\n');
        }
        Self::paginate_content(&out, path_str, start, end, with_hashes, &ctx.project_dir)
    }

    /// 读取 Word (.docx)：从 word/document.xml 提取文本
    fn read_word(
        path: &Path,
        path_str: &str,
        start: usize,
        end: Option<usize>,
        with_hashes: bool,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let mut xml = String::new();
        archive.by_name("word/document.xml")?
            .read_to_string(&mut xml)
            .with_context(|| "reading word/document.xml")?;
        let text = extract_docx_text(&xml);
        Self::paginate_content(&text, path_str, start, end, with_hashes, &ctx.project_dir)
    }

    /// 读取 PPT (.pptx)：提取每张幻灯片的文本
    fn read_ppt(
        path: &Path,
        path_str: &str,
        start: usize,
        end: Option<usize>,
        with_hashes: bool,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let mut slides: Vec<(u32, String)> = Vec::new();
        for i in 0..archive.len() {
            let entry = archive.by_index(i)?;
            let name = entry.name().to_string();
            if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                let num: u32 = name.trim_start_matches("ppt/slides/slide")
                    .trim_end_matches(".xml")
                    .parse()
                    .unwrap_or(0);
                slides.push((num, name));
            }
        }
        slides.sort_by_key(|(n, _)| *n);
        let mut out = String::new();
        for (i, (_, name)) in slides.iter().enumerate() {
            let mut xml = String::new();
            archive.by_name(name)?.read_to_string(&mut xml)?;
            let text = extract_pptx_text(&xml);
            out.push_str(&format!("## Slide {}\n{}\n\n", i + 1, text));
        }
        Self::paginate_content(&out, path_str, start, end, with_hashes, &ctx.project_dir)
    }

    /// 读取 PDF
    fn read_pdf(
        path: &Path,
        path_str: &str,
        start: usize,
        end: Option<usize>,
        with_hashes: bool,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let text = pdf_extract::extract_text(path)
            .map_err(|e| anyhow::anyhow!("PDF extract failed for {}: {}", path.display(), e))?;
        Self::paginate_content(&text, path_str, start, end, with_hashes, &ctx.project_dir)
    }

    /// 读取 HTML 文件并转为文本
    fn read_html_file(
        path: &Path,
        path_str: &str,
        start: usize,
        end: Option<usize>,
        with_hashes: bool,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let html = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let content = html2text::from_read(html.as_bytes(), 120)
            .map_err(|e| anyhow::anyhow!("html2text: {}", e))?;
        Self::paginate_content(&content, path_str, start, end, with_hashes, &ctx.project_dir)
    }

    /// 读取图片：base64 编码 + 视觉模型描述（若有）
    /// 返回结构：
    ///   {type: "image", path, media_type, width, height, size_bytes, description}
    /// description 由 ctx.app_config 中找到的视觉模型异步生成（超时由 app_config.image_description_timeout_secs 决定）；失败/无视觉模型时为 null
    /// `prompt` 为可选自定义问题；为 None 时使用默认提示
    async fn read_image(path: &Path, prompt: Option<&str>, ctx: &ToolContext) -> Result<ToolOutput> {
        let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

        // 读取字节并解析尺寸
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading image bytes {}", path.display()))?;
        // bytes 用于 image::io::Reader 解析尺寸，之后不需要保留
        drop(bytes);

        let (width, height) = image::io::Reader::open(path)
            .ok()
            .and_then(|r| r.into_dimensions().ok())
            .unwrap_or((0, 0));

        // 推断 media_type
        let media_type = infer_image_media_type(&path.display().to_string());

        // 准备 JSON 对象（description 后续填充）
        // 注：不构造 data_url，因为 ContentBlock::Image 只有 path + media_type，
        // 各 LLM adapter 会自己读文件再 base64 编码；data_url 是冗余的 MB 级内存浪费。
        let mut result = serde_json::json!({
            "type": "image",
            "path": path.display().to_string(),
            "media_type": media_type,
            "width": width,
            "height": height,
            "size_bytes": size_bytes,
            "description": serde_json::Value::Null,
        });

        // 仅在主模型不支持图片输入时才调用视觉模型生成描述。
        // 主模型支持图片时，ContentBlock::Image 直接发给 LLM 看图，
        // description 冗余且会浪费 token + 延迟；直接返回 null。
        if !ctx.current_model.supports_image() {
            if let Some(description) = Self::try_describe_image(path, media_type, prompt, ctx).await {
                if let Some(obj) = result.as_object_mut() {
                    obj.insert("description".to_string(), serde_json::Value::String(description));
                }
            }
        }

        Ok(ToolOutput::Sync { result })
    }

    /// 用 ctx.app_config 中找到的视觉模型描述图片
    /// 超时由 `app_config.image_description_timeout_secs` 控制（默认 8 秒）
    /// `prompt` 为自定义问题；为 None 时使用默认提示
    /// 失败/超时/无视觉模型：返回 None
    async fn try_describe_image(
        path: &Path,
        media_type: &str,
        prompt: Option<&str>,
        ctx: &ToolContext,
    ) -> Option<String> {
        use crate::llm::create_adapter;
        use crate::tools::image::find_vision_model;
        use crate::types::{ContentBlock, Message as McoderMessage, Role};

        // m13: 复用 image 工具的 find_vision_model（vision role 优先，否则任意 supports_image）
        let vision_model = find_vision_model(&ctx.app_config)?;
        let path_str = path.display().to_string();

        let llm = match create_adapter(&vision_model) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("failed to create vision adapter: {}", e);
                return None;
            }
        };

        // m10: 系统提示设角色，用户消息直接发图片（无重复文本）
        let system_msg = McoderMessage::system(
            "You are a vision assistant. Describe images concisely."
        );
        // m11: 调用方传入的 prompt 优先；否则用默认
        let user_text = prompt.unwrap_or("Describe this image concisely.");
        let user_msg = McoderMessage::new(Role::User, vec![
            ContentBlock::Text {
                text: user_text.to_string(),
            },
            ContentBlock::Image {
                path: path_str.clone(),
                media_type: media_type.to_string(),
            },
        ]);
        let messages = vec![system_msg, user_msg];

        // m9: 超时从 app_config 读；默认 8 秒
        let timeout_secs = ctx.app_config.image_description_timeout_secs;
        let fut = async move {
            llm.chat(&messages, &[], &vision_model).await
        };
        let resp = match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            fut,
        ).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::warn!("vision model call failed for {}: {}", path_str, e);
                return None;
            }
            Err(_) => {
                tracing::warn!("vision model call timed out for {}", path_str);
                return None;
            }
        };

        let description = resp.content.unwrap_or_default();
        if description.trim().is_empty() {
            None
        } else {
            Some(description)
        }
    }

    /// 读取 gzip 压缩文本
    fn read_gzipped(
        path: &Path,
        path_str: &str,
        start: usize,
        end: Option<usize>,
        with_hashes: bool,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let file = std::fs::File::open(path)?;
        let mut decoder = flate2::read::GzDecoder::new(file);
        let mut content = String::new();
        decoder.read_to_string(&mut content)
            .with_context(|| format!("decompressing {}", path.display()))?;
        Self::paginate_content(&content, path_str, start, end, with_hashes, &ctx.project_dir)
    }
}

/// 根据扩展名检测内容格式
fn detect_format(path: &Path) -> &'static str {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).unwrap_or_default();
    if name.ends_with(".tar.gz") || ext == "tgz" || ext == "tar" {
        "archive"
    } else if ext == "gz" {
        "gzipped"
    } else if ext == "zip" {
        "archive"
    } else if ext == "xlsx" || ext == "xls" {
        "excel"
    } else if ext == "docx" {
        "word"
    } else if ext == "pptx" {
        "ppt"
    } else if ext == "pdf" {
        "pdf"
    } else if ext == "html" || ext == "htm" {
        "html"
    } else if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") {
        "image"
    } else {
        "text"
    }
}

/// 格式化文件大小
fn format_size(bytes: u64) -> String {
    if bytes < 1024 { format!("{}B", bytes) }
    else if bytes < 1024 * 1024 { format!("{:.1}KB", bytes as f64 / 1024.0) }
    else if bytes < 1024 * 1024 * 1024 { format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0)) }
    else { format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0)) }
}

/// 根据文件路径推断图片的 MIME 类型
fn infer_image_media_type(path_str: &str) -> &'static str {
    let lower = path_str.to_lowercase();
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

/// Excel 单元格转字符串（用访问器避免依赖具体枚举变体）
fn cell_to_str(cell: &calamine::Data) -> String {
    if cell.is_empty() { return String::new(); }
    if let Some(s) = cell.as_string() { return s.to_string(); }
    if let Some(f) = cell.as_f64() {
        if f.fract() == 0.0 { return format!("{}", f as i64); }
        return f.to_string();
    }
    format!("{:?}", cell)
}

/// 从 docx 的 word/document.xml 提取纯文本（<w:t> 文本，<w:p> 段落换行）
fn extract_docx_text(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut out = String::new();
    let mut in_t = false;
    let mut paragraph_text = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                match e.name().as_ref() {
                    b"w:p" => { paragraph_text.clear(); }
                    b"w:t" => { in_t = true; }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                match e.name().as_ref() {
                    b"w:p" => {
                        if !paragraph_text.is_empty() {
                            out.push_str(&paragraph_text);
                            out.push('\n');
                            paragraph_text.clear();
                        }
                    }
                    b"w:t" => { in_t = false; }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if in_t {
                    if let Ok(t) = e.unescape() {
                        paragraph_text.push_str(&t);
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                if e.name().as_ref() == b"w:p" {
                    out.push('\n');
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// 从 pptx 的 slideN.xml 提取文本（<a:t> 元素，每个元素后换行）
fn extract_pptx_text(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut out = String::new();
    let mut in_t = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == b"a:t" { in_t = true; }
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == b"a:t" {
                    in_t = false;
                    out.push('\n');
                }
            }
            Ok(Event::Text(e)) => {
                if in_t {
                    if let Ok(t) = e.unescape() {
                        out.push_str(&t);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

impl ReadTool {
    /// action="default": 读文件，返回带 hash 前缀的行（原 ReadTool 逻辑）
    async fn read_default(args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
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
                "hint": "Use read action=more/full with handle for full content."
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
                "hint": "Use read action=more or action=full with handle for omitted lines."
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

    /// action="more": 按 handle + offset/limit 分页读取（原 ReadMoreTool 逻辑）
    fn read_more(args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
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

    /// action="full": 返回 handle 对应的完整内容（原 ReadFullTool 逻辑）
    fn read_full(args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let handle: String = serde_json::from_value(args["handle"].clone())?;
        let content = SandboxStore::read(&ctx.project_dir, &handle)?.unwrap_or_default();
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "handle": handle,
            "content": content,
            "bytes": content.len()
        }) })
    }

    /// action="original": 获取摘要对应的原文（原 ReadOriginalTool 逻辑）
    fn read_original(args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let handle: String = serde_json::from_value(args["handle"].clone())?;
        let content = SandboxStore::read(&ctx.project_dir, &handle)?.unwrap_or_default();
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "handle": handle,
            "original": content,
            "bytes": content.len()
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
