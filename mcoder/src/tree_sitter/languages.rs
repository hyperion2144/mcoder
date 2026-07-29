use std::path::Path;

/// P2-7: 扩展语言支持（设计文档 §8.4: "扩展到 20+ 语言"）
/// 原 5 种 + 新增 9 种 = 14 种
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Go,
    // P2-7: 新增语言
    C,
    Cpp,
    Java,
    Ruby,
    CSharp,
    Bash,
    Json,
    Css,
    Html,
    Unknown,
}

impl Language {
    pub fn from_path(path: &Path) -> Self {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        // 也参考文件名（Makefile, Dockerfile 等）
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        match ext {
            "rs" => Language::Rust,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
            "py" | "pyi" => Language::Python,
            "go" => Language::Go,
            // P2-7: 新增
            "c" | "h" => Language::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Language::Cpp,
            "java" => Language::Java,
            "rb" => Language::Ruby,
            "cs" => Language::CSharp,
            "sh" | "bash" | "zsh" => Language::Bash,
            "json" => Language::Json,
            "css" | "scss" | "less" => Language::Css,
            "html" | "htm" | "xhtml" => Language::Html,
            _ => {
                // 按文件名识别
                if matches!(name, "Makefile" | "makefile" | "GNUmakefile") {
                    Language::Bash
                } else if matches!(name, "Dockerfile" | "Containerfile") {
                    Language::Bash
                } else {
                    Language::Unknown
                }
            }
        }
    }

    pub fn tree_sitter_language(self) -> Option<tree_sitter::Language> {
        match self {
            Language::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
            Language::JavaScript => Some(tree_sitter_javascript::LANGUAGE.into()),
            Language::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            Language::Python => Some(tree_sitter_python::LANGUAGE.into()),
            Language::Go => Some(tree_sitter_go::LANGUAGE.into()),
            // P2-7: 新增
            Language::C => Some(tree_sitter_c::LANGUAGE.into()),
            Language::Cpp => Some(tree_sitter_cpp::LANGUAGE.into()),
            Language::Java => Some(tree_sitter_java::LANGUAGE.into()),
            Language::Ruby => Some(tree_sitter_ruby::LANGUAGE.into()),
            Language::CSharp => Some(tree_sitter_c_sharp::LANGUAGE.into()),
            Language::Bash => Some(tree_sitter_bash::LANGUAGE.into()),
            Language::Json => Some(tree_sitter_json::LANGUAGE.into()),
            Language::Css => Some(tree_sitter_css::LANGUAGE.into()),
            Language::Html => Some(tree_sitter_html::LANGUAGE.into()),
            Language::Unknown => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Python => "python",
            Language::Go => "go",
            // P2-7: 新增
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Java => "java",
            Language::Ruby => "ruby",
            Language::CSharp => "csharp",
            Language::Bash => "bash",
            Language::Json => "json",
            Language::Css => "css",
            Language::Html => "html",
            Language::Unknown => "unknown",
        }
    }
}
