// 设计文档 §8.6 M4: HTTP 服务器
// 提供 Web 客户端静态文件服务 + ACME HTTP-01 挑战响应
// 浏览器直接访问 server 的 HTTP 端口即可使用 Web 客户端

use crate::transport::acme::ChallengeMap;
use crate::transport::pairing::PairingInfo;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// HTTP 服务器配置
pub struct HttpServerConfig {
    /// Web 客户端静态文件目录
    pub web_dir: Option<PathBuf>,
    /// ACME 挑战响应表
    pub challenges: ChallengeMap,
    /// 配对信息（用于 /api/pairing，仅返回非敏感字段）
    pub pairing: PairingInfo,
}

/// 启动 HTTP 服务器
/// 设计文档 §8.6: 浏览器直接访问 server，走 wss，配对串认证
pub async fn start_http_server(host: &str, port: u16, config: HttpServerConfig) -> Result<()> {
    let addr = format!("{}:{}", host, port);
    let listener = TcpListener::bind(&addr).await?;
    let config = Arc::new(config);

    tracing::info!("HTTP server listening on http://{}", addr);

    let cfg = config.clone();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let cfg = cfg.clone();
                    tokio::spawn(async move {
                        // P0-3: 支持 HTTP/1.1 Keep-Alive，在单连接上处理多个请求
                        // 每个连接独立处理，避免慢连接阻塞 acceptor
                        if let Err(e) = handle_connection_loop(stream, peer, cfg).await {
                            tracing::debug!("HTTP connection from {} ended: {}", peer, e);
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("HTTP accept error: {}", e);
                }
            }
        }
    });

    Ok(())
}

/// 单个 TCP 连接的处理循环（支持 Keep-Alive）
async fn handle_connection_loop(
    mut stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    config: Arc<HttpServerConfig>,
) -> Result<()> {
    // P0-3: 设置读超时，避免 Keep-Alive 连接无限期占用
    let _ = stream.set_nodelay(true);

    loop {
        // 读取请求行 + 头部
        let mut buf = vec![0u8; 16384]; // P0-3: 16KB，支持更长的请求头
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(30), // Keep-Alive 空闲超时
            stream.read(&mut buf),
        )
        .await
        .map_err(|_| anyhow::anyhow!("read timeout"))??;

        if n == 0 {
            return Ok(()); // 客户端关闭连接
        }

        // 解析请求行 + 头部
        let request = String::from_utf8_lossy(&buf[..n]);
        let mut lines = request.lines();
        let request_line = lines.next().unwrap_or("");
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 2 {
            send_response(&mut stream, 400, "text/plain", "Bad Request", false).await?;
            return Ok(());
        }

        let method = parts[0];
        let path_with_query = parts[1];

        // 解析头部：收集 Connection、Content-Length、Accept-Encoding
        let mut keep_alive = false;
        let mut content_length: usize = 0;
        let mut accept_encoding = String::new();

        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                let name = name.trim().to_ascii_lowercase();
                let value = value.trim();
                match name.as_str() {
                    "connection" => {
                        keep_alive = value.eq_ignore_ascii_case("keep-alive");
                    }
                    "content-length" => {
                        content_length = value.parse().unwrap_or(0);
                    }
                    "accept-encoding" => {
                        accept_encoding = value.to_string();
                    }
                    _ => {}
                }
            }
        }

        // P0-3: 读取请求体（如果有）
        if content_length > 0 {
            // 当前 buf 中可能已包含部分请求体
            let header_end = request.find("\r\n\r\n").map(|p| p + 4).unwrap_or(n);
            let mut body = buf[header_end..n].to_vec();
            if body.len() < content_length {
                let mut remaining = vec![0u8; content_length - body.len()];
                stream.read_exact(&mut remaining).await?;
                body.extend_from_slice(&remaining);
            }
            // 当前路由不使用请求体，仅消费掉
            let _ = body;
        }

        let path = path_with_query.split('?').next().unwrap_or(path_with_query);

        // P1-7: 日志包含 peer 地址
        tracing::debug!("HTTP {} {} from {}", method, path, peer);

        // 路由分发
        let response = if path.starts_with("/.well-known/acme-challenge/") {
            handle_acme_challenge(path, &config).await
        } else if path == "/api/pairing" && method == "GET" {
            // P0-1: /api/pairing 不再返回完整 pairing_string（含 token）
            // 仅返回非敏感信息，token 通过带外方式（QR 码、终端显示）传递
            handle_pairing_api(&config)
        } else if path == "/api/shutdown" && method == "POST" {
            // 优雅关闭：返回 200 后异步退出进程
            let resp = HttpResponse::new(200, "application/json", b"{\"ok\":true}".to_vec(), false);
            // 在发送响应后退出
            send_response_full(&mut stream, resp, false, &accept_encoding).await?;
            tracing::info!("shutdown requested via HTTP /api/shutdown, exiting...");
            std::process::exit(0);
        } else if method == "GET" {
            serve_static_file(path, &config, &accept_encoding).await
        } else {
            HttpResponse::new(405, "text/plain", b"Method Not Allowed".to_vec(), false)
        };

        // 发送响应
        let close = !keep_alive;
        send_response_full(&mut stream, response, keep_alive, &accept_encoding).await?;

        if close {
            return Ok(());
        }
    }
}

