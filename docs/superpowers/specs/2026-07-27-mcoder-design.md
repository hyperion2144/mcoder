# mcoder 设计文档

**版本**: 0.1.0
**日期**: 2026-07-27
**状态**: Draft（待用户 review）

---

## 0. 概述

### 0.1 目标

构建一个个人使用的 coding agent 平台 **mcoder**，满足：

- **弱模型友好**：工具调用协议稳定，参数简洁，弱模型也能轻松调用
- **token 节省**：从工具设计、结果 sandbox、定义缓存、context 压缩等多维度节省
- **远程能力**：其他电脑/手机可连接指定机器上的 agent
- **模型可配置**：兼容 OpenAI / Anthropic / Gemini 三大协议，任意切换
- **多端**：TUI（M0）、桌面 GUI（M4）、移动端（M4）
- **代码智能**：集成 LSP、调试、代码图谱（AST 级）
- **自测**：内置浏览器工具、Computer Use
- **协作**：子代理、多代理通信、工作流（blueprint 式 spec 驱动开发）
- **扩展**：skills / commands / MCP / hooks / 插件系统
- **轻量**：性能好、客户端小（拒绝 Electron，用 Tauri + Capacitor）

### 0.2 技术栈决策

| 决策项 | 选择 | 理由 |
|---|---|---|
| 核心语言 | Rust | 性能最佳、Tauri 原生、tree-sitter/LSP/DAP 生态成熟 |
| 前端技术 | Web（React + TypeScript） | 与桌面/移动共享逻辑层 |
| TUI 框架 | Ink（Node） | React 组件模型，逻辑层可复用 |
| 桌面框架 | Tauri（M4） | 二进制小（~10MB），原生 Rust |
| 移动框架 | Capacitor（M4） | Web 技术打包，AI 写 Web 强 |
| 通信协议 | WebSocket + JSON-RPC | 双工、实时、NAT 友好、LSP/MCP 事实标准 |
| AST 解析 | tree-sitter | Rust 生态成熟，多语言支持 |
| 存储 | SQLite + FTS5 | 关系查询 + 全文检索 |
| 会话日志 | JSONL | append-only、流式、人类可读 |

### 0.3 参考项目

- **context-mode**（https://github.com/mksglu/context-mode）：sandbox 工具输出 + SQLite+FTS5 事件追踪 + "Think in Code" 思想
- **blueprint**（https://github.com/hyperion2144/blueprint）：Roadmap → Change spec 驱动开发，7 类 artifact，3 子代理（planner/executor/reviewer）

### 0.4 命名约定

- 程序名：`mcoder`
- CLI 命令：`mcoder`
- 默认项目目录：`.mcoder/`
- 全局目录：`~/.mcoder/`
- 配对协议：`mcoder://<token>@<host>:<port>?tls=<auto|on|off>`

---

## 1. 总体架构

### 1.1 架构变体选择

采用 **变体 A：模块化单二进制 server + 独立 Node TUI client**。

- `mcoder` server：单个 Rust 二进制，内部模块化
- `mcoder` TUI：Node 包（Ink），通过 WS 连本地或远程 server
- 嵌入式模式：`mcoder` 命令 fork server 子进程 + 启动 TUI

### 1.2 架构图

```
┌──────────────────────────────────────────────────────────────────────┐
│                        mcoder server (Rust)                          │
│                                                                      │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────────────────┐ │
│  │  Transport   │   │   Session    │   │      LLM Adapters        │ │
│  │  (WS+JSONRPC)│◄─►│  Manager     │◄─►│  OpenAI/Anthropic/Gemini │ │
│  │  + Pairing   │   │  (多会话)     │   │  (统一 trait)             │ │
│  └──────┬───────┘   └──────┬───────┘   └──────────┬───────────────┘ │
│         │                  │                      │                  │
│         │                  ▼                      ▼                  │
│  ┌──────┴──────────────────────────────────────────────────────────┐│
│  │                     Core Runtime                                ││
│  │  Agent Loop  │  Tool Registry  │  Tool Protocol Adapter         ││
│  │  Role System │  Async Tasks    │  (外部 JSON ↔ 内部 ToolCall)    ││
│  │  Plan/Goal/  │  (hash cache,   │                                 ││
│  │  Loop Modes  │  schema cache)  │                                 ││
│  └──────┬──────────────────────────────────────────────────────────┘│
│         │                                                            │
│         ▼                                                            │
│  ┌──────────────────────────────────────────────────────────────────┐│
│  │                     Tools (内置)                                 ││
│  │  bash(batch+sandbox) │ edit(hash+sed+batch) │ read(trunc+handle)││
│  │  write │ code_exec(subprocess+rlimit) │ plan │ todo │ task       ││
│  │  + File Change Journal (undo)                                    ││
│  └──────┬──────────────────────────────────────────────────────────┘│
│         │                                                            │
│         ▼                                                            │
│  ┌──────────────────────────────────────────────────────────────────┐│
│  │            Persistence (SQLite + FTS5)                           ││
│  │  sessions(jsonl) │ messages │ tool_calls │ tool_outputs │        ││
│  │  plans │ todos │ journal │ tasks │ memory(M1+) │ graph(M2+)     ││
│  └──────┬──────────────────────────────────────────────────────────┘│
│         │                                                            │
│         ▼                                                            │
│  ┌──────────────────────────────────────────────────────────────────┐│
│  │            tree-sitter (AST 解析 + 行 hash)                      ││
│  └──────────────────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────────────┘
                              ▲ WS+JSON-RPC
                              │
        ┌─────────────────────┼─────────────────────┐
        │                     │                     │
┌───────┴───────┐    ┌────────┴────────┐   ┌────────┴────────┐
│  TUI (Node)   │    │ Desktop (Tauri) │   │ Mobile (Capacitor)│
│   Ink         │    │   Web 前端       │   │   Web 前端        │
│   M0          │    │   M4            │   │   M4             │
└───────────────┘    └─────────────────┘   └──────────────────┘
```

### 1.3 关键设计原则

- **server 无状态外**：所有状态在 SQLite，server 重启可恢复
- **session 与项目绑定**：一个项目目录 = 一个 session 上下文根
- **1 项目 N 会话**：一个项目下可有多个会话，会话间独立但共享项目级状态
- **多 client 同步**：多 client 连同一 session，server 广播事件
- **工具调用全在 server 端执行**，client 只负责渲染

---

## 2. 存储分层

### 2.1 目录结构

