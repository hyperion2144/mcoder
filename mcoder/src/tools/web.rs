// web.rs -- 内置 web 搜索工具（多 provider 支持）
// provider 优先级：config 配置的 > duckduckgo(默认)
// tavily: AI Agent 专用，返回干净结构化 JSON，1000次/月免费
// serper: Google 结果，2500次免费
// duckduckgo: 无需 API key，HTML 解析，零配置兜底

use crate::types::{ToolOutput, ToolSchema};
use anyhow::Result;
use async_trait::async_trait;

pub struct WebSearchTool;
pub struct WebFetchTool;

#[async_trait]
impl crate::tools::Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_search".into(),
            description: "Search the web for real-time information. Returns search result titles, URLs, and snippets.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results (default 5)",
                        "default": 5
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &crate::tools::ToolContext) -> Result<ToolOutput> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("query required"))?;
        let max = args["max_results"]
            .as_u64()
            .unwrap_or(5)
            .min(10) as usize;

        let cfg = &ctx.app_config.web_search;
        let provider = cfg.provider();

        let results = match provider {
            "tavily" if cfg.has_api_key() => search_tavily(query, max, &cfg.api_key).await,
            "serper" if cfg.has_api_key() => search_serper(query, max, &cfg.api_key).await,
            _ => search_ddg(query, max).await,
        };

        match results {
            Ok(r) => Ok(ToolOutput::Sync {
                result: serde_json::json!({
                    "query": query,
                    "provider": provider,
                    "results": r,
                    "count": r.len()
                }),
            }),
            Err(e) => {
                // 如果配置的 provider 失败，fallback 到 duckduckgo
                if provider != "duckduckgo" {
                    tracing::warn!("web_search provider {} failed: {}, falling back to duckduckgo", provider, e);
                    match search_ddg(query, max).await {
                        Ok(r) => Ok(ToolOutput::Sync {
                            result: serde_json::json!({
                                "query": query,
                                "provider": "duckduckgo (fallback)",
                                "results": r,
                                "count": r.len()
                            }),
                        }),
                        Err(e2) => Ok(ToolOutput::Sync {
                            result: serde_json::json!({
                                "query": query,
                                "error": format!("search failed: {} (fallback also failed: {})", e, e2)
                            }),
                        }),
                    }
                } else {
                    Ok(ToolOutput::Sync {
                        result: serde_json::json!({
                            "query": query,
                            "error": format!("search failed: {}", e)
                        }),
                    })
                }
            }
        }
    }
}

#[async_trait]
impl crate::tools::Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_fetch".into(),
            description: "Fetch content from a URL and return as text. Useful for reading documentation pages, API references, and articles.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to fetch"
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Maximum characters to return (default 8000)",
                        "default": 8000
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &crate::tools::ToolContext) -> Result<ToolOutput> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("url required"))?;
        let max_chars = args["max_chars"]
            .as_u64()
            .unwrap_or(8000) as usize;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Mozilla/5.0 (compatible; mcoder/1.0)")
            .build()?;

        let resp = client.get(url).send().await?;
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = resp.text().await?;

        let text = if content_type.contains("text/html") {
            html_to_text(&body)
        } else {
            body
        };

        let truncated = if text.len() > max_chars {
            format!("{}...\n[truncated, {} chars total]", &text[..max_chars], text.len())
        } else {
            text
        };

        Ok(ToolOutput::Sync {
            result: serde_json::json!({
                "url": url,
                "content": truncated,
                "content_type": content_type
            }),
        })
    }
}

// ===== Tavily =====

