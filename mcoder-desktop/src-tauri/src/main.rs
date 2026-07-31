// 设计文档 §8.6.1: mcoder desktop (Tauri) 后端
// Tauri 原生 Rust 后端，负责：应用启动时自动检测/拉起本地 mcoder server，
// 暴露 get_server_info / stop_server 命令供前端自动连接，无需手动配对。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Serialize)]
struct ServerInfo {
    url: String,
    token: String,
}

/// mcoder 全局配置目录（与 mcoder/src/config.rs::global_config_dir 一致）
/// 优先读 MCODER_HOME 环境变量，否则用 ~/.mcoder
fn global_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MCODER_HOME") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .map(|h| h.join(".mcoder"))
        .unwrap_or_else(|| PathBuf::from(".mcoder"))
}

/// 检测 mcoder server 是否在运行（TCP 连接到 WS 端口，与 mcoder 自身 is_server_running 一致）
/// 不存在 /api/health 端点，故用 TCP 连接探测
async fn is_server_running(host: &str, port: u16) -> bool {
    let addr = format!("{}:{}", host, port);
    tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    .is_ok()
}

/// 查找 mcoder 二进制：PATH -> ~/.cargo/bin/mcoder -> ~/.mcoder/mcoder
fn find_mcoder_binary() -> Option<String> {
    // 1. Check PATH
    if which::which("mcoder").is_ok() {
        return Some("mcoder".to_string());
    }
    // 2. Check common locations
    let home = dirs::home_dir()?;
    let candidates = [
        home.join(".cargo/bin/mcoder"),
        home.join(".mcoder/mcoder"),
    ];
    for c in &candidates {
        if c.exists() {
            return Some(c.to_string_lossy().to_string());
        }
    }
    None
}

/// 读取已持久化的配对 token（与 mcoder transport/pairing.rs::load_persisted_token 一致）
/// 路径：~/.mcoder/credentials.toml，格式：pairing_token = "xxx"
fn read_token() -> String {
    let cred_path = global_config_dir().join("credentials.toml");
    if let Ok(content) = std::fs::read_to_string(&cred_path) {
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("pairing_token") {
                let val = val.trim_start();
                if let Some(val) = val.strip_prefix('=') {
                    let val = val.trim().trim_matches('"');
                    if !val.is_empty() {
                        return val.to_string();
                    }
                }
            }
        }
    }
    "missing-token".to_string()
}

/// 等待 server 就绪（轮询 TCP 连接）
async fn wait_for_server(host: &str, port: u16, timeout_secs: u64) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(timeout_secs) {
        if is_server_running(host, port).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// 检测/拉起本地 mcoder server，返回连接信息（url + token）
#[tauri::command]
async fn get_server_info() -> Result<ServerInfo, String> {
    let host = "127.0.0.1";
    let port: u16 = 7654;

    if !is_server_running(host, port).await {
        // 尝试启动 server
        let mcoder = find_mcoder_binary()
            .ok_or_else(|| "mcoder binary not found. Install mcoder first.".to_string())?;

        Command::new(&mcoder)
            .args(["server", "--host", host, "--port", &port.to_string(), "--detach"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to start server: {}", e))?;

        if !wait_for_server(host, port, 15).await {
            return Err("server failed to start within 15 seconds".to_string());
        }
    }

    let token = read_token();
    Ok(ServerInfo {
        url: format!("ws://{}:{}", host, port),
        token,
    })
}

/// 停止本地 mcoder server
/// 策略：
/// 1. 尝试 HTTP /api/shutdown（HTTP 端口 = WS 端口 + 1，仅在配置了 --domain 时启动）
/// 2. fallback：调用 `mcoder stop` 命令（走 lock file + signal）
#[tauri::command]
async fn stop_server() -> Result<(), String> {
    let host = "127.0.0.1";
    let port: u16 = 7654;

    // 尝试 HTTP shutdown 端点（快超时，本地模式无 HTTP 时快速失败）
    let http_url = format!("http://{}:{}/api/shutdown", host, port + 1);
    let http_ok = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok()
        .map(|c| async move {
            c.post(&http_url).send().await
                .map(|r| r.status().is_success())
                .unwrap_or(false)
        });

    if let Some(fut) = http_ok {
        if fut.await {
            return Ok(());
        }
    }

    // fallback：调用 `mcoder stop` 命令（不依赖 HTTP 端口，走 lock file + signal）
    let mcoder_bin = find_mcoder_binary()
        .ok_or_else(|| "mcoder binary not found".to_string())?;

    std::process::Command::new(&mcoder_bin)
        .args(["stop", "--host", host, "--port", &port.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("failed to run mcoder stop: {}", e))?;

    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![get_server_info, stop_server])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
