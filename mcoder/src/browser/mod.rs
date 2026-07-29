// 设计文档 §8.7 M5 自测: 浏览器工具
// - 内置 headless Chrome（通过 CDP 控制）
// - 9 个工具：browser_open/navigate/click/type/screenshot/snapshot/eval/console/network
// - 用途：agent 自己启动前端 → 测试 → 截图分析 → 修 bug
// - token 节省：snapshot 比 screenshot 省
// - 安全：默认需用户确认每步操作（可配置白名单自动批准）

pub mod tools;

use anyhow::{Context, Result};
use headless_chrome::Browser as HeadlessBrowser;
use headless_chrome::LaunchOptions;
use headless_chrome::Tab;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 设计 P0-2 修复: 导航时注入的 console 监听器 JS
/// 拦截 console.log/error/warn/info 和 window.onerror，存到 window.__consoleLogs
const CONSOLE_HOOK_JS: &str = r#"
(() => {
    window.__consoleLogs = window.__consoleLogs || [];
    if (window.__consoleHooked) return;
    const orig = { log: console.log, error: console.error, warn: console.warn, info: console.info };
    ['log','error','warn','info'].forEach(level => {
        console[level] = (...args) => {
            window.__consoleLogs.push({
                level,
                args: args.map(a => {
                    try { return typeof a === 'object' ? JSON.stringify(a) : String(a); }
                    catch(e) { return String(a); }
                }),
                time: Date.now()
            });
            orig[level].apply(console, args);
        };
    });
    window.addEventListener('error', e => {
        window.__consoleLogs.push({
            level: 'error',
            args: [e.message + ' (' + (e.filename||'') + ':' + e.lineno + ')'],
            time: Date.now()
        });
    });
    window.__consoleHooked = true;
})()
"#;

/// 浏览器管理器：管理 headless Chrome 实例 + 活跃 tab
/// 懒启动 - 首次调用 browser_open 时才启动 Chrome
/// 维护活跃 tab 避免每次操作都开新 tab（设计 P2-2 修复）
/// headless_chrome 的操作是同步的，用 Mutex 保护 + spawn_blocking 执行（设计 P1-3 修复）
pub struct BrowserManager {
    browser: Mutex<Option<HeadlessBrowser>>,
    /// 活跃 tab URL（用于跟踪当前页）
    active_tab_url: Mutex<Option<String>>,
}

impl BrowserManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            browser: Mutex::new(None),
            active_tab_url: Mutex::new(None),
        })
    }

    /// 确保 Chrome 已启动
    /// 返回浏览器是否刚启动
    pub async fn ensure_started(&self) -> Result<bool> {
        let mut guard = self.browser.lock().await;
        if guard.is_some() {
            return Ok(false);
        }
        let browser = HeadlessBrowser::new(LaunchOptions {
            headless: true,
            ..Default::default()
        }).context("launching headless Chrome (is Chrome/Chromium installed?)")?;
        *guard = Some(browser);
        tracing::info!("headless Chrome started");
        Ok(true)
    }

    /// 打开或导航到 URL（在锁内同步执行，复用或新建 tab）
    /// 设计 P0-2 修复: 导航后注入 console 监听器
    /// 设计 P2-2 修复: 复用现有 tab 而非每次新建
    pub async fn open_or_navigate(&self, url: &str) -> Result<String> {
        self.ensure_started().await?;
        let guard = self.browser.lock().await;
        let browser = guard.as_ref().context("browser not started")?;

        // 复用现有 tab 或新建
        // headless_chrome: get_tabs() 返回 &Arc<Mutex<Vec<Arc<Tab>>>>
        let tabs_lock = browser.get_tabs();
        let tab = {
            let tabs = tabs_lock.lock().unwrap();
            if tabs.is_empty() {
                None // 释放锁后 new_tab
            } else {
                Some(tabs[0].clone())
            }
        };
        let tab = match tab {
            Some(t) => t,
            None => browser.new_tab().context("creating new tab")?,
        };

        // 同步操作（持 browser 锁，但 navigate+wait 通常在数秒内完成）
        tab.navigate_to(url).context("navigating")?;
        tab.wait_for_element("body").context("waiting for body")?;

        // 设计 P0-2 修复: 注入 console 监听器（在页面加载后尽早注入）
        let _ = tab.evaluate(CONSOLE_HOOK_JS, false);

        let title_val = tab.evaluate("document.title", true)
            .context("evaluating title")?
            .value
            .unwrap_or_default();
        let title = title_val.as_str().unwrap_or("").to_string();

        // 更新活跃 URL
        drop(guard);
        self.set_active_url(url.to_string()).await;

        Ok(title)
    }

    /// 在活跃 tab 上执行操作（spawn_blocking 中运行同步代码）
    /// 设计 P1-3 修复: 先 clone Tab，释放锁，再 spawn_blocking
    ///
    /// Tab 内部是 Arc，clone 是廉价的引用计数增加
    pub async fn with_tab<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Tab) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        self.ensure_started().await?;
        // 锁内获取 tab clone，立即释放锁
        let tab = {
            let guard = self.browser.lock().await;
            let browser = guard.as_ref().context("browser not started")?;
            let tabs_lock = browser.get_tabs();
            let tabs = tabs_lock.lock().unwrap();
            tabs.last()
                .ok_or_else(|| anyhow::anyhow!("no tab available (call browser_open first)"))?
                .clone()
        }; // 锁释放

        // spawn_blocking 执行同步操作，不阻塞 async runtime
        let result = tokio::task::spawn_blocking(move || f(&tab))
            .await
            .context("spawn_blocking for tab operation")??;
        Ok(result)
    }

    /// 更新活跃 tab URL
    pub async fn set_active_url(&self, url: String) {
        let mut guard = self.active_tab_url.lock().await;
        *guard = Some(url);
    }

    /// 获取活跃 tab URL
    pub async fn active_url(&self) -> Option<String> {
        self.active_tab_url.lock().await.clone()
    }

    /// 关闭浏览器
    pub async fn shutdown(&self) {
        let mut guard = self.browser.lock().await;
        *guard = None; // drop HeadlessBrowser 会自动杀进程
        tracing::info!("headless browser closed");
    }
}
