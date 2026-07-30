// 设计文档 §8.7 M5 自测: Computer Use 工具集
// 合并为 2 个工具：screen（screenshot/click/type/key/scroll）、app（list/open/focus）
// 实现：enigo 0.2（键鼠）+ screenshots（截屏）+ 平台命令（应用管理）

use crate::tools::{Tool, ToolContext};
use crate::types::{ToolOutput, ToolSchema};
use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::Engine;
use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use serde_json::Value;
use std::sync::Arc;

/// 将字符串键名解析为 enigo::Key
fn parse_key(name: &str) -> Result<Key> {
    let key = match name.to_lowercase().as_str() {
        // 修饰键
        "ctrl" | "control" => Key::Control,
        "shift" => Key::Shift,
        "alt" | "option" => Key::Alt,
        "meta" | "cmd" | "command" | "super" => Key::Meta,
        // 特殊键
        "enter" | "return" => Key::Return,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "escape" | "esc" => Key::Escape,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        // F1-F12
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        // 单字符
        _ if name.chars().count() == 1 => {
            let c = name.chars().next().unwrap();
            Key::Unicode(c)
        }
        _ => anyhow::bail!("unknown key: '{}' (supported: ctrl/shift/alt/cmd, enter/tab/space/backspace/delete/esc, arrows, home/end/pageup/pagedown, f1-f12, or single character)", name),
    };
    Ok(key)
}

// ============================================================
// screen - 统一屏幕操作工具，通过 action 参数分派
// action: "screenshot" | "click" | "type" | "key" | "scroll"
// ============================================================
pub struct ScreenTool;

#[async_trait]
impl Tool for ScreenTool {
    fn name(&self) -> &str {
        "screen"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "screen".into(),
            description: "Unified screen interaction tool. Dispatch by 'action': \
                screenshot (capture main display as base64 PNG), \
                click (click at x,y coordinates), \
                type (type text at cursor), \
                key (press key/combo e.g. Cmd+C), \
                scroll (scroll at coordinates).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["screenshot", "click", "type", "key", "scroll"],
                        "description": "Screen action to perform"
                    },
                    "display": { "type": "integer", "description": "[screenshot] Display index (0=main), default 0" },
                    "x": { "type": "integer", "description": "[click|scroll] X coordinate (pixels from left)" },
                    "y": { "type": "integer", "description": "[click|scroll] Y coordinate (pixels from top)" },
                    "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "[click] Mouse button, default 'left'" },
                    "double": { "type": "boolean", "description": "[click] Double-click, default false" },
                    "text": { "type": "string", "description": "[type] Text to type" },
                    "keys": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "[key] Keys to press, e.g. [\"cmd\", \"c\"] or [\"enter\"]. Last key is the main key; preceding are modifiers held down."
                    },
                    "amount": { "type": "integer", "description": "[scroll] Scroll amount (positive=down, negative=up). Typical range -10 to 10." },
                    "direction": { "type": "string", "enum": ["vertical", "horizontal"], "description": "[scroll] Scroll direction, default 'vertical'" }
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
            "screenshot" => self.execute_screenshot(args).await,
            "click" => self.execute_click(args).await,
            "type" => self.execute_type(args).await,
            "key" => self.execute_key(args).await,
            "scroll" => self.execute_scroll(args).await,
            other => Ok(ToolOutput::Error {
                message: format!(
                    "unknown action: {} (expected: screenshot|click|type|key|scroll)",
                    other
                ),
            }),
        }
    }
}

