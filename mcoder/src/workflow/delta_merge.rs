// delta spec 合并引擎
// 解析 delta spec 的 ADDED/MODIFIED/REMOVED 三段，合并到全局 spec
// ADDED -> 追加到全局 spec 的 ## Requirements 末尾
// MODIFIED -> 替换全局 spec 中同名 requirement（用标题匹配）
// REMOVED -> 从全局 spec 中删除同名 requirement
use anyhow::Result;
use regex::Regex;

/// markdown heading 行
#[derive(Debug, Clone)]
struct Heading {
    level: usize,
    text: String,
    /// heading 下直到下一个同级或更高级 heading 的内容
    content: String,
    /// 内容开始的行号（内部使用）
    content_start: usize,
}

/// 解析 markdown 为 heading 列表（仅顶层 level 2 和 level 3）
fn parse_headings(markdown: &str) -> Vec<Heading> {
    let heading_re = Regex::new(r"^(#{2,3})\s+(.+)$").unwrap();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut headings: Vec<Heading> = Vec::new();
    let mut current_idx: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        if let Some(cap) = heading_re.captures(line) {
            let level = cap[1].len();
            let text = cap[2].trim().to_string();

            // 结束上一个 heading 的内容收集
            if let Some(idx) = current_idx {
                let content_end = i;
                let content = lines[headings[idx].content_start..content_end]
                    .join("\n")
                    .trim()
                    .to_string();
                headings[idx].content = content;
            }

            let heading = Heading {
                level,
                text,
                content: String::new(),
                content_start: i + 1,
            };
            headings.push(heading);
            current_idx = Some(headings.len() - 1);
        }
    }

    // 结束最后一个 heading 的内容收集
    if let Some(idx) = current_idx {
        let content = lines[headings[idx].content_start..]
            .join("\n")
            .trim()
            .to_string();
        headings[idx].content = content;
    }

    headings
}

/// 从 delta spec 中提取指定段（ADDED/MODIFIED/REMOVED）的 requirement 列表
/// 返回 (标题, 内容) 对，标题为 "Requirement: X" 格式
fn extract_delta_requirements(delta_spec: &str, section_title: &str) -> Vec<(String, String)> {
    let headings = parse_headings(delta_spec);
    let mut result = Vec::new();

    // 找到 ## {section_title} 段
    let mut in_section = false;
    for heading in &headings {
        if heading.level == 2 {
            in_section = heading.text == section_title;
        } else if heading.level == 3 && in_section {
            // 段内的 ### Requirement: X
            if heading.text.starts_with("Requirement:") {
                result.push((heading.text.clone(), heading.content.clone()));
            }
        }
    }

    result
}

/// 检查 delta spec 是否包含语义段（ADDED/MODIFIED/REMOVED）
fn has_semantic_sections(delta_spec: &str) -> bool {
    delta_spec.contains("## ADDED Requirements")
        || delta_spec.contains("## MODIFIED Requirements")
        || delta_spec.contains("## REMOVED Requirements")
}

/// 从全局 spec 中提取 ## Requirements 段的起止行号
fn find_requirements_section(global_spec: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = global_spec.lines().collect();
    let section_re = Regex::new(r"^##\s+Requirements\s*$").unwrap();
    let next_section_re = Regex::new(r"^##\s+").unwrap();

    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        if section_re.is_match(line.trim()) {
            start = Some(i);
        } else if start.is_some() && next_section_re.is_match(line.trim()) {
            // 遇到下一个 ## 段，结束
            return Some((start.unwrap(), i));
        }
    }

    // Requirements 是最后一个 ## 段
    start.map(|s| (s, lines.len()))
}

/// 合并 delta spec 到全局 spec
/// 返回合并后的全局 spec 内容
pub fn merge_delta_spec(delta_spec: &str, global_spec: &str) -> Result<String> {
    // 如果 delta 不含语义段，直接返回 delta 作为完整替换
    if !has_semantic_sections(delta_spec) {
        return Ok(delta_spec.to_string());
    }

    let mut result = global_spec.to_string();

    // 1. REMOVED: 从全局 spec 中删除匹配的 requirement
    let removed = extract_delta_requirements(delta_spec, "REMOVED Requirements");
    if !removed.is_empty() {
        result = remove_requirements(&result, &removed);
    }

    // 2. MODIFIED: 替换全局 spec 中同名的 requirement
    let modified = extract_delta_requirements(delta_spec, "MODIFIED Requirements");
    if !modified.is_empty() {
        result = modify_requirements(&result, &modified);
    }

    // 3. ADDED: 追加到全局 spec 的 ## Requirements 末尾
    let added = extract_delta_requirements(delta_spec, "ADDED Requirements");
    if !added.is_empty() {
        result = add_requirements(&result, &added);
    }

    Ok(result)
}

/// 从全局 spec 中删除指定的 requirement
fn remove_requirements(global_spec: &str, removed: &[(String, String)]) -> String {
    let lines: Vec<&str> = global_spec.lines().collect();
    let heading_re = Regex::new(r"^(#{2,3})\s+(.+)$").unwrap();

    let remove_titles: Vec<&str> = removed.iter().map(|(t, _)| t.as_str()).collect();
    let mut result_lines = Vec::new();
    let mut skip_until_level = None;

    for line in &lines {
        if let Some(cap) = heading_re.captures(line.trim()) {
            let level = cap[1].len();
            let text = cap[2].trim();

            if let Some(skip_level) = skip_until_level {
                if level <= skip_level {
                    skip_until_level = None;
                }
            }

            if skip_until_level.is_none() {
                // 检查是否需要删除
                if level == 3 && remove_titles.contains(&text) {
                    skip_until_level = Some(level);
                    continue;
                }
            }
        }

        if skip_until_level.is_none() {
            result_lines.push(*line);
        }
    }

    result_lines.join("\n")
}