```
~/.mcoder/                              # 全局
├── config.toml                         # 模型密钥、默认配置、role 配置
├── credentials.toml                    # 配对 token 等
├── sessions/                           # 所有项目的所有会话
│   └── <project_hash>/                 # 按项目路径 hash 分组
│       ├── <session_id>.jsonl          # 消息流（append-only）
│       └── <session_id>.meta.json      # 会话元数据
├── experiences/                        # 全局经验沉淀（M1+）
│   └── sqlite.db                       # FTS5 检索
└── pairing.db                          # 配对 token、活跃连接

<project>/.mcoder/                      # 项目级
├── config.toml                         # 项目级覆盖配置
├── memory/                             # 项目级跨会话记忆（M1+）
│   └── sqlite.db                       # FTS5 检索
├── sandbox/                            # 工具输出 sandbox
│   └── outputs.db                      # SQLite，按 handle 查
├── plans/                              # plan 模式产物
│   └── <plan_id>.json
├── journal/                            # 文件变更审计（undo）
│   └── journal.db
├── graph/                              # 代码图谱（M2+）
│   └── graph.db                        # SQLite，AST 节点
├── tree-sitter-cache/                  # AST 解析缓存
└── workflow/                           # 工作流 artifacts（M3+）
```

### 2.2 存储分工

| 数据 | 格式 | 理由 |
|---|---|---|
| 会话消息 | jsonl | append-only、流式、人类可读、易备份 |
| 工具 sandbox 输出 | SQLite | 按 handle 随机读取、可能很大 |
| plans/todos | json | 结构化、量小、可手编 |
| 文件变更 journal | SQLite | 时序、可查询、undo 链 |
| 代码图谱 | SQLite | 关系查询、AST 节点多 |
| 记忆/经验 | SQLite + FTS5 | 全文检索召回 |
| tree-sitter 缓存 | 文件 | 按 (file, mtime) 索引 |

### 2.3 session_id 生成

格式：`YYYYMMDD-HHMMSS-<short_uuid>`，可读且唯一。

session 文件第一行写入元数据（项目路径、创建时间、模型配置、标题等）。

### 2.4 多 client 同步

- server 维护每个 session 的内存状态（当前 agent loop、订阅的 client 列表）
- 消息追加到 jsonl 后广播给所有订阅 client
- 新 client 连接时先读 jsonl 历史再订阅新消息

---

## 3. 核心运行时

### 3.1 LLM Adapter（统一 trait）

```rust
#[async_trait]
pub trait LlmAdapter: Send + Sync {
    fn name(&self) -> &str;  // "openai" / "anthropic" / "gemini"
    
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        config: &ModelConfig,
    ) -> Result<ChatStream>;
    
    fn supports_tool_cache(&self) -> bool;
}

pub struct Message {
    pub role: Role,           // System / User / Assistant / Tool
    pub content: Vec<ContentBlock>,
}

pub enum ContentBlock {
    Text(String),
    ToolUse { id: String, name: String, args: Value },
    ToolResult { id: String, output: ToolOutput },
}

pub struct ChatStream {
    pub text_rx: mpsc::Receiver<String>,
    pub tool_call_rx: mpsc::Receiver<ToolCall>,
    pub usage: oneshot::Receiver<Usage>,
}
```

**关键设计**：
- 内部统一 `Message` / `ContentBlock` / `ToolCall`，adapter 负责与三家协议互转
- 流式响应拆两个 channel：文本流 + 工具调用流
- `supports_tool_cache()` 让 adapter 自报能力，core 据此决定是否每次重传完整 schema

### 3.2 工具协议（双层方案）

**外部**：模型输出原生 JSON 格式（OpenAI/Anthropic/Gemini 各自的 function calling 格式）

**内部**：统一 `ToolCall { name, args: Value }`

**adapter 翻译**：外部格式 ↔ 内部格式互转

**token 节省来源**（不靠改 wire format）：
1. 工具定义压缩（第一次发完整 schema，后续只发引用）
2. 工具设计本身省参数（hashline edit、auto-inference、bash 批量、sandbox 大输出）
3. 工具结果 sandbox（原始大输出不进上下文，只返回摘要 + handle）
4. 结果缓存（幂等调用复用结果）

### 3.3 Tool Registry

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    schema_cache: HashMap<String, ToolSchema>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn schema(&self) -> ToolSchema;
    fn short_desc(&self) -> &str;
    fn async_capable(&self) -> bool { false }
    
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput>;
}

pub struct ToolContext {
    pub session_id: String,
    pub project_path: PathBuf,
    pub sandbox: SandboxHandle,
    pub tree_sitter: TreeSitterHandle,
    pub journal: JournalHandle,
    pub cancellation: CancellationToken,
}
```

### 3.4 Role 系统

参考 OMP 思想，把 mode/subagent/type 统一为 role 概念。

```rust
pub struct Role {
    pub name: String,
    pub system_prompt: String,
    pub model: Option<ModelConfig>,
    pub allowed_tools: Vec<String>,
    pub max_tokens: Option<u32>,        // None = 无限
    pub max_iters: Option<u32>,
    pub timeout: Option<u32>,
    pub loop_condition: Option<String>, // 仅 loop role 使用
}

pub struct RoleRegistry {
    roles: HashMap<String, Role>,
}
```

**内置 role**：

| Role | 用途 | 默认模型策略 | 工具白名单 |
|---|---|---|---|
| `default` | 普通对话 | 全局默认 | 全部工具 |
| `plan` | plan 模式（规划） | 可配强模型 | read/graph/plan_create |
| `execute` | plan 执行阶段 | 可配快模型 | 全部工具 |
| `review` | review 阶段 | 可配强模型 | read/graph/bash(只读) |
| `goal` | goal 模式 | 全局默认 | 全部工具 + todo |
| `loop` | loop 模式 | 全局默认 | 全部工具 |
| `subagent` | 通用子代理（M3） | 可指定 | 调用时配置 |

**会话级 role 切换**：
- `/mode plan` = 切换到 plan role
- `/mode goal` = 切换到 goal role
- `/mode normal` = 回到 default
- plan approve 后自动切到 execute role

**配置示例**：
```toml
[roles.default]
model = "gpt-4o"

[roles.plan]
model = "claude-opus-4"
max_iters = 5

[roles.execute]
model = "deepseek-v3"
max_iters = 50

[roles.review]
model = "claude-opus-4"
max_iters = 10
```

### 3.5 Agent Loop

```rust
pub struct AgentLoop {
    adapter: Arc<dyn LlmAdapter>,
    registry: Arc<ToolRegistry>,
    session: SessionHandle,
    role: Role,
    task_manager: Arc<AsyncTaskManager>,
    max_iters: u32,           // 默认 50
    cancellation: CancellationToken,
}

