// 设计文档 §8.6.1: mcoder desktop (Tauri) 后端
// Tauri 原生 Rust 后端，主要负责窗口管理和原生 API 暴露
// 业务逻辑通过 WebSocket 连接到 mcoder server，不在 Tauri 后端重复实现

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
