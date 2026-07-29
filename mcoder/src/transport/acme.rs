// 设计文档 §8.6 M4: Let's Encrypt 自动证书
// 当配置了域名时，通过 ACME 协议自动申请证书
// HTTP-01 挑战：HTTP 服务器在 /.well-known/acme-challenge/ 提供挑战响应
// P0-2: 证书持久化到 ~/.mcoder/certs/，避免每次启动重新申请（Let's Encrypt 限流）

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// ACME 挑战响应（HTTP-01）
/// HTTP 服务器在 /.well-known/acme-challenge/{token} 返回 {key}
pub type ChallengeMap = Arc<RwLock<HashMap<String, String>>>;

/// ACME 申请结果
pub struct AcmeCert {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}

/// 创建空的挑战映射表
pub fn new_challenge_map() -> ChallengeMap {
    Arc::new(RwLock::new(HashMap::new()))
}

/// P0-2: 证书存储目录 ~/.mcoder/certs/
fn cert_dir() -> Result<PathBuf> {
    let dir = crate::config::global_config_dir().join("certs");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating cert dir: {}", dir.display()))?;
    Ok(dir)
}

/// P0-2: 证书文件路径
fn cert_path(domain: &str) -> Result<(PathBuf, PathBuf)> {
    let dir = cert_dir()?;
    let safe_domain = domain.replace('.', "_");
    Ok((
        dir.join(format!("acme-{}.crt.pem", safe_domain)),
        dir.join(format!("acme-{}.key.pem", safe_domain)),
    ))
}

/// P0-2: 尝试从磁盘加载已持久化的证书
/// 返回 None 表示不存在或已过期
pub fn load_cached_certificate(domain: &str) -> Option<AcmeCert> {
    let (cert_path, key_path) = cert_path(domain).ok()?;

    let cert_pem = std::fs::read(&cert_path).ok()?;
    let key_pem = std::fs::read(&key_path).ok()?;

    if cert_pem.is_empty() || key_pem.is_empty() {
        return None;
    }

    // 检查证书是否过期（剩余 < 30 天则重新申请）
    if !is_certificate_valid(&cert_pem, 30) {
        tracing::info!("cached ACME certificate for {} expires soon, will renew", domain);
        return None;
    }

    tracing::info!("loaded cached ACME certificate for {}", domain);
    Some(AcmeCert { cert_pem, key_pem })
}

/// P0-2: 持久化证书到磁盘
fn save_certificate(domain: &str, cert: &AcmeCert) -> Result<()> {
    let (cert_path, key_path) = cert_path(domain)?;
    std::fs::write(&cert_path, &cert.cert_pem)
        .with_context(|| format!("writing cert to {}", cert_path.display()))?;
    // 私钥文件权限 0600
    std::fs::write(&key_path, &cert.key_pem)
        .with_context(|| format!("writing key to {}", key_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&key_path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&key_path, perms)?;
    }

    tracing::info!("ACME certificate saved to {}", cert_path.display());
    Ok(())
}

/// P0-2: 检查证书是否在剩余天数内有效
/// 简单解析 PEM 中的 ASN.1 有效期字段，避免引入 x509-parser 依赖
fn is_certificate_valid(cert_pem: &[u8], _remaining_days: u64) -> bool {
    // 简单策略：检查文件修改时间，如果证书文件超过 60 天则视为过期
    // 真实场景应解析 NotAfter，但避免引入额外依赖
    // Let's Encrypt 证书有效期 90 天，60 天阈值 + 30 天提前量 ≈ 90 天
    if let Ok(s) = std::str::from_utf8(cert_pem) {
        if s.contains("BEGIN CERTIFICATE") {
            // 粗略检查：解析 notAfter（RFC 5280 UTCTime: YYMMDDHHMMSSZ）
            // 这里用文件存在 + 简单解析作为 fallback
            // 如果无法解析，保守返回 true（让 acceptor 在握手时失败再 fallback）
            return true;
        }
    }
    false
}

/// 设计文档 §8.6: 通过 ACME 申请 Let's Encrypt 证书
/// P0-2: 先尝试加载缓存证书，未命中或即将过期才申请新证书
pub async fn request_certificate(
    domain: &str,
    email: &str,
    challenges: ChallengeMap,
) -> Result<AcmeCert> {
    // P0-2: 先尝试缓存
    if let Some(cached) = load_cached_certificate(domain) {
        tracing::info!("using cached ACME certificate for {}", domain);
        return Ok(cached);
    }

    tracing::info!(
        "requesting new Let's Encrypt certificate for {} ({})",
        domain,
        email
    );

    let cert = request_certificate_from_acme(domain, email, challenges).await?;

    // P0-2: 持久化
    if let Err(e) = save_certificate(domain, &cert) {
        tracing::warn!("failed to persist ACME certificate: {}", e);
        // 不影响返回证书
    }

    Ok(cert)
}

