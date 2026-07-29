// 编号体系 + 变更图谱的可追溯性验证
// 验证 PR -> DS -> T -> spec 的引用链
// 解析 markdown 中的 refs: PR-{id} / refs: DS-{id} / spec_ref: SHALL-{id} 字段
use anyhow::Result;
use regex::Regex;
use std::collections::HashSet;
use std::path::Path;

/// 可追溯性验证报告
#[derive(Debug, Clone, Default)]
pub struct TraceReport {
    /// 孤儿编号：定义了但未被任何下游引用
    pub orphans: Vec<String>,
    /// 缺失引用：引用了不存在的编号
    pub missing_refs: Vec<String>,
}

impl TraceReport {
    pub fn is_clean(&self) -> bool {
        self.orphans.is_empty() && self.missing_refs.is_empty()
    }
}

/// 已定义的编号集合（按前缀分组）
#[derive(Debug, Default)]
struct DefinedIds {
    pr: HashSet<String>,  // PR-N
    ds: HashSet<String>,  // DS-N
    d: HashSet<String>,   // D-N
    t: HashSet<String>,   // T-N
    shall: HashSet<String>, // SHALL-N
    rv: HashSet<String>,  // RV-N
}

/// 从 markdown 文本中提取所有编号定义
/// 识别 ### PR-N: / ### DS-N: / ### D-N: / T-N: / SHALL-N / RV-N 模式
fn extract_defined_ids(content: &str) -> DefinedIds {
    let mut ids = DefinedIds::default();

    // 匹配标题行或列表项中的编号定义
    // PR-N: 出现在 ### PR-N: 或 - [x] T-N: 等
    let pr_re = Regex::new(r"\bPR-(\d+)\b").unwrap();
    let ds_re = Regex::new(r"\bDS-(\d+)\b").unwrap();
    let d_re = Regex::new(r"\bD-(\d+)\b").unwrap();
    let t_re = Regex::new(r"\bT-(\d+)\b").unwrap();
    let shall_re = Regex::new(r"\bSHALL-(\d+)\b").unwrap();
    let rv_re = Regex::new(r"\bRV-(\d+)\b").unwrap();

    for cap in pr_re.captures_iter(content) {
        ids.pr.insert(format!("PR-{}", &cap[1]));
    }
    for cap in ds_re.captures_iter(content) {
        ids.ds.insert(format!("DS-{}", &cap[1]));
    }
    // D-N 需要避免和 DS-N 冲突，但正则 \bD-(\d+)\b 不会匹配 DS-1
    // 因为 DS-1 中 D 后面跟 S，不匹配 D-\d
    for cap in d_re.captures_iter(content) {
        ids.d.insert(format!("D-{}", &cap[1]));
    }
    for cap in t_re.captures_iter(content) {
        ids.t.insert(format!("T-{}", &cap[1]));
    }
    for cap in shall_re.captures_iter(content) {
        ids.shall.insert(format!("SHALL-{}", &cap[1]));
    }
    for cap in rv_re.captures_iter(content) {
        ids.rv.insert(format!("RV-{}", &cap[1]));
    }

    ids
}

/// 从 markdown 文本中提取所有引用（refs: 和 spec_ref: 字段）
/// refs: PR-{id} / refs: DS-{id} / spec_ref: SHALL-{id}
fn extract_references(content: &str) -> Vec<String> {
    let mut refs = Vec::new();

    // 匹配 **refs**: X-N, Y-N 或 refs: X-N
    let refs_re = Regex::new(r"(?i)\*?\*?refs\*?\*?:\s*(.+)").unwrap();
    let spec_ref_re = Regex::new(r"(?i)spec_ref\*?\*?:\s*(.+)").unwrap();

    // 提取 refs 字段中的所有编号
    for cap in refs_re.captures_iter(content) {
        let field_value = &cap[1];
        // 从字段值中提取所有 X-N 格式的编号
        let id_re = Regex::new(r"\b(PR|DS|D|T|SHALL|RV)-(\d+)\b").unwrap();
        for id_cap in id_re.captures_iter(field_value) {
            refs.push(format!("{}-{}", &id_cap[1], &id_cap[2]));
        }
    }

    // 提取 spec_ref 字段中的 SHALL-N 引用
    for cap in spec_ref_re.captures_iter(content) {
        let field_value = &cap[1];
        // spec_ref 可能是 specs/domain/spec.md#requirement-name 格式
        // 也可能是 SHALL-N 格式
        let shall_re = Regex::new(r"\bSHALL-(\d+)\b").unwrap();
        for shall_cap in shall_re.captures_iter(field_value) {
            refs.push(format!("SHALL-{}", &shall_cap[1]));
        }
    }

    refs
}

