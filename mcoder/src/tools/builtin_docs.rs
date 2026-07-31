//! 内置文档：mcoder:// URI 解析
//!
//! URI 格式：
//! - mcoder://                  → 列出所有内置文档（标题 + 描述）
//! - mcoder://help              → 完整 help 文档（工具列表 + 使用说明）
//! - mcoder://config            → 完整 config 文档（所有配置项 + 示例）
//!
//! 文档用 Markdown 字符串直接嵌入到代码（无运行时文件读取）

use serde::{Deserialize, Serialize};

/// 内置文档元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinDocMeta {
    pub id: String,
    pub title: String,
    pub description: String,
}

/// 列出所有内置文档
pub fn list_builtin_docs() -> Vec<BuiltinDocMeta> {
    vec![
        BuiltinDocMeta {
            id: "help".to_string(),
            title: "mcoder Tool Reference".to_string(),
            description: "Complete list of built-in tools, their parameters, and usage patterns"
                .to_string(),
        },
        BuiltinDocMeta {
            id: "config".to_string(),
            title: "mcoder Configuration Guide".to_string(),
            description: "All configuration options in ~/.mcoder/config.toml with examples and default values".to_string(),
        },
    ]
}

/// 解析 mcoder:// URI，返回文档内容（Markdown 字符串）
/// 支持：
/// - "mcoder://" 或 "mcoder://list" / "mcoder://index" → 列出所有文档
/// - "mcoder://<id>" 或 "mcoder://<id>.md" → 返回指定文档（大小写不敏感）
/// - "mcoder:<id>" → 同上，兼容无 // 形式
/// - 找不到 → 返回 None
pub fn resolve_mcoder_uri(uri: &str) -> Option<String> {
    let path = uri
        .strip_prefix("mcoder://")
        .or_else(|| uri.strip_prefix("mcoder:"))?;
    let path = path.trim_start_matches('/').trim_end_matches('/');
    if path.is_empty() || path == "list" || path == "index" {
        return Some(format_list());
    }
    // 剥离 .md 后缀（兼容 mcoder://help.md 形式）+ 小写化（大小写不敏感）
    let id = path.strip_suffix(".md").unwrap_or(path).to_lowercase();
    match id.as_str() {
        "help" => Some(HELP_DOC.to_string()),
        "config" => Some(CONFIG_DOC.to_string()),
        _ => None,
    }
}

/// 已注册的内置文档 id 列表（用于错误提示）
pub fn known_doc_ids() -> &'static [&'static str] {
    &["help", "config"]
}

/// 格式化文档列表（Markdown）
fn format_list() -> String {
    let docs = list_builtin_docs();
    let mut out = String::from("# Built-in Documents\n\n");
    out.push_str("Use `read(\"mcoder://<id>\")` to fetch a document.\n\n");
    for doc in &docs {
        out.push_str(&format!(
            "- **{}** — `mcoder://{}`  \n  {}\n",
            doc.title, doc.id, doc.description
        ));
    }
    out
}

// ==================== help.md ====================

const HELP_DOC: &str = r#"# mcoder Tool Reference

This is the canonical reference for all built-in tools. Each entry lists the
tool name, purpose, parameters, and when to use it.

## File Operations

### read
Read a file, URL, or built-in document.

| Parameter | Type | Description |
|-----------|------|-------------|
| `path` | string | File path, `http(s)://` URL, or `mcoder://` URI (e.g. `mcoder://help`) |
| `start` | int | Start line (1-based, default 1) |
| `end` | int | End line (inclusive, optional) |
| `limit` | int | Max number of lines |
| `with_hashes` | bool | Prepend each line with content hash for `edit` tool (default true) |
| `format` | string | Force format: text \| url \| excel \| word \| ppt \| pdf \| image \| html \| archive \| directory |
| `depth` | int | Directory recursion depth (default 2) |
| `action` | string | `default` \| `more` \| `full` \| `original` |

Examples:
- `read("src/main.rs", start=1, end=50)` — read first 50 lines
- `read("https://example.com/api")` — fetch URL via web_fetch
- `read("mcoder://config")` — read built-in config guide
- `read("mcoder://")` — list built-in docs