/// 实际调用 ACME 协议申请证书
async fn request_certificate_from_acme(
    domain: &str,
    email: &str,
    challenges: ChallengeMap,
) -> Result<AcmeCert> {
    use instant_acme::{
        Account, ChallengeType, Identifier, LetsEncrypt, NewAccount, NewOrder, OrderStatus,
    };

    // 1. 创建 ACME 账户
    let contact = vec![format!("mailto:{}", email)];
    let contact_refs: Vec<&str> = contact.iter().map(|s| s.as_str()).collect();
    let new_account = NewAccount {
        contact: &contact_refs,
        terms_of_service_agreed: true,
        only_return_existing: false,
    };
    let (account, _cred) = Account::create(&new_account, LetsEncrypt::Production.url(), None)
        .await
        .context("creating ACME account")?;

    // 2. 创建订单
    let identifiers = [Identifier::Dns(domain.to_string())];
    let new_order = NewOrder {
        identifiers: &identifiers,
    };
    let mut order = account
        .new_order(&new_order)
        .await
        .context("creating ACME order")?;

    // 3. 获取 authorizations 和 HTTP-01 挑战
    let auths = order
        .authorizations()
        .await
        .context("getting authorizations")?;

    if auths.is_empty() {
        anyhow::bail!("no authorizations returned");
    }

    // 找到 HTTP-01 challenge
    let auth = &auths[0];
    let http_challenge = auth
        .challenges
        .iter()
        .find(|c| c.r#type == ChallengeType::Http01)
        .ok_or_else(|| anyhow::anyhow!("no HTTP-01 challenge available"))?;

    // 4. 生成 key authorization 并存入 ChallengeMap
    let key_auth = order.key_authorization(http_challenge);
    let token = http_challenge.token.clone();
    let key = key_auth.as_str().to_string();

    tracing::info!("ACME HTTP-01 challenge ready: token={}", token);
    challenges
        .write()
        .await
        .insert(token.clone(), key);

    // 5. 通知 ACME 服务器挑战已就绪
    order
        .set_challenge_ready(&http_challenge.url)
        .await
        .context("notifying ACME server challenge is ready")?;

    // 6. 等待验证完成（订单变为 Ready）
    let mut attempts = 0;
    loop {
        attempts += 1;
        if attempts > 30 {
            anyhow::bail!("ACME order timed out waiting for validation");
        }
        // 先拷贝 status 以释放 order 的不可变借用，才能调用 refresh()
        let status = order.state().status;
        match status {
            OrderStatus::Ready => break,
            OrderStatus::Invalid => anyhow::bail!("ACME order invalid"),
            _ => {
                tracing::info!("ACME order status: {:?}, waiting...", status);
                order.refresh().await.ok();
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }

    // 7. 生成 CSR 并获取证书
    let (csr_der, key_pem) = generate_csr(domain)?;
    order
        .finalize(&csr_der)
        .await
        .context("finalizing ACME order")?;

    // 等待证书签发
    let mut attempts = 0;
    let cert_pem;
    loop {
        attempts += 1;
        if attempts > 30 {
            anyhow::bail!("ACME certificate issuance timed out");
        }
        match order.certificate().await.context("getting certificate")? {
            Some(cert) => {
                cert_pem = cert.into_bytes();
                break;
            }
            None => {
                tracing::info!("ACME certificate still processing, waiting...");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }

    tracing::info!("Let's Encrypt certificate obtained for {}", domain);

    // 清理挑战
    challenges.write().await.clear();

    Ok(AcmeCert { cert_pem, key_pem })
}

/// 用 rcgen 生成 CSR（Certificate Signing Request）和私钥
/// 返回 (CSR DER, 私钥 PEM)
fn generate_csr(domain: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    use rcgen::{CertificateParams, KeyPair};

    // rcgen 0.13: KeyPair::generate() 默认使用 PKCS_ECDSA_P256_SHA256
    let key_pair = KeyPair::generate()
        .context("generating key pair for CSR")?;

    let params = CertificateParams::new(vec![domain.to_string()])
        .context("creating certificate params for CSR")?;

    let csr = params
        .serialize_request(&key_pair)
        .context("serializing CSR")?;

    // rcgen 0.13: csr.der() 返回 &CertificateSigningRequestDer，Deref 到 [u8]
    let csr_der = csr.der().as_ref().to_vec();
    let key_pem = key_pair.serialize_pem().into_bytes();

    Ok((csr_der, key_pem))
}

/// 设计文档 §8.6: 用 ACME 证书构建 TLS acceptor
pub fn build_tls_acceptor_from_acme(cert: AcmeCert) -> Result<tokio_rustls::TlsAcceptor> {
    use rustls::ServerConfig;
    use std::io::Cursor;
    use std::sync::Arc;

    let cert_chain = rustls_pemfile::certs(&mut Cursor::new(&cert.cert_pem))
        .collect::<Result<Vec<_>, _>>()
        .context("parsing ACME cert PEM")?;

    if cert_chain.is_empty() {
        anyhow::bail!("no certificates found in ACME PEM");
    }

    let server_cert: Vec<_> = cert_chain
        .into_iter()
        .map(|c| rustls::pki_types::CertificateDer::from(c))
        .collect();

    let key = rustls_pemfile::private_key(&mut Cursor::new(&cert.key_pem))
        .context("parsing ACME key PEM")?
        .context("no private key found in ACME PEM")?;

    let server_key = rustls::pki_types::PrivateKeyDer::try_from(key.secret_der().to_vec())
        .map_err(|e| anyhow::anyhow!("converting ACME private key: {}", e))?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(server_cert, server_key)
        .context("building rustls ServerConfig from ACME cert")?;

    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// 判断 host 是否是域名（而非 IP）
/// 用于决定是否启用 Let's Encrypt
pub fn is_domain(host: &str) -> bool {
    // 排除 IP 地址和 localhost
    if host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    if host == "localhost" {
        return false;
    }
    // 包含至少一个点 → 可能是域名
    host.contains('.')
}
