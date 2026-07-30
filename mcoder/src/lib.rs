// mcoder lib - 暴露给集成测试的子模块
// 设计：main.rs 是 binary；lib.rs 暴露 pub 子模块供 tests/ 目录下的集成测试使用
// 这是"binary + lib"双 crate 模式：bin 默认编译 src/main.rs，lib 编译 src/lib.rs
// 测试代码引用 `mcoder::ask_user::...` 即可

pub mod agent;
pub mod ask_user;
pub mod browser;
pub mod code_graph;
pub mod commands;
pub mod computer_use;
pub mod config;
pub mod debug;
pub mod llm;
pub mod lsp;
pub mod memory;
pub mod persistence;
pub mod plugin;
pub mod session_manager;

pub mod generation_fence;
pub mod resume_policy;
pub mod skills;
pub mod todo_gate;
pub mod tools;
pub mod transport;
pub mod tree_sitter;
pub mod types;
pub mod utils;
pub mod workflow;