### write
Write content to a file (overwrite). Creates parent dirs.

| Parameter | Type | Description |
|-----------|------|-------------|
| `file` | string | File path |
| `content` | string | File content |
| `create_only` | bool | Fail if file already exists (default false) |

### edit
Hash-anchored edit. One call can apply multiple operations across files.
Each item auto-detects operation from its fields:

| Fields provided | Operation |
|-----------------|-----------|
| `pattern` + `replacement` (with `start` + `end`) | sed-style |
| `start` (no `content`, no `pattern`) | delete (end optional) |
| `position` | insert (needs `anchor` + `content`) |
| `anchor` + `content` (no `position`) | replace (`expect` optional) |

Item fields: `file`, `anchor?`, `content?`, `expect?`, `position?`, `start?`,
`end?`, `pattern?`, `replacement?`, `flags?`

### glob
Find files matching a pattern.

| Parameter | Type | Description |
|-----------|------|-------------|
| `pattern` | string | Glob pattern (e.g. `**/*.rs`) |
| `path` | string | Root path (default cwd) |
| `type` | string | `file` \| `directory` \| `any` |
| `max_results` | int | Cap on matches (default 1000) |

### grep
Regex search over file contents.

| Parameter | Type | Description |
|-----------|------|-------------|
| `pattern` | string | Regex pattern |
| `path` | string | Root path |
| `glob` | string | File pattern to filter (e.g. `*.rs`) |
| `case_insensitive` | bool | Case-insensitive match |
| `output_mode` | string | `content` \| `files_with_matches` \| `count` |
| `-C` / `-A` / `-B` | int | Context lines around match |
| `head_limit` | int | Max matches |

## Execution

### bash
Run a shell command, return stdout + stderr.

| Parameter | Type | Description |
|-----------|------|-------------|
| `command` | string | Shell command |
| `cwd` | string | Working directory |
| `timeout_ms` | int | Timeout (default 30000) |
| `sandbox` | string | Optional sandbox profile |

### launch
Manage **background processes** (dev servers, watchers, long-running tasks).
Each process has an `id` (uuid short) and optional `name`. Output streams
into a 5000-line ring buffer accessible via `action=logs`.

| Action | Description |
|--------|-------------|
| `start` | Spawn a background process. Returns `{id, name?, pid, status}`. |
| `stop` | SIGTERM → timeout → SIGKILL. Cleans up by_name. |
| `status` | Get current state + pid + uptime. |
| `logs` | Get buffered stdout/stderr (with `tail` and `since_ts`). |
| `list` | List all processes in the current session. |

Start parameters: `command`, `cwd?`, `name?`, `env?` (object of strings).

### task
Spawn an async background task. Returns immediately; result retrievable via
`task.list` or polled with `task.get`.

## Code Intelligence

### lsp_*
- `lsp_hover(file, line, column)` — show type/doc at position
- `lsp_definition(file, line, column)` — go to definition
- `lsp_references(file, line, column)` — find all references
- `lsp_rename(file, line, column, new_name)` — rename symbol
- `lsp_formatting(file)` — format document
- `lsp_diagnostics(file?)` — get diagnostics (all files if omitted)

Supported languages: Rust (rust-analyzer), TypeScript/TSX
(typescript-language-server), Go (gopls), Python (pylsp).

### graph_search
Search the local code graph (symbols, references, file relationships).

### web_search / web_fetch
Search the web and fetch URL content.

| Provider | API Key Required | Free Tier |
|----------|------------------|-----------|
| Tavily | Yes | 1000/month |
| Serper (Google) | Yes | 2500/month |
| DuckDuckGo | No | Unlimited (rate-limited) |

Set in config: `[web_search] provider = "tavily" api_key = "..."`.

## Workflow & Tasks

### workflow
Drive the spec-driven workflow. Actions: `init`, `continue`, `step`,
`status`, `ff` (fast-forward loop), `roadmap` (initial planning).

