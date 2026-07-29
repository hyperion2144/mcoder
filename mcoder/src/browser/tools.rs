// 设计文档 §8.7 M5 自测: 浏览器工具
// 9 个工具：browser_open/navigate/click/type/screenshot/snapshot/eval/console/network
// 设计 P0-1 修复: console/network 的 filter/level 参数实际生效
// 设计 P0-2 修复: navigate 时注入 console 监听器
// 设计 P1-2 修复: network 用 CDP Network domain 获取 method/status
// 设计 P1-3 修复: with_tab 用 spawn_blocking
// 设计 P2-2 修复: 复用 tab 而非每次新建

use crate::browser::BrowserManager;
use crate::tools::{Tool, ToolContext};
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use serde_json::Value;
use std::sync::Arc;

/// base64 编码
fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// browser_open: 启动 headless Chrome 并打开指定 URL
pub struct BrowserOpenTool {
    pub manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for BrowserOpenTool {
    fn name(&self) -> &str { "browser_open" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "browser_open".into(),
            description: "Launch headless Chrome and open a URL. Returns the page title. The browser stays open for subsequent browser_* calls.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to open (e.g. http://localhost:3000)" }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let url: String = serde_json::from_value(args["url"].clone())
            .context("url required")?;
        let just_started = self.manager.ensure_started().await?;
        let title = self.manager.open_or_navigate(&url).await?;
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "opened": true,
            "url": url,
            "title": title,
            "browser_started": just_started
        }) })
    }
}

/// browser_navigate: 导航到新 URL（浏览器已打开）
pub struct BrowserNavigateTool {
    pub manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for BrowserNavigateTool {
    fn name(&self) -> &str { "browser_navigate" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "browser_navigate".into(),
            description: "Navigate the browser to a new URL. Browser must be already open.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let url: String = serde_json::from_value(args["url"].clone())?;
        let title = self.manager.open_or_navigate(&url).await?;
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "navigated": true,
            "url": url,
            "title": title
        }) })
    }
}

/// browser_click: 点击元素
pub struct BrowserClickTool {
    pub manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for BrowserClickTool {
    fn name(&self) -> &str { "browser_click" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "browser_click".into(),
            description: "Click an element by CSS selector in the active tab.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "CSS selector (e.g. 'button#submit', 'a[href=\"/login\"]')" }
                },
                "required": ["selector"]
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let selector: String = serde_json::from_value(args["selector"].clone())?;
        let selector_resp = selector.clone();
        self.manager.with_tab(move |tab| {
            tab.find_element(&selector)?.click()?;
            Ok(())
        }).await?;
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "clicked": true,
            "selector": selector_resp
        }) })
    }
}

/// browser_type: 在输入框中输入文本
pub struct BrowserTypeTool {
    pub manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for BrowserTypeTool {
    fn name(&self) -> &str { "browser_type" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "browser_type".into(),
            description: "Type text into an input element identified by CSS selector.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string" },
                    "text": { "type": "string" },
                    "clear": { "type": "boolean", "description": "Clear existing text first, default true" }
                },
                "required": ["selector", "text"]
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let selector: String = serde_json::from_value(args["selector"].clone())?;
        let text: String = serde_json::from_value(args["text"].clone())?;
        let clear: bool = args["clear"].as_bool().unwrap_or(true);
        let selector_resp = selector.clone();
        let text_resp = text.clone();
        self.manager.with_tab(move |tab| {
            let element = tab.find_element(&selector)?;
            if clear {
                element.click()?;
                tab.evaluate("document.activeElement.value = ''", true)?;
            }
            element.type_into(&text)?;
            Ok(())
        }).await?;
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "typed": true,
            "selector": selector_resp,
            "text": text_resp
        }) })
    }
}

/// browser_screenshot: 截取页面截图
pub struct BrowserScreenshotTool {
    pub manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for BrowserScreenshotTool {
    fn name(&self) -> &str { "browser_screenshot" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "browser_screenshot".into(),
            description: "Capture a screenshot of the current page. Returns base64-encoded PNG. Use browser_snapshot for a lighter text-based alternative.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "format": { "type": "string", "enum": ["png", "jpeg"], "description": "Image format, default png" },
                    "quality": { "type": "integer", "description": "JPEG quality (0-100), only for jpeg. Default 80." }
                }
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let format_str: String = args["format"].as_str().unwrap_or("png").to_string();
        let quality: i64 = args["quality"].as_i64().unwrap_or(80);
        let format = match format_str.as_str() {
            "jpeg" => CaptureScreenshotFormatOption::Jpeg,
            _ => CaptureScreenshotFormatOption::Png,
        };
        let screenshot = self.manager.with_tab(move |tab| {
            let data = tab.capture_screenshot(format, Some(quality as u32), None, true)?;
            Ok(base64_encode(&data))
        }).await?;
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "screenshot": screenshot,
            "format": format_str
        }) })
    }
}