/// 读取文件内容，不存在则返回空字符串
fn read_file_optional(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// 验证 change 目录的可追溯性
/// 检查 PR -> DS -> T -> spec 的引用链
pub fn verify_traceability(change_dir: &Path) -> Result<TraceReport> {
    let proposal_content = read_file_optional(&change_dir.join("proposal.md"));
    let design_content = read_file_optional(&change_dir.join("design.md"));
    let tasks_content = read_file_optional(&change_dir.join("tasks.md"));

    // 收集各 artifact 中定义的编号
    let proposal_ids = extract_defined_ids(&proposal_content);
    let design_ids = extract_defined_ids(&design_content);
    let task_ids = extract_defined_ids(&tasks_content);

    // 合并所有已定义编号
    let mut all_defined = DefinedIds::default();
    for id in &proposal_ids.pr {
        all_defined.pr.insert(id.clone());
    }
    for id in &design_ids.ds {
        all_defined.ds.insert(id.clone());
    }
    for id in &design_ids.d {
        all_defined.d.insert(id.clone());
    }
    for id in &task_ids.t {
        all_defined.t.insert(id.clone());
    }

    // 收集所有引用
    let design_refs = extract_references(&design_content);
    let task_refs = extract_references(&tasks_content);

    let mut all_refs: Vec<String> = Vec::new();
    all_refs.extend(design_refs);
    all_refs.extend(task_refs);

    let mut report = TraceReport::default();

    // 检查缺失引用：引用了不存在的编号
    for refer in &all_refs {
        let exists = if refer.starts_with("PR-") {
            all_defined.pr.contains(refer)
        } else if refer.starts_with("DS-") {
            all_defined.ds.contains(refer)
        } else if refer.starts_with("D-") {
            all_defined.d.contains(refer)
        } else if refer.starts_with("T-") {
            all_defined.t.contains(refer)
        } else if refer.starts_with("SHALL-") {
            all_defined.shall.contains(refer)
        } else if refer.starts_with("RV-") {
            all_defined.rv.contains(refer)
        } else {
            true
        };

        if !exists {
            let msg = format!("引用 {} 未找到对应定义", refer);
            if !report.missing_refs.contains(&msg) {
                report.missing_refs.push(msg);
            }
        }
    }

    // 检查孤儿编号：定义了但未被下游引用
    // DS 应该引用 PR，T 应该引用 DS
    let all_refs_set: HashSet<&str> = all_refs.iter().map(|s| s.as_str()).collect();

    for pr in &proposal_ids.pr {
        if !all_refs_set.contains(pr.as_str()) {
            report.orphans.push(format!("{} 定义但未被 design/task 引用", pr));
        }
    }
    for ds in &design_ids.ds {
        if !all_refs_set.contains(ds.as_str()) {
            report.orphans.push(format!("{} 定义但未被 task 引用", ds));
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_defined_ids() {
        let content = "### PR-1: first deliverable\n### PR-2: second deliverable\n";
        let ids = extract_defined_ids(content);
        assert!(ids.pr.contains("PR-1"));
        assert!(ids.pr.contains("PR-2"));
    }

    #[test]
    fn test_extract_references() {
        let content = "- **refs**: DS-1\n- **spec_ref**: SHALL-2\n";
        let refs = extract_references(content);
        assert!(refs.contains(&"DS-1".to_string()));
        assert!(refs.contains(&"SHALL-2".to_string()));
    }

    #[test]
    fn test_d_n_does_not_match_ds_n() {
        let content = "### DS-1: design\n### D-1: decision\n";
        let ids = extract_defined_ids(content);
        assert!(ids.ds.contains("DS-1"));
        assert!(ids.d.contains("D-1"));
        assert!(!ids.d.contains("DS-1"));
    }
}