impl ScreenTool {
    /// action=screenshot: 截取屏幕截图
    async fn execute_screenshot(&self, args: Value) -> Result<ToolOutput> {
        let display_idx: usize = args["display"].as_u64().unwrap_or(0) as usize;

        // 截屏操作是同步的且较快，用 spawn_blocking 避免阻塞 runtime
        let png_b64 = tokio::task::spawn_blocking(move || -> Result<String> {
            let screens = screenshots::Screen::all()
                .context("listing displays")?;
            let screen = screens.get(display_idx)
                .ok_or_else(|| anyhow::anyhow!("display index {} out of range ({} displays)", display_idx, screens.len()))?;
            let img = screen.capture()
                .context("capturing screen")?;
            // 编码为 PNG
            let mut buf = Vec::with_capacity(1024 * 768);
            let dyn_img = image::DynamicImage::ImageRgba8(img);
            dyn_img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                .context("encoding PNG")?;
            Ok(base64::engine::general_purpose::STANDARD.encode(&buf))
        }).await
            .context("spawn_blocking for screenshot")??;

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "screenshot": png_b64,
            "format": "png",
            "display": display_idx
        }) })
    }

    /// action=click: 点击屏幕坐标
    async fn execute_click(&self, args: Value) -> Result<ToolOutput> {
        let x: i32 = args["x"].as_i64().unwrap_or(0) as i32;
        let y: i32 = args["y"].as_i64().unwrap_or(0) as i32;
        let button_str: String = args["button"].as_str().unwrap_or("left").to_string();
        let double_click: bool = args["double"].as_bool().unwrap_or(false);

        let button = match button_str.as_str() {
            "right" => Button::Right,
            "middle" => Button::Middle,
            _ => Button::Left,
        };

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut enigo = Enigo::new(&Settings::default())
                .context("creating Enigo instance")?;
            enigo.move_mouse(x, y, Coordinate::Abs)
                .context("moving mouse")?;
            enigo.button(button, Direction::Click)
                .context("clicking")?;
            if double_click {
                std::thread::sleep(std::time::Duration::from_millis(50));
                enigo.button(button, Direction::Click)?;
            }
            Ok(())
        }).await
            .context("spawn_blocking for click")??;

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "clicked": true,
            "x": x,
            "y": y,
            "button": button_str,
            "double": double_click
        }) })
    }

    /// action=type: 输入文本
    async fn execute_type(&self, args: Value) -> Result<ToolOutput> {
        let text: String = serde_json::from_value(args["text"].clone())
            .context("text required")?;

        // 设计 P1-4 修复: 用字符数（chars().count()）而非字节数（len()）
        // 对中文等多字节字符，字符数更准确
        let char_count = text.chars().count();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut enigo = Enigo::new(&Settings::default())
                .context("creating Enigo instance")?;
            enigo.text(&text)
                .context("typing text")?;
            Ok(())
        }).await
            .context("spawn_blocking for type")??;

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "typed": true,
            "length": char_count
        }) })
    }

    /// action=key: 按键（支持组合键）
    async fn execute_key(&self, args: Value) -> Result<ToolOutput> {
        let keys_arr: Vec<String> = serde_json::from_value(args["keys"].clone())
            .context("keys array required")?;
        if keys_arr.is_empty() {
            anyhow::bail!("keys array must not be empty");
        }

        let keys_str = keys_arr.join("+");

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut enigo = Enigo::new(&Settings::default())
                .context("creating Enigo instance")?;

            // 解析所有键
            let parsed: Vec<Key> = keys_arr.iter()
                .map(|s| parse_key(s))
                .collect::<Result<Vec<_>>>()?;

            // 最后一个键是主键，前面的都是修饰键
            let (modifiers, main_key) = parsed.split_at(parsed.len() - 1);
            let main_key = main_key[0];

            // 按下所有修饰键
            for m in modifiers {
                enigo.key(*m, Direction::Press)?;
            }
            // 按下并释放主键
            enigo.key(main_key, Direction::Click)?;
            // 释放所有修饰键
            for m in modifiers {
                enigo.key(*m, Direction::Release)?;
            }
            Ok(())
        }).await
            .context("spawn_blocking for key")??;

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "pressed": true,
            "keys": keys_str
        }) })
    }

    /// action=scroll: 滚动
    async fn execute_scroll(&self, args: Value) -> Result<ToolOutput> {
        let x: i32 = args["x"].as_i64().unwrap_or(0) as i32;
        let y: i32 = args["y"].as_i64().unwrap_or(0) as i32;
        let amount: i32 = args["amount"].as_i64().unwrap_or(0) as i32;
        let direction: String = args["direction"].as_str().unwrap_or("vertical").to_string();
        let direction_resp = direction.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut enigo = Enigo::new(&Settings::default())
                .context("creating Enigo instance")?;
            // 先移动到指定位置
            if x != 0 || y != 0 {
                enigo.move_mouse(x, y, Coordinate::Abs)?;
            }
            let axis = match direction.as_str() {
                "horizontal" => enigo::Axis::Horizontal,
                _ => enigo::Axis::Vertical,
            };
            enigo.scroll(amount, axis)?;
            Ok(())
        }).await
            .context("spawn_blocking for scroll")??;

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "scrolled": true,
            "x": x,
            "y": y,
            "amount": amount,
            "direction": direction_resp
        }) })
    }
}

// ============================================================
// app - 统一应用管理工具，通过 action 参数分派
// action: "list" | "open" | "focus"
// ============================================================
pub struct AppTool;

#[async_trait]
impl Tool for AppTool {
    fn name(&self) -> &str {
        "app"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "app".into(),
            description: "Unified application management tool. Dispatch by 'action': \
                list (list installed applications), \
                open (open an app by name), \
                focus (bring app window to foreground).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "open", "focus"],
                        "description": "App action to perform"
                    },
                    "filter": { "type": "string", "description": "[list] Filter app names by substring (case-insensitive)" },
                    "name": { "type": "string", "description": "[open|focus] Application name (e.g. 'Safari', 'firefox')" }
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
            "list" => self.execute_list(args).await,
            "open" => self.execute_open(args).await,
            "focus" => self.execute_focus(args).await,
            other => Ok(ToolOutput::Error {
                message: format!(
                    "unknown action: {} (expected: list|open|focus)",
                    other
                ),
            }),
        }
    }
}

