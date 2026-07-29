mod agent;
// 设计文档 §8.7 M5: 浏览器工具（headless Chrome 自测）
mod browser;
// 设计文档 §8.7 M5: Computer Use（桌面级自测）
mod computer_use;
mod code_graph;
mod config;
// 设计文档 §8.4.3: DAP 调试子系统
mod debug;
mod llm;
mod lsp;
mod memory;
mod persistence;
mod plugin;
mod session_manager;
mod tools;
mod transport;
mod tree_sitter;
mod types;
mod workflow;

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
    },
    /// Start TUI client (connects to a running server)
    Tui {
        #[arg(long, default_value = "ws://127.0.0.1:7654")]
        url: String,
        #[arg(long)]
        token: Option<String>,
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
}

/// 启动 server 的共享逻辑：返回 (WsServer, AppConfig, project_dir)
/// 设计文档 §1.1: server 无状态外，所有状态在 SQLite
/// 设计文档 §8.6: 当配置了域名时，自动申请 Let's Encrypt 证书
async fn start_server(host: &str, port: u16) -> anyhow::Result<(
    Arc<transport::ws_server::WsServer>,
    Arc<types::AppConfig>,
    std::path::PathBuf,
)> {
    start_server_full(host, port, None, None, None, None).await
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

    // Initialize async task manager（用于后台执行长任务和子代理）
    let task_manager = Arc::new(agent::async_tasks::TaskManager::new());

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
    let (mut tools_reg, subagent_tool) = tools::build_full_registry();

    // 设计文档 §8.3.4: 加载 skills（全局 ~/.mcoder/skills/ + 项目 .mcoder/skills/）
    let global_skills_dir = crate::config::global_config_dir().join("skills");
    let project_skills_dir = project_dir.join("skills");
    match plugin::skills::build_skill_tools(&global_skills_dir, &project_skills_dir).await {
        Ok((_skill_registry, skill_tools)) => {
            if !skill_tools.is_empty() {
                tracing::info!("registered {} skill tools", skill_tools.len());
            }
            tools_reg.register_all(skill_tools);
        }
        Err(e) => {
            tracing::warn!("failed to load skills: {}", e);
        }
    }

    // 设计文档 §8.3.2: 启动 MCP servers 并注册其工具
    let mcp_manager = Arc::new(plugin::mcp::McpManager::new());
    if !app_config.mcp_servers.is_empty() {
        match mcp_manager.start_all(&app_config.mcp_servers).await {
            Ok(mcp_tools) => {
                tracing::info!("registered {} MCP tools", mcp_tools.len());
                tools_reg.register_all(mcp_tools);
            }
            Err(e) => {
                tracing::warn!("failed to start MCP servers: {}", e);
            }
        }
    }

    let tools = Arc::new(tools_reg);

    // 设计文档 §8.5: 注入 SubagentTool 依赖（late binding）
    // 子代理根据 role 选择 model 和工具白名单
    let default_model_config = app_config.models.get(&app_config.default_model)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("default_model '{}' not found in config", app_config.default_model))?;
    let default_llm = llm::create_adapter(&default_model_config)?;
    subagent_tool.set_dependencies(Arc::new(tools::subagent::SubagentDeps {
        default_llm,
        default_model_config,
        tools: tools.clone(),
        role_registry: role_registry.clone(),
    })).await;

    let mgr = session_manager::SessionManager::new(
        tools,
        app_config.clone(),
        plugins.clone(),
        task_manager,
        role_registry,
        experience_store,
        mcp_manager,
    );

    // 设计文档 §8.3.3: 触发 OnStart hook（server 启动时）
    let _ = plugins.run_hooks(
        crate::plugin::HookPoint::OnStart,
        crate::plugin::HookContext::new(crate::plugin::HookPoint::OnStart, ""),
    ).await;

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
/// `mcoder tui` = 仅启动 TUI（连接已运行的 server）
fn spawn_tui_process(url: &str, token: &str) -> anyhow::Result<std::process::Child> {
    // 优先使用 bundled 的 mcoder-tui（dist/index.js）
    // 退回到全局安装的 mcoder-tui
    let candidates: Vec<String> = vec![
        // 1. 同目录的 mcoder-tui/dist/index.js（开发模式）
        std::env::current_exe()?
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("mcoder-tui").join("dist").join("index.js").to_string_lossy().to_string())
            .unwrap_or_default(),
        // 2. 全局安装的 mcoder-tui
        "mcoder-tui".to_string(),
    ];

    for candidate in &candidates {
        if candidate.is_empty() { continue; }
        let result = std::process::Command::new("node")
            .arg(candidate)
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
    anyhow::bail!("failed to spawn TUI process; install with `npm i -g @mcoder/tui` or run `npm run build` in mcoder-tui/")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    // 设计文档 §1.1 / §6.13: 无 subcommand → 嵌入式模式（server + TUI）
    // 子命令 → 单独模式
    match cli.command {
        None => {
            // 嵌入式：启动 server，然后 spawn TUI 子进程；TUI 退出后关 server
            let host = "127.0.0.1";
            let port = 7654u16;
            let (server, _config, _project_dir) = start_server(host, port).await?;

            // 等待 server 就绪（已 listen 即可，WsServer::start 内部已 bind）
            let url = format!("ws://{}:{}", host, port);
            let token = server.pairing_info().token.clone();

            // spawn TUI 子进程
            let mut child = match spawn_tui_process(&url, &token) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("warning: failed to spawn TUI ({}); server continues alone", e);
                    tokio::signal::ctrl_c().await?;
                    return Ok(());
                }
            };

            // 等 TUI 退出或 Ctrl+C
            let wait_fut = async {
                let _ = child.wait();
            };
            tokio::pin!(wait_fut);
            tokio::select! {
                _ = wait_fut => {
                    println!("TUI exited, shutting down server...");
                }
                _ = tokio::signal::ctrl_c() => {
                    println!("\nCtrl+C received, shutting down...");
                    let _ = child.kill();
                }
            }
            Ok(())
        }

        Some(Commands::Server { host, port, domain, email, http_port, web_dir }) => {
            let (server, _config, _project_dir) = start_server_full(
                &host,
                port,
                domain.as_deref(),
                email.as_deref(),
                http_port,
                web_dir.as_deref(),
            ).await?;

            // keep running
            tokio::signal::ctrl_c().await?;
            println!("\nshutting down...");
            let _ = server;
            Ok(())
        }

        Some(Commands::Tui { url, token }) => {
            // 仅启动 TUI，连接到已运行的 server
            // token 未指定时，从 ~/.mcoder/credentials.toml 读取
            let token = token.unwrap_or_else(|| {
                crate::transport::pairing::load_persisted_token()
                    .unwrap_or_else(|| "missing-token".into())
            });
            let mut child = spawn_tui_process(&url, &token)?;
            let _ = child.wait();
            Ok(())
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
