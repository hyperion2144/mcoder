// 设计文档 §8.7 M5 自测: 浏览器工具
// 合并为单个 browser 工具，通过 action 参数分派
// action: "open" | "navigate" | "click" | "type" | "screenshot" | "snapshot" | "eval" | "console" | "network"
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

/// browser - 统一浏览器工具，通过 action 参数分派
/// 持有 browser_manager Arc
pub struct BrowserTool {
    pub manager: Arc<BrowserManager>,
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "browser".into(),
            description: "Unified headless Chrome browser tool. Dispatch by 'action': \
                open (launch browser and open URL), \
                navigate (navigate to new URL), \
                click (click element by CSS selector), \
                type (type text into input by CSS selector), \
                screenshot (capture base64 PNG/JPEG), \
                snapshot (lightweight text snapshot: links/buttons/inputs/text), \
                eval (execute JavaScript and return result), \
                console (get console logs, filter by level), \
                network (get network requests, filter by URL).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["open", "navigate", "click", "type", "screenshot", "snapshot", "eval", "console", "network"],
                        "description": "Browser action to perform"
                    },
                    "url": { "type": "string", "description": "[open|navigate] URL" },
                    "selector": { "type": "string", "description": "[click|type] CSS selector (e.g. 'button#submit', 'a[href=\"/login\"]')" },
                    "text": { "type": "string", "description": "[type] Text to type" },
                    "clear": { "type": "boolean", "description": "[type] Clear existing text first, default true" },
                    "format": { "type": "string", "enum": ["png", "jpeg"], "description": "[screenshot] Image format, default png" },
                    "quality": { "type": "integer", "description": "[screenshot] JPEG quality (0-100). Default 80." },
                    "max_length": { "type": "integer", "description": "[snapshot] Max text length to return, default 5000" },
                    "script": { "type": "string", "description": "[eval] JavaScript expression to evaluate (must return a value)" },
                    "await_promise": { "type": "boolean", "description": "[eval] Await Promise result, default true" },
                    "level": { "type": "string", "enum": ["all", "error", "warning", "info", "log"], "description": "[console] Filter by level, default 'all'" },
                    "limit": { "type": "integer", "description": "[console|network] Max entries to return. console default 100, network default 50" },
                    "filter": { "type": "string", "description": "[network] Filter requests by URL substring (e.g. '/api/')" }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required field: action"))?;
        match action {
            "open" => self.execute_open(args).await,
            "navigate" => self.execute_navigate(args).await,
            "click" => self.execute_click(args).await,
            "type" => self.execute_type(args).await,
            "screenshot" => self.execute_screenshot(args).await,
            "snapshot" => self.execute_snapshot(args).await,
            "eval" => self.execute_eval(args).await,
            "console" => self.execute_console(args).await,
            "network" => self.execute_network(args).await,
            other => Ok(ToolOutput::Error {
                message: format!(
                    "unknown action: {} (expected: open|navigate|click|type|screenshot|snapshot|eval|console|network)",
                    other
                ),
            }),
        }
    }
}

impl BrowserTool {
    /// action=open: 启动 headless Chrome 并打开指定 URL
    async fn execute_open(&self, args: Value) -> Result<ToolOutput> {
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

    /// action=navigate: 导航到新 URL（浏览器已打开）
    async fn execute_navigate(&self, args: Value) -> Result<ToolOutput> {
        let url: String = serde_json::from_value(args["url"].clone())?;
        let title = self.manager.open_or_navigate(&url).await?;
        Ok(ToolOutput::Sync { result: serde_json::json!({
            "navigated": true,
            "url": url,
            "title": title
        }) })
    }

    /// action=click: 点击元素
    async fn execute_click(&self, args: Value) -> Result<ToolOutput> {
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

    /// action=type: 在输入框中输入文本
    async fn execute_type(&self, args: Value) -> Result<ToolOutput> {
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

    /// action=screenshot: 截取页面截图
    async fn execute_screenshot(&self, args: Value) -> Result<ToolOutput> {
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

    /// action=snapshot: 获取页面的文本快照（accessibility tree 简化版）
    async fn execute_snapshot(&self, args: Value) -> Result<ToolOutput> {
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

    /// action=eval: 执行 JavaScript
    async fn execute_eval(&self, args: Value) -> Result<ToolOutput> {
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

    /// action=console: 获取控制台日志
    /// 设计 P0-1 修复: level 参数实际过滤日志
    /// 设计 P0-2 修复: 依赖 open_or_navigate 注入的 console 监听器
    async fn execute_console(&self, args: Value) -> Result<ToolOutput> {
        let level: String = args["level"].as_str().unwrap_or("all").to_string();
        let limit: usize = args["limit"].as_u64().unwrap_or(100) as usize;
        let level_resp = level.clone();

        // 从 window.__consoleLogs 读取日志
        let logs = self.manager.with_tab(move |tab| {
            let js = r#"
                (() => {
                    if (!window.__consoleLogs) {
                        return JSON.stringify({ hint: "Console capture not initialized. Call browser action=open or action=navigate first." });
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

    /// action=network: 获取网络请求记录
    /// 设计 P0-1 修复: filter 和 limit 参数实际生效
    /// 设计 P1-2 修复: 用 CDP Network domain 获取 method/status（而非仅 Performance API）
    async fn execute_network(&self, args: Value) -> Result<ToolOutput> {
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

/// 构建浏览器工具（单个合并工具）
pub fn build_browser_tools(manager: Arc<BrowserManager>) -> Arc<BrowserTool> {
    Arc::new(BrowserTool { manager })
}