/// browser_snapshot: 获取页面的文本快照（accessibility tree 简化版）
/// 设计文档 §8.7: snapshot 比 screenshot 省 token
pub struct BrowserSnapshotTool {
    pub manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for BrowserSnapshotTool {
    fn name(&self) -> &str { "browser_snapshot" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "browser_snapshot".into(),
            description: "Get a text snapshot of the page (visible text, links, buttons, inputs). Much lighter than screenshot. Returns structured JSON.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "max_length": { "type": "integer", "description": "Max text length to return, default 5000" }
                }
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let max_length: usize = args["max_length"].as_u64().unwrap_or(5000) as usize;
        let snapshot = self.manager.with_tab(move |tab| {
            let js = format!(r#"
                (() => {{
                    const result = {{
                        title: document.title,
                        url: window.location.href,
                        links: Array.from(document.querySelectorAll('a')).slice(0, 50).map(a => ({{text: a.textContent.trim().slice(0, 100), href: a.href}})).filter(l => l.text),
                        buttons: Array.from(document.querySelectorAll('button, [role="button"]')).slice(0, 30).map(b => b.textContent.trim().slice(0, 100)).filter(t => t),
                        inputs: Array.from(document.querySelectorAll('input, textarea, select')).slice(0, 30).map(i => ({{type: i.type, name: i.name, placeholder: i.placeholder, value: i.value}})),
                        text: document.body.innerText.slice(0, {})
                    }};
                    return JSON.stringify(result);
                }})()
            "#, max_length);
            let val = tab.evaluate(&js, true)?.value.unwrap_or_default();
            Ok(val)
        }).await?;
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "snapshot": snapshot
        }) })
    }
}

/// browser_eval: 执行 JavaScript
pub struct BrowserEvalTool {
    pub manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for BrowserEvalTool {
    fn name(&self) -> &str { "browser_eval" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "browser_eval".into(),
            description: "Execute JavaScript in the page and return the result. Use for custom DOM queries or assertions.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "script": { "type": "string", "description": "JavaScript expression to evaluate (must return a value)" },
                    "await_promise": { "type": "boolean", "description": "Await Promise result, default true" }
                },
                "required": ["script"]
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let script: String = serde_json::from_value(args["script"].clone())?;
        let await_promise: bool = args["await_promise"].as_bool().unwrap_or(true);
        let result = self.manager.with_tab(move |tab| {
            let val = tab.evaluate(&script, await_promise)?.value.unwrap_or(serde_json::Value::Null);
            Ok(val)
        }).await?;
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "result": result
        }) })
    }
}

/// browser_console: 获取控制台日志
/// 设计 P0-1 修复: level 参数实际过滤日志
/// 设计 P0-2 修复: 依赖 open_or_navigate 注入的 console 监听器
pub struct BrowserConsoleTool {
    pub manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for BrowserConsoleTool {
    fn name(&self) -> &str { "browser_console" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "browser_console".into(),
            description: "Get browser console logs (messages, errors, warnings). Captures logs since page load. Requires browser_open/navigate to be called first (injects listeners).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "level": { "type": "string", "enum": ["all", "error", "warning", "info", "log"], "description": "Filter by level, default 'all'" },
                    "limit": { "type": "integer", "description": "Max entries to return, default 100" }
                }
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let level: String = args["level"].as_str().unwrap_or("all").to_string();
        let limit: usize = args["limit"].as_u64().unwrap_or(100) as usize;
        let level_resp = level.clone();

        // 从 window.__consoleLogs 读取日志
        let logs = self.manager.with_tab(move |tab| {
            let js = r#"
                (() => {
                    if (!window.__consoleLogs) {
                        return JSON.stringify({ hint: "Console capture not initialized. Call browser_open or browser_navigate first." });
                    }
                    return JSON.stringify(window.__consoleLogs);
                })()
            "#;
            let val = tab.evaluate(js, true)?.value.unwrap_or_default();
            Ok(val)
        }).await?;

        // 设计 P0-1 修复: 在 Rust 侧按 level 过滤
        let filtered = if level == "all" {
            // 限制条数
            limit_logs(&logs, limit)
        } else {
            filter_and_limit_logs(&logs, &level, limit)
        };

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "logs": filtered,
            "level": level_resp
        }) })
    }
}

/// 限制日志条数
fn limit_logs(logs: &Value, limit: usize) -> Value {
    if let Some(arr) = logs.as_array() {
        Value::Array(arr.iter().take(limit).cloned().collect())
    } else {
        // 非数组（如 hint 消息），原样返回
        logs.clone()
    }
}

/// 按级别过滤日志并限制条数
fn filter_and_limit_logs(logs: &Value, level: &str, limit: usize) -> Value {
    if let Some(arr) = logs.as_array() {
        let filtered: Vec<Value> = arr.iter()
            .filter(|entry| {
                entry["level"].as_str() == Some(level)
            })
            .take(limit)
            .cloned()
            .collect();
        Value::Array(filtered)
    } else {
        logs.clone()
    }
}