async fn search_tavily(query: &str, max: usize, api_key: &str) -> Result<Vec<serde_json::Value>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let resp = client
        .post("https://api.tavily.com/search")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&serde_json::json!({
            "query": query,
            "max_results": max,
            "search_depth": "basic",
            "include_answer": false
        }))
        .send()
        .await?;

    let data: serde_json::Value = resp.json().await?;
    let results = data["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|r| {
                    serde_json::json!({
                        "title": r["title"].as_str().unwrap_or(""),
                        "url": r["url"].as_str().unwrap_or(""),
                        "snippet": r["content"].as_str().unwrap_or("")
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(results)
}

// ===== Serper =====

async fn search_serper(query: &str, max: usize, api_key: &str) -> Result<Vec<serde_json::Value>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let resp = client
        .post("https://google.serper.dev/search")
        .header("X-API-KEY", api_key)
        .json(&serde_json::json!({
            "q": query,
            "num": max
        }))
        .send()
        .await?;

    let data: serde_json::Value = resp.json().await?;
    let results = data["organic"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|r| {
                    serde_json::json!({
                        "title": r["title"].as_str().unwrap_or(""),
                        "url": r["link"].as_str().unwrap_or(""),
                        "snippet": r["snippet"].as_str().unwrap_or("")
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(results)
}

// ===== DuckDuckGo (无需 API key，零配置兜底) =====

async fn search_ddg(query: &str, max: usize) -> Result<Vec<serde_json::Value>> {
    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(query)
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (compatible; mcoder/1.0)")
        .build()?;

    let resp = client.get(&url).send().await?;
    let html = resp.text().await?;

    Ok(parse_ddg_html(&html, max))
}

/// 解析 DuckDuckGo HTML 搜索结果
fn parse_ddg_html(html: &str, max: usize) -> Vec<serde_json::Value> {
    let mut results = Vec::new();

    for chunk in html.split("result__a") {
        if results.len() >= max {
            break;
        }
        let href = chunk
            .split("href=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .map(|s| s.to_string());

        let title = chunk
            .split('>')
            .nth(1)
            .and_then(|s| s.split('<').next())
            .map(|s| s.trim().to_string());

        let snippet = chunk
            .split("result__snippet")
            .nth(1)
            .and_then(|s| s.split('>').nth(1))
            .and_then(|s| s.split('<').next())
            .map(|s| s.trim().to_string());

        if let (Some(href), Some(title)) = (href, title) {
            let clean_url = if href.starts_with("//duckduckgo.com/l/?uddg=") {
                href.split("uddg=")
                    .nth(1)
                    .and_then(|s| s.split('&').next())
                    .and_then(|s| urlencoding::decode(s).ok())
                    .map(|s| s.to_string())
                    .unwrap_or(href)
            } else {
                href
            };

            if title.is_empty() || title.contains("Ad ") {
                continue;
            }

            results.push(serde_json::json!({
                "title": title,
                "url": clean_url,
                "snippet": snippet.unwrap_or_default()
            }));
        }
    }

    results
}

/// 简单 HTML -> 文本转换
/// 跳过 script/style/noscript 标签内的内容，移除其他标签，合并空白
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    // 简单状态机：检测开始标签 / 结束标签，决定是否输出字符
    let mut in_skip = false; // 当前是否在 script/style 内

    let lower = html.to_lowercase();
    let mut chars = html.char_indices().peekable();

    while let Some((idx, c)) = chars.next() {
        if c == '<' {
            // 检测是否是 script/style/noscript 开始或结束标签
            let rest = &lower[idx..];
            if rest.starts_with("<script") || rest.starts_with("<style") || rest.starts_with("<noscript") {
                // 找到 > 跳过整个标签
                if let Some(end) = html[idx..].find('>') {
                    // 看后面是否紧跟着 closing tag
                    let after_open = idx + end + 1;
                    in_skip = true;
                    // 直接跳到 > 之后
                    while let Some((i, _ch)) = chars.next() {
                        if i >= after_open - 1 {
                            break;
                        }
                    }
                    continue;
                }
            }
            in_tag = true;
            continue;
        }
        if c == '>' {
            in_tag = false;
            continue;
        }
        if in_tag {
            // 输出空格作为标签分隔符（避免两个相邻文本粘连）
            if !out.is_empty() && !out.ends_with(' ') && !out.ends_with('\n') {
                out.push(' ');
            }
            continue;
        }
        if in_skip {
            // 检查是否是结束标签
            if c == '<' {
                let rest = &lower[idx..];
                if rest.starts_with("</script") || rest.starts_with("</style") || rest.starts_with("</noscript") {
                    if let Some(end) = html[idx..].find('>') {
                        let after = idx + end + 1;
                        in_skip = false;
                        // 跳到 > 之后
                        while let Some((i, _)) = chars.next() {
                            if i >= after - 1 {
                                break;
                            }
                        }
                        continue;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }

    // 解码常见 HTML 实体
    let decoded = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");

    // 压缩连续空白
    decoded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
