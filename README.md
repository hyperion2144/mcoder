# mcoder

A self-hosted, multi-client coding agent platform. Run your own AI coding assistant with full control over models, tools, and data.

## Features

- **Multi-protocol LLM support** — compatible with OpenAI, Anthropic, Gemini, and OpenAI Responses APIs. Mix models per role (coder / reviewer / planner).
- **Rich tool ecosystem** — file editing, bash execution, AST-aware refactoring (rename / extract / inline), code graph queries, memory store, sandboxed code execution, plan & workflow management, subagents, and undo via file journal.
- **Tree-sitter code intelligence** — symbol extraction and cross-reference tracking across 13+ languages (Rust, JS/TS, Python, Go, C/C++, Java, Ruby, C#, Bash, JSON, CSS, HTML).
- **LSP integration** — semantic-level rename and diagnostics via Language Server Protocol.
- **Browser & Computer Use** — headless Chrome automation and desktop interaction (screenshot, click, type) for self-testing workflows.
- **Multi-project sessions** — one server, many projects. Sessions are organized by project path and stored globally under `~/.mcoder/sessions/`.
- **Three client runtimes** — TUI (terminal), Desktop (Tauri), Mobile (Capacitor). All share a unified Catppuccin Mocha design system.
- **Secure transport** — WebSocket with TLS via self-signed certs or automatic Let's Encrypt (ACME) certificates for domain deployments.
- **Local-first** — all state (sessions, memory, code graph, journal) lives in SQLite under `~/.mcoder/`. No data leaves your machine except LLM API calls.

## Architecture

```
mcoder/
├── mcoder/              # Rust backend (agent server)
│   └── src/
│       ├── agent/       # Agent loop, async tasks, roles
│       ├── browser/     # Headless Chrome tools
│       ├── code_graph/  # Tree-sitter symbol graph
│       ├── computer_use/# Desktop automation
│       ├── debug/       # DAP debugging subsystem
│       ├── llm/         # LLM adapters (OpenAI/Anthropic/Gemini)
│       ├── lsp/         # Language Server Protocol client
│       ├── memory/      # SQLite + FTS5 memory store
│       ├── persistence/ # Session JSONL + sandbox
│       ├── plugin/      # Hooks, MCP, skills
│       ├── tools/       # Tool implementations
│       ├── transport/   # WS server, TLS, ACME, HTTP
│       └── workflow/    # Roadmap / milestone tracking
├── mcoder-tui/          # Terminal client (React + Ink)
├── mcoder-desktop/      # Desktop client (Tauri + React)
└── mcoder-mobile/       # Mobile client (Capacitor + React)
```

## Quick Start

### Prerequisites

- Rust 1.75+ (toolchain stable)
- Node.js 18+ and npm
- A configured LLM provider (OpenAI / Anthropic / Gemini compatible)

### Build

```bash
cargo build --release
```

### Configure

Create `~/.mcoder/config.toml`:

```toml
default_model = "my-model"

[models.my-model]
name = "My Model"
protocol = "anthropic"          # or "openai" | "gemini" | "openai_responses"
api_key = "${MY_API_KEY}"       # supports env var expansion
base_url = "https://api.example.com/anthropic"  # /v1 auto-appended for anthropic if missing
context_window = 200000
max_tokens = 8192

[roles.coder]
model = "my-model"

[server]
host = "127.0.0.1"
port = 7654
```

### Run

Start the server (default `127.0.0.1:7654`):

```bash
./target/release/mcoder server
```

Connect a client:

```bash
# TUI
./target/release/mcoder tui

# Show pairing info (QR code + URL for mobile)
./target/release/mcoder pair

# List sessions
./target/release/mcoder sessions
```

### Domain deployment with auto TLS

```bash
mcoder server --domain coder.example.com --email you@example.com
```

Let's Encrypt certificates are auto-provisioned, persisted to `~/.mcoder/certs/`, and renewed 30 days before expiry.

## Tools

| Tool | Description |
|------|-------------|
| `ls` / `read` / `write` / `edit` / `grep` | File operations with journal-backed undo |
| `bash` | Shell execution with stdout/stderr capture |
| `ast_rename` / `ast_extract` / `ast_inline` | Tree-sitter / LSP based refactoring |
| `graph_index` / `graph_query` / `graph_find` / `graph_callers` / `graph_callees` / `graph_references` | Code graph queries |
| `memory_store` / `memory_search` / `memory_list` | Project & experience memory (FTS5) |
| `plan_create` / `todo` | Plan and todo management |
| `sandbox_read` | Sandboxed output reading |
| `undo` | Undo last file operation via journal |
| `workflow_create` / `workflow_query` | Roadmap and milestone tracking |
| `subagent` | Spawn background sub-agents |
| `browser_*` | Headless Chrome automation |
| `screen_*` / `click` / `type` | Desktop computer-use |

## Clients

### Desktop (Tauri)

```bash
cd mcoder-desktop
npm install
npm run tauri dev
```

### Mobile (Capacitor)

```bash
cd mcoder-mobile
npm install
npm run build
npx cap sync
npx cap open ios     # or android
```

### TUI

```bash
cd mcoder-tui
npm install
npm run build
node dist/index.js
```

## Configuration

- Config: `~/.mcoder/config.toml`
- Credentials: `~/.mcoder/credentials.toml`
- Sessions: `~/.mcoder/sessions/<escaped_project_path>/`
- Certificates: `~/.mcoder/certs/`
- Project state: `.mcoder/` in project root (journal, graph, memory, workflow)

### Environment variables

API keys support `${ENV_VAR}` expansion:

```toml
api_key = "${ANTHROPIC_API_KEY}"
```

## Development

```bash
# Build
cargo build

# Run tests
cargo test

# Run E2E tool tests (requires running server)
cd mcoder-tui && node e2e-tools-test.cjs
```

## License

MIT