impl AgentLoop {
    pub async fn run(&mut self, user_msg: String) -> Result<RunResult> {
        self.session.append_message(Message::user(user_msg))?;
        
        for iter in 0..self.max_iters {
            // 1. 注入 role 特定上下文（todo 状态、plan 状态等）
            self.inject_role_context();
            
            // 2. 注入已完成的后台任务结果
            let completed = self.task_manager.drain_completed();
            for (task_id, result) in completed {
                self.session.append_message(Message::task_completed(task_id, result))?;
            }
            
            // 3. 调用 LLM（流式）
            let stream = self.adapter.chat(
                self.session.messages(),
                self.registry.schemas_for(&self.role.allowed_tools),
                self.role.model.as_ref().unwrap_or_default(),
            ).await?;
            
            // 4. 收集响应
            let (text, tool_calls, usage) = collect_stream(stream).await?;
            self.session.append_message(Message::assistant(text, tool_calls.clone()))?;
            
            // 5. 没有工具调用且无 running 后台任务 → 结束
            if tool_calls.is_empty() && !self.task_manager.has_running() {
                return Ok(RunResult::Completed);
            }
            
            // 6. 执行工具（同步并发 + 异步后台）
            let results = self.execute_tools_mixed(tool_calls).await?;
            for r in &results {
                self.session.append_message(Message::tool_result(r))?;
            }
            
            // 7. 检查 loop mode 条件
            if let Some(condition) = self.role.loop_condition() {
                if self.check_loop_condition(condition)? {
                    return Ok(RunResult::Completed);
                }
            }
        }
        
        Ok(RunResult::MaxItersReached)
    }
}
```

### 3.6 三种模式行为

**Plan Mode**（role = plan → execute）：
```
用户消息 → agent 调用 plan_create(steps, files_affected)
→ server 暂停 loop，向所有 client 广播 PlanCreated 事件
→ 用户在 TUI approve / edit / reject
→ approve 后切换到 execute role，loop 继续
→ 每步执行完更新 plan 状态
→ 全部完成或用户中断时结束
```

**Goal Mode**（role = goal）：
- 每轮注入当前 todo 列表到 system message
- agent 自主调用 `todo` 工具增删改 todo
- 没有显式终止条件，靠模型主动停或 max_iters
- 适合开放式任务

**Loop Mode**（role = loop）：
- 配置 `loop_until: "no errors"` + `max_iters: 100`
- 每轮结束后用小 LLM 调用（或规则判断）检查条件
- 条件满足或超限则停
- 适合"反复跑测试直到全绿"等场景

### 3.7 异步任务

工具和子代理都可声明为 async 执行。

```rust
pub enum ToolOutput {
    Sync(ToolResult),
    AsyncTask {
        task_id: TaskId,
        handle: String,
        status_msg: String,
    },
}

pub struct AsyncTaskManager {
    tasks: HashMap<TaskId, AsyncTask>,
}

pub enum TaskStatus {
    Running,
    Completed(ToolResult),
    Failed(Error),
    Cancelled,
}
```

**调用方式**（智能判断 + 可覆盖）：
- 工具内部根据特征判断默认 sync/async（如 bash 长命令默认 async）
- 模型可传 `async: true/false` 显式覆盖

**任务完成通知**：
- 任务完成后结果作为新消息追加到 session
- 下一轮 LLM 调用自动看到
- 不打断当前 loop
- 若 loop 已结束，结果仍追加，下次用户消息时模型可见；不主动唤醒

**内置 task 管理工具**：
| 工具 | 作用 |
|---|---|
| `task_status(task_id)` | 查询状态 |
| `task_wait(task_id, timeout_ms)` | 阻塞等待 |
| `task_list(filter?)` | 列出所有任务 |
| `task_cancel(task_id)` | 取消任务 |

### 3.8 错误处理与重试

| 错误类型 | 策略 |
|---|---|
| LLM API 网络错误 | 指数退避重试 3 次（1s/2s/4s） |
| LLM API 限流 429 | 重试 5 次，遵守 Retry-After |
| LLM 返回无效 JSON | 重试 1 次，附"上次输出格式错误"提示 |
| 工具参数校验失败 | 返回错误给模型，让模型自纠正 |
| 工具执行错误 | 返回错误给模型，让模型自纠正 |
| 工具执行 panic | 捕获，返回内部错误，记录日志 |
| 工具超时 | CancellationToken 触发，返回 timeout 错误 |
| 连续 N 轮无 tool_call 也无 done | max_iters 兜底 |

**原则**：错误尽量返回给模型，让模型自己决定重试还是换路径。只有不可恢复的错误（如 API key 无效）才直接终止 loop。

### 3.9 并发与取消

| 工具类型 | 执行方式 |
|---|---|
| 只读工具（read/grep/task_status） | 完全并发 |
| 写工具（edit/write） | 同步执行，受 file lock 串行 |
| bash 同步模式 | 受 bash lock 串行 |
| bash async 模式 | 后台执行，不占 lock |
| 子代理（M3） | 默认后台异步 |
| 浏览器/Computer Use（M5） | 后台异步 |

- 同一文件的多个 edit 通过 edit_batch 原子化
- 用户可随时发中断信号 → CancellationToken 传播到所有工具
- LLM 流式响应也可中断

---

## 4. 内置工具集（M0）

### 4.1 统一约定

- 参数尽量扁平，少嵌套
- 必填参数尽量少，可选参数自动推断
- 大输出走 sandbox，返回 handle
- 异步工具智能判断 + `async: bool` 可覆盖
- 所有写文件的工具都过 File Change Journal

### 4.2 bash 工具

```typescript
// 同步模式
bash({
  cmd: string,
  cwd?: string,                   // 默认项目根
  timeout?: number,               // 秒，默认 120
  env?: Record<string, string>,
  async?: boolean                 // 覆盖智能判断
})

// 批量模式
bash_batch({
  cmds: [{ cmd, cwd?, timeout?, env? }],
  stop_on_error?: boolean,        // 默认 true
  parallel?: boolean              // 默认 false
})
```

**智能判断异步**：
- 命令含 `watch`/`&`/`tail -f`/`dev`/`serve`/`start` → 默认 async
- timeout > 60s 且命令含 `build`/`test`/`install` → 默认 async
- 其他默认 sync

**返回（sync）**：
```json
{
  "exit_code": 0,
  "stdout_summary": "末 20 行",
  "stderr_summary": "末 10 行",
  "stdout_lines": 1523,
  "stderr_lines": 0,
  "handle": "out_a1b2c3",
  "truncated": true
}
```

**返回（async）**：
```json
{"status": "started", "task_id": "t_x1y2", "msg": "running in background"}
```

### 4.3 edit 工具族（hash 锚点）

**read 工具返回内容时，每行带 hash 前缀**：
```
  a1b2c3d4│  1│ fn main() {
  e5f6g7h8│  2│     println!("hello");
  9a8b7c6d│  3│ }