### plan
Create / update a structured plan. The user approves the plan before it
executes.

### todo
In-session todo list. Persisted in `~/.mcoder/sessions/<id>/todo.sqlite`.
Use `add` / `update` / `remove` / `list` / `replace` / `clear_completed`.

## Browser & Desktop Automation

### browser_*
- `browser_navigate(url)` — open URL
- `browser_snapshot()` — get accessibility tree
- `browser_click(ref)` — click element
- `browser_type(ref, text)` — type into field
- `browser_screenshot()` — capture PNG

### screen_*, app_*, mouse_*, keyboard_*
OS-level automation. **Dangerous tools** — require explicit approval.

## Interactive

### ask_user
Block and wait for the user to answer a structured question. Useful when
multiple valid approaches exist and you need clarification.

## Tips for Agents

1. Always use `read` before `edit` to verify current content and line numbers.
2. Prefer `edit` over `write` for surgical changes — it preserves context
   and is reversible.
3. Use `glob` + `grep` to locate files before reading; large repos need
   targeted queries.
4. When launching long-running processes, give them a `name` so subsequent
   calls can reference them by name instead of id.
5. LSP diagnostics are pushed asynchronously after write/edit; you will
   see them as a context reminder on your next tool call.
6. Built-in docs (`mcoder://`) are always available; consult them when
   unsure about tool parameters or config options.

## Configuration Reference

For all configuration options, see `mcoder://config`.
"#;

// ==================== config.md ====================

const CONFIG_DOC: &str = r#"# mcoder Configuration Guide

mcoder reads configuration from `~/.mcoder/config.toml` on startup. All
sections are optional; defaults apply when a section is omitted.

## File Location

- macOS / Linux: `~/.mcoder/config.toml`
- Windows: `%USERPROFILE%\.mcoder\config.toml`

Use `mcoder config show` to print the active config (after defaults are
applied).

## Top-Level Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_model` | string | `"sonnet"` | Default model name (must exist in `models`) |
| `loop_max_iters` | int | `100` | Max tool-call iterations per agent turn |
| `log_level` | string | `"info"` | `trace` \| `debug` \| `info` \| `warn` \| `error` |

## Models

```toml
[models.sonnet]
provider = "anthropic"
model = "claude-sonnet-4-5"
api_key = "sk-ant-..."
max_tokens = 8000
temperature = 0.7

[models.gpt4]
provider = "openai"
model = "gpt-4"
api_key = "sk-..."
base_url = "https://api.openai.com/v1"   # optional
```

| Field | Type | Description |
|-------|------|-------------|
| `provider` | string | `anthropic` \| `openai` \| `custom` |
| `model` | string | Provider-specific model identifier |
| `api_key` | string | API key (or use env var `${ANTHROPIC_API_KEY}`) |
| `base_url` | string | Custom endpoint (for proxies / OpenAI-compatible APIs) |
| `max_tokens` | int | Max output tokens per request |
| `temperature` | float | Sampling temperature (0.0 - 1.0) |

Switch at runtime with `/model set <name>` (TUI slash command).

## Tools

```toml
[tools]
auto_approve = ["browser_navigate", "browser_snapshot"]

[tools.lsp_diagnostics]
post_write = true
wait_ms = 1500
min_severity = "warning"
max_results = 50

[tools.launch]
max_processes_per_session = 20
max_log_lines_per_process = 5000
default_stop_timeout_ms = 3000
```

### `auto_approve`
Tools listed here skip the confirmation prompt even when flagged dangerous.
Use sparingly — typically for safe read-only operations.

### `[tools.lsp_diagnostics]`
After `write` / `edit`, spawn a background LSP task. When LSP finishes
analyzing (after `wait_ms`), push diagnostics to:
1. The frontend (`LspDiagnostics` ServerEvent — inline error display)
2. The next tool call's LLM context (drained from `PendingDiagnosticsStore`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `post_write` | bool | `true` | Enable async diagnostics |
| `wait_ms` | int | `1500` | Wait time for LSP server (rust-analyzer: 1500, tsserver: 800) |
| `min_severity` | string | `"warning"` | Min severity: `error` \| `warning` \| `information` \| `hint` |
| `max_results` | int | `50` | Cap on diagnostics returned per file |

