// mcoder binary - main.rs 仅做 CLI 入口
// 实际模块由 src/lib.rs 声明（lib + bin 模式），保证 integration tests 能访问
#[allow(unused_imports)]
use mcoder_lib::{
    agent, ask_user, browser, code_graph, commands, computer_use, config, debug, llm, lsp, memory,
    persistence, plugin, session_manager, skills, tools, transport, tree_sitter, types, utils,
    workflow,
};

use clap::{Parser, Subcommand};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "mcoder", version, about = "Self-hosted coding agent")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the agent server
    Server {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 7654)]
        port: u16,
        /// 设计文档 §8.6: 域名（启用 Let's Encrypt 自动证书）
        #[arg(long)]
        domain: Option<String>,
        /// ACME 账户邮箱
        #[arg(long)]
        email: Option<String>,
        /// HTTP 端口（Web 客户端 + ACME 挑战，默认 = port + 1）
        #[arg(long)]
        http_port: Option<u16>,
        /// Web 客户端静态文件目录
        #[arg(long)]
        web_dir: Option<String>,
        /// 后台守护进程模式（detach）：server 在后台运行，日志写入 ~/.mcoder/mcoder.log
        #[arg(long)]
        detach: bool,
    },
    /// Start TUI client. Auto-starts a local server if none is running.
    Tui {
        #[arg(long, default_value = "ws://127.0.0.1:7654")]
        url: String,
        #[arg(long)]
        token: Option<String>,
        /// 不要自动拉起 server（仅连接已运行的实例）
        #[arg(long)]
        no_spawn: bool,
    },
    /// Show pairing info (QR code + URL)
    Pair {
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        #[arg(long, default_value_t = 7654)]
        port: u16,
    },
    /// List sessions
    Sessions,
    /// Stop a running daemon server
    Stop {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 7654)]
        port: u16,
    },
}

