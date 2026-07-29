// 设计文档 §4.4: hashline 模式 edit 的 forward-looking scaffolding
// Parser/LineInfo 已定义但尚未接入 EditTool 的 hashline 模式
#![allow(dead_code)]

pub mod languages;

use crate::tree_sitter::languages::Language;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone)]
pub struct FileParseResult {
    pub lines: Vec<LineInfo>,
}

#[derive(Debug, Clone)]
pub struct LineInfo {
    pub line_number: usize,
    pub hash: String,
    pub content: String,
}

pub struct Parser {
    cache: Mutex<HashMap<PathBuf, CacheEntry>>,
}

struct CacheEntry {
    mtime: std::time::SystemTime,
    result: FileParseResult,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn parse_file(&self, path: &Path) -> Result<FileParseResult> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading file: {}", path.display()))?;
        let mtime = std::fs::metadata(path)?.modified()?;

        let mut cache = self.cache.lock().unwrap();
        if let Some(entry) = cache.get(path) {
            if entry.mtime == mtime {
                return Ok(entry.result.clone());
            }
        }

        let lang = Language::from_path(path);
        let result = parse_content(&content, lang);

        cache.insert(path.to_path_buf(), CacheEntry { mtime, result: result.clone() });
        Ok(result)
    }

    pub fn get_line_hashes(&self, path: &Path) -> Result<Vec<(usize, String)>> {
        let result = self.parse_file(path)?;
        Ok(result.lines.iter().map(|l| (l.line_number, l.hash.clone())).collect())
    }

    pub fn find_hash(&self, path: &Path, hash: &str) -> Result<Option<usize>> {
        let result = self.parse_file(path)?;
        Ok(result.lines.iter().find(|l| l.hash == hash).map(|l| l.line_number))
    }

    pub fn hash_range(&self, path: &Path, start_hash: &str, end_hash: &str) -> Result<Option<(usize, usize)>> {
        let result = self.parse_file(path)?;
        let start = result.lines.iter().position(|l| l.hash == start_hash);
        let end = result.lines.iter().position(|l| l.hash == end_hash);
        match (start, end) {
            (Some(s), Some(e)) => Ok(Some((result.lines[s].line_number, result.lines[e].line_number))),
            _ => Ok(None),
        }
    }
}

fn parse_content(content: &str, _lang: Language) -> FileParseResult {
    let lines = content.lines().enumerate().map(|(i, line)| {
        LineInfo {
            line_number: i + 1,
            hash: hash_line(line),
            content: line.to_string(),
        }
    }).collect();

    FileParseResult { lines }
}

pub fn hash_line(line: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(line.as_bytes());
    let result = hasher.finalize();
    result.iter().take(8).map(|b| format!("{:02x}", b)).collect()
}

pub fn format_with_hashes(lines: &[LineInfo], start: usize, end: usize) -> String {
    lines.iter()
        .filter(|l| l.line_number >= start && l.line_number <= end)
        .map(|l| format!("  {}│{:>3}│ {}", l.hash, l.line_number, l.content))
        .collect::<Vec<_>>()
        .join("\n")
}