### `[tools.launch]`
Configures the `launch` tool (background process management).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_processes_per_session` | int | `20` | Cap on concurrent processes |
| `max_log_lines_per_process` | int | `5000` | Ring buffer size per process |
| `default_stop_timeout_ms` | int | `3000` | Time to wait after SIGTERM before SIGKILL |

## LSP

```toml
[lsp]
auto_start = true
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `auto_start` | bool | `true` | Start LSP server on first file access |

LSP server discovery uses PATH (`rust-analyzer`, `typescript-language-server`,
`gopls`, `pylsp`). Install the relevant language server for your project.

## Web Search

```toml
[web_search]
provider = "tavily"     # tavily | serper | duckduckgo
api_key = "tvly-..."
timeout_secs = 30
```

| Provider | Free Tier | Notes |
|----------|-----------|-------|
| Tavily | 1000/month | Best quality, returns clean snippets |
| Serper (Google) | 2500/month | Raw Google results |
| DuckDuckGo | Unlimited | No API key, rate-limited |

If `provider` is omitted or the API key is missing, falls back to DuckDuckGo.

## Image / Vision

```toml
[image]
description_timeout_secs = 8
max_image_size_mb = 20
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `description_timeout_secs` | int | `8` | Timeout for vision model to describe images |
| `max_image_size_mb` | int | `20` | Max size of image read from disk |

## Workflow

```toml
[workflow]
default_mode = "spec"
max_steps = 50
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_mode` | string | `"spec"` | Initial workflow mode |
| `max_steps` | int | `50` | Max workflow steps before forcing stop |

## MCP (Model Context Protocol)

```toml
[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
env = { ALLOWED_DIRS = "/tmp" }

[[mcp.servers]]
name = "github"
command = "mcp-server-github"
env = { GITHUB_TOKEN = "ghp_..." }
```

Each entry spawns an MCP server via stdio. Tools from these servers
appear as `<server_name>_<tool_name>` in the tool registry.

## Sessions

```toml
[sessions]
storage_dir = "~/.mcoder/sessions"
auto_archive_after_days = 30
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `storage_dir` | string | `~/.mcoder/sessions` | JSONL transcript storage |
| `auto_archive_after_days` | int | `30` | Archive sessions older than N days |

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `MCODER_HOME` | Override config directory (default `~/.mcoder`) |
| `MCODER_LOG` | Override log level (`trace`/`debug`/`info`/...) |
| `ANTHROPIC_API_KEY` | Anthropic API key (alternative to config) |
| `OPENAI_API_KEY` | OpenAI API key |
| `MCODER_MODEL` | Default model override |

## Examples

### Minimal Config

```toml
default_model = "sonnet"

[models.sonnet]
provider = "anthropic"
model = "claude-sonnet-4-5"
api_key = "sk-ant-..."
```

### Full Config with All Sections

```toml
default_model = "sonnet"
log_level = "info"
loop_max_iters = 100

[models.sonnet]
provider = "anthropic"
model = "claude-sonnet-4-5"
api_key = "sk-ant-..."
max_tokens = 8000

[models.gpt4]
provider = "openai"
model = "gpt-4"
api_key = "sk-..."

[tools]
auto_approve = ["browser_snapshot"]

[tools.lsp_diagnostics]
post_write = true
wait_ms = 1500

[tools.launch]
max_processes_per_session = 20
default_stop_timeout_ms = 3000

[lsp]
auto_start = true

[web_search]
provider = "tavily"
api_key = "tvly-..."

[image]
description_timeout_secs = 8

[workflow]
default_mode = "spec"
max_steps = 50

[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

[sessions]
storage_dir = "~/.mcoder/sessions"
auto_archive_after_days = 30
```

## Validation

After editing the config, run `mcoder config validate` to check for
syntax errors and missing required fields. Invalid configs cause mcoder
to fall back to defaults on startup.
"#;