/// 替换全局 spec 中同名的 requirement
fn modify_requirements(global_spec: &str, modified: &[(String, String)]) -> String {
    let lines: Vec<&str> = global_spec.lines().collect();
    let heading_re = Regex::new(r"^(#{2,3})\s+(.+)$").unwrap();

    let modify_map: std::collections::HashMap<&str, &str> = modified
        .iter()
        .map(|(t, c)| (t.as_str(), c.as_str()))
        .collect();

    let mut result_lines = Vec::new();
    let mut skip_until_level = None;

    for line in &lines {
        if let Some(cap) = heading_re.captures(line.trim()) {
            let level = cap[1].len();
            let text = cap[2].trim();

            if let Some(skip_level) = skip_until_level {
                if level <= skip_level {
                    skip_until_level = None;
                }
            }

            if skip_until_level.is_none() {
                // 检查是否需要替换
                if level == 3 {
                    if let Some(new_content) = modify_map.get(text) {
                        skip_until_level = Some(level);
                        // 写入新的标题和内容
                        result_lines.push(format!("### {}", text));
                        if !new_content.is_empty() {
                            result_lines.push(String::new());
                            result_lines.push(new_content.to_string());
                        }
                        continue;
                    }
                }
            }
        }

        if skip_until_level.is_none() {
            result_lines.push((*line).to_string());
        }
    }

    result_lines.join("\n")
}

/// 追加新的 requirement 到全局 spec 的 ## Requirements 末尾
fn add_requirements(global_spec: &str, added: &[(String, String)]) -> String {
    if added.is_empty() {
        return global_spec.to_string();
    }

    // 找到 ## Requirements 段
    if let Some((_start, end)) = find_requirements_section(global_spec) {
        let lines: Vec<&str> = global_spec.lines().collect();
        let mut result_lines: Vec<String> = lines[..end].iter().map(|s| s.to_string()).collect();

        // 追加新的 requirement
        for (title, content) in added {
            result_lines.push(String::new());
            result_lines.push(format!("### {}", title));
            if !content.is_empty() {
                result_lines.push(String::new());
                result_lines.push(content.clone());
            }
        }

        // 追加 ## Requirements 段之后的内容
        for line in &lines[end..] {
            result_lines.push((*line).to_string());
        }

        result_lines.join("\n")
    } else {
        // 全局 spec 没有 ## Requirements 段，创建一个
        let mut result = global_spec.to_string();
        if !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str("\n## Requirements\n");
        for (title, content) in added {
            result.push_str(&format!("\n### {}\n", title));
            if !content.is_empty() {
                result.push_str(&format!("\n{}\n", content));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_semantic_sections() {
        assert!(has_semantic_sections("## ADDED Requirements\n"));
        assert!(has_semantic_sections("## MODIFIED Requirements\n"));
        assert!(has_semantic_sections("## REMOVED Requirements\n"));
        assert!(!has_semantic_sections("# Some other spec\n"));
    }

    #[test]
    fn test_extract_delta_requirements() {
        let delta = "## ADDED Requirements\n\n### Requirement: new-feature\ncontent here\n\n## MODIFIED Requirements\n\n### Requirement: existing\nnew content\n";
        let added = extract_delta_requirements(delta, "ADDED Requirements");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].0, "Requirement: new-feature");
        assert_eq!(added[0].1, "content here");

        let modified = extract_delta_requirements(delta, "MODIFIED Requirements");
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0].0, "Requirement: existing");
    }

    #[test]
    fn test_merge_added() {
        let global = "# Spec\n\n## Requirements\n\n### Requirement: existing\nold content\n";
        let delta = "## ADDED Requirements\n\n### Requirement: new-feature\nnew content\n";
        let merged = merge_delta_spec(delta, global).unwrap();
        assert!(merged.contains("Requirement: existing"));
        assert!(merged.contains("Requirement: new-feature"));
        assert!(merged.contains("new content"));
    }

    #[test]
    fn test_merge_modified() {
        let global = "# Spec\n\n## Requirements\n\n### Requirement: feature\nold content\n";
        let delta = "## MODIFIED Requirements\n\n### Requirement: feature\nupdated content\n";
        let merged = merge_delta_spec(delta, global).unwrap();
        assert!(merged.contains("Requirement: feature"));
        assert!(merged.contains("updated content"));
        assert!(!merged.contains("old content"));
    }

    #[test]
    fn test_merge_removed() {
        let global = "# Spec\n\n## Requirements\n\n### Requirement: feature\ncontent\n\n### Requirement: other\nkeep\n";
        let delta = "## REMOVED Requirements\n\n### Requirement: feature\ncontent\n";
        let merged = merge_delta_spec(delta, global).unwrap();
        assert!(!merged.contains("Requirement: feature"));
        assert!(merged.contains("Requirement: other"));
    }

    #[test]
    fn test_merge_non_semantic_full_replace() {
        let global = "# Old Spec\n";
        let delta = "# New Spec\n";
        let merged = merge_delta_spec(delta, global).unwrap();
        assert_eq!(merged, "# New Spec\n");
    }
}