/// HTTP 响应
struct HttpResponse {
    status: u16,
    content_type: String,
    body: Vec<u8>,
    /// 是否可缓存（带 hash 的静态资源）
    cache_immutable: bool,
}

impl HttpResponse {
    fn new(status: u16, content_type: &str, body: Vec<u8>, cache_immutable: bool) -> Self {
        Self {
            status,
            content_type: content_type.to_string(),
            body,
            cache_immutable,
        }
    }
}

/// ACME HTTP-01 挑战响应
/// GET /.well-known/acme-challenge/{token} → 返回 {key}
async fn handle_acme_challenge(path: &str, config: &HttpServerConfig) -> HttpResponse {
    let token = path.strip_prefix("/.well-known/acme-challenge/").unwrap_or("");
    if token.is_empty() {
        return HttpResponse::new(404, "text/plain", b"Not Found".to_vec(), false);
    }

    let challenges = config.challenges.read().await;
    if let Some(key) = challenges.get(token) {
        HttpResponse::new(
            200,
            "application/octet-stream",
            key.as_bytes().to_vec(),
            false,
        )
    } else {
        HttpResponse::new(404, "text/plain", b"Not Found".to_vec(), false)
    }
}

/// /api/pairing 端点：仅返回非敏感信息
/// P0-1 修复: 不再返回 pairing_string（含 token），token 必须通过带外方式获取
fn handle_pairing_api(config: &HttpServerConfig) -> HttpResponse {
    // P1-1 修复: tls 用 as_str() 而非 Debug 格式
    let body = serde_json::json!({
        "host": config.pairing.host,
        "port": config.pairing.port,
        "tls": config.pairing.tls.as_str(),
        "urls": config.pairing.urls,
        // token 不返回；客户端通过终端显示的 QR 码或配对串获取
        "token_required": true,
    });
    let body_str = serde_json::to_string(&body).unwrap_or_default();
    HttpResponse::new(
        200,
        "application/json",
        body_str.into_bytes(),
        false,
    )
}

/// 静态文件服务（Web 客户端）
/// 支持 SPA fallback：文件不存在时返回 index.html
async fn serve_static_file(
    path: &str,
    config: &HttpServerConfig,
    _accept_encoding: &str,
) -> HttpResponse {
    let web_dir = match &config.web_dir {
        Some(d) => d,
        None => {
            // 没有配置 web_dir，返回简单提示页（不含 token）
            let html = r#"<!DOCTYPE html>
<html><body style="background:#0d1117;color:#8b949e;font-family:sans-serif;padding:40px">
<h2 style="color:#58a6ff">mcoder server is running</h2>
<p>Web client is not configured. Use TUI, desktop, or mobile client to connect.</p>
<p>Get pairing string from server terminal output.</p>
</body></html>"#;
            return HttpResponse::new(
                200,
                "text/html; charset=utf-8",
                html.as_bytes().to_vec(),
                false,
            );
        }
    };

    // 安全路径处理：防止路径遍历
    let relative = path.trim_start_matches('/');
    let safe = Path::new(relative);
    let full = web_dir.join(safe);
    if !full.starts_with(web_dir) {
        return HttpResponse::new(403, "text/plain", b"Forbidden".to_vec(), false);
    }

    let file_path = if full.is_dir() || !full.exists() {
        // SPA fallback
        let index = web_dir.join("index.html");
        if index.exists() {
            index
        } else {
            return HttpResponse::new(404, "text/plain", b"Not Found".to_vec(), false);
        }
    } else {
        full
    };

    match tokio::fs::read(&file_path).await {
        Ok(content) => {
            let mime = guess_mime(&file_path);
            // P2-1: 带 hash 的静态资源（如 index-AbCd1234.js）可永久缓存
            let cache_immutable = has_content_hash(&file_path);
            HttpResponse::new(200, &mime, content, cache_immutable)
        }
        Err(_) => HttpResponse::new(404, "text/plain", b"Not Found".to_vec(), false),
    }
}

