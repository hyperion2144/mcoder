// 设计文档 §5.3: verify_token/parse_pairing_string 为 forward-looking client-side API
// server 端直接对比 token；客户端解析保留供未来原生客户端使用
#![allow(dead_code)]

use anyhow::Result;
use qrcode::QrCode;
use qrcode::render::unicode;
use serde::{Deserialize, Serialize};

/// 设计文档 §5.1: 配对机制
/// 配对字符串格式: mcoder://<token>@<host>:<port>?tls=<auto|on|off>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingInfo {
    pub host: String,
    pub port: u16,
    pub token: String,
    pub tls: TlsMode,
    /// mcoder:// 配对串
    pub pairing_string: String,
    /// 实际连接的 ws/wss URL 列表
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    /// 自动：本地 ws://，非本地 wss://
    Auto,
    /// 强制 TLS
    On,
    /// 不使用 TLS
    Off,
}

impl Default for TlsMode {
    fn default() -> Self { TlsMode::Auto }
}

impl TlsMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            TlsMode::Auto => "auto",
            TlsMode::On => "on",
            TlsMode::Off => "off",
        }
    }
}

/// 生成配对信息
/// 设计文档 §5.1: mcoder://<token>@<host>:<port>?tls=<auto|on|off>
/// 优先使用已持久化的 token，避免重启后已配对客户端失联
pub fn generate_pairing(host: &str, port: u16) -> Result<PairingInfo> {
    let token = load_persisted_token().unwrap_or_else(|| {
        let t = uuid::Uuid::new_v4().simple().to_string();
        let _ = persist_pairing_token(&t);
        t
    });
    let tls = TlsMode::Auto;

    // 生成 mcoder:// 配对串
    let pairing_string = format!("mcoder://{}@{}:{}?tls={}", token, host, port, tls.as_str());

    // 生成实际连接 URL 列表
    let mut urls = Vec::new();
    let use_tls = match tls {
        TlsMode::Auto => !is_local_host(host),
        TlsMode::On => true,
        TlsMode::Off => false,
    };

    if is_wildcard_addr(host) {
        // 通配地址：生成本地 + 局域网 URL
        urls.push(format!("ws://127.0.0.1:{}/?token={}", port, token));
        urls.push(format!("ws://localhost:{}/?token={}", port, token));
    } else if use_tls {
        urls.push(format!("wss://{}:{}/?token={}", host, port, token));
    } else {
        urls.push(format!("ws://{}:{}/?token={}", host, port, token));
    }

    Ok(PairingInfo {
        host: host.to_string(),
        port,
        token,
        tls,
        pairing_string,
        urls,
    })
}

/// 渲染 QR 码（终端显示）
pub fn render_qr(content: &str) -> String {
    match QrCode::new(content.as_bytes()) {
        Ok(code) => {
            code.render::<unicode::Dense1x2>()
                .dark_color(unicode::Dense1x2::Light)
                .light_color(unicode::Dense1x2::Dark)
                .build()
        }
        Err(_) => String::new(),
    }
}

/// 验证 token
pub fn verify_token(query: &str, expected: &str) -> bool {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == "token" && v == expected {
                return true;
            }
        }
    }
    false
}

/// 判断 host 是否为本地地址（不需要 TLS）
fn is_local_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "0.0.0.0" | "::" | "::1" | "[::1]")
}

/// 判断 host 是否为通配地址
fn is_wildcard_addr(host: &str) -> bool {
    matches!(host, "0.0.0.0" | "::")
}

/// 设计文档 §5.1: 持久化配对 token 到 ~/.mcoder/credentials.toml
/// 避免每次生成新 token 导致已配对客户端失联
pub fn persist_pairing_token(token: &str) -> Result<()> {
    let cred_path = crate::config::global_config_dir().join("credentials.toml");
    if let Some(parent) = cred_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // 简单的 key=value 文件，避免引入 toml 写入依赖
    let content = format!("# mcoder credentials\npairing_token = \"{}\"\n", token);
    std::fs::write(&cred_path, content)?;
    tracing::info!("pairing token persisted to {}", cred_path.display());
    Ok(())
}

/// 读取已持久化的配对 token（如果存在）
pub fn load_persisted_token() -> Option<String> {
    let cred_path = crate::config::global_config_dir().join("credentials.toml");
    let content = std::fs::read_to_string(&cred_path).ok()?;
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("pairing_token") {
            let val = val.trim_start();
            if let Some(val) = val.strip_prefix('=') {
                let val = val.trim().trim_matches('"');
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// 解析 mcoder:// 配对串
/// 格式: mcoder://<token>@<host>:<port>?tls=<auto|on|off>
pub fn parse_pairing_string(s: &str) -> Result<PairingInfo> {
    let s = s.trim();

    // 去掉 mcoder:// 前缀
    let rest = s.strip_prefix("mcoder://")
        .ok_or_else(|| anyhow::anyhow!("invalid pairing string: must start with mcoder://"))?;

    // 分离 query string
    let (main, query) = rest.split_once('?').unwrap_or((rest, ""));

    // 解析 tls 参数
    let tls = if let Some(tls_val) = query.split('&')
        .find_map(|p| p.strip_prefix("tls=").map(|s| s.to_string()))
    {
        match tls_val.as_str() {
            "auto" => TlsMode::Auto,
            "on" => TlsMode::On,
            "off" => TlsMode::Off,
            _ => TlsMode::Auto,
        }
    } else {
        TlsMode::Auto
    };

    // 分离 token@host:port
    let (token, host_port) = main.split_once('@')
        .ok_or_else(|| anyhow::anyhow!("invalid pairing string: missing '@' separator"))?;

    // 分离 host:port
    let (host, port_str) = host_port.rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("invalid pairing string: missing ':' before port"))?;
    let port: u16 = port_str.parse()
        .map_err(|_| anyhow::anyhow!("invalid port: {}", port_str))?;

    let pairing_string = format!("mcoder://{}@{}:{}?tls={}", token, host, port, tls.as_str());

    let use_tls = match tls {
        TlsMode::Auto => !is_local_host(host),
        TlsMode::On => true,
        TlsMode::Off => false,
    };
    let url = if use_tls {
        format!("wss://{}:{}/?token={}", host, port, token)
    } else {
        format!("ws://{}:{}/?token={}", host, port, token)
    };

    Ok(PairingInfo {
        host: host.to_string(),
        port,
        token: token.to_string(),
        tls,
        pairing_string,
        urls: vec![url],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pairing_format() {
        let info = generate_pairing("192.168.1.10", 7654).unwrap();
        assert!(info.pairing_string.starts_with("mcoder://"));
        assert!(info.pairing_string.contains("@192.168.1.10:7654"));
        assert!(info.pairing_string.contains("tls=auto"));
    }

    #[test]
    fn test_parse_pairing() {
        let s = "mcoder://a1b2c3d4e5f6@192.168.1.10:7654?tls=auto";
        let info = parse_pairing_string(s).unwrap();
        assert_eq!(info.token, "a1b2c3d4e5f6");
        assert_eq!(info.host, "192.168.1.10");
        assert_eq!(info.port, 7654);
        assert_eq!(info.tls, TlsMode::Auto);
    }

    #[test]
    fn test_local_no_tls() {
        let info = generate_pairing("127.0.0.1", 7654).unwrap();
        assert!(info.urls[0].starts_with("ws://"));
    }
}
