# mcoder Code Wiki

> 本文档为 mcoder 项目的结构化代码百科，涵盖项目整体架构、主要模块职责、关键类与函数说明、依赖关系以及项目运行方式。

---

## 目录

1. [项目概述](#1-项目概述)
2. [项目整体架构](#2-项目整体架构)
3. [技术栈](#3-技术栈)
4. [目录结构](#4-目录结构)
5. [后端核心模块（mcoder/）](#5-后端核心模块mcoder)
   - [5.1 入口与 CLI（main.rs / lib.rs）](#51-入口与-climainrs--librs)
   - [5.2 核心类型（types.rs）](#52-核心类型typesrs)
   - [5.3 配置系统（config.rs）](#53-配置系统configrs)
   - [5.4 会话管理器（session_manager.rs）](#54-会话管理器session_managerrs)
   - [5.5 Agent 子系统（agent/）](#55-agent-子系统agent)
   - [5.6 LLM 适配器（llm/）](#56-llm-适配器llm)
   - [5.7 工具系统（tools/）](#57-工具系统tools)
   - [5.8 传输层（transport/）](#58-传输层transport)
   - [5.9 持久化层（persistence/）](#59-持久化层persistence)
   - [5.10 记忆系统（memory/）](#510-记忆系统memory)
   - [5.11 代码图谱（code_graph/）](#511-代码图谱code_graph)
   - [5.12 Tree-sitter 集成（tree_sitter/）](#512-tree-sitter-集成tree_sitter)
   - [5.13 LSP 客户端（lsp/）](#513-lsp-客户端lsp)
   - [5.14 调试子系统（debug/）](#514-调试子系统debug)
   - [5.15 浏览器与桌面自动化（browser/ & computer_use/）](#515-浏览器与桌面自动化browser--computer_use)
   - [5.16 插件系统（plugin/）](#516-插件系统plugin)
   - [5.17 工作流引擎（workflow/）](#517-工作流引擎workflow)
   - [5.18 技能与命令（skills/ & commands/）](#518-技能与命令skills--commands)
   - [5.19 支撑模块](#519-支撑模块)
6. [客户端应用](#6-客户端应用)
   - [6.1 TUI 终端客户端（mcoder-tui/）](#61-tui-终端客户端mcoder-tui)
   - [6.2 桌面客户端（mcoder-desktop/）](#62-桌面客户端mcoder-desktop)
   - [6.3 移动客户端（mcoder-mobile/）](#63-移动客户端mcoder-mobile)
7. [依赖关系](#7-依赖关系)
8. [关键数据流](#8-关键数据流)
9. [项目运行方式](#9-项目运行方式)
10. [测试体系](#10-测试体系)

---

## 1. 项目概述

**mcoder** 是一个自托管的多客户端 AI 编程代理平台。它允许用户运行自己的 AI 编程助手，对模型、工具与数据拥有完全控制权。

核心特性：

- **多协议 LLM 支持**：兼容 OpenAI Chat Completions、OpenAI Responses、Anthropic Messages、Google Gemini 以及 Ollama / OpenAI 兼容协议；可为不同角色（coder / reviewer / planner）混用模型。
- **丰富工具生态**：文件编辑、bash 执行、AST 感知重构（重命名 / 抽取 / 内联）、代码图谱查询、记忆存储、沙箱代码执行、计划与工作流管理、子代理、基于文件日志的撤销等。
- **Tree-sitter 代码智能**：跨 14 种语言（Rust、JS/TS、Python、Go、C/C++、Java、Ruby、C#、Bash、JSON、CSS、HTML）的符号抽取与交叉引用追踪。
- **LSP 集成**：通过 Language Server Protocol 实现语义级重命名与诊断。
- **浏览器与桌面自动化**：无头 Chrome 自动化与桌面交互（截图、点击、输入），用于自测试工作流。
- **多项目会话**：一个服务器，多个项目；会话按项目路径组织，全局存储于 `~/.mcoder/sessions/`。
- **三端客户端运行时**：TUI（终端）、Desktop（Tauri）、Mobile（Capacitor），共享统一的 Catppuccin Mocha 设计系统。
- **安全传输**：WebSocket + TLS（自签证书或 Let's Encrypt ACME 自动证书）。
- **本地优先**：所有状态（会话、记忆、代码图谱、日志）存于 `~/.mcoder/` 下的 SQLite，除 LLM API 调用外无数据外传。
- **三级权限系统**：YOLO（全部自动执行）/ Standard（默认；审批写操作）/ Strict（全部审批）。

---

## 2. 项目整体架构

mcoder 采用**单服务器 + 多客户端**的星形架构：

```
┌─────────────────────────────────────────────────────────────────┐
│                        mcoder server (Rust)                      │
│  ┌───────────────┐   ┌───────────────┐   ┌───────────────────┐  │
│  │  Transport    │   │ SessionManager│   │   AgentSession     │  │
│  │  WS + HTTP +  │──▶│ (会话调度核心) │──▶│  (agent loop)      │  │
│  │  TLS + ACME   │   │               │   │                    │  │
│  └──────┬────────┘   └───────┬───────┘   └─────────┬──────────┘  │
│         │ JSON-RPC           │                      │             │
│         │            ┌───────┴────────┐   ┌─────────┴──────────┐  │
│         │            │ ToolRegistry   │   │   LLM Adapters      │  │
│         │            │ (30+ 工具)     │   │ (OpenAI/Anthropic/  │  │
│         │            └───────┬────────┘   │  Gemini/Responses)  │  │
│         │                    │            └─────────────────────┘  │
│         │   ┌────────────────┼────────────────────┐               │
│         │   ▼                ▼                    ▼               │
│  ┌──────┴────────┐  ┌──────────────┐  ┌──────────────────────┐    │
│  │ persistence   │  │ code_graph   │  │ memory / lsp / debug │    │
│  │ (SQLite+JSONL)│  │ (tree-sitter)│  │ plugin / workflow    │    │
│  └───────────────┘  └──────────────┘  └──────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
          ▲ ws/wss + JSON-RPC          ▲ ws/wss + JSON-RPC
          │                            │
   ┌──────┴──────┐              ┌──────┴──────┐              ┌──────────┐
   │  mcoder-tui │              │mcoder-desktop│             │mcoder-mobile│
   │ (Ink/React) │              │ (Tauri/React)│             │(Capacitor) │
   └─────────────┘              └──────────────┘             └────────────┘
```

**架构要点：**

1. **后端是唯一的状态与算力中枢**：所有 LLM 调用、工具执行、持久化都在 Rust 服务器内完成；客户端仅负责 UI 与交互。
2. **WebSocket JSON-RPC 2.0 通信**：客户端通过 `ws://` 或 `wss://` 连接，首条消息为 `auth`（携带配对 token），后续为标准 JSON-RPC 请求/响应/通知。
3. **per-session + per-project 资源隔离**：每个会话有独立的 `CancellationToken`、`TaskManager`、`SessionStateStore`；每个项目有独立的 memory/journal/code_graph/lsp/debug/workflow 资源集合（`ProjectResources`）。
4. **三端共享逻辑层**：Desktop 与 Mobile 通过 `@mcoder/shared/*` 包别名直接复用 TUI 的 `WsClient`、Zustand stores、slash command 分发器、AskCard / PermissionCard 等逻辑代码，仅渲染层不同。

---

## 3. 技术栈

### 后端（mcoder/）

| 类别 | 技术 |
|------|------|
| 语言 | Rust 1.75+（edition 2021） |
| 异步运行时 | tokio（full features） |
| Web 框架 | tokio-tungstenite（WS）、自研 HTTP（flate2 gzip） |
| TLS | tokio-rustls + rustls + rcgen（自签）+ instant-acme（Let's Encrypt） |
| 序列化 | serde + serde_json + toml + serde_yaml |
| 数据库 | rusqlite（bundled，code_graph/memory/workflow）+ sqlx（async sqlite，session_state/async_tasks） |
| 代码解析 | tree-sitter 0.25 + 13 个语言 grammar crate |
| LLM | reqwest（rustls-tls） |
| Token 估算 | tiktoken-rs（cl100k_base） |
| 文档读取 | calamine（Excel）、pdf-extract、html2text、zip、tar、quick-xml |
| 浏览器 | headless_chrome |
| 桌面自动化 | enigo（键鼠）、screenshots（截屏）、image |
| CLI | clap v4 |
| 日志 | tracing + tracing-subscriber |

### 客户端

| 端 | 框架 | 状态管理 | 传输 |
|----|------|----------|------|
| TUI | React 18 + Ink 5 | Zustand 4 | ws 8（Node）/ 全局 WebSocket |
| Desktop | React 18 + Tauri v2 | Zustand 4 | @mcoder/shared（WsClient） |
| Mobile | React 18 + Capacitor 6 | Zustand 4 | @mcoder/shared（WsClient） |

---

## 4. 目录结构

```
mcoder/
├── Cargo.toml                 # Workspace 根（members = ["mcoder"]）
├── Cargo.lock
├── README.md
├── DESIGN.md                  # 三端 UI 设计规范（Catppuccin Mocha）
├── install.sh / install.ps1   # 安装脚本
│
├── mcoder/                    # Rust 后端（agent server，lib + bin 双 crate）
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs            # binary 入口（CLI：server/tui/pair/sessions/stop）
│   │   ├── lib.rs             # lib 入口（pub mod 声明，供集成测试）
│   │   ├── types.rs           # 核心类型（Message/ContentBlock/AppConfig/ModelConfig...）
│   │   ├── config.rs          # 配置加载/合并/保存、路径工具
│   │   ├── session_manager.rs # 会话管理器（调度核心，4500+ 行）
│   │   ├── ask_user.rs        # 结构化用户提问工具
│   │   ├── permission.rs      # 权限审批网关
│   │   ├── todo_gate.rs       # todo 闸门决策
│   │   ├── resume_policy.rs   # 会话恢复决策
│   │   ├── i18n.rs            # 后端国际化
│   │   ├── generation_fence.rs
│   │   ├── agent/             # Agent 循环、角色、压缩、异步任务
│   │   ├── llm/               # LLM 适配器（4 协议）
│   │   ├── tools/             # 工具实现 + 注册表
│   │   ├── transport/         # WS/HTTP/TLS/ACME/pairing/JSON-RPC
│   │   ├── persistence/       # JSONL + SQLite 持久化
│   │   ├── memory/            # SQLite + FTS5 记忆
│   │   ├── code_graph/        # tree-sitter 代码图谱
│   │   ├── tree_sitter/       # tree-sitter 语言注册
│   │   ├── lsp/               # LSP 客户端
│   │   ├── debug/             # DAP 调试
│   │   ├── browser/           # 无头 Chrome
│   │   ├── computer_use/      # 桌面自动化
│   │   ├── plugin/            # Hook + MCP
│   │   ├── workflow/          # 工作流引擎
│   │   ├── skills/            # 技能系统（builtin + 加载器）
│   │   ├── commands/          # slash 命令分发
│   │   └── utils/             # shell 工具
│   └── tests/                 # 集成测试（14 个）
│
├── mcoder-tui/                # 终端客户端（React + Ink）
├── mcoder-desktop/            # 桌面客户端（Tauri + React）
└── mcoder-mobile/             # 移动客户端（Capacitor + React）
```

---

## 5. 后端核心模块（mcoder/）

### 5.1 入口与 CLI（main.rs / lib.rs）

**文件**：[main.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/main.rs)、[lib.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/lib.rs)

项目采用 **lib + bin 双 crate 模式**：`lib.rs` 声明所有 `pub mod` 供集成测试引用（`mcoder::ask_user::...`），`main.rs` 仅做 CLI 入口。

`Cli` 通过 clap 定义子命令：

| 命令 | 说明 |
|------|------|
| `server` | 启动 agent server（`--host/--port/--domain/--email/--http_port/--web_dir/--detach`） |
| `tui` | 启动 TUI（自动拉起本地 server） |
| `pair` | 显示配对信息（QR 码 + URL） |
| `sessions` | 列出会话 |
| `stop` | 停止运行中的 daemon server |
| 无命令 | 嵌入式模式：后台拉起 server + 启动 TUI |

**关键函数：**

- `start_server_full()`：完整启动流程，依次初始化配置、经验库、插件管理器、`RoleRegistry`、`ToolRegistry`（`build_full_registry()`）、skills/commands 注册表、MCP servers、`SessionManager`，最后启动 HTTP server + WS server（含 TLS 决策）。
- `spawn_tui_process()`：按优先级查找 TUI 可执行文件（standalone 单文件 → `dist/index.js` → 全局 `mcoder-tui`）。
- `spawn_detached_server()` / `spawn_detached_child()`：跨平台守护进程化（Unix `setsid` / Windows `CREATE_NO_WINDOW | DETACHED_PROCESS`）。
- `try_acquire_server_lock()` / `read_lock_pid()`：基于 `~/.mcoder/server.lock` 的单实例锁（原子 `create_new`）。
- `process_is_alive(pid)`：跨平台进程存活检测（Unix `kill(pid,0)` / Windows `OpenProcess`）。

**启动兜底机制**：`default_model` 不存在或 adapter 创建失败时**仍启动 server**，仅 subagent 工具不可用；UI 通过 Setup Mode 引导用户添加 provider。

### 5.2 核心类型（types.rs）

**文件**：[types.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/types.rs)

集中定义后端所有核心数据结构。

**消息模型：**

- `Role` 枚举：`System / User / Assistant / Tool`。
- `ContentBlock` 枚举（`#[serde(tag="type")]`）：`Text { text }`、`ToolUse { id, name, args }`、`ToolResult { id, output }`、`Image { path, media_type }`。
- `Message` 结构体：含 `id`（uuid）、`parent_id`（消息树分叉）、`role`、`content: Vec<ContentBlock>`、`usage: Option<Usage>`、`display_only`（仅 UI 展示不送 LLM）。

**工具类型：**

- `ToolCall { id, name, args }`。
- `ToolOutput` 枚举：`Sync { result }` / `AsyncTask { task_id, handle, status_msg }` / `Error { message }`。
- `ToolSchema { name, description, parameters }`。

**取消信号：**

- `CancellationToken`：基于 `tokio::sync::watch` 的一次性触发、多次感知取消信号。`cancel()` 触发、`is_cancelled()` 同步检查、`cancelled().await` 异步等待、`child()` 创建子 token（父取消子也取消）。

**模型配置：**

- `ModelProtocol` 枚举：`OpenaiChat / OpenaiCompatible / OpenaiResponses / Anthropic / Gemini`。
- `ModelConfig`：`name / protocol / api_key / base_url / context_window` + 扩展生成参数（`temperature / max_tokens / top_p / top_k / frequency_penalty / presence_penalty / stop / thinking_depth / extra: HashMap`）。`supports(modality)` / `supports_image()` 判断输入模态。
- `ProviderConfig`：供应商级配置，含 `synthesize_model_config(model_name)` 共享方法（从 provider + model name 合成 `ModelConfig`）。
- `ModelParams`：per-model 参数覆盖（存储于 `ProviderConfig.model_params`）。
- `ThinkingDepth` 枚举（5 档：None/Low/Medium/High/Max）+ `thinking_to_native(protocol, depth)` 映射到各协议原生参数。

**应用配置：**

- `AppConfig`：顶层配置，含 `default_model / default_provider / providers / models / roles / loop_max_iters / compact / tui / server / hooks / mcp_servers / memory / tools / permission / language` 等。
- `PermissionConfig` + `PermissionLevel`（Yolo/Standard/Strict）+ `requires_approval(tool_name)` + `is_readonly_tool(tool_name)`。
- `CompactConfig`：上下文压缩策略（`strategy / threshold / keep_recent / keep_first / tool_results / summary_model / layered_summary` 等）。
- `protocol_schema(protocol)`：返回各协议参数的 JSON schema（供 UI 渲染控件）。

### 5.3 配置系统（config.rs）

**文件**：[config.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/config.rs)

- `global_config_dir()`：`$MCODER_HOME` 或 `~/.mcoder`。
- `project_config_dir(project)`：`<project>/.mcoder`。
- `global_experiences_db_path()`：`~/.mcoder/experiences/sqlite.db`（跨项目经验库）。
- `load_config(project?)`：深度合并全局 `~/.mcoder/config.toml` + 项目级 `<project>/.mcoder/config.toml`（table 递归合并、array 追加、scalar 覆盖）。错误明确报告而非静默吞掉。
- `expand_env_var(s)`：展开 `${ENV_VAR}`。**关键设计**：`load_config` 不展开，保留 `${ENV_VAR}` 字面量；由 `create_adapter` / `test_provider` 在使用时展开，`save_config` 写盘时保留占位符，避免明文 key 泄露。
- `save_config(config)`：原子写（tmp + rename）。
- `ensure_dirs(project?)`：创建全局/项目目录树并确保 `.mcoder/` 在 `.gitignore` 中。

### 5.4 会话管理器（session_manager.rs）

**文件**：[session_manager.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/session_manager.rs)

这是后端的**调度核心**，约 4500 行，负责会话生命周期、agent loop 执行、工具调度、事件广播、配置运行时变更等。

**核心结构：**

- `SessionManager`：持有 `sessions: RwLock<HashMap<String, Arc<SessionEntry>>>`、`tools`、`config: Arc<RwLock<AppConfig>>`（运行时可变）、`plugins`、`role_registry`、`experience_store`、`mcp_manager`、`project_resources`（per-project 资源缓存）、`command_dispatcher`、`event_tx: broadcast::Sender<ServerEvent>`、`ask_registry`、`permission_registry`、`lsp_diag_store`、`launch_manager`、`session_thinking_overrides`。
- `SessionEntry`：每个会话的运行时状态——`session: Mutex<AgentSession>`、`cancellation: CancellationToken`、`client_count`、`loop_running: AtomicBool`（CAS 防并发 loop）、`generation: AtomicU64`（fencing，防旧 loop 写新 loop 状态）、`last_unfinished_todo_fingerprint`、`todo_gate_strikes`、`task_manager`、`pending_injections`。
- `ProjectResources`：per-project 资源集合（memory_store / journal / code_graph / lsp_manager / debug_manager / workflow），通过 `get_or_create_resources()` 双检锁缓存。
- `SessionSnapshot`：attach 时返回的完整快照（session 元数据 + messages + todos + plan + pending_ask + tasks + context + can_resume）。

**`ServerEvent` 枚举**（广播给所有订阅 client）：`Message / ToolCallStart / ToolCallDone / SessionCreated / SessionList / PlanCreated / RoleChanged / ModelChanged / SessionDone / TodoUpdated / AskPending / AskAnswered / AskCancelled / PermissionPending / PermissionResolved / UsageUpdated / LspDiagnostics / LaunchOutput / LaunchExited / Error / ConfigUpdated / Custom`。

**关键方法：**

- `create_session(project, title, model_name)`：创建会话，解析模型、创建 LLM adapter、初始化 `AgentSession`、分配 `TaskManager`、持久化 `loop_state=idle`。
- `send_message(session_id, content)` / `send_message_with_images(...)`：CAS `loop_running` 防并发 → 注入已完成异步任务 → 处理 pending ask → 记忆自动召回 → 添加用户消息 → `spawn_run_loop`。
- `resume_session(session_id)`：基于 `resume_policy::decide_resume` 决策矩阵（Conflict/WaitingForUser/HealStopped/NoWork/Start），注入 `[session resumed]` 系统消息（含未完成 todo + interrupted task），复用 `spawn_run_loop`。
- `spawn_run_loop(sid, entry)`：每次 spawn 递增 `generation` token；旧 loop 在清理前先比较 generation，不等则短路（避免 clobbering 新 loop 状态）。
- `run_agent_loop(session_id)`：**agent 主循环**。每轮：检查取消 → 文件快照 batch → 注入已完成任务 → 注入 role 上下文 → BeforeLlmCall hook → `agent.run_once()`（LLM 调用，select 取消）→ 广播消息 + usage → 提取 tool_calls → **只读工具并发执行 / 写工具串行执行**（`split_tool_calls` + `execute_readonly_concurrent`）→ 写工具前权限审批 + role 白名单 + 危险工具确认 + BeforeToolCall/BeforeFileChange hook → 执行 → AfterToolCall/AfterFileChange hook → 每轮后 `maybe_compact` + `check_loop_condition` → todo gate 决策（最多 3 strikes）。
- `attach_session_with_offset(session_id, offset)`：attach 时若内存无则从 JSONL 重放；按 offset 返回增量消息；构造 `SessionSnapshot`。
- `checkout(session_id, message_id)`：消息树分支切换（更新 `current_head_id`，不剪枝）。
- `set_role` / `set_model`：运行时切换，持久化到 session_state 并广播。
- `approve_plan(session_id, action, edited_plan)`：approve/reject/edit，写 DB + 切 role + 设 loop_state。
- `answer_ask` / `cancel_ask`：ask_user 的 RPC 端，内存优先 + DB fallback（服务重启路径）。
- `submit_permission`：权限审批决议提交。
- Provider CRUD：`add_provider` / `update_provider` / `delete_provider` / `set_default` / `set_model_params` / `test_provider`，均在单次 write lock 内 read-modify-write（消除 TOCTOU），先 `save_config` 再替换内存再 `broadcast_config_updated`。
- `handoff` / `handoff_back`：交接文档生成 + 子 session 创建/注入。
- `shutdown()`：触发 OnStop hook，关闭所有 MCP/LSP/DAP server 与 launch 后台进程。

**关键不变量：**

1. `replace_config` 是唯一改 `self.config` 的入口；写路径顺序：`save_config` → `write lock` 替换 → `broadcast_config_updated`。
2. `AgentSession` 持有自己的 `Arc<ModelConfig>` clone，配置替换不影响已运行的 agent loop。
3. `loop_running` CAS + `generation` fencing 双重防并发。

### 5.5 Agent 子系统（agent/）

**文件**：[agent/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/agent/mod.rs)、[role.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/agent/role.rs)、[async_tasks.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/agent/async_tasks.rs)、[compaction.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/agent/compaction.rs)

#### AgentSession（mod.rs）

单个会话的 agent 状态机，持有 `session: JsonlSession`、`messages: Vec<Message>`、`model_config: Arc<ModelConfig>`、`llm: SharedLLM`、`tools`、`role_registry`、`cumulative_usage`、`current_head_id`（消息树末端）、`compaction_layers`。

关键方法：

- `run_once()`：取 `messages_along_head_path()`（消息树分支隔离，过滤 `display_only`）→ 非视觉模型过滤 Image 块 → 注入 workflow 状态 → `llm.chat()` → `process_response()`。
- `execute_tool(call, ctx)`：执行前后检查取消；处理图片 read 结果（视觉模型插入 Image user message，非视觉模型用 description）。
- `maybe_compact(cfg)`：多级压缩策略——渐进式（ToolResult 分级压缩 + Image 替换）→ layered summary → LLM 摘要中间段 → workflow 上下文重注入。
- `inject_role_context(session_state)`：plan/execute 注入 plan 状态，goal/loop 注入 todo 状态。
- `switch_role` / `set_model` / `ensure_system_prompt` / `refresh_system_prompt`：system prompt 分静态段（Identity + Principles + Extensions + AGENTS.md）与会话段（Date/Platform/CWD/Git），便于 LLM cache_control。

#### 角色系统（role.rs）

`Role { name, system_prompt, model?, allowed_tools, max_tokens?, max_iters?, timeout?, loop_condition? }`。`RoleRegistry` 预置 12 个内建角色：`default`（全工具，50 轮）、`plan`（只读 + graph + plan + memory，5 轮，loop_condition=plan_created）、`execute`（全工具）、`review`（只读，无 bash）、`goal`（100 轮）、`loop`（无限，3600s 超时）、`subagent`（600s）、`planner`/`executor`/`reviewer`（workflow 角色）、`codebase-scanner`、`vision`（1 轮，120s）。

#### 异步任务（async_tasks.rs）

`TaskManager`（per-session）：`spawn(name, args, f)` 立即写 DB（status=running）再 spawn tokio task；`TaskStatus` 枚举（Pending/Running/Completed/Failed/Cancelled/Interrupted）。`Interrupted` 状态在服务重启时由 `mark_orphans_interrupted` 原子标记，**绝不自动重跑**。

#### 上下文压缩（compaction.rs）

- `TokenCounter`：tiktoken `cl100k_base`，`OnceLock` 缓存，失败回退 4 字符/token。
- **Tool-aware 压缩**：按工具名分级——`compact_read_output`（头 25 + 尾 15 行）、`compact_bash_output`（保留全部 stderr + stdout 尾部）、`compact_grep_output`、`compact_launch_output` 等。
- **LLM 摘要**：`llm_summarize_messages`（20K 输入上限，≤500 词）、`summarize_middle_as_system`。
- **分层摘要**：`SummaryLayer`，超长 session 维护 N 层历史摘要。
- **图片压缩**：`strip_images_with_describe`（视觉模型异步生成 description）。

### 5.6 LLM 适配器（llm/）

**文件**：[llm/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/llm/mod.rs)、[anthropic.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/llm/anthropic.rs)、[openai.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/llm/openai.rs)、[openai_responses.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/llm/openai_responses.rs)、[gemini.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/llm/gemini.rs)、[retry.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/llm/retry.rs)

**核心 trait：**

```rust
#[async_trait]
pub trait LLMAdapter: Send + Sync {
    fn name(&self) -> &str;
    fn supports_tool_cache(&self) -> bool { false }
    async fn chat(&self, messages: &[Message], tools: &[ToolSchema], config: &ModelConfig) -> Result<LLMResponse>;
    async fn chat_stream(&self, messages: &[Message], tools: &[ToolSchema], config: &ModelConfig, tx: mpsc::Sender<LLMEvent>) -> Result<()>;
}
pub type SharedLLM = Arc<dyn LLMAdapter>;
```

**关键类型：**

- `LLMResponse { content: Option<String>, tool_calls: Vec<ToolCall>, usage: Option<Usage> }`。
- `Usage { prompt_tokens, completion_tokens, total_tokens, cache_read_input_tokens, cache_creation_input_tokens }`（后两个 Anthropic 专有）。
- `LLMEvent` 枚举（流式）：`ContentDelta / ToolCallStart / ToolCallDelta / ToolCallDone / Done`。

**`create_adapter(config)`** 工厂：展开 `${ENV_VAR}`，按 `ModelProtocol` 分派到对应 adapter，包成 `Arc`。

**四个 adapter 实现：**

| Adapter | 端点 | 鉴权 | 特性 |
|---------|------|------|------|
| `AnthropicAdapter` | `/v1/messages` | `x-api-key` + `anthropic-version` | prompt caching（system 块 + 最后一个 tool 加 `cache_control: ephemeral`）；thinking budget_tokens；`supports_tool_cache()=true` |
| `OpenAIAdapter` | `/chat/completions` | bearer | tool_calls 增量索引流式；`include_usage`；image_url data URI |
| `OpenAIResponsesAdapter` | `/v1/responses` | bearer | 新结构化 API；System→developer，Tool→user；function_call 输出项；类型化 SSE 事件 |
| `GeminiAdapter` | `generateContent` | `?key=` query | `systemInstruction` 顶层；functionDeclarations；id→name 预扫描映射；thinkingConfig 本地处理 |

**重试策略（retry.rs）：** `with_retry<F>()` 通用包装。网络错误重试 3 次（指数退避 1/2/4s）；HTTP 429 重试 3 次（`Retry-After` 头，上限 60s）；5xx 重试 1 次；JSON 解析失败重试 1 次；4xx（非 429）立即失败。`chat_stream` 路径不走重试。

### 5.7 工具系统（tools/）

**文件**：[tools/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/mod.rs)

**核心抽象：**

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn schema(&self) -> ToolSchema;
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput>;
}
pub type SharedTool = Arc<dyn Tool>;
pub struct ToolRegistry { tools: HashMap<String, SharedTool> }
```

**`ToolContext`**（每次执行由 SessionManager 构造，注入所有依赖）：`session_id / tool_call_id / project_path / project_dir / project_hash / journal / memory_store / experience_store / code_graph / lsp_manager / debug_manager / task_manager / workflow / session_state / event_tx / cancellation / app_config / mcp_manager / current_model / lsp_diag_store / launch_manager`。

**`build_full_registry()`** 注册 30+ 工具：

| 工具族 | 文件 | 工具 |
|--------|------|------|
| 文件 | [file.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/file.rs) | `read / write / edit / ls / grep` |
| Shell | [bash.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/bash.rs) | `bash` |
| Web | [web.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/web.rs) | `web_search / web_fetch` |
| 进程 | [launch.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/launch.rs) | `launch`（后台进程管理） |
| 代码图谱 | code_graph/tools.rs | `graph_search / graph_file_symbols / graph_index / graph_relations` |
| 记忆 | memory/tools.rs | `memory`（action=store/search/list） |
| AST | [ast_edit.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/ast_edit.rs) | `ast_edit`（rename/extract/inline） |
| 计划 | [plan.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/plan.rs) | `plan / todo` |
| 代码执行 | [code_exec.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/code_exec.rs) | `code_exec` |
| 沙箱 | [sandbox.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/sandbox.rs) | `sandbox_read` |
| 任务 | [task.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/task.rs) | `task`（query/get/cancel） |
| 撤销 | [undo.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/undo.rs) | `undo`（基于 journal） |
| 图片 | [image.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/image.rs) | `image`（view/send） |
| 子代理 | [subagent.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/subagent.rs) | `subagent`（late binding） |
| 工作流 | [workflow.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/workflow.rs) | `workflow` |
| 调试 | debug/tools.rs | `debug_*` |
| LSP | lsp/tools.rs | `lsp`（action=diagnose/hover/definition/references/rename/format） |
| 浏览器 | browser/tools.rs | `browser_*` |
| 桌面 | computer_use/ | `screen_* / click / type` |
| 提问 | [ask_user.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/ask_user.rs) | `ask_user` |
| 技能 | tools/mod.rs | `skill_use`（use/list） |
| MCP 元工具 | [mcp_meta.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/mcp_meta.rs) | `mcp_list / mcp_call` |
| 日志 | [journal.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tools/journal.rs) | `FileJournal`（文件变更日志，undo 支持） |

**`READONLY_TOOLS` 白名单**：只读工具可安全并发执行（`read / ls / grep / graph_* / memory_search / task / plan_query / subagent / lsp_diagnose` 等）。

### 5.8 传输层（transport/）

**文件**：[ws_server.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/transport/ws_server.rs)、[http_server.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/transport/http_server.rs)、[jsonrpc.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/transport/jsonrpc.rs)、[acme.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/transport/acme.rs)、[tls.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/transport/tls.rs)、[pairing.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/transport/pairing.rs)

#### WsServer

- `start_with_tls(host, port, session_mgr, tls_acceptor)`：绑定 TcpListener，决定 TLS（ACME acceptor → 自签 → 明文），spawn accept loop。
- **连接生命周期**：TLS 握手 → WS upgrade → **auth**（首条消息须为 `{"method":"auth","params":{"token"}}`，10s 超时）→ 发送 `session.welcome`（server_version + sessions + capabilities）→ 心跳（60s 超时，Ping/Pong/任意消息重置）。
- **事件循环**：`tokio::select!` 三分支——心跳超时 / 入站消息（`handle_request` 分派）/ `ServerEvent` 广播（按 `attached_session` 过滤，全局事件广播所有 client）。
- `check_attached_session()`：session-scoped RPC 的校验辅助，防跨 session 访问。

#### JSON-RPC 方法路由（`handle_request`）

主要 RPC 方法：`ping / sessions.list / sessions.create / session.attach / session.close / session.delete / sessions.messages / sessions.send / session.cancel / session.resume / session.tree / session.checkout / session.mode.set/get/list / session.model.set/get / session.approve / ask.pending/answer/cancel / tool.call / tools.list / task.list/cancel / config.get/set/list_models/list_providers/list_protocols/add_provider/update_provider/delete_provider/set_default/test_provider/set_model_params/get_model_params/get_protocol_schema/quick_thinking/set_language / session.list_children/handoff/handoff_back / command.call/list / server.stats/shutdown`。

#### HttpServer

- 路由：`GET /.well-known/acme-challenge/{token}`（ACME HTTP-01）、`GET /api/pairing`（非敏感配对信息）、`POST /api/shutdown`、`GET *`（静态文件 + SPA fallback）。
- 支持 HTTP/1.1 Keep-Alive、gzip 压缩、内容哈希缓存（`max-age=31536000, immutable`）、路径穿越防护。

#### TLS

- 自签证书：`load_or_generate_cert()`，SAN 含 `localhost` + `127.0.0.1` + 所有本机网卡 IP。
- `should_use_tls(tls_mode, host)`：Auto（本地不加密）/ On / Off。

#### ACME（Let's Encrypt）

- `request_certificate(domain, email, challenges)`：创建账户 → 下单 → HTTP-01 挑战 → 轮询 → 生成 CSR（ECDSA P-256）→ finalize → 持久化到 `~/.mcoder/certs/acme-{domain}.crt.pem`。
- `is_domain(host)`：判断是否为合法域名（含 `.`、非 IP、非 localhost）。

#### Pairing

- `PairingInfo { host, port, token, tls, pairing_string, urls }`。
- 配对串格式：`mcoder://<token>@<host>:<port>?tls=<auto|on|off>`。
- token 持久化到 `~/.mcoder/credentials.toml`（重启不变，避免失效已配对 client）。
- `render_qr(content)`：终端 QR 码（`qrcode` crate + Dense1x2 渲染）。

### 5.9 持久化层（persistence/）

**文件**：[mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/persistence/mod.rs)、[jsonl.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/persistence/jsonl.rs)、[session_state.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/persistence/session_state.rs)、[async_task_store.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/persistence/async_task_store.rs)、[sandbox.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/persistence/sandbox.rs)

- `DbPool = SqlitePool`（sqlx async，max 5 连接）。
- **JSONL 会话日志**（jsonl.rs）：`JsonlSession` 追加式日志，每条消息一行 JSON；`SessionMeta` 存为 `*.meta.json`（含 `session_id / project_path / title / model / current_head_id / parent_session_id / source / subagent_role`）。会话 ID 格式 `YYYYMMDD-<uuid8>`，存储于 `~/.mcoder/sessions/<escaped_project>/`。
- **per-session 统一 DB**（session_state.rs）：`SessionStateStore` 单一 `session_state.db`（per-project `<project>/.mcoder/`，全局 fallback）。全局 `POOL_CACHE` 保证同路径共享一个 SqlitePool。Schema：`todos / session_state / pending_ask / pending_plan / session_attrs / async_tasks / async_task_injections`。
  - Todos：`TodoInput / TodoRecord / TodoSummary`，状态 `pending|in_progress|completed|cancelled`，优先级 `high|medium|low`，**同一 session 至多一个 in_progress**。
  - Loop state：`idle|running|stopped|waiting_for_user` + stop_reason。
  - Key-value attrs：用于 role/model 快照复原。
  - Pending Ask/Plan：终态保留不删，首决议优先（`state='pending'` 才成功）。
- **异步任务持久化**（async_task_store.rs）：`AsyncTaskStore`，`AsyncTaskState`（Queued/Running/Completed/Failed/Cancelled/Interrupted）。`mark_orphans_interrupted` 重启时原子标记；`list_undelivered_terminal_tasks` 经 LEFT JOIN `async_task_injections` 找未注入任务；`mark_task_injected` 幂等。
- **沙箱输出**（sandbox.rs）：`SandboxOutput`，`store_output / get_output / read_range`。

### 5.10 记忆系统（memory/）

**文件**：[memory/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/memory/mod.rs)、[tools.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/memory/tools.rs)

跨会话持久记忆，两种 scope：

- `MemoryScope::Project`：per-project，按 `project_hash`（SHA-256 路径前 16 字节 hex）隔离。
- `MemoryScope::Experience`：全局跨项目共享（`~/.mcoder/experiences/sqlite.db`）。

`MemoryStore { conn: Mutex<Connection> }`：`memories` 表 + `memories_fts` FTS5 虚拟表（key/content/tags）+ AFTER INSERT/DELETE/UPDATE 触发器同步。方法：`store / update / delete / search(query, scope?, project_hash?, limit) / list_project / list_experiences`。

`MemoryTool`（`memory`）：统一工具 action=store/search/list。search 同时查项目记忆 + 全局经验并合并。

会话首条用户消息时触发**自动召回**（`inject_recalled_memory`），将相关记忆作为 system 消息注入。

### 5.11 代码图谱（code_graph/）

**文件**：[code_graph/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/code_graph/mod.rs)、[schema.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/code_graph/schema.rs)、[store.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/code_graph/store.rs)、[symbol_extractor.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/code_graph/symbol_extractor.rs)、[tools.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/code_graph/tools.rs)

基于 tree-sitter 的代码符号图谱，持久化到 SQLite。

- `Symbol { id, file_path, name, kind: SymbolKind, language, start_line/end_line, signature, doc_comment, parent_id }`。`SymbolKind`：Function/Method/Class/Struct/Enum/Interface/Trait/Module/Variable/Constant/TypeAlias/Import。
- `SymbolEdge { source_symbol_id, target_name, edge_type: EdgeKind, file_path, line, col }`。`EdgeKind`：Calls/Imports/Extends/Implements。边按 `target_name` 链接（目标无需先索引）。
- `CodeGraph::index_file(path)`：tree-sitter 解析 → 删旧符号+边 → 插入合成 `<module>` 符号 → 插入所有符号（构建 name→Vec<(id,start,end)> 映射，按行范围匹配同名符号）→ 插入边。`index_dir` 跳过 `target/node_modules/.git/dist/.mcoder`。
- `GraphStore`（SQLite）：`symbols / symbol_refs / file_meta / symbols_fts（FTS5）/ symbol_edges`。`needs_reindex(path, mtime)` 增量索引。
- **符号抽取器**：per-language `*_symbol_kind` 映射 + `extract_edges`（call/import/extends/implements）。`extract_last_ident` 剥离 `::`/`.`/`/` 路径。
- **工具**：`graph_search`（symbol/file）、`graph_file_symbols`、`graph_index`、`graph_relations`（callers/callees/references；callees 聚合同名 Function/Method 并标记 ambiguous）。

### 5.12 Tree-sitter 集成（tree_sitter/）

**文件**：[tree_sitter/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tree_sitter/mod.rs)、[languages.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/tree_sitter/languages.rs)

- `Language` 枚举：14 种（Rust/JavaScript/TypeScript/Python/Go/C/Cpp/Java/Ruby/CSharp/Bash/Json/Css/Html/Unknown）。`from_path` 扩展名映射 + `Makefile`/`Dockerfile`→Bash。`tree_sitter_language()` 返回各 grammar crate 的 `tree_sitter::Language`。
- `Parser`：基于 mtime 的缓存解析，`parse_file / get_line_hashes / find_hash / hash_range`（SHA-256 前 8 字节 hex）。

### 5.13 LSP 客户端（lsp/）

**文件**：[lsp/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/lsp/mod.rs)、[tools.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/lsp/tools.rs)、[diagnostics_store.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/lsp/diagnostics_store.rs)

管理 per-language LSP server 进程（rust-analyzer / typescript-language-server / gopls / pylsp），stdio JSON-RPC。

- `LspClient`：`initialize`（声明 didOpen/didChange/didClose/hover/definition/references/rename/formatting/diagnostic 能力）→ 文档同步（`did_open` 缓存文本+version=1，`did_change` 全量同步+version 单调递增）→ LSP ops（hover/definition/references/rename/formatting/diagnostics）。`read_loop` 解析 `Content-Length` 头 + JSON-RPC body，分派响应 + 处理 `publishDiagnostics`。
- `LspManager`：per-language 懒启动 client（双检锁），`ensure_open(path)` 幂等 didOpen（文件级锁）。
- `apply_text_edits(text, edits)`：逆序应用 TextEdit，UTF-16 字符偏移→UTF-8 字节偏移转换。
- `LspTool`（`lsp`）：统一工具 action=diagnose/hover/definition/references/rename/format。rename/format 写盘后记 journal + did_change 同步。
- **异步诊断**：`PendingDiagnosticsStore`（per-session 队列）。write/edit 后台 LSP 任务 push 诊断，SessionManager 在下次 tool call 前 `drain` 拼成 system message 注入 LLM context。

### 5.14 调试子系统（debug/）

**文件**：[debug/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/debug/mod.rs)、[tools.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/debug/tools.rs)

DAP（Debug Adapter Protocol）调试管理。`DebugManager` 管理 DAP adapter，`debug_*` 工具支持启动/断点/步进/状态查询。`debug_get_state` 是只读操作。

### 5.15 浏览器与桌面自动化（browser/ & computer_use/）

**文件**：[browser/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/browser/mod.rs)、[computer_use/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/computer_use/mod.rs)

- **browser/**：`BrowserManager` 基于 `headless_chrome`，提供 `browser_open/navigate/snapshot/click/type/scroll/eval` 等工具。
- **computer_use/**：基于 `enigo`（键鼠模拟）+ `screenshots`（截屏）+ `image`，提供 `screen_*` / `click` / `type` 工具。

这两类工具默认为**危险工具**（`is_dangerous`：`browser_*` / `screen_*` / `app_*` 前缀），需用户确认或加入 `[tools] auto_approve` 白名单。

### 5.16 插件系统（plugin/）

**文件**：[plugin/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/plugin/mod.rs)、[hooks.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/plugin/hooks.rs)、[mcp.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/plugin/mcp.rs)

#### Hook 系统

- `HookPoint` 枚举：`OnStart / BeforeLlmCall / AfterLlmCall / BeforeToolCall / AfterToolCall / OnSessionCreate / OnSessionEnd / BeforeFileChange / AfterFileChange / OnStop / PreCompact`。
- `HookContext { hook, session_id, data }`；`HookResult { allow, modified_data?, message? }`（`allow=false` 中止后续 hook + 原操作）。
- `PluginManager`：`run_hooks(point, ctx)` 顺序执行 handler，传播 `modified_data`，遇 `allow=false` 停止。
- `ShellHookHandler`：从 `[[hooks]]` 配置加载，`substitute(ctx)` 替换 `$SESSION_ID/$TOOL/$FILE/$ARGS`，`sh -c`（Unix）/ `cmd /C`（Windows）执行。

#### MCP（Model Context Protocol）

- `McpClient`：JSON-RPC 2.0 over stdio。`start` spawn 进程 → `initialize`/`initialized`/`tools/list` 握手（protocolVersion `2024-11-05`）。`start_sse` 走 SSE transport（GET `/sse` 拿 endpoint，POST 发请求）。
- `McpToolWrapper`：把 MCP tool 包成 mcoder `Tool`，命名 `mcp__{server}__{tool}`。
- `McpManager`：`start_all(servers)` 按 config 选 transport（`url`→SSE，`command`→stdio），返回 `Vec<SharedTool>`。工具不直接注册，通过 `mcp_list`/`mcp_call` 元工具发现。

### 5.17 工作流引擎（workflow/）

**文件**：[workflow/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/workflow/mod.rs)、[types.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/workflow/types.rs)、[store.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/workflow/store.rs)、[orchestrator.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/workflow/orchestrator.rs)、[context.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/workflow/context.rs)、[traceability.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/workflow/traceability.rs)

spec 驱动的 5 阶段工作流（propose→plan→apply→review→archive），7+1 种 artifact（RM/MS/CH/PR/DS/SP/T/RV），序列编号，文件系统存 artifact 内容 + SQLite 存元数据。

- `WorkflowPhase`：Propose/Plan/Apply/Review/Archive。
- `WorkflowProfile`：Lite（顺序/可选 TDD/任意 review pass）/ Standard（并行/强制 TDD/全部 pass）。
- `WorkflowStore`：`next_id(artifact_type)` 事务计数器（`"RM-1"` 等），`transition_phase`（propose→...→archive，进 Archive 设 status=completed）。
- `WorkflowOrchestrator::schedule_for_phase`：Plan→planner，Apply→executor，Review→reviewer（自动 spawn subagent）。
- `context.rs`：`read_workflow_state`（纯磁盘派生，无状态机）+ `build_compact_context`（≤4KB XML block）。
- `traceability.rs`：`verify_traceability` 解析 artifact 中的 `PR-N`/`DS-N`/`T-N`/`SHALL-N` 定义与 `refs:`/`spec_ref:` 引用，报告 orphan（定义未引用）与 missing_refs（引用未定义）。

### 5.18 技能与命令（skills/ & commands/）

**文件**：[skills/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/skills/mod.rs)、[commands/mod.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/commands/mod.rs)

- **skills/**：`SkillRegistry` 加载全局 `~/.mcoder/skills/` + 项目 `.mcoder/skills/` 的 Skill（文件夹 + SKILL.md，支持渐进式披露）。内建技能：`commit / debug / explain / review / simplify / tdd`。`skill_use` 工具让 LLM 调用（use/list）。
- **commands/**：`CommandDispatcher` 分发 slash 命令（元命令 + 自定义命令 + user-invocable skills）。`DispatchResult`：`meta / custom_command / skill / unknown`。自定义命令从 `~/.mcoder/commands/` + `.mcoder/commands/` 加载。

### 5.19 支撑模块

**文件**：[ask_user.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/ask_user.rs)、[permission.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/permission.rs)、[todo_gate.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/todo_gate.rs)、[resume_policy.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/resume_policy.rs)、[i18n.rs](file:///Users/mutou/vault/projects/mcoder/mcoder/src/i18n.rs)

- **ask_user.rs**：结构化用户提问。`AskRequest { questions }`（1-4 题，每题 2-4 选项，Single/Multi 模式）。`AskRegistry` per-session 单 pending，`submit_validated` 原子校验+DB+notify。`verify_or_cancel_restart_pending_ask` 重启安全纯函数（拒绝给无主 ToolUse 追加孤儿 ToolResult）。
- **permission.rs**：`PermissionRegistry` per-session per-request 审批池。`check_and_wait(cfg, session_id, call)` 若需审批则注册 pending、发 Pending、await notify/60s 超时（自动 deny）。
- **todo_gate.rs**：`decide_todo_gate` 纯函数，最多 3 strikes；fingerprint（`status|priority|content`）变化重置 strike；MAX_STRIKES 后 `FinishWithReminder`（不自动 cancel todo）。
- **resume_policy.rs**：`decide_resume` 纯函数决策矩阵（running→Conflict；waiting_for_user 无 pending→HealStopped；有 pending→WaitingForUser；未完成 todo/interrupted task/特定 stop_reason→Start；否则 NoWork）。
- **i18n.rs**：`t(key, lang)` 翻译用户可见文本（en/zh），LLM prompt 保持英文。`current_lang()` / `set_current_lang()` 运行时缓存。

---

## 6. 客户端应用

三端客户端共享统一的 Catppuccin Mocha 设计系统（参见 [DESIGN.md](file:///Users/mutou/vault/projects/mcoder/DESIGN.md)）：5 种角色色（interaction=warning / execution=accent / thinking=mauve / done=muted / error），8 倍数间距节奏，`ShimmerText` 流光 loading，统一卡片词汇。Desktop 与 Mobile 通过 `@mcoder/shared/*` 包别名复用 TUI 的 `WsClient`、Zustand stores、slash command 分发器、AskCard/PermissionCard 等逻辑代码。

### 6.1 TUI 终端客户端（mcoder-tui/）

**技术栈**：React 18 + Ink 5 + Zustand 4 + ws 8。

**WebSocket JSON-RPC 客户端**（[rpc/client.ts](file:///Users/mutou/vault/projects/mcoder/mcoder-tui/src/rpc/client.ts)）：`WsClient` 平台无关（`Transport` 接口，优先 `globalThis.WebSocket`，回退 `require('ws')`）。握手发 `auth`（id:0），ack 后启 30s ping 心跳；指数退避重连（最多 5 次），重连后 `session.attach` 带 `offset=currentMessageCount` 增量补推。

**状态管理**（Zustand stores）：
- `useSessionStore`：连接状态、sessions、currentSessionId、role/model、context 用量、pendingPlan/todos、loopState、lspServers 等。
- `useMessagesStore`：messages、streaming、expandedToolCalls、inputHistory（上下键导航）。
- `useUiStore`：currentView（chat/sessions/todos/tasks/config/help/diff/tree/model/setting/provider/thinking）。
- `useAskStore` / `usePermissionStore`：pending ask/permission 卡片状态。

**关键组件**（[components/](file:///Users/mutou/vault/projects/mcoder/mcoder-tui/src/components)）：`MessageList`、`ToolCallCard`、`AskUserCard`、`PermissionCard`、`PlanApproval`、`SessionList`、`TodoView`、`TodoSummaryBar`、`TaskMonitor`、`ConfigView`、`ProviderView`、`ModelView`、`ThinkingPicker`、`TreeView`、`CommandPicker`、`InputBox`、`ResumeBar`、`SubagentBar`、`ShimmerText`。

**通知路由**：`client.onNotification` 将 `message / tool_call_* / session.* / permission.* / config_updated / error` 路由到 stores；ask/permission 通知插入占位 `tool_use` 块（虚拟工具名 `ask_user` / `__permission_pending__`）使卡片内联渲染。

**Slash 命令**（[commands/index.ts](file:///Users/mutou/vault/projects/mcoder/mcoder-tui/src/commands/index.ts)）：`dispatchSlashCommand` 剥离 `/`，部分客户端拦截（`/thinking`、`/handoff`、`/lang`），其余转发 `command.call`。

### 6.2 桌面客户端（mcoder-desktop/）

**技术栈**：Tauri v2 + React 18 + Zustand 4 + lucide-react。

**Tauri 后端**（[src-tauri/src/main.rs](file:///Users/mutou/vault/projects/mcoder/mcoder-desktop/src-tauri/src/main.rs)）：两个 `#[tauri::command]`：
- `get_server_info`：TCP 探测 `127.0.0.1:7654`，未运行则定位 `mcoder` 二进制（PATH → `~/.cargo/bin/mcoder` → `~/.mcoder/mcoder`）并 spawn `mcoder server --detach`，轮询 15s；读 `~/.mcoder/credentials.toml` 取 token，返回 `{url, token}` 实现零配对自动连接。
- `stop_server`：`POST /api/shutdown`，回退 `mcoder stop`。

**前端**：三栏布局——左 `FileTree`（240px）、中聊天区、右面板（Graph/Diff/Tree/文件预览，400px）。两阶段导航：ProjectList → Sessions 视图。复用 `@mcoder/shared/*` 全部逻辑。Settings modal 含 General / Providers 两 tab。

**关键组件**：`ProjectList`、`SessionTabs`、`FileTree`、`GraphView`（SVG 代码图谱）、`DiffViewer`（git diff 着色）、`TreeView`（消息树）、`PlanPanel`、`TodoPanel`、`ProviderPanel`（provider CRUD）、`CommandPicker`。

### 6.3 移动客户端（mcoder-mobile/）

**技术栈**：Capacitor 6 + React 18 + Zustand 4 + lucide-react。原生壳：`android/`（Gradle，`com.mcoder.mobile`）+ `ios/`（Xcode）。

**配对流程**（[PairingScreen](file:///Users/mutou/vault/projects/mcoder/mcoder-mobile/src/components/PairingScreen.tsx)）：文本输入 `mcoder://` 配对串 + "Scan QR Code" 按钮（`navigator.mediaDevices.getUserMedia` + 原生 `BarcodeDetector`，不支持时回退手动输入）。配对串持久化到 Capacitor Preferences（`mcoder_pairing` key）。

**移动端差异**：单列触摸友好布局；离线消息队列（断网时入队，重连回放）；底部 sheet 替代下拉（model/thinking 选择）；Drawer 抽屉列跨项目 session；IME 组合输入处理（防 CJK 输入闪烁）；全屏 Settings 页；大触摸区。

---

## 7. 依赖关系

### 7.1 后端模块依赖图

```
main.rs ──▶ config ──▶ types
   │           │
   ├──▶ session_manager ──▶ agent ──▶ llm ──▶ types
   │        │      │          │       │
   │        │      │          ├──▶ tools ──▶ code_graph / memory / lsp / debug / browser / computer_use / plugin
   │        │      │          ├──▶ compaction ──▶ llm
   │        │      │          └──▶ role
   │        │      ├──▶ persistence (jsonl / session_state / async_task_store / sandbox)
   │        │      ├──▶ plugin (hooks / mcp)
   │        │      ├──▶ workflow
   │        │      ├──▶ ask_user / permission / todo_gate / resume_policy
   │        │      ├──▶ skills / commands
   │        │      └──▶ i18n
   │        │
   │        └──▶ transport (ws_server ──▶ jsonrpc / http_server / tls / acme / pairing)
   │
   └──▶ transport::pairing
```

**关键依赖特性：**

- `SessionManager` 是依赖汇聚点，持有 tools/config/plugins/role_registry/experience_store/mcp_manager/command_dispatcher/ask_registry。
- `AgentSession` 依赖 `llm`（SharedLLM）、`tools`（ToolRegistry）、`role_registry`、`persistence::jsonl`。
- `ToolContext` 是工具执行的依赖注入容器，打破工具与具体项目的耦合。
- `SubagentTool` 使用 **late binding**（`set_dependencies`）解决循环依赖。

### 7.2 客户端依赖

- TUI 是逻辑源头；Desktop/Mobile 通过 `@mcoder/shared/*` 包别名（vite.config 配置）复用 TUI 的 `rpc/client`、`store`、`commands`、`utils/pairing`、`ask`、`permission`、`rpc/sessionSnapshot`、`store/clearSessionUiState`、`toolCard/ToolCardHtml`。
- 三端共享 `rpc/config.ts`（Provider CRUD 辅助，各端拷贝）。

### 7.3 主要外部依赖（workspace.dependencies）

| 用途 | crate |
|------|-------|
| 异步运行时 | tokio |
| 序列化 | serde / serde_json / toml / serde_yaml |
| 数据库 | rusqlite（bundled）/ sqlx（async sqlite） |
| WS | tokio-tungstenite / tungstenite |
| HTTP 客户端 | reqwest（rustls-tls） |
| TLS | tokio-rustls / rustls / rustls-pemfile / rcgen / instant-acme |
| Tree-sitter | tree-sitter + 13 语言 grammar |
| Token 估算 | tiktoken-rs |
| 浏览器 | headless_chrome |
| 桌面自动化 | enigo / screenshots / image |
| 文档读取 | calamine / pdf-extract / html2text / zip / tar / quick-xml |
| 图像编码 | base64 |
| 压缩 | flate2 |
| 错误 | anyhow / thiserror |

---

## 8. 关键数据流

### 8.1 用户消息 → Agent 响应

```
client.send("sessions.send", {session_id, content})
  └─▶ WsServer.handle_request
        └─▶ SessionManager.send_message
              ├─ CAS loop_running (防并发)
              ├─ inject_completed_tasks（注入已完成异步任务）
              ├─ try_handle_text_for_pending_ask（若 pending ask）
              ├─ inject_recalled_memory（记忆自动召回）
              ├─ add_message(user_msg) → JSONL 持久化 + 内存
              ├─ broadcast ServerEvent::Message
              └─ spawn_run_loop
                    └─ run_agent_loop（循环）
                          ├─ 检查 CancellationToken
                          ├─ agent.run_once()
                          │    ├─ messages_along_head_path（分支隔离）
                          │    ├─ llm.chat(messages, tools, config)
                          │    └─ process_response → add_message(assistant_msg)
                          ├─ broadcast Message + UsageUpdated
                          ├─ split_tool_calls（只读组 + 写组）
                          ├─ 只读工具并发执行（execute_readonly_concurrent）
                          ├─ 写工具串行执行：
                          │    ├─ 权限审批（permission_registry.check_and_wait）
                          │    ├─ role 白名单检查
                          │    ├─ 危险工具确认
                          │    ├─ BeforeToolCall / BeforeFileChange hook
                          │    ├─ agent.execute_tool(tc, ctx)
                          │    └─ AfterToolCall / AfterFileChange hook
                          ├─ maybe_compact（上下文压缩）
                          ├─ check_loop_condition
                          └─ todo_gate 决策（Finish/Continue/FinishWithReminder）
```

### 8.2 会话恢复（resume）

```
client.send("session.resume", {session_id})
  └─▶ SessionManager.resume_session
        ├─ load_session_from_jsonl（若内存无）
        ├─ 读取 DB loop_state / stop_reason
        ├─ 检测 interrupted tasks
        ├─ resume_policy::decide_resume（决策矩阵）
        │    ├─ Conflict → 409
        │    ├─ WaitingForUser → 返回 waiting
        │    ├─ HealStopped → 自愈 stopped
        │    ├─ NoWork → 返回 requires_user_input
        │    └─ Start → 继续
        ├─ CAS loop_running
        ├─ 注入 [session resumed] 系统消息（含未完成 todo + interrupted task）
        ├─ persist loop_state=running
        └─ spawn_run_loop
```

### 8.3 权限审批流

```
写工具执行前
  └─▶ PermissionConfig.requires_approval(tool_name)
        ├─ Yolo → None（除非在 yolo_deny）
        ├─ Standard → 只读 None / 写 Some(reason)
        └─ Strict → 非 auto/只读 Some(reason)
              └─▶ permission_registry.check_and_wait
                    ├─ 注册 pending + 发 PermissionPending 事件
                    ├─ client 渲染审批卡片
                    ├─ client.send("permission.submit", {decision})
                    ├─ notify 唤醒 waiter（或 60s 超时 auto-deny）
                    └─ 发 PermissionResolved 事件
```

---

## 9. 项目运行方式

### 9.1 前置条件

- Rust 1.75+（stable 工具链）
- Node.js 18+ 与 npm
- 一个已配置的 LLM provider（OpenAI / Anthropic / Gemini / Ollama 兼容）

### 9.2 构建后端

```bash
cargo build --release
```

### 9.3 配置

创建 `~/.mcoder/config.toml`：

```toml
default_model = "my-model"

[providers.openai-official]
name = "openai-official"
protocol = "openai"          # openai | openai_responses | anthropic | ollama | gemini | custom
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}" # 支持 ${ENV_VAR} 展开
models = ["gpt-4o", "gpt-4o-mini"]
enabled = true

[roles.coder]
model = "my-model"

[server]
host = "127.0.0.1"
port = 7654
```

兼容旧式扁平 `[models.<name>]` 格式（保留读取，不优先）。

### 9.4 运行服务器

```bash
# 前台模式（默认 127.0.0.1:7654）
./target/release/mcoder server

# 守护进程模式
./target/release/mcoder server --detach

# 域名部署 + 自动 TLS（Let's Encrypt）
mcoder server --domain coder.example.com --email you@example.com
```

HTTP 服务器（Web 客户端 + ACME 挑战）默认监听 `port + 1`。

### 9.5 连接客户端

```bash
# 嵌入式模式（无 subcommand）：后台拉起 server + 启动 TUI
./target/release/mcoder

# TUI（自动拉起本地 server）
./target/release/mcoder tui

# 显示配对信息（QR 码 + URL，用于 mobile）
./target/release/mcoder pair

# 列出会话
./target/release/mcoder sessions

# 停止 daemon
./target/release/mcoder stop
```

### 9.6 运行三端客户端

```bash
# Desktop（Tauri）
cd mcoder-desktop && npm install && npm run tauri dev

# Mobile（Capacitor）
cd mcoder-mobile && npm install && npm run build && npx cap sync && npx cap open ios  # 或 android

# TUI（开发模式）
cd mcoder-tui && npm install && npm run dev
# TUI（生产）
cd mcoder-tui && npm run build && node dist/index.js
# TUI 单文件可执行
cd mcoder-tui && ./build-standalone.sh
```

### 9.7 配置与数据位置

| 用途 | 路径 |
|------|------|
| 全局配置 | `~/.mcoder/config.toml` |
| 凭证 | `~/.mcoder/credentials.toml`（pairing_token） |
| 会话 | `~/.mcoder/sessions/<escaped_project>/` |
| 证书 | `~/.mcoder/certs/` |
| 全局经验库 | `~/.mcoder/experiences/sqlite.db` |
| 全局技能 | `~/.mcoder/skills/` |
| 项目状态 | `<project>/.mcoder/`（memory.db / graph.db / journal / workflow / session_state.db / skills / commands） |
| server 锁 | `~/.mcoder/server.lock` |
| server 日志 | `~/.mcoder/mcoder.log`（daemon 模式） |

环境变量：`MCODER_HOME` 覆盖全局配置目录；API key 支持 `${ENV_VAR}` 展开。

---

## 10. 测试体系

### 10.1 后端集成测试（mcoder/tests/）

14 个集成测试覆盖关键子系统：

| 测试文件 | 覆盖点 |
|----------|--------|
| `ask_user_registry.rs` / `ask_user_validation.rs` | ask_user 注册表与校验 |
| `generation_fencing.rs` | generation fencing 防 clobbering |
| `phase4_pending_persistence.rs` | pending ask/plan 持久化 |
| `phase5_async_tasks_persistence.rs` | 异步任务持久化 |
| `phase5c_*.rs`（5 个） | per-session 状态 DB、resume、attach、unified session state |
| `restart_ask_safety.rs` | 重启后 ask 安全 |
| `session_resume.rs` / `session_snapshot.rs` / `session_todos.rs` | 会话恢复、快照、todo |
| `final_server_p1.rs` | server 端 P1 |

运行：`cargo test`

### 10.2 TUI 测试

`mcoder-tui/` 下含 `test-ask.mjs`、`test-todo-summary.mjs`、`test-session-snapshot.ts`、`test-resume-state.ts`、`test-phase5c-*.ts` 等。

运行：`cd mcoder-tui && npm test`

### 10.3 E2E 工具测试

需运行中的 server：

```bash
cd mcoder-tui && node e2e-tools-test.cjs
cd mcoder-tui && node e2e-full-test.cjs
```

---

## 附录：设计规范参考

- [DESIGN.md](file:///Users/mutou/vault/projects/mcoder/DESIGN.md)：三端 UI 设计规范（Catppuccin Mocha、角色色、边框风格、间距节奏、Loading 流光、移除 AI 味清单、违规检查清单、运行时 Provider 管理）。
- [docs/superpowers/](file:///Users/mutou/vault/projects/mcoder/docs/superpowers)：设计文档与计划（mcoder M0 设计、设计文档）。
- [docs/permission.md](file:///Users/mutou/vault/projects/mcoder/docs/permission.md)：权限系统文档。