/// 发送完整响应（支持 Keep-Alive 和 gzip）
async fn send_response_full(
    stream: &mut tokio::net::TcpStream,
    mut response: HttpResponse,
    keep_alive: bool,
    accept_encoding: &str,
) -> Result<()> {
    let status_text = match response.status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };

    // P2-1: gzip 压缩（仅对文本类且体积 > 1KB）
    let mut content_encoding = String::new();
    if accept_encoding.contains("gzip")
        && response.body.len() > 1024
        && is_compressible(&response.content_type)
    {
        if let Ok(compressed) = gzip_encode(&response.body) {
            response.body = compressed;
            content_encoding = "gzip".to_string();
        }
    }

    let mut header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: {}\r\n",
        response.status,
        status_text,
        response.content_type,
        response.body.len(),
        if keep_alive { "keep-alive" } else { "close" }
    );

    if !content_encoding.is_empty() {
        header.push_str(&format!("Content-Encoding: {}\r\n", content_encoding));
    }

    // P2-1: 缓存头
    if response.cache_immutable {
        header.push_str("Cache-Control: public, max-age=31536000, immutable\r\n");
    } else if response.content_type.starts_with("text/html") {
        header.push_str("Cache-Control: no-cache\r\n");
    }

    header.push_str("\r\n");

    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&response.body).await?;
    stream.flush().await?;
    Ok(())
}

/// 兼容旧接口：发送简单文本响应
async fn send_response(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
    _keep_alive: bool,
) -> Result<()> {
    let response = HttpResponse::new(status, content_type, body.as_bytes().to_vec(), false);
    send_response_full(stream, response, false, "").await
}

/// 根据文件扩展名猜测 MIME 类型
fn guess_mime(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") | Some("htm") => "text/html; charset=utf-8".to_string(),
        Some("css") => "text/css; charset=utf-8".to_string(),
        Some("js") | Some("mjs") => "application/javascript; charset=utf-8".to_string(),
        Some("json") => "application/json; charset=utf-8".to_string(),
        Some("png") => "image/png".to_string(),
        Some("jpg") | Some("jpeg") => "image/jpeg".to_string(),
        Some("gif") => "image/gif".to_string(),
        Some("svg") => "image/svg+xml".to_string(),
        Some("ico") => "image/x-icon".to_string(),
        Some("woff") => "font/woff".to_string(),
        Some("woff2") => "font/woff2".to_string(),
        Some("ttf") => "font/ttf".to_string(),
        Some("wasm") => "application/wasm".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

/// 判断 MIME 类型是否适合 gzip 压缩
fn is_compressible(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || content_type.contains("javascript")
        || content_type.contains("json")
        || content_type.contains("svg")
        || content_type.contains("xml")
        || content_type.contains("wasm")
}

/// 判断文件名是否包含内容 hash（如 index-AbCd1234.js）
fn has_content_hash(path: &Path) -> bool {
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        // 匹配 name-XXXX 或 name.XXXX 格式，其中 XXXX 至少 6 个字符
        if let Some(pos) = stem.rfind(|c: char| c == '-' || c == '.') {
            let hash = &stem[pos + 1..];
            return hash.len() >= 6 && hash.chars().all(|c| c.is_ascii_alphanumeric());
        }
    }
    false
}

/// gzip 编码
fn gzip_encode(data: &[u8]) -> Result<Vec<u8>> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    Ok(encoder.finish()?)
}