/// 完整版启动：支持 ACME 证书 + HTTP 服务器 + Web 客户端
async fn start_server_full(
    host: &str,
    port: u16,
    domain: Option<&str>,
    email: Option<&str>,
    http_port: Option<u16>,
    web_dir: Option<&str>,
) -> anyhow::Result<(
    Arc<transport::ws_server::WsServer>,
    Arc<types::AppConfig>,
    std::path::PathBuf,
)> {
    let project = std::env::current_dir()?;
    let project_dir = crate::config::project_config_dir(&project);

    // 确保目录结构存在
    crate::config::ensure_dirs(Some(&project))?;

    // 加载配置（全局 + 项目级合并）
    let app_config = Arc::new(crate::config::load_config(Some(&project))?);

    // 计算 project_hash（用于会话隔离、记忆隔离）
    // 注意: 多项目改造后 project_hash 改由 ProjectResources 按需计算，此处保留仅为兼容
    let _project_hash = crate::memory::project_hash(&project);

    // Initialize code graph
    // 注意: 多项目改造后 graph 改由 ProjectResources::for_project 按需创建
    let _graph = code_graph::CodeGraph::new(&project_dir.join("graph.db"), &project)?;

    // Initialize memory store (project-level: .mcoder/memory.db)
    // 注意: 多项目改造后 project memory 改由 ProjectResources::for_project 按需创建
    let _memory = Arc::new(memory::MemoryStore::open(&project_dir.join("memory.db"))?);

    // 设计文档 §2.1/§8.3: 全局经验库 (~/.mcoder/experiences/sqlite.db)
    // 跨项目共享的经验沉淀
    let experience_store = Arc::new(memory::MemoryStore::open(
        &crate::config::global_experiences_db_path(),
    )?);

    // Initialize file journal（用于 write/edit/ast_edit 的 undo 支持）
    // 注意: 多项目改造后 journal 改由 ProjectResources::for_project 按需创建
    let _journal = Arc::new(tools::journal::FileJournal::new(&project_dir)?);

    // Initialize workflow store
    // 注意: 多项目改造后 workflow 改由 ProjectResources::for_project 按需创建
    let _workflow = Arc::new(workflow::WorkflowStore::open(&project_dir.join("workflow.db"))?);

    // Initialize plugin manager
    let plugins = Arc::new(plugin::PluginManager::new());

    // 设计文档 §8.3.3: 从配置加载 hooks（shell 命令钩子）
    plugins.load_hooks_from_config(&app_config.hooks).await?;

    // Phase 5: TaskManager 改为 per-session（不再全局）；SessionManager 内部按需创建
    // 这里不再创建全局 task_manager

    // 设计文档 §3.4: 初始化 RoleRegistry 并合并配置
    let mut role_registry = agent::role::RoleRegistry::new();
    role_registry.merge_config(&app_config.roles, &app_config.models);
    let role_registry = Arc::new(role_registry);

    // 设计文档 §8.4.3: 初始化 DAP 调试管理器
    // 注意: 多项目改造后 debug_manager 改由 ProjectResources::for_project 按需创建
    let _debug_manager = debug::DebugManager::new();

    // 设计文档 §8.4.2: 初始化 LSP 管理器
    // 按需懒启动各语言的 LSP server（rust-analyzer / tsserver / gopls / pylsp）
    // 与 code_graph 协同：图谱做粗粒度查询，LSP 做精粒度操作
    // 注意: 多项目改造后 lsp_manager 改由 ProjectResources::for_project 按需创建
    let _lsp_manager = lsp::LspManager::new(project.clone());

    // Build tools with all dependencies
    let (mut tools_reg, subagent_tool, ask_user_tool, ask_registry) = tools::build_full_registry();

    // 设计文档 §8.3.4: 加载 skills（全局 ~/.mcoder/skills/ + 项目 .mcoder/skills/）
    // Skill = 能力扩展包（文件夹 + SKILL.md），支持渐进式披露
    let global_skills_dir = crate::config::global_config_dir().join("skills");
    let project_skills_dir = project_dir.join("skills");
    let skill_registry = match skills::build_registry(&global_skills_dir, &project_skills_dir).await {
        Ok(reg) => {
            let count = reg.list().await.len();
            tracing::info!("loaded {} skills", count);
            reg
        }
        Err(e) => {
            tracing::warn!("failed to load skills: {}", e);
            std::sync::Arc::new(skills::SkillRegistry::new())
        }
    };

    // 加载自定义 slash commands（全局 ~/.mcoder/commands/ + 项目 .mcoder/commands/）
    let global_commands_dir = crate::config::global_config_dir().join("commands");
    let project_commands_dir = project_dir.join("commands");
    let command_registry = match commands::build_registry(&global_commands_dir, &project_commands_dir).await {
        Ok(reg) => {
            let count = reg.list().await.len();
            tracing::info!("loaded {} custom commands", count);
            reg
        }
        Err(e) => {
            tracing::warn!("failed to load commands: {}", e);
            std::sync::Arc::new(commands::CommandRegistry::new())
        }
    };

    // 构建 skill_use 工具（让 LLM 能调用 skill，action=use/list）
    let skill_use_tool: tools::SharedTool = Arc::new(tools::SkillUseTool {
        registry: skill_registry.clone(),
    });
    tools_reg.register(skill_use_tool);

    // 设计文档 §8.3.2: 启动 MCP servers（工具不直接注册，通过 mcp_list/mcp_call 元工具发现）
    let mcp_manager = Arc::new(plugin::mcp::McpManager::new());
    if !app_config.mcp_servers.is_empty() {
        match mcp_manager.start_all(&app_config.mcp_servers).await {
            Ok(mcp_tools) => {
                tracing::info!("MCP servers started, {} tools available via mcp_list/mcp_call", mcp_tools.len());
            }
            Err(e) => {
                tracing::warn!("failed to start MCP servers: {}", e);
            }
        }
    }

    let tools = Arc::new(tools_reg);

    // 设计文档 §8.5: 注入 SubagentTool 依赖（late binding）
    // 子代理根据 role 选择 model 和工具白名单
    //
    // 设计文档 §provider: 启动兜底 - default_model 不存在时仍启动，
    // 让 UI 引导用户添加 provider；subagent 路径依赖 default_model 时才报错
    //
    // S1 修复: 同时查 cfg.models 和 cfg.providers，让 provider 级别的 model
    // 也能作为 default_model 在启动时被识别
    let default_model_config: Option<crate::types::ModelConfig> = app_config.models.get(&app_config.default_model).cloned()
        .or_else(|| {
            // S1: 查 providers -- 遍历所有 provider 的 models 列表
            for (pname, p) in &app_config.providers {
                for mname in &p.models {
                    if mname == &app_config.default_model {
                        return Some(synthesize_model_from_provider(p, mname));
                    }
                }
                // 也尝试 "provider/model" 形式
                let prefixed = format!("{pname}/{}", app_config.default_model);
                for mname in &p.models {
                    if prefixed == *mname {
                        return Some(synthesize_model_from_provider(p, mname));
                    }
                }
            }
            None
        });
    let (default_model_config, default_llm) = match default_model_config {
        Some(m) => match llm::create_adapter(&m) {
            Ok(llm) => (Some(m), Some(llm)),
            Err(e) => {
                tracing::warn!(
                    "default_model '{}' adapter creation failed: {}; starting in setup mode",
                    m.name, e
                );
                (None, None)
            }
        },
        None => {
            tracing::warn!(
                "default_model '{}' not found in config.models or providers; starting in setup mode (use UI to add a provider)",
                app_config.default_model
            );
            (None, None)
        }
    };
    if let (Some(m), Some(llm)) = (default_model_config, default_llm) {
        subagent_tool.set_dependencies(Arc::new(tools::subagent::SubagentDeps {
            default_llm: llm,
            default_model_config: m,
            tools: tools.clone(),
            role_registry: role_registry.clone(),
        })).await;
    } else {
        tracing::warn!(
            "subagent tool disabled (no valid default_model); session_manager will return friendly error on subagent spawn"
        );
    }

    // 构建 slash command 分发器
    let command_dispatcher = Arc::new(commands::CommandDispatcher::new(
        command_registry.clone(),
        skill_registry.clone(),
    ));

    let mgr = session_manager::SessionManager::new(
        tools,
        app_config.clone(),
        plugins.clone(),
        role_registry,
        experience_store,
        mcp_manager,
        command_dispatcher,
        ask_registry.clone(),
    );

    // ask_user 工具：late binding 注入 event_tx（在 SessionManager 创建后）
    ask_user_tool.set_event_tx(mgr.event_tx());

    // 设计文档 §8.3.3: 触发 OnStart hook（server 启动时）
    let _ = plugins.run_hooks(
        crate::plugin::HookPoint::OnStart,
        crate::plugin::HookContext::new(crate::plugin::HookPoint::OnStart, ""),
    ).await;

    // P1-5: 启动时枚举所有项目，遍历 session_state.db
    // mark_orphans_interrupted：把上一次服务周期没来得及终态化的 queued/running
    // task 原子转为 interrupted。attach 仍兜底（load_session_from_jsonl → get_or_create_task_manager
    // 也会调一次）。
    if let Err(e) = mgr.mark_startup_orphans().await {
        tracing::warn!("startup orphan sweep failed: {}", e);
    }

    // 设计文档 §8.6: HTTP 服务器 + TLS 决策
    // P1-2: 统一流程：先决定配置 → 启动 HTTP → 决定 TLS 来源 → 启动 WS
    let challenges = transport::acme::new_challenge_map();
    let http_port_val = http_port.unwrap_or(port + 1);
    let web_dir_path = web_dir.map(std::path::PathBuf::from);
    let pairing_info = transport::pairing::generate_pairing(host, port)?;

    // 1. 启动 HTTP 服务器（始终启动，用于 Web 客户端 + ACME 挑战响应）
    let http_config = transport::http_server::HttpServerConfig {
        web_dir: web_dir_path,
        challenges: challenges.clone(),
        pairing: pairing_info.clone(),
    };
    transport::http_server::start_http_server(host, http_port_val, http_config).await?;
    println!("HTTP server (Web client + ACME) listening on http://{}:{}", host, http_port_val);

    // 2. 决定 TLS 来源
    //    - 域名场景：ACME 证书（失败则 fallback 自签）
    //    - 本地场景：根据 tls 模式（auto/on → 自签，off → 无 TLS）
    let mut tls_acceptor: Option<tokio_rustls::TlsAcceptor> = None;

    if let Some(domain) = domain {
        if transport::acme::is_domain(domain) {
            let email = email.unwrap_or("admin@localhost");
            tracing::info!("requesting Let's Encrypt certificate for {} ({})", domain, email);

            // 申请 ACME 证书（P0-2: 自动使用缓存）
            match transport::acme::request_certificate(domain, email, challenges.clone()).await {
                Ok(cert) => {
                    match transport::acme::build_tls_acceptor_from_acme(cert) {
                        Ok(acceptor) => {
                            tracing::info!("ACME certificate loaded for {}", domain);
                            tls_acceptor = Some(acceptor);
                        }
                        Err(e) => {
                            tracing::warn!("failed to build TLS acceptor from ACME cert: {}, falling back to self-signed", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("ACME certificate request failed: {}, falling back to self-signed cert", e);
                }
            }
        } else {
            tracing::warn!("'{}' is not a valid domain, skipping ACME", domain);
        }
    }

    // 3. 启动 WebSocket 服务器（wss:// 如果有 TLS）
    //    start_with_tls 内部会根据 tls 模式和 tls_acceptor 决定最终行为：
    //    - 有 ACME acceptor → 用 ACME 证书
    //    - 无 acceptor + tls 模式 auto/on → 自签证书
    //    - 无 acceptor + tls 模式 off → 无 TLS
    let server = transport::ws_server::WsServer::start_with_tls(host, port, mgr, tls_acceptor).await?;

    println!("mcoder server listening on {}", server.addr);
    println!("Pairing URL: {}", server.pairing_info().urls[0]);

    Ok((server, app_config, project_dir))
}

/// 设计文档 §1.1 / §6.13: 嵌入式模式
/// `mcoder` 命令（无 subcommand）= fork server + 启动 TUI
/// `mcoder tui` = 启动 TUI（若本地 server 未运行则自动拉起）
fn spawn_tui_process(url: &str, token: &str) -> anyhow::Result<std::process::Child> {
    // 优先级：
    // 1. 同目录的 mcoder-tui 单文件可执行（Bun compile 产物，无 Node.js 依赖）
    // 2. 同目录的 mcoder-tui/dist/index.js（开发模式，需要 node）
    // 3. 全局安装的 mcoder-tui（PATH 中）
    let exe = std::env::current_exe()?;
    let parent = exe.parent().unwrap_or(std::path::Path::new("."));

    // 1. 单文件可执行
    let standalone = parent.join("mcoder-tui");
    if standalone.exists() {
        let result = std::process::Command::new(&standalone)
            .arg("--url").arg(url)
            .arg("--token").arg(token)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn();
        if let Ok(child) = result {
            return Ok(child);
        }
    }

    // 2. 开发模式：node dist/index.js
    let dev_path = parent
        .parent()
        .map(|p| p.join("mcoder-tui").join("dist").join("index.js"))
        .unwrap_or_default();
    if !dev_path.as_os_str().is_empty() && dev_path.exists() {
        let result = std::process::Command::new("node")
            .arg(&dev_path)
            .arg("--url").arg(url)
            .arg("--token").arg(token)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn();
        if let Ok(child) = result {
            return Ok(child);
        }
    }

    // 3. 全局安装
    let result = std::process::Command::new("mcoder-tui")
        .arg("--url").arg(url)
        .arg("--token").arg(token)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn();
    if let Ok(child) = result {
        return Ok(child);
    }

    anyhow::bail!("failed to spawn TUI; build with `cd mcoder-tui && ./build-standalone.sh` or install with `npm i -g @mcoder/tui`")
}

/// 从 ws://host:port URL 中解析 (host, port)
fn parse_ws_url(url: &str) -> Option<(String, u16)> {
    let url = url.strip_prefix("ws://").or_else(|| url.strip_prefix("wss://"))?;
    let url = url.split('?').next().unwrap_or(url);
    let url = url.strip_prefix('/').unwrap_or(url);
    let (host, port_str) = url.rsplit_once(':')?;
    let port: u16 = port_str.parse().ok()?;
    Some((host.to_string(), port))
}

/// 检测 server 是否在运行（尝试 TCP 连接）
async fn is_server_running(host: &str, port: u16) -> bool {
    use std::net::ToSocketAddrs;
    let addr = format!("{}:{}", host, port);
    let addrs: Vec<_> = match addr.to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(_) => return false,
    };
    for addr in addrs {
        if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok() {
            return true;
        }
    }
    false
}

/// server 锁文件路径：~/.mcoder/server.lock
fn server_lock_path() -> std::path::PathBuf {
    crate::config::global_config_dir().join("server.lock")
}

/// 读取锁文件中的 PID（如果文件存在且进程存活）
fn read_lock_pid() -> Option<u32> {
    let path = server_lock_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let pid: u32 = content.trim().parse().ok()?;
    // 检查进程是否存活（跨平台）
    if pid > 0 && process_is_alive(pid) {
        Some(pid)
    } else {
        // 进程已死，清理锁文件
        let _ = std::fs::remove_file(&path);
        None
    }
}

/// 尝试获取 server 锁（原子创建锁文件）
/// 成功返回 true，失败（锁已被持有）返回 false
fn try_acquire_server_lock() -> bool {
    let path = server_lock_path();
    let parent = path.parent().unwrap_or(std::path::Path::new("."));
    let _ = std::fs::create_dir_all(parent);

    // 原子创建：create_new 确保只有一个进程能创建
    #[cfg(unix)]
    {
        // Unix: 用 OpenOptionsExt::mode 设置文件权限
        use std::os::unix::fs::OpenOptionsExt;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&path)
        {
            Ok(mut f) => {
                use std::io::Write;
                let _ = write!(f, "{}", std::process::id());
                true
            }
            Err(_) => read_lock_pid().is_none(),
        }
    }

    #[cfg(windows)]
    {
        // Windows: create_new 已是原子操作，无需 mode
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                use std::io::Write;
                let _ = write!(f, "{}", std::process::id());
                true
            }
            Err(_) => read_lock_pid().is_none(),
        }
    }
}