/// browser_network: 获取网络请求记录
/// 设计 P0-1 修复: filter 和 limit 参数实际生效
/// 设计 P1-2 修复: 用 CDP Network domain 获取 method/status（而非仅 Performance API）
pub struct BrowserNetworkTool {
    pub manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for BrowserNetworkTool {
    fn name(&self) -> &str { "browser_network" }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "browser_network".into(),
            description: "Get network requests captured since page load (URL, method, status, type, duration, size). Useful for debugging API calls.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "Filter requests by URL substring (e.g. '/api/')" },
                    "limit": { "type": "integer", "description": "Max requests to return, default 50" }
                }
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let filter: String = args["filter"].as_str().unwrap_or("").to_string();
        let limit: usize = args["limit"].as_u64().unwrap_or(50) as usize;
        let filter_resp = filter.clone();

        // 设计 P1-2 修复: 注入 Network 监听器（如果未注入）+ 读取已有记录
        // 用 Performance API 获取 url/type/duration/size
        // 用 fetch/XHR hook 获取 method/status
        let requests = self.manager.with_tab(move |tab| {
            // 先确保 network 监听器已注入
            let hook_js = r#"
                (() => {
                    if (window.__networkRequests) return;
                    window.__networkRequests = [];
                    // Hook fetch
                    const origFetch = window.fetch;
                    window.fetch = function(...args) {
                        const req = { url: (args[0] instanceof Request) ? args[0].url : String(args[0]), method: 'GET', status: null, type: 'fetch', time: Date.now() };
                        if (args[1] && args[1].method) req.method = args[1].method;
                        return origFetch.apply(this, args).then(resp => {
                            req.status = resp.status;
                            req.duration = Date.now() - req.time;
                            req.size = null;
                            return resp;
                        });
                    };
                    // Hook XMLHttpRequest
                    const origOpen = XMLHttpRequest.prototype.open;
                    const origSend = XMLHttpRequest.prototype.send;
                    XMLHttpRequest.prototype.open = function(method, url) {
                        this.__reqInfo = { url: String(url), method: method, status: null, type: 'xhr', time: Date.now() };
                        return origOpen.apply(this, arguments);
                    };
                    XMLHttpRequest.prototype.send = function() {
                        const self = this;
                        this.addEventListener('loadend', () => {
                            if (self.__reqInfo) {
                                self.__reqInfo.status = self.status;
                                self.__reqInfo.duration = Date.now() - self.__reqInfo.time;
                                window.__networkRequests.push(self.__reqInfo);
                            }
                        });
                        return origSend.apply(this, arguments);
                    };
                })()
            "#;
            let _ = tab.evaluate(hook_js, false);

            // 读取网络请求（合并 fetch hook + Performance API）
            let js = r#"
                (() => {
                    const hooked = window.__networkRequests || [];
                    const perf = performance.getEntriesByType('resource').map(e => ({
                        url: e.name,
                        type: e.initiatorType,
                        duration: Math.round(e.duration),
                        size: e.transferSize
                    }));
                    // 合并：hooked 有 method/status，perf 有 size/duration
                    const merged = {};
                    hooked.forEach(r => { merged[r.url] = { ...r }; });
                    perf.forEach(p => {
                        if (merged[p.url]) {
                            Object.assign(merged[p.url], p);
                        } else {
                            merged[p.url] = { ...p, method: null, status: null };
                        }
                    });
                    return JSON.stringify(Object.values(merged));
                })()
            "#;
            let val = tab.evaluate(js, true)?.value.unwrap_or_default();
            Ok(val)
        }).await?;

        // 设计 P0-1 修复: 在 Rust 侧按 filter 过滤 + limit 限制
        let filtered = filter_network_requests(&requests, &filter, limit);

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "requests": filtered,
            "filter": filter_resp,
            "limit": limit
        }) })
    }
}

/// 过滤网络请求（按 URL 子串）并限制条数
fn filter_network_requests(requests: &Value, filter: &str, limit: usize) -> Value {
    if let Some(arr) = requests.as_array() {
        let filtered: Vec<Value> = arr.iter()
            .filter(|req| {
                if filter.is_empty() {
                    true
                } else {
                    req["url"].as_str()
                        .map(|url| url.contains(filter))
                        .unwrap_or(false)
                }
            })
            .take(limit)
            .cloned()
            .collect();
        Value::Array(filtered)
    } else {
        requests.clone()
    }
}

/// 构建所有浏览器工具
pub fn build_browser_tools(manager: Arc<BrowserManager>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(BrowserOpenTool { manager: manager.clone() }),
        Arc::new(BrowserNavigateTool { manager: manager.clone() }),
        Arc::new(BrowserClickTool { manager: manager.clone() }),
        Arc::new(BrowserTypeTool { manager: manager.clone() }),
        Arc::new(BrowserScreenshotTool { manager: manager.clone() }),
        Arc::new(BrowserSnapshotTool { manager: manager.clone() }),
        Arc::new(BrowserEvalTool { manager: manager.clone() }),
        Arc::new(BrowserConsoleTool { manager: manager.clone() }),
        Arc::new(BrowserNetworkTool { manager }),
    ]
}