```

#### `edit_replace`
```typescript
edit_replace({
  file: string,
  anchor: string,            // 8 字符 hash
  content: string,
  expect?: string            // 乐观锁：原行 hash 必须等于此值
})
```

#### `edit_insert`
```typescript
edit_insert({
  file: string,
  anchor: string,
  position: "before" | "after",
  content: string
})
```

#### `edit_delete`
```typescript
edit_delete({
  file: string,
  start: string,
  end?: string               // 不含则只删 start 一行
})
```

#### `edit_sed`
```typescript
edit_sed({
  file: string,
  start: string,
  end: string,
  pattern: string,
  replacement: string,
  flags?: string             // 默认 "g"
})
```

#### `edit_batch`
```typescript
edit_batch({
  edits: [{ file, op: "replace"|"insert"|"delete"|"sed", ...args }]
})
```

**返回（所有 edit）**：
```json
{
  "ok": true,
  "file": "src/main.rs",
  "new_hashes": ["a1b2c3d4", "e5f6g7h8"],
  "diff_preview": "@@ -10,3 +10,4 @@\n...",
  "journal_id": 42
}
```

**错误处理**：
- hash 未找到 → 返回当前文件所有 hash 列表 + 行号
- `expect` 不匹配 → 返回"文件已被修改，当前 hash 是 xxx"
- file 不存在 → 错误（write 工具创建新文件）

**为什么 hash 锚点解决所有 swap 问题**：
- ❌ "行数传多了内容只传变化" → 没有行数了，只有 hash 锚点
- ❌ "行数传少了但给了上下文" → 同上
- ❌ "格式正确但多行只有一行变化" → 每个 edit 只指定变化行
- ❌ "滥用 swap" → 没有 swap，只有 3 个明确操作
- ✅ hash 冲突：8 字符 SHA256 前缀，冲突概率 ~1/4 亿
- ✅ 模型生成错 hash：工具返回当前所有 hash 列表，模型自纠正
- ✅ 行号漂移：没有行号，hash 不受编辑影响

### 4.4 read 工具族

#### `read`
```typescript
read({
  file: string,
  start?: number,            // 默认 1
  end?: number,
  with_hashes?: boolean      // 默认 true
})
```

**截断规则**：
- 行数 ≤ 500：全返回
- 行数 > 500：返回首 100 + 末 100 + 中间摘要 + handle
- 单行 > 500 字符：折行显示，全量存 sandbox

#### `read_more`
```typescript
read_more({ handle: string, offset: number, limit?: number })
```

#### `read_full`
```typescript
read_full({ handle: string })
// 走 sandbox，建议用 read_more 分页
```

#### `read_original`
```typescript
read_original({ handle: string })
// 摘要的原文
```

### 4.5 write 工具

```typescript
write({
  file: string,
  content: string,
  create_only?: boolean      // 默认 false
})
```

### 4.6 code_exec 工具

```typescript
code_exec({
  lang: "shell" | "javascript" | "python" | "rust",
  code: string,
  cwd?: string,
  timeout?: number,          // 默认 30s
  async?: boolean
})
```

**各语言执行方式**：
- `shell`：bash 子进程
- `javascript`：node 子进程
- `python`：python3 子进程
- `rust`：通过 `rust-script` 或 cargo临时项目执行（需预装）

**沙箱限制**：
- CPU: 50% 单核 30s
- 内存: 256MB
- 文件系统: 只能写 cwd 下临时目录
- 网络: 默认禁用（可配置允许）
- 进程: 不能 fork 子进程逃逸

**返回**：与 bash 一致（sandbox + handle）。

### 4.7 plan / todo 工具

#### `plan_create`
```typescript
plan_create({
  steps: [{
    description: string,
    files_affected?: string[],
    depends_on?: number[]
  }]
})
```

#### `plan_update`
```typescript
plan_update({
  step_id: number,
  status: "in_progress" | "done" | "skipped" | "failed",
  note?: string
})
```

#### `todo`
```typescript
todo({
  action: "list" | "add" | "update" | "remove",
  id?: string,
  content?: string,
  status?: "pending" | "in_progress" | "done",
  priority?: "high" | "medium" | "low"
})
```

### 4.8 task 管理工具

`task_status` / `task_wait` / `task_list` / `task_cancel`（见 §3.7）。

### 4.9 File Change Journal（撤销机制）

在工具执行层和真实文件系统之间加一层 journal，所有文件写入都经过 journal 记录。

```rust
pub struct FileJournal {
    entries: Vec<JournalEntry>,
}

pub struct JournalEntry {
    pub id: u64,
    pub session_id: String,
    pub timestamp: Instant,
    pub tool: String,
    pub file: PathBuf,
    pub action: FileAction,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub before_snapshot: PathBuf,  // 变更前内容存 sandbox
    pub reversible: bool,
}

pub enum FileAction {
    Create, Modify, Delete, Rename { from: PathBuf },
}
```

**强制约束策略**（不靠强制，靠监控 + 审计）：

1. **bash/code_exec 工具内置文件监控**：
   - 工具执行前后，对比 cwd 下所有 tracked 文件的 hash
   - 检测到变化 → 自动写入 journal
   - 检测到新文件 → 标记为 `reversible: true`

2. **每轮 agent loop 结束后扫描**：
   - 对比本轮开始时的文件快照
   - 任何漏检的变更都补录到 journal

3. **journal 驱动的 undo**：
   - `/undo` 撤销最后一次文件变更（任何工具产生的）
   - `/undo <entry_id>` 撤销指定变更
   - `/undo --list` 查看变更历史
   - undo 也会写一条新 journal entry

**性能优化**：
- hash 对比用 mtime 过滤（只 hash mtime 变了的文件）
- 大项目用 .gitignore 规则过滤
- 变更前内容存 `<project>/.mcoder/sandbox/journal.db`

**gitignore 处理**：
- `.mcoder/` 默认写入项目根 `.gitignore`（首次启动时）
- 若项目无 `.gitignore` 则创建

---

## 5. 通信协议

### 5.1 配对机制

**配对字符串格式**：
```
mcoder://<token>@<host>:<port>?tls=<auto|on|off>
```

例：
```
mcoder://a1b2c3d4e5f6@192.168.1.10:7654?tls=auto
mcoder://a1b2c3d4e5f6@home.tail-xxxx.ts.net:7654
```

**生成流程**：
1. `mcoder` 启动时生成随机 32 字符 token，存 `~/.mcoder/credentials.toml`
2. `mcoder pair` 命令打印配对字符串 + 终端 QR 码
3. 用户在客户端输入配对串或扫描 QR

**token 用途**：客户端首帧认证；一个 server 一个 token，多客户端共用。

### 5.2 连接握手

```
1. 客户端 → server：WS 连接 ws://host:port/pair?token=<token>
2. server 验证 token：
   ✅ → 升级为 WS，发送 Welcome
   ❌ → 关闭连接（code 4001）
3. Welcome 消息：
   {
     "jsonrpc": "2.0",
     "method": "session.welcome",
     "params": {
       "server_version": "0.1.0",
       "sessions": [{"id": "...", "project": "/path", "title": "..."}],
       "capabilities": ["plan_mode", "goal_mode", "loop_mode", ...]
     }
   }
4. 客户端选择会话：session.create / session.attach
```

**TLS 处理**：
- `tls=auto`（默认）：本地 ws://，非本地 wss://
- M0 默认 ws://，依赖用户用 tailscale/SSH 隧道加密
- wss:// 推到 M4

### 5.3 JSON-RPC 消息格式

**请求**（C→S）：
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "session.create",
  "params": {"project": "/path", "title": "重构登录模块"}
}
```

