//! Generic HTTP retry wrapper with exponential backoff.
//!
//! Retry policy:
//! - Network errors (timeout / connection failure): exponential backoff, 3 retries (1s, 2s, 4s)
//! - HTTP 429 rate-limited: read `Retry-After` header, wait then retry (up to 3 times)
//! - HTTP 5xx server errors: retry once
//! - HTTP 4xx (non-429): no retry, fail fast
//! - Invalid JSON response: retry once

use anyhow::{Result, bail};
use std::future::Future;
use std::time::Duration;

/// Error type that drives the retry loop.
///
/// Each variant carries enough metadata for [`with_retry`] to decide whether
/// to retry, how long to back off, and when to give up.
#[derive(Debug)]
pub enum RetryError {
    /// Transport-level failure (DNS, TCP, TLS, timeout) from `reqwest`.
    Network(reqwest::Error),
    /// Non-2xx HTTP response.
    ///
    /// `retry_after_secs` is parsed from the `Retry-After` header (seconds form only,
    /// as Gemini/OpenAI/Anthropic all use seconds).
    HttpStatus {
        status: u16,
        body: String,
        retry_after_secs: Option<u64>,
    },
    /// Body read or JSON decode failure.
    Json(reqwest::Error),
    /// Non-retryable logic error (e.g. malformed response payload, missing data).
    /// Causes immediate failure without retry.
    Fatal(String),
}

impl std::fmt::Display for RetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetryError::Network(e) => write!(f, "network error: {e}"),
            RetryError::HttpStatus { status, body, .. } => {
                write!(f, "HTTP {status} error: {body}")
            }
            RetryError::Json(e) => write!(f, "json decode error: {e}"),
            RetryError::Fatal(msg) => write!(f, "fatal: {msg}"),
        }
    }
}

impl std::error::Error for RetryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RetryError::Network(e) | RetryError::Json(e) => Some(e),
            RetryError::HttpStatus { .. } | RetryError::Fatal(_) => None,
        }
    }
}

/// Categorise a raw [`reqwest::Error`] from the send/decode pipeline into the
/// appropriate [`RetryError`] bucket.
///
/// - `is_timeout()` / `is_connect()` → [`RetryError::Network`]
/// - `is_decode()` → [`RetryError::Json`]
/// - everything else (e.g. body read failure mid-stream) → [`RetryError::Network`]
impl From<reqwest::Error> for RetryError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_decode() {
            RetryError::Json(e)
        } else {
            RetryError::Network(e)
        }
    }
}

const MAX_NETWORK_RETRIES: u32 = 3;
const MAX_RATE_LIMIT_RETRIES: u32 = 3;
const MAX_SERVER_ERROR_RETRIES: u32 = 1;
const MAX_JSON_RETRIES: u32 = 1;

/// Run `f` with the retry policy described in the module docs.
///
/// `f` is called repeatedly until it either returns `Ok(T)` or exhausts the
/// retry budget for the failing error category, in which case the last error
/// is wrapped into an `anyhow::Error` with a `RetryError` cause.
///
/// Typical usage inside an adapter `chat`:
///
/// ```ignore
/// with_retry(|| async {
///     let resp = self.client.post(&url)
///         .bearer_auth(&api_key)
///         .json(&body)
///         .send()
///         .await
///         .map_err(RetryError::from)?;
///     if !resp.status().is_success() {
///         let status = resp.status().as_u16();
///         let retry_after_secs = resp.headers()
///             .get("retry-after")
///             .and_then(|v| v.to_str().ok())
///             .and_then(|s| s.parse::<u64>().ok());
///         let text = resp.text().await.unwrap_or_default();
///         return Err(RetryError::HttpStatus { status, body: text, retry_after_secs });
///     }
///     let data: MyResponse = resp.json().await.map_err(RetryError::from)?;
///     Ok(/* build LLMResponse */)
/// }).await
/// ```
pub async fn with_retry<F, Fut, T>(mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<T, RetryError>>,
{
    let mut network_retries = 0u32;
    let mut rate_limit_retries = 0u32;
    let mut server_error_retries = 0u32;
    let mut json_retries = 0u32;

    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                let (delay_opt, give_up) = classify(&e, &mut network_retries, &mut rate_limit_retries, &mut server_error_retries, &mut json_retries);
                if give_up {
                    bail!(e);
                }
                if let Some(d) = delay_opt {
                    tokio::time::sleep(d).await;
                }
            }
        }
    }
}

/// Decide whether to retry, and if so how long to wait.
///
/// Returns `(delay, give_up)`. `give_up == true` means the retry budget is
/// exhausted for this error category and the caller should surface the error.
fn classify(
    e: &RetryError,
    network_retries: &mut u32,
    rate_limit_retries: &mut u32,
    server_error_retries: &mut u32,
    json_retries: &mut u32,
) -> (Option<Duration>, bool) {
    match e {
        RetryError::Network(_) => {
            *network_retries += 1;
            if *network_retries > MAX_NETWORK_RETRIES {
                return (None, true);
            }
            // Exponential backoff: 1s, 2s, 4s
            let secs = 1u64 << (*network_retries - 1);
            (Some(Duration::from_secs(secs)), false)
        }
        RetryError::HttpStatus { status, retry_after_secs, .. } => {
            let status_code = *status;
            if status_code == 429 {
                *rate_limit_retries += 1;
                if *rate_limit_retries > MAX_RATE_LIMIT_RETRIES {
                    return (None, true);
                }
                // Prefer Retry-After (cap at 60s to avoid very long waits);
                // fall back to 1s/2s/4s exponential backoff.
                let delay = match *retry_after_secs {
                    Some(secs) => Duration::from_secs(secs.min(60)),
                    None => Duration::from_secs(1 << (*rate_limit_retries - 1)),
                };
                (Some(delay), false)
            } else if status_code >= 500 {
                *server_error_retries += 1;
                if *server_error_retries > MAX_SERVER_ERROR_RETRIES {
                    return (None, true);
                }
                (Some(Duration::from_secs(1)), false)
            } else {
                // 4xx (non-429): no retry
                (None, true)
            }
        }
        RetryError::Json(_) => {
            *json_retries += 1;
            if *json_retries > MAX_JSON_RETRIES {
                return (None, true);
            }
            (Some(Duration::from_secs(1)), false)
        }
        RetryError::Fatal(_) => {
            // Non-retryable: fail immediately
            (None, true)
        }
    }
}

/// Helper: extract `Retry-After` (seconds) from a `reqwest::Response`'s headers.
///
/// Supports the delta-seconds form (`Retry-After: 120`). The HTTP-date form is
/// not supported by design — all three target providers (OpenAI/Anthropic/Gemini)
/// use seconds.
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}