/// 释放 server 锁（删除锁文件）
fn release_server_lock() {
    let _ = std::fs::remove_file(server_lock_path());
}

/// 检查进程是否存活（跨平台）
#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    // Unix: kill(pid, 0) 返回 0 表示进程存在
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    // Windows: OpenProcess + GetExitCodeProcess
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut exit_code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        ok != 0 && exit_code == STILL_ACTIVE as u32
    }
}

/// 以守护进程方式启动 server（跨平台 detach）
/// 父进程立即退出，子进程在后台运行，日志写入 log_path
fn spawn_detached_server(host: &str, port: u16) -> anyhow::Result<u32> {
    let exe = std::env::current_exe()?;
    let log_dir = crate::config::global_config_dir();
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("mcoder.log");

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("server")
        .arg("--host").arg(host)
        .arg("--port").arg(port.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_file.try_clone()?))
        .stderr(std::process::Stdio::from(log_file));

    spawn_detached_child(&mut cmd)
}

/// 跨平台 kill 进程
#[cfg(unix)]
fn kill_process(pid: u32) {
    // Unix: SIGTERM
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

#[cfg(windows)]
fn kill_process(pid: u32) {
    // Windows: TerminateProcess
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_TERMINATE,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

/// Unix: setsid 脱离控制终端；Windows: CREATE_NO_WINDOW + DETACHED_PROCESS
#[cfg(unix)]
fn spawn_detached_child(cmd: &mut std::process::Command) -> anyhow::Result<u32> {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    let pid = child.id();
    std::mem::forget(child);
    Ok(pid)
}

#[cfg(windows)]
fn spawn_detached_child(cmd: &mut std::process::Command) -> anyhow::Result<u32> {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW (0x08000000) | DETACHED_PROCESS (0x00000008)
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const DETACHED_PROCESS: u32 = 0x00000008;
    cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    let child = cmd.spawn()?;
    let pid = child.id();
    std::mem::forget(child);
    Ok(pid)
}

/// 等待 server 就绪（最多 10 秒）
async fn wait_for_server(host: &str, port: u16) -> bool {
    for _ in 0..50 {
        if is_server_running(host, port).await {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    false
}

/// 向运行中的 server 发送 shutdown 请求
async fn stop_server(host: &str, port: u16) -> anyhow::Result<()> {
    let url = format!("http://{}:{}/api/shutdown", host, port + 1);
    let client = reqwest::Client::new();
    let resp = client.post(&url).send().await;
    match resp {
        Ok(r) if r.status().is_success() => {
            // 等待进程退出后清理锁文件
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            release_server_lock();
            Ok(())
        }
        Ok(r) => anyhow::bail!("server responded with {}", r.status()),
        Err(e) => anyhow::bail!("cannot connect to server at {}: {}", url, e),
    }
}

/// M11 修复: 委托给 ProviderConfig::synthesize_model_config 共享方法
fn synthesize_model_from_provider(p: &types::ProviderConfig, model_name: &str) -> types::ModelConfig {
    p.synthesize_model_config(model_name)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    // 设计文档 §1.1 / §6.13: 无 subcommand → 嵌入式模式（server + TUI）
    // 子命令 → 单独模式
    match cli.command {
        None => {
            let host = "127.0.0.1";
            let port = 7654u16;
            let url = format!("ws://{}:{}", host, port);

            // 检测 server 是否已运行
            if !is_server_running(host, port).await {
                // 未运行 -> 后台拉起（detach），不随 TUI 退出而关闭
                println!("starting server at {}...", url);
                let pid = spawn_detached_server(&host, port)?;
                if wait_for_server(&host, port).await {
                    println!("server ready (pid: {})", pid);
                } else {
                    eprintln!("warning: server failed to start in time");
                }
            }

            // 获取 token
            let token = crate::transport::pairing::load_persisted_token()
                .unwrap_or_else(|| "missing-token".into());

            // spawn TUI 子进程
            let mut child = match spawn_tui_process(&url, &token) {
                Ok(c) => c,
                Err(e) => {
                    return Err(e);
                }
            };
            let _ = child.wait();
            Ok(())
        }

        Some(Commands::Server { host, port, domain, email, http_port, web_dir, detach }) => {
            // --detach: 守护进程模式，fork 子进程后父进程立即退出
            if detach {
                // 检查是否已有 server 在运行
                if is_server_running(&host, port).await {
                    println!("server already running at {}:{}", host, port);
                    return Ok(());
                }
                let pid = spawn_detached_server(&host, port)?;
                // 等待子进程就绪
                if wait_for_server(&host, port).await {
                    println!("mcoder server started in background (pid: {})", pid);
                    println!("logs: {}/mcoder.log", crate::config::global_config_dir().display());
                    println!("stop with: mcoder stop --port {}", port);
                } else {
                    eprintln!("warning: server may not have started; check logs at ~/.mcoder/mcoder.log");
                }
                return Ok(());
            }

            // 前台模式：获取锁后启动
            if !try_acquire_server_lock() {
                if is_server_running(&host, port).await {
                    eprintln!("server already running at {}:{}", host, port);
                    return Ok(());
                }
                anyhow::bail!("cannot acquire server lock (another instance may be starting); remove ~/.mcoder/server.lock if stale");
            }

            let result = start_server_full(
                &host,
                port,
                domain.as_deref(),
                email.as_deref(),
                http_port,
                web_dir.as_deref(),
            ).await;

            match result {
                Ok((server, _config, _project_dir)) => {
                    // keep running
                    tokio::signal::ctrl_c().await?;
                    println!("\nshutting down...");
                    let _ = server;
                    release_server_lock();
                    Ok(())
                }
                Err(e) => {
                    release_server_lock();
                    Err(e)
                }
            }
        }

        Some(Commands::Tui { url, token, no_spawn }) => {
            // TUI 模式：默认自动检测本地 server，未运行则拉起
            let (host, port) = parse_ws_url(&url)
                .unwrap_or(("127.0.0.1".into(), 7654));
            let is_local = host == "127.0.0.1" || host == "localhost" || host == "0.0.0.0";

            let token = token.unwrap_or_else(|| {
                crate::transport::pairing::load_persisted_token()
                    .unwrap_or_else(|| "missing-token".into())
            });

            // 自动拉起逻辑：本地 URL + server 未运行 + 未禁用 spawn
            // 多 TUI 并发安全：先检测端口，再检测锁，最后原子获取锁
            let _server_child: Option<std::process::Child> = if is_local && !no_spawn {
                if is_server_running(&host, port).await {
                    None  // 已在运行，直接连接
                } else if read_lock_pid().is_some() {
                    // 有其他进程正在启动 server，等待它就绪
                    println!("waiting for server to start...");
                    if wait_for_server(&host, port).await {
                        None
                    } else {
                        eprintln!("warning: server start timed out; connecting anyway");
                        None
                    }
                } else {
                    // 没有锁，尝试获取锁后拉起 server
                    if !try_acquire_server_lock() {
                        // 获取锁失败——另一个进程刚抢到了，等待
                        println!("waiting for server to start...");
                        if wait_for_server(&host, port).await {
                            None
                        } else {
                            eprintln!("warning: server start timed out; connecting anyway");
                            None
                        }
                    } else {
                        // 获取锁成功，以 detach 方式启动 server
                        println!("starting server at {}...", url);
                        let pid = spawn_detached_server(&host, port)?;
                        if wait_for_server(&host, port).await {
                            println!("server ready (pid: {})", pid);
                            // detach 的 server 独立运行，TUI 退出后不关闭
                            // 锁由 server 子进程管理，但这里我们 fork 的是 detach 模式
                            // server 子进程不会自己获取锁，所以锁由当前进程持有
                            // 但当前进程退出后锁会残留——需要 server 子进程在退出时清理
                            // 简化方案：detach 的 server 不用锁，靠端口检测去重
                            release_server_lock();
                        } else {
                            eprintln!("warning: server failed to start in time");
                            release_server_lock();
                        }
                        None
                    }
                }
            } else {
                None
            };

            let mut child = match spawn_tui_process(&url, &token) {
                Ok(c) => c,
                Err(e) => {
                    return Err(e);
                }
            };
            let _ = child.wait();
            Ok(())
        }

        Some(Commands::Stop { host, port }) => {
            // 优先尝试 HTTP /api/shutdown 端点
            match stop_server(&host, port).await {
                Ok(()) => {
                    println!("server at {}:{} stopped", host, port);
                    Ok(())
                }
                Err(_) => {
                    // fallback：跨平台 kill 进程
                    eprintln!("cannot reach server API, trying to kill by pid...");
                    if let Some(pid) = read_lock_pid() {
                        kill_process(pid);
                    }
                    release_server_lock();
                    println!("sent kill signal to processes on port {}", port);
                    Ok(())
                }
            }
        }

        Some(Commands::Pair { host, port }) => {
            let pairing = transport::pairing::generate_pairing(&host, port)?;
            println!("=== mcoder pairing ===");
            println!("Pairing string: {}", pairing.pairing_string);
            println!("\nConnect via:");
            for url in &pairing.urls {
                println!("  {}", url);
            }
            println!("\nScan QR code to connect:");
            let qr = transport::pairing::render_qr(&pairing.pairing_string);
            if !qr.is_empty() {
                println!("\n{}", qr);
            }
            Ok(())
        }

        Some(Commands::Sessions) => {
            let project = std::env::current_dir()?;
            let sessions = persistence::jsonl::JsonlSession::list(Some(&project))?;
            if sessions.is_empty() {
                println!("No sessions found.");
            } else {
                println!("{:<24} {:<20} {}", "ID", "Model", "Title");
                println!("{}", "-".repeat(70));
                for s in sessions {
                    println!("{:<24} {:<20} {}", s.session_id, s.model, s.title);
                }
            }
            Ok(())
        }
    }
}