**响应**（S→C）：
```json
{"jsonrpc": "2.0", "id": 1, "result": {"session_id": "..."}}
```

**通知**（S→C，无 id）：
```json
{"jsonrpc": "2.0", "method": "session.message", "params": {...}}
```

**错误**：
```json
{"jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": "..."}}
```

### 5.4 RPC 方法清单（M0）

#### 会话管理
| 方法 | 方向 | 说明 |
|---|---|---|
| `session.create` | C→S | 创建新会话 |
| `session.attach` | C→S | 附加到现有会话 |
| `session.list` | C→S | 列出所有会话 |
| `session.close` | C→S | 关闭会话 |
| `session.delete` | C→S | 删除会话数据 |
| `session.welcome` | S→C | 连接成功推送 |

#### 会话交互
| 方法 | 方向 | 说明 |
|---|---|---|
| `session.message` | S→C | 推送新消息 |
| `session.user_input` | C→S | 发送用户输入 |
| `session.cancel` | C→S | 取消当前 loop |
| `session.mode.set` | C→S | 设置 role |
| `session.mode.event` | S→C | 推送 role 事件 |
| `session.approve` | C→S | approve plan |

#### 配置管理
| 方法 | 方向 | 说明 |
|---|---|---|
| `config.get` | C→S | 读取配置 |
| `config.set` | C→S | 修改配置 |
| `config.list_models` | C→S | 列出模型 |

#### 任务管理
| 方法 | 方向 | 说明 |
|---|---|---|
| `task.list` | C→S | 列出后台任务 |
| `task.cancel` | C→S | 取消任务 |
| `task.event` | S→C | 推送任务状态 |

#### 服务器
| 方法 | 方向 | 说明 |
|---|---|---|
| `server.stats` | C→S | 资源使用、token 统计 |
| `server.shutdown` | C→S | 关闭 server |

### 5.5 多客户端同步

- server 维护 `session_id → Vec<WsConn>` 订阅列表
- 任何影响 session 状态的事件都广播给所有订阅者
- 写盘和广播解耦：先写盘（持久化），再广播（一致性）
- 新 client attach 时先读 jsonl 重放历史，再接收新事件

**冲突处理**：
- 多 client 同时发 `user_input` → 串行处理，后到的返回 409
- `cancel` 来自任何 client 都生效，广播 `cancelled`
- `approve` 先到先服务

### 5.6 心跳与重连

- 客户端每 30s 发 `ping`，server 回 `pong`
- 60s 无心跳 → server 视为断开
- 客户端断线自动重连（指数退避，最多 5 次）
- 重连后用 `session.attach` 恢复，按 jsonl offset 补推错过的事件

### 5.7 传输层实现

**Rust 端**：`tokio-tungstenite` + `tokio` + `serde_json`
**Node 端**：`ws` 或 `isomorphic-ws`

---

## 6. TUI 客户端

### 6.1 技术栈

- **框架**：Ink（React for CLIs）
- **React** + TypeScript
- **WS 客户端**：`ws`
- **状态管理**：Zustand
- **样式**：Ink 内置 `<Box>`/`<Text>` flexbox
- **输入**：`ink-text-input` + 自定义快捷键
- **参数解析**：`yargs-parser`

### 6.2 界面布局

```
┌──────────────────────────────────────────────────────────────────────┐
│  User: 帮我重构 login.ts ...                                          │  ← 可滚动区
│                                                                      │
│  Assistant: ...                                                      │
│  ▸ read(src/login.ts) ✓ 152 lines                                   │
│  ▸ edit_replace(...) ✓                                               │
│  ▸ bash_batch(2 cmds) ⏳ running [task_id: t_x1y2]                  │
│                                                                      │
│  ┌─ Plan ────────────────────────────────────────────────────────┐  │
│  │ 1. ✓ 重构 login.ts 主体                                       │  │
│  │ 2. ⏳ 跑测试验证                                               │  │
│  └────────────────────────────────────────────────────────────────┘  │
│  [y] approve  [e] edit  [n] reject                                   │
│                                                                      │
├──────────────────────────────────────────────────────────────────────┤  ← 固定区
│ mcoder · 重构登录模块 · plan · gpt-4o · 12.4k/128k · $0.03 · 2 tasks│
│ ~/projects/myapp · main · 3 files changed                            │
├──────────────────────────────────────────────────────────────────────┤
│ > /commit -m "refactor: use async/await"                            │
└──────────────────────────────────────────────────────────────────────┘
```

**关键**：
- 上方消息区可滚动（PgUp/PgDn/鼠标滚轮）
- 下方 context line + 输入框**始终固定**，不随消息滚动
- 滚动查看历史时也能输入

### 6.3 输入框区域结构

**第一层：会话上下文条**（context line）
```
mcoder · 重构登录模块 · plan · gpt-4o · 12.4k/128k · $0.03 · 2 tasks
```

| 字段 | 含义 |
|---|---|
| `mcoder` | 程序名 |
| `重构登录模块` | 会话标题 |
| `plan` | 当前 role |
| `gpt-4o` | 当前模型 |
| `12.4k/128k` | 上下文用量 |
| `$0.03` | 本会话累计成本 |
| `2 tasks` | 后台任务数 |

**第二层：项目上下文条**（project line）
```
~/projects/myapp · main · 3 files changed
```

| 字段 | 含义 |
|---|---|
| `~/projects/myapp` | 项目路径（缩写） |
| `main` | 当前 git 分支 |
| `3 files changed` | 本会话改动文件数 |

**第三层：输入框**

多行输入，左侧 `>` 提示符。

### 6.4 状态指示

- agent 思考中：输入框上方加 spinner `⠋ thinking...`
- 工具执行中：context line 显示 `1 tool running`
- 等待 approve：输入框上方显示 `[y] approve [e] edit [n] reject`
- 错误：context line 红色显示

### 6.5 紧凑模式

会话上下文条 + 项目上下文条可合并为一行：
```
mcoder · 重构登录模块 · ~/projects/myapp · main · plan · gpt-4o · 12.4k/128k · $0.03
```

配置 `tui.compact = true` 启用。

### 6.6 工具调用卡片

```
▸ read(src/login.ts) ✓ 152 lines                     [Enter 展开]
  ┌────────────────────────────────────────────────┐
  │ Args: {"file": "src/login.ts"}                 │
  │ Result: 152 lines, handle=out_a1b2c3           │
  │ [r] 查看完整输出                                │
  └────────────────────────────────────────────────┘
```

- 默认折叠（只看一行）
- 异步任务显示进度/状态
- 失败高亮红色

### 6.7 主要视图