impl AppTool {
    /// action=list: 列出已安装应用
    async fn execute_list(&self, args: Value) -> Result<ToolOutput> {
        let filter: String = args["filter"].as_str().unwrap_or("").to_lowercase();

        #[cfg(target_os = "macos")]
        {
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg("ls /Applications /System/Applications 2>/dev/null | sed 's/\\.app$//' | sort -u")
                .output()
                .await
                .context("listing applications")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let apps: Vec<String> = stdout.lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .filter(|s| filter.is_empty() || s.to_lowercase().contains(&filter))
                .collect();
            Ok(ToolOutput::Sync { result: serde_json::json!({
                "apps": apps,
                "count": apps.len(),
                "platform": "macos"
            }) })
        }
        #[cfg(target_os = "linux")]
        {
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg("ls /usr/share/applications/*.desktop 2>/dev/null | xargs -I{} basename {} .desktop | sort -u")
                .output()
                .await
                .context("listing applications")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let apps: Vec<String> = stdout.lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .filter(|s| filter.is_empty() || s.to_lowercase().contains(&filter))
                .collect();
            Ok(ToolOutput::Sync { result: serde_json::json!({
                "apps": apps,
                "count": apps.len(),
                "platform": "linux"
            }) })
        }
        #[cfg(target_os = "windows")]
        {
            let output = tokio::process::Command::new("powershell")
                .args(&["-Command", "Get-ChildItem 'C:\\Program Files','C:\\Program Files (x86)' -Directory | Select-Object -ExpandProperty Name"])
                .output()
                .await
                .context("listing applications")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let apps: Vec<String> = stdout.lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .filter(|s| filter.is_empty() || s.to_lowercase().contains(&filter))
                .collect();
            Ok(ToolOutput::Sync { result: serde_json::json!({
                "apps": apps,
                "count": apps.len(),
                "platform": "windows"
            }) })
        }
    }

    /// action=open: 打开应用
    async fn execute_open(&self, args: Value) -> Result<ToolOutput> {
        let name: String = serde_json::from_value(args["name"].clone())
            .context("name required")?;

        #[cfg(target_os = "macos")]
        {
            let output = tokio::process::Command::new("open")
                .args(&["-a", &name])
                .output()
                .await
                .context("running 'open -a'")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("failed to open app '{}': {}", name, stderr.trim());
            }
        }
        #[cfg(target_os = "linux")]
        {
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(format!("xdg-open $(which {} 2>/dev/null || echo {})", name, name))
                .output()
                .await
                .context("running xdg-open")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("failed to open app '{}': {}", name, stderr.trim());
            }
        }
        #[cfg(target_os = "windows")]
        {
            let output = tokio::process::Command::new("cmd")
                .args(&["/C", "start", "", &name])
                .output()
                .await
                .context("running start")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("failed to open app '{}': {}", name, stderr.trim());
            }
        }

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "opened": true,
            "name": name
        }) })
    }

    /// action=focus: 聚焦应用窗口
    async fn execute_focus(&self, args: Value) -> Result<ToolOutput> {
        let name: String = serde_json::from_value(args["name"].clone())
            .context("name required")?;

        #[cfg(target_os = "macos")]
        {
            let script = format!("tell application \"{}\" to activate", name.replace("\"", "\\\""));
            let output = tokio::process::Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .output()
                .await
                .context("running osascript")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("failed to focus app '{}': {}", name, stderr.trim());
            }
        }
        #[cfg(target_os = "linux")]
        {
            let output = tokio::process::Command::new("wmctrl")
                .args(&["-a", &name])
                .output()
                .await
                .context("running wmctrl (is wmctrl installed?)")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("failed to focus app '{}': {} (install wmctrl for window management)", name, stderr.trim());
            }
        }
        #[cfg(target_os = "windows")]
        {
            let ps_script = format!(
                "$app = Get-Process -Name '{}' -ErrorAction SilentlyContinue | Select-Object -First 1; if ($app) {{ [Microsoft.VisualBasic.Interaction]::AppActivate($app.Id) }}",
                name
            );
            let output = tokio::process::Command::new("powershell")
                .args(&["-Command", &ps_script])
                .output()
                .await
                .context("running powershell")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("failed to focus app '{}': {}", name, stderr.trim());
            }
        }

        Ok(ToolOutput::Sync { result: serde_json::json!({
            "focused": true,
            "name": name
        }) })
    }
}

// ============================================================
// 构建所有 Computer Use 工具
// ============================================================
pub fn build_computer_use_tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(ScreenTool), Arc::new(AppTool)]
}
