// 设计文档 §8.6 M4: wss 支持
// - 自签证书生成（IP 场景）
// - rustls ServerConfig 构建
// - 证书持久化到 ~/.mcoder/certs/ 避免每次启动重新生成

use anyhow::{Context, Result};
use rustls::ServerConfig;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;

/// TLS 证书对（PEM 格式）
pub struct CertPair {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}

/// 证书存储路径：~/.mcoder/certs/
fn certs_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".mcoder").join("certs")
}

/// 设计文档 §8.6: 自签证书生成
/// - 生成自签 RSA 证书，CN=localhost，SAN 包含 localhost + 127.0.0.1
/// - 持久化到 ~/.mcoder/certs/{cert.pem, key.pem}
/// - 首次生成后后续启动直接加载
pub fn load_or_generate_cert() -> Result<CertPair> {
    let dir = certs_dir();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");

    // 尝试加载已有证书
    if cert_path.exists() && key_path.exists() {
        let cert_pem = std::fs::read(&cert_path)
            .with_context(|| format!("reading cert: {}", cert_path.display()))?;
        let key_pem = std::fs::read(&key_path)
            .with_context(|| format!("reading key: {}", key_path.display()))?;
        tracing::info!("loaded existing self-signed cert from {}", dir.display());
        return Ok(CertPair { cert_pem, key_pem });
    }

    // 生成新证书
    tracing::info!("generating new self-signed certificate...");
    let cert = generate_self_signed_cert()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating certs dir: {}", dir.display()))?;
    std::fs::write(&cert_path, &cert.cert_pem)?;
    std::fs::write(&key_path, &cert.key_pem)?;
    tracing::info!("self-signed cert saved to {}", dir.display());
    Ok(cert)
}

/// 用 rcgen 生成自签证书
/// 设计 P2-1 修复: SAN 包含本机所有网卡 IP，避免局域网连接时证书 SAN 不匹配
fn generate_self_signed_cert() -> Result<CertPair> {
    // 收集 SAN 条目：localhost + 127.0.0.1 + 本机所有网卡 IP
    let mut san_entries: Vec<String> = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ];

    // 获取本机所有网卡 IP
    if let Ok(addrs) = local_ip_addresses() {
        for ip in addrs {
            let ip_str = ip.to_string();
            if !san_entries.contains(&ip_str) && ip_str != "::1" {
                san_entries.push(ip_str);
            }
        }
    }

    tracing::info!("generating self-signed cert with SAN: {:?}", san_entries);
    let cert = rcgen::generate_simple_self_signed(san_entries)?;
    let cert_pem = cert.cert.pem().into_bytes();
    let key_pem = cert.key_pair.serialize_pem().into_bytes();
    Ok(CertPair { cert_pem, key_pem })
}

/// 获取本机所有网卡的 IP 地址（IPv4 + IPv6）
/// 设计 P2-1: 用于将本机 IP 加入证书 SAN
fn local_ip_addresses() -> Result<Vec<std::net::IpAddr>> {
    use std::net::ToSocketAddrs;

    // 方法 1: 通过 UDP socket 获取本机出口 IP（不发送数据）
    // 这是跨平台的方式，无需依赖外部 crate
    let mut ips = Vec::new();

    // 尝试连接 8.8.8.8:80（不实际发包），获取本机使用的 IP
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                let ip = addr.ip();
                if !ip.is_loopback() {
                    ips.push(ip);
                }
            }
        }
    }

    // 方法 2: 通过 DNS 解析本机 hostname 获取更多 IP
    // 获取 hostname（跨平台）
    #[cfg(target_os = "windows")]
    let hostname = std::env::var("COMPUTERNAME").unwrap_or_default();
    #[cfg(not(target_os = "windows"))]
    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if !hostname.is_empty() {
        if let Ok(addrs) = (hostname.as_str(), 0).to_socket_addrs() {
            for addr in addrs {
                let ip = addr.ip();
                if !ip.is_loopback() && !ips.contains(&ip) {
                    ips.push(ip);
                }
            }
        }
    }

    Ok(ips)
}

/// 构建 rustls TlsAcceptor
pub fn build_tls_acceptor() -> Result<TlsAcceptor> {
    let pair = load_or_generate_cert()?;

    // 解析证书
    let cert_chain = rustls_pemfile::certs(&mut Cursor::new(&pair.cert_pem))
        .collect::<Result<Vec<_>, _>>()
        .context("parsing cert PEM")?;
    let server_cert = rustls::pki_types::CertificateDer::from(cert_chain[0].clone());

    // 解析私钥
    let key = rustls_pemfile::private_key(&mut Cursor::new(&pair.key_pem))
        .context("parsing key PEM")?
        .context("no private key found in PEM")?;
    let server_key = rustls::pki_types::PrivateKeyDer::try_from(key.secret_der().to_vec())
        .map_err(|e| anyhow::anyhow!("converting private key: {}", e))?;

    // 构建 ServerConfig
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![server_cert], server_key)
        .context("building rustls ServerConfig")?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// 设计文档 §8.6: 判断是否需要 TLS
/// - tls=on → 强制 TLS
/// - tls=off → 不使用 TLS
/// - tls=auto → 本地（127.0.0.1/localhost）不用 TLS，非本地用 TLS
pub fn should_use_tls(tls_mode: &crate::transport::pairing::TlsMode, host: &str) -> bool {
    use crate::transport::pairing::TlsMode;
    match tls_mode {
        TlsMode::On => true,
        TlsMode::Off => false,
        TlsMode::Auto => !is_local_host(host),
    }
}

/// 判断 host 是否为本地地址
/// P2-5: 补全 IPv6 通配地址 :: 和 [::1]
fn is_local_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "0.0.0.0" | "::" | "::1" | "[::1]")
}

/// 证书目录路径（用于测试/调试）
pub fn cert_dir() -> PathBuf {
    certs_dir()
}

/// 证书是否存在
pub fn cert_exists() -> bool {
    let dir = certs_dir();
    dir.join("cert.pem").exists() && dir.join("key.pem").exists()
}