| 视图 | 快捷键 | 说明 |
|---|---|---|
| 主聊天 | 默认 | 消息流 + 输入框 |
| 会话列表 | Ctrl+S | 所有项目所有会话 |
| Plan 详情 | Ctrl+P | plan mode 时 |
| Todo 视图 | Ctrl+T | goal mode 时 |
| 任务监控 | Ctrl+K | 后台任务 |
| 配置 | Ctrl+, | 模型/配置 |

### 6.8 输入框交互

- 多行编辑：`Enter` 换行，`Shift+Enter` 或 `Ctrl+Enter` 发送
- 文件路径补全：输入 `@` 触发文件选择器
- 历史记录：上下箭头切换
- 斜杠命令：见 §6.9

### 6.9 Slash Commands

**调用语法**：

| 类型 | 语法 | 示例 |
|---|---|---|
| 内置 command | `/<name> [args]` | `/mode plan` `/model gpt-4o` |
| 自定义 command | `/<name> [args]` | `/commit -m "msg"` |
| Skill 显式调用 | `/skill:<name> [args]` | `/skill:tdd` |
| 工作流 command | `/<name> [args]` | `/workflow propose my-change` |

**参数解析**（yargs-parser）：
```
/commit -m "refactor login" --no-verify
→ { _: ["commit"], m: "refactor login", verify: false }

/sessions list --project ~/foo
→ { _: ["sessions", "list"], project: "~/foo" }

/model set gpt-4o
→ { _: ["model", "set", "gpt-4o"] }
```

**M0 内置 commands**：
| Command | 说明 |
|---|---|
| `/help` | 显示所有可用 commands |
| `/mode <normal\|plan\|goal\|loop>` | 切换 role |
| `/model <list\|set <name>>` | 模型管理 |
| `/sessions <list\|new\|open\|delete>` | 会话管理 |
| `/undo [id\|--list]` | 撤销文件变更 |
| `/diff` | 查看本会话文件改动 |
| `/compact` | 手动压缩上下文 |
| `/cancel` | 取消当前 agent loop |
| `/task <list\|cancel <id>>` | 后台任务管理 |
| `/config <get\|set> <key> [value]` | 配置管理 |
| `/pair` | 显示配对串 + QR |
| `/exit` | 退出 |

### 6.10 实时反馈

- 流式文本：assistant 文本流式到达时实时渲染
- 工具执行：工具调用一开始就显示卡片，状态实时更新
- 任务状态：后台任务状态变化时刷新
- Plan/Todo：状态变化时推送更新

### 6.11 项目结构

```
mcoder-tui/                           # Node 包
├── package.json
├── tsconfig.json
├── src/
│   ├── index.tsx                     # 入口
│   ├── App.tsx                       # 根组件
│   ├── components/
│   │   ├── ChatView.tsx
│   │   ├── MessageList.tsx
│   │   ├── MessageItem.tsx
│   │   ├── ToolCallCard.tsx
│   │   ├── ContextLine.tsx           # 会话上下文条
│   │   ├── ProjectLine.tsx           # 项目上下文条
│   │   ├── InputBox.tsx
│   │   ├── SessionList.tsx
│   │   ├── PlanView.tsx
│   │   ├── TodoView.tsx
│   │   └── TaskMonitor.tsx
│   ├── store/                        # Zustand 状态
│   │   ├── session.ts
│   │   ├── messages.ts
│   │   └── ui.ts
│   ├── rpc/                          # JSON-RPC 客户端
│   │   ├── client.ts
│   │   └── types.ts
│   ├── commands/                     # 斜杠命令
│   │   ├── registry.ts
│   │   └── builtin.tsx
│   └── utils/
│       ├── pairing.ts
│       └── format.ts
```

### 6.12 共享层（为 M4 铺路）

以下部分设计为平台无关，未来桌面/移动 Web 客户端可直接复用：
- `src/rpc/`：JSON-RPC 客户端
- `src/store/`：状态管理
- `src/commands/`：斜杠命令逻辑
- `src/utils/`：工具函数
- `src/components/` 中的逻辑组件（不含渲染）

### 6.13 二进制分发

- TUI 通过 npm 全局安装：`npm i -g @mcoder/tui`
- `mcoder` Rust 二进制和 TUI Node 包独立发布
- `mcoder` 启动时检查 TUI 是否安装，未安装则提示
- 嵌入式模式：`mcoder` 命令内部 `spawn` TUI 子进程

---

## 7. 配置参考

### 7.1 全局配置 `~/.mcoder/config.toml`

```toml
# 模型配置
[models.gpt-4o]
protocol = "openai"
api_key = "sk-..."
base_url = "https://api.openai.com/v1"
context_window = 128000

[models.claude-opus-4]
protocol = "anthropic"
api_key = "sk-ant-..."
base_url = "https://api.anthropic.com"

[models.gemini-pro]
protocol = "gemini"
api_key = "..."
base_url = "https://generativelanguage.googleapis.com"

[models.deepseek-v3]
protocol = "openai"      # DeepSeek 兼容 OpenAI
api_key = "..."
base_url = "https://api.deepseek.com/v1"

# 默认模型
[default]
model = "gpt-4o"

# Role 配置
[roles.default]
model = "gpt-4o"

[roles.plan]
model = "claude-opus-4"
max_iters = 5

[roles.execute]
model = "deepseek-v3"
max_iters = 50

[roles.review]
model = "claude-opus-4"
max_iters = 10

# Agent Loop 配置
[loop]
max_iters = 50

# 压缩策略
[compact]
strategy = "auto"           # auto | manual | aggressive | off
threshold = 0.8             # 上下文占比阈值
keep_recent = 5
keep_first = 2
tool_results = "summarize"  # summarize | keep | drop

# TUI 配置
[tui]
compact = false
theme = "default"

# Server 配置
[server]
host = "127.0.0.1"
port = 7654
```

### 7.2 项目级配置 `<project>/.mcoder/config.toml`

可覆盖全局配置，例：
```toml
[default]
model = "deepseek-v3"   # 这个项目用 deepseek

[roles.plan]
model = "gpt-4o"        # 但规划用 gpt-4o
```

---

## 8. 完整 Roadmap

### 8.1 里程碑总览

| 里程碑 | 目标 | 核心交付 | 验收标准 |
|---|---|---|---|
| **M0 地基** | 跑通"模型↔工具↔TUI"闭环 | core runtime + 3 adapter + 7 工具 + TUI | 能用 TUI 让模型读改跑一个 Rust 项目 |
| **M1 可用** | 单机真正可用 | 记忆 + 插件(skills/commands/MCP) + hooks | 跨会话记住决策，能装 MCP 用其工具 |
| **M2 代码智能** | agent 真懂代码 | 代码图谱 + LSP + 调试 | 图谱驱动的查询/重构，断点调试 |
| **M3 协作** | 能干大活 | 子代理 + 工作流(blueprint 式) | 多代理协作完成大项目变更 |
| **M4 多端** | 全平台 | Tauri 桌面 + Capacitor 移动 + wss | 手机能连主机 agent |
| **M5 自测** | agent 自测 | 浏览器 + Computer Use | agent 自己测前端/GUI 项目 |

### 8.2 M0 地基（详细）

**范围**：
- §1 总体架构（变体 A）
- §2 存储分层
- §3 核心运行时（adapter trait / tool registry / agent loop / role 系统 / 异步任务 / 错误处理）
- §4 内置工具集（bash/bash_batch/edit 族/read 族/write/code_exec/plan/todo/task 管理 + File Journal）
- §5 通信协议（WS + JSON-RPC + 配对 + 多端同步）
- §6 TUI 客户端（Ink + 固定输入框 + slash commands）

**子任务拆分**：

| # | 任务 | 依赖 |
|---|---|---|
| M0.1 | 项目初始化（cargo workspace + mcoder-tui 子目录） | - |
| M0.2 | SQLite + FTS5 持久化层 | M0.1 |
| M0.3 | tree-sitter 集成 + 行 hash | M0.1 |
| M0.4 | LLM Adapter trait + OpenAI adapter | M0.2 |
| M0.5 | Anthropic + Gemini adapter | M0.4 |
| M0.6 | Tool Registry + 7 个工具实现 + File Journal | M0.2, M0.3 |
| M0.7 | Agent Loop + Role 系统 + 异步任务 | M0.4, M0.6 |
| M0.8 | WS server + JSON-RPC + 配对 | M0.7 |
| M0.9 | 多会话管理 + 多 client 同步 | M0.8 |
| M0.10 | TUI 主聊天视图 + 固定输入框 | M0.8 |
| M0.11 | TUI 工具卡片 + 流式渲染 | M0.10 |
| M0.12 | TUI 会话列表 + plan/todo/任务视图 + slash commands | M0.10 |
| M0.13 | 嵌入式启动 + 配对 QR 码 | M0.8, M0.10 |
| M0.14 | 端到端测试（Rust 项目 + TS 项目） | 全部 |

**M0 验收标准**：
1. `mcoder` 一条命令启动 server + TUI
2. 配置 OpenAI/Anthropic/Gemini 任一模型，能对话
3. 让 agent 读改跑一个 Rust 项目（read → edit_replace → bash cargo test）
4. 用 bash_batch 一次跑多个命令，大输出走 sandbox
5. plan 模式下能 approve/reject
6. goal 模式下 todo 实时更新
7. 两个 TUI client 连同一 session，消息同步
8. `mcoder pair` 打印配对串 + QR，另一台机器 TUI 输入串能连上
9. `/undo` 能撤销任何工具的文件变更
10. `/mode plan` 切换 role，plan role 用配置的模型

### 8.3 M1 可用（概要）

#### 1. 记忆系统
- **项目级记忆**（`<project>/.mcoder/memory/sqlite.db`）：
  - 自动捕获：每轮结束抽取关键决策、文件改动、错误修复 → 存 FTS5
  - 召回：新会话开始时按当前任务语义检索相关记忆，注入 system message
  - 手动记忆：`remember("决策：用 hash 锚点而非行号")` 工具
- **全局经验沉淀**（`~/.mcoder/experiences/sqlite.db`）：
  - 跨项目跨会话共享
  - 错误修复模式、工具使用技巧、配置经验
  - 召回：任务相关时主动注入，或 agent 调用 `recall("how to ...")` 工具
- **沉淀工具**：`precipitate(type, content, tags[])` 把经验写入全局库

#### 2. 插件系统（skills/commands/MCP）
- **Skills**：可加载的技能包（YAML/MD 描述 + 可执行脚本）
  - 放 `~/.mcoder/skills/` 或 `<project>/.mcoder/skills/`
  - 每个 skill 声明 trigger（关键词/正则）+ action（工具调用模板）
  - `/skill:<name>` 显式调用，或自然语言触发
- **Commands**：斜杠命令扩展
  - `/command-name` 调用，支持参数解析（yargs-parser）
  - 用户自定义命令放 `~/.mcoder/commands/`
- **MCP**：实现 MCP server 协议，可加载第三方 MCP server
  - 配置 `mcp_servers` in config.toml
  - MCP 工具自动注册到 Tool Registry
  - 支持 stdio + SSE 两种 transport

#### 3. Hooks
- 生命周期事件：`session.start` / `pre_tool_use` / `post_tool_use` / `pre_llm_call` / `post_llm_call` / `session.end` / `pre_compact`
- Hook 配置：`~/.mcoder/hooks.toml` 或 `<project>/.mcoder/hooks.toml`
- Hook 可执行 shell 命令或调用内置工具
- 用途：自动 format、lint、git auto-commit、context-mode 式路由等

#### 4. Context 压缩（可配置）
- 上下文接近模型窗口阈值时触发自动压缩
- 压缩策略：保留首尾 N 轮 + 中间摘要 + 工具调用结果转 handle 引用
- 用户可手动 `/compact`
- 配置见 §7.1 `[compact]`

#### 5. 内置 skills/commands 示例（M1 新增，M0 已有 /diff /undo）
- `/branch` 创建 git 分支
- skill: `tdd`（test-driven 流程）
- skill: `commit`（生成 conventional commit）
- skill: `review`（代码审查流程）
- 注：`/diff` 和 `/undo` 在 M0 已内置（依赖 File Journal）

### 8.4 M2 代码智能（概要）

#### 1. 代码图谱（AST 级别）
- **解析器**：tree-sitter（已 M0 引入），扩展到 20+ 语言
- **存储**：`<project>/.mcoder/graph/graph.db`（SQLite）
- **节点类型**：file / module / class / function / method / variable / import / call
- **关系**：defines / calls / imports / extends / implements / references
- **增量更新**：每轮 edit 后按文件 mtime 增量解析
- **查询工具**：
  - `graph_query(cypher_like)` 或 `graph_find(symbol, type)`
  - `graph_callers(fn)` / `graph_callees(fn)` / `graph_references(symbol)`
- **AST Edit**：
  - `ast_rename(symbol, new_name)` 跨文件重命名（类 IDE）
  - `ast_extract(file, range, new_name)` 提取为函数/变量
  - `ast_inline(symbol)` 内联
  - 基于 tree-sitter 的语法感知编辑

#### 2. LSP 集成
- **支持语言**：Rust (rust-analyzer) / TS (tsserver) / Go (gopls) / Python (pylsp) 等
- **能力**：诊断、跳转定义、hover、引用查找、重命名、格式化
- **架构**：LSP client 在 server 端管理多语言服务器进程
- **与图谱协同**：图谱做粗粒度查询，LSP 做精粒度操作
- **工具**：`lsp_diagnose(file)` / `lsp_hover(file, line, col)` / `lsp_rename(file, line, col, new_name)`

#### 3. 调试子系统
- **DAP client**：支持 Debug Adapter Protocol
- **支持**：Rust (lldb-dap) / Node / Python / Go
- **工具**：
  - `debug_start(config)` 启动 debug 会话（launch/attach）
  - `debug_set_breakpoint(file, line, condition?)`
  - `debug_continue()` / `debug_step_over()` / `debug_step_in()` / `debug_step_out()`
  - `debug_eval(expression)`
  - `debug_get_state()` 拿调用栈、变量、当前行
- **agent 自驱**：agent 可自己启动 debug、打断点、分析失败、修代码、再跑

### 8.5 M3 协作（概要）

#### 1. 子代理系统
- **通用子代理**：内置 `subagent` 工具
  ```
  subagent({task: "...", max_tokens: 10000, max_iters: 20, timeout: 300, model: "gpt-4o-mini"})
  ```
- **自定义子代理**：`~/.mcoder/agents/` 下 YAML 定义
  - role / system_prompt / allowed_tools / model / limits
- **指定模型**：子代理可指定模型（通过 role 配置）
- **通信规则**：
  - 子代理之间**可直接通信**（询问），但**不能调度**
  - 通信通过 `ask_agent(agent_id, question)` 工具
  - 主 agent 调度子代理（`subagent` 工具）
- **生命周期**：
  - 默认后台异步（§3.7）
  - max_tokens / max_iters 可配置（含无限）
  - 超时时间可配置
  - **失败检测（双阈值）**：
    - 任意工具连续失败 N 次 → 标记失败停掉（N=3）
    - 单轮内同一工具失败 M 次也停（M=5，防死循环）
  - 子代理通过主动调用 `done(result)` 返回
- **上下文隔离**：子代理有独立 context window

#### 2. 工作流系统（blueprint 式）
- **核心概念**：
  - Roadmap（活的）→ Change（spec 驱动的单元）
  - 7 类 artifact：proposal / design / tasks / spec / review / roadmap / config
  - 编号体系：PR-N / DS-N / D-N / T-N / SP-N
- **5 步循环**：propose → plan → apply → review → archive
- **3 个内置子代理**：planner / executor / reviewer
- **可选 TDD**：behavior 任务走 RED-GREEN-REFACTOR
- **存储**：`<project>/.mcoder/workflow/`（默认，可配）
- **变更图谱**：所有 artifact 通过编号关联
- **CLI**：`/workflow` slash command（init/propose/plan/apply/review/archive/continue/list）
- **自然语言触发**：通过 system prompt 注入触发规则
- **profile**：
  - lite（顺序执行、TDD 可选、review 任意通过）
  - standard（并行、TDD 强制、review 全通过）

### 8.6 M4 多端（概要）

#### 1. 桌面客户端（Tauri）
- **前端**：React + Web 技术（与 TUI 共享逻辑层）
- **后端**：Rust（Tauri 原生）
- **优势**：二进制小（~10MB）、性能好、与 server 同语言
- **UI**：比 TUI 更丰富的可视化（图谱可视化、diff viewer、文件树）
- **共享**：TUI 的 `rpc/store/commands/utils` 直接复用

#### 2. 移动客户端（Capacitor）
- **前端**：React + Web
- **打包**：Capacitor 打包成 Android/iOS
- **优化**：弱网友好、触摸交互、简化视图
- **远程连接**：默认走配对串连主机 server

#### 3. wss 支持
- Let's Encrypt 自动证书（域名场景）
- 自签证书（IP 场景）
- 配对串 `tls=on` 强制 wss

#### 4. Web 客户端（可选）
- 浏览器直接访问 server
- 走 wss，配对串认证

### 8.7 M5 自测（概要）

#### 1. 浏览器工具
- **内置浏览器**：服务端启动 headless Chrome
- **工具**：
  - `browser_open(url)` / `browser_navigate(url)`
  - `browser_click(selector)` / `browser_type(selector, text)`
  - `browser_screenshot()` / `browser_snapshot()`（accessibility tree）
  - `browser_eval(js)` / `browser_console()` / `browser_network()`
- **用途**：agent 自己启动前端 → 测试 → 截图分析 → 修 bug
- **token 节省**：snapshot 比 screenshot 省

#### 2. Computer Use
- **桌面自动化**：accessibility API 或截图 + 视觉模型
- **工具**：
  - `screen_screenshot()` / `screen_click(x, y)` / `screen_type(text)`
  - `screen_key(key)` / `screen_scroll(x, y, direction)`
  - `app_list()` / `app_open(name)` / `app_focus(name)`
- **实现**：Rust 用 `enigo` + `screenshots` + accessibility API
- **用途**：测试非 Web 项目（GUI 应用、原生应用）
- **安全**：默认需用户确认每步操作（可配置白名单自动批准）

### 8.8 跨里程碑事项

| 事项 | M0 | M1 | M2 | M3 | M4 | M5 |
|---|---|---|---|---|---|---|
| token 节省优化 | 工具设计 + sandbox + 定义缓存 | context 压缩 | 图谱替代 grep | 子代理隔离 context | - | snapshot 替代 screenshot |
| 错误处理完善 | 基础重试 | hook 兜底 | LSP 诊断 | 子代理失败检测 | 网络重连 | 操作确认 |
| 性能优化 | 基础并发 | - | 增量图谱 | - | - | - |
| 文档 | 设计文档 | 用户手册 | - | 工作流指南 | - | - |

---

## 9. 待后续讨论的开放问题

以下问题在 M0 实现过程中需要进一步明确，但不阻塞 M0 设计启动：

1. **代码图谱查询语言**：用 Cypher-like、SQL-like 还是自定义 DSL？（M2 决定）
2. **LSP 进程管理策略**：按需启动 vs 常驻？（M2 决定）
3. **子代理通信协议**：同步 ask 还是异步消息？（M3 决定）
4. **工作流 artifact 模板**：具体字段细节（M3 决定，参考 blueprint）
5. **移动端离线策略**：断线时如何缓存用户输入（M4 决定）
6. **Computer Use 视觉模型**：用云端模型还是本地模型（M5 决定）

---

## 10. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| Rust 开发速度慢 | M0 工期长 | 优先用成熟 crate（tokio/serde/tree-sitter/sqlx），不造轮子 |
| 三家 adapter 差异大 | adapter 维护成本 | 抽象 trait + 单元测试覆盖每家协议 |
| hash 锚点冲突 | 极低概率误编辑 | 8 字符 SHA256 前缀（~1/4 亿冲突率）+ expect 乐观锁 |
| File Journal 性能 | 大项目扫描慢 | mtime 过滤 + .gitignore 过滤 + 增量 hash |
| TUI 复杂布局 | Ink 性能瓶颈 | 虚拟滚动 + 只渲染可见区域 |
| 远程连接安全 | token 泄露 | 32 字符随机 token + 建议用 tailscale 加密隧道 |

---

**文档结束**

下一步：用户 review 本文档 → 通过后 invoke writing-plans skill 生成 M0 实现计划。
