//! 后端 i18n 模块
//! 通过 config.language 配置，支持 "en" 和 "zh"
//! 用法: i18n::t("error.session_not_found", &lang)

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

static LANG_CACHE: OnceLock<RwLock<String>> = OnceLock::new();

/// 翻译 key -> { lang -> text }
/// 只存用户可见的文本（错误消息、命令描述、handoff prompt 等）
/// LLM prompt（system prompt、workflow prompt、tool description）保持英文不翻译
static TRANSLATIONS: OnceLock<HashMap<&'static str, HashMap<&'static str, &'static str>>> =
    OnceLock::new();

fn build_translations() -> HashMap<&'static str, HashMap<&'static str, &'static str>> {
    let mut m = HashMap::new();

    // ===== 错误消息 =====
    m.insert("error.session_not_found", {
        let mut l = HashMap::new();
        l.insert("en", "session not found");
        l.insert("zh", "会话未找到");
        l
    });
    m.insert("error.no_active_session", {
        let mut l = HashMap::new();
        l.insert("en", "no active session");
        l.insert("zh", "无活跃会话");
        l
    });
    m.insert("error.agent_loop_running", {
        let mut l = HashMap::new();
        l.insert("en", "agent loop already running");
        l.insert("zh", "代理循环已在运行");
        l
    });
    m.insert("error.path_not_exist", {
        let mut l = HashMap::new();
        l.insert("en", "path does not exist");
        l.insert("zh", "路径不存在");
        l
    });
    m.insert("error.provider_not_found", {
        let mut l = HashMap::new();
        l.insert("en", "provider not found");
        l.insert("zh", "供应商未找到");
        l
    });
    m.insert("error.invalid_params", {
        let mut l = HashMap::new();
        l.insert("en", "invalid params");
        l.insert("zh", "参数无效");
        l
    });
    m.insert("error.permission_denied", {
        let mut l = HashMap::new();
        l.insert("en", "permission denied");
        l.insert("zh", "权限不足");
        l
    });
    m.insert("error.usage_mode", {
        let mut l = HashMap::new();
        l.insert("en", "usage: /mode <role>");
        l.insert("zh", "用法：/mode <角色>");
        l
    });
    m.insert("error.usage_model_set", {
        let mut l = HashMap::new();
        l.insert("en", "usage: /model set <name>");
        l.insert("zh", "用法：/model set <名称>");
        l
    });
    m.insert("error.usage_model", {
        let mut l = HashMap::new();
        l.insert("en", "usage: /model [list|set <name>]");
        l.insert("zh", "用法：/model [list|set <名称>]");
        l
    });
    m.insert("error.usage_sessions_sub", {
        let mut l = HashMap::new();
        l.insert("en", "usage: /sessions");
        l.insert("zh", "用法：/sessions");
        l
    });
    m.insert("error.usage_sessions", {
        let mut l = HashMap::new();
        l.insert("en", "usage: /sessions <list|new|open <id>|delete <id>>");
        l.insert("zh", "用法：/sessions <list|new|open <ID>|delete <ID>>");
        l
    });
    m.insert("error.usage_task_cancel", {
        let mut l = HashMap::new();
        l.insert("en", "usage: /task cancel <id>");
        l.insert("zh", "用法：/task cancel <ID>");
        l
    });
    m.insert("error.usage_task", {
        let mut l = HashMap::new();
        l.insert("en", "usage: /task <list|cancel <id>>");
        l.insert("zh", "用法：/task <list|cancel <ID>>");
        l
    });
    m.insert("error.usage_config", {
        let mut l = HashMap::new();
        l.insert("en", "usage: /config <get|set> <key> [value]");
        l.insert("zh", "用法：/config <get|set> <键> [值]");
        l
    });
    m.insert("error.usage_config_get", {
        let mut l = HashMap::new();
        l.insert("en", "usage: /config get <key>");
        l.insert("zh", "用法：/config get <键>");
        l
    });
    m.insert("error.usage_config_set", {
        let mut l = HashMap::new();
        l.insert("en", "usage: /config set <key> <value>");
        l.insert("zh", "用法：/config set <键> <值>");
        l
    });
    m.insert("error.unsupported_lang", {
        let mut l = HashMap::new();
        l.insert("en", "unsupported language; use 'en' or 'zh'");
        l.insert("zh", "不支持的语言；请使用 'en' 或 'zh'");
        l
    });
    m.insert("error.usage_workflow", {
        let mut l = HashMap::new();
        l.insert(
            "en",
            "usage: /workflow <init|roadmap|propose|plan|apply|review|archive|continue|ff|loop|list> [change_id]",
        );
        l.insert(
            "zh",
            "用法：/workflow <init|roadmap|propose|plan|apply|review|archive|continue|ff|loop|list> [变更ID]",
        );
        l
    });
    m.insert("error.unknown_meta", {
        let mut l = HashMap::new();
        l.insert("en", "unknown meta command");
        l.insert("zh", "未知元命令");
        l
    });
    m.insert("error.auth_failed", {
        let mut l = HashMap::new();
        l.insert("en", "authentication failed: invalid or missing token");
        l.insert("zh", "身份验证失败：令牌无效或缺失");
        l
    });
    m.insert("error.tool_not_allowed", {
        let mut l = HashMap::new();
        l.insert("en", "tool is not allowed in current role");
        l.insert("zh", "当前角色不允许使用此工具");
        l
    });
    m.insert("error.model_not_found", {
        let mut l = HashMap::new();
        l.insert("en", "model not found");
        l.insert("zh", "模型未找到");
        l
    });
    m.insert("error.cannot_connect", {
        let mut l = HashMap::new();
        l.insert("en", "cannot reach server; check network and base_url");
        l.insert("zh", "无法连接服务器；请检查网络和 base_url");
        l
    });
    m.insert("error.session_id_required", {
        let mut l = HashMap::new();
        l.insert("en", "session_id is required");
        l.insert("zh", "必须提供 session_id");
        l
    });
    m.insert("error.message_id_required", {
        let mut l = HashMap::new();
        l.insert("en", "message_id is required");
        l.insert("zh", "必须提供 message_id");
        l
    });
    m.insert("error.model_required", {
        let mut l = HashMap::new();
        l.insert("en", "model is required");
        l.insert("zh", "必须提供 model");
        l
    });
    m.insert("error.task_id_required", {
        let mut l = HashMap::new();
        l.insert("en", "task_id is required");
        l.insert("zh", "必须提供 task_id");
        l
    });
    m.insert("error.parent_session_id_required", {
        let mut l = HashMap::new();
        l.insert("en", "parent_session_id is required");
        l.insert("zh", "必须提供 parent_session_id");
        l
    });
    m.insert("error.handoff_params_required", {
        let mut l = HashMap::new();
        l.insert("en", "session_id and task_prompt are required");
        l.insert("zh", "必须提供 session_id 和 task_prompt");
        l
    });
    m.insert("error.from_session_id_required", {
        let mut l = HashMap::new();
        l.insert("en", "from_session_id is required");
        l.insert("zh", "必须提供 from_session_id");
        l
    });
    m.insert("error.input_required", {
        let mut l = HashMap::new();
        l.insert("en", "input is required");
        l.insert("zh", "必须提供 input");
        l
    });
    m.insert("error.not_attached", {
        let mut l = HashMap::new();
        l.insert("en", "caller is not attached to a session");
        l.insert("zh", "调用方尚未附加到任何会话");
        l
    });
    m.insert("error.cross_session_denied", {
        let mut l = HashMap::new();
        l.insert("en", "cross-session access denied");
        l.insert("zh", "禁止跨会话访问");
        l
    });
    m.insert("error.method_not_found", {
        let mut l = HashMap::new();
        l.insert("en", "method not found");
        l.insert("zh", "方法未找到");
        l
    });

    // ===== Slash command 描述 =====
    m.insert("cmd.help", {
        let mut l = HashMap::new();
        l.insert("en", "show available commands");
        l.insert("zh", "显示可用命令");
        l
    });
    m.insert("cmd.mode", {
        let mut l = HashMap::new();
        l.insert("en", "switch role (normal|plan|goal|loop|execute|review)");
        l.insert("zh", "切换角色 (normal|plan|goal|loop|execute|review)");
        l
    });
    m.insert("cmd.model", {
        let mut l = HashMap::new();
        l.insert("en", "switch model (interactive picker; list|set <name>)");
        l.insert("zh", "切换模型 (交互选择; list|set <名称>)");
        l
    });
    m.insert("cmd.sessions", {
        let mut l = HashMap::new();
        l.insert("en", "session management (list|new|open|delete)");
        l.insert("zh", "会话管理 (list|new|open|delete)");
        l
    });
    m.insert("cmd.undo", {
        let mut l = HashMap::new();
        l.insert("en", "undo file changes");
        l.insert("zh", "撤销文件更改");
        l
    });
    m.insert("cmd.diff", {
        let mut l = HashMap::new();
        l.insert("en", "view git diff");
        l.insert("zh", "查看 git diff");
        l
    });
    m.insert("cmd.cancel", {
        let mut l = HashMap::new();
        l.insert("en", "cancel current agent loop");
        l.insert("zh", "取消当前代理循环");
        l
    });
    m.insert("cmd.task", {
        let mut l = HashMap::new();
        l.insert("en", "background task management");
        l.insert("zh", "后台任务管理");
        l
    });
    m.insert("cmd.config", {
        let mut l = HashMap::new();
        l.insert("en", "config management (get|set)");
        l.insert("zh", "配置管理 (get|set)");
        l
    });
    m.insert("cmd.pair", {
        let mut l = HashMap::new();
        l.insert("en", "show pairing info");
        l.insert("zh", "显示配对信息");
        l
    });
    m.insert("cmd.exit", {
        let mut l = HashMap::new();
        l.insert("en", "exit");
        l.insert("zh", "退出");
        l
    });
    m.insert("cmd.workflow", {
        let mut l = HashMap::new();
        l.insert("en", "spec-driven workflow orchestration");
        l.insert("zh", "规范驱动的工作流编排");
        l
    });
    m.insert("cmd.tree", {
        let mut l = HashMap::new();
        l.insert("en", "view message tree (fork/switch branches)");
        l.insert("zh", "查看消息树 (分叉/切换分支)");
        l
    });
    m.insert("cmd.setting", {
        let mut l = HashMap::new();
        l.insert("en", "open settings panel");
        l.insert("zh", "打开设置面板");
        l
    });
    m.insert("cmd.remote", {
        let mut l = HashMap::new();
        l.insert("en", "switch to a remote server connection");
        l.insert("zh", "切换到远程服务器连接");
        l
    });
    m.insert("cmd.thinking", {
        let mut l = HashMap::new();
        l.insert("en", "toggle thinking depth");
        l.insert("zh", "切换思考深度");
        l
    });
    m.insert("cmd.handoff", {
        let mut l = HashMap::new();
        l.insert("en", "handoff to a new session");
        l.insert("zh", "交接到新会话");
        l
    });
    m.insert("cmd.handoff_back", {
        let mut l = HashMap::new();
        l.insert("en", "handoff back to parent session");
        l.insert("zh", "回传到父会话");
        l
    });
    m.insert("cmd.lang", {
        let mut l = HashMap::new();
        l.insert("en", "switch interface language (en|zh)");
        l.insert("zh", "切换界面语言 (en|zh)");
        l
    });

    // ===== 通用 UI 文本 =====
    m.insert("ui.connected", {
        let mut l = HashMap::new();
        l.insert("en", "connected");
        l.insert("zh", "已连接");
        l
    });
    m.insert("ui.disconnected", {
        let mut l = HashMap::new();
        l.insert("en", "disconnected");
        l.insert("zh", "未连接");
        l
    });
    m.insert("ui.running", {
        let mut l = HashMap::new();
        l.insert("en", "running");
        l.insert("zh", "运行中");
        l
    });
    m.insert("ui.settings", {
        let mut l = HashMap::new();
        l.insert("en", "Settings");
        l.insert("zh", "设置");
        l
    });
    m.insert("ui.providers", {
        let mut l = HashMap::new();
        l.insert("en", "Providers");
        l.insert("zh", "供应商");
        l
    });
    m.insert("ui.subagents", {
        let mut l = HashMap::new();
        l.insert("en", "Subagents");
        l.insert("zh", "子代理");
        l
    });
    m.insert("ui.no_messages", {
        let mut l = HashMap::new();
        l.insert("en", "No messages yet");
        l.insert("zh", "暂无消息");
        l
    });
    m.insert("ui.no_session", {
        let mut l = HashMap::new();
        l.insert("en", "No session selected");
        l.insert("zh", "未选择会话");
        l
    });
    m.insert("ui.send_message", {
        let mut l = HashMap::new();
        l.insert("en", "type a message or /help for commands");
        l.insert("zh", "输入消息或 /help 查看命令");
        l
    });
    m.insert("ui.enter_to_send", {
        let mut l = HashMap::new();
        l.insert("en", "Enter to send");
        l.insert("zh", "Enter 发送");
        l
    });
    m.insert("ui.new_session", {
        let mut l = HashMap::new();
        l.insert("en", "New Session");
        l.insert("zh", "新建会话");
        l
    });
    m.insert("ui.add_provider", {
        let mut l = HashMap::new();
        l.insert("en", "Add Provider");
        l.insert("zh", "添加供应商");
        l
    });
    m.insert("ui.no_providers", {
        let mut l = HashMap::new();
        l.insert("en", "No providers configured");
        l.insert("zh", "未配置供应商");
        l
    });
    m.insert("ui.language", {
        let mut l = HashMap::new();
        l.insert("en", "Language");
        l.insert("zh", "语言");
        l
    });
    m.insert("ui.general", {
        let mut l = HashMap::new();
        l.insert("en", "General");
        l.insert("zh", "通用");
        l
    });
    m.insert("ui.server_connection", {
        let mut l = HashMap::new();
        l.insert("en", "Server Connection");
        l.insert("zh", "服务器连接");
        l
    });
    m.insert("ui.remote_server", {
        let mut l = HashMap::new();
        l.insert("en", "Remote Server");
        l.insert("zh", "远程服务器");
        l
    });
    m.insert("ui.connect", {
        let mut l = HashMap::new();
        l.insert("en", "Connect");
        l.insert("zh", "连接");
        l
    });
    m.insert("ui.model", {
        let mut l = HashMap::new();
        l.insert("en", "Model");
        l.insert("zh", "模型");
        l
    });
    m.insert("ui.role", {
        let mut l = HashMap::new();
        l.insert("en", "Role");
        l.insert("zh", "角色");
        l
    });
    m.insert("ui.max_iterations", {
        let mut l = HashMap::new();
        l.insert("en", "Max Iterations");
        l.insert("zh", "最大迭代次数");
        l
    });
    m.insert("ui.compact_threshold", {
        let mut l = HashMap::new();
        l.insert("en", "Compact Threshold");
        l.insert("zh", "压缩阈值");
        l
    });
    m.insert("ui.compact_keep_recent", {
        let mut l = HashMap::new();
        l.insert("en", "Compact Keep Recent");
        l.insert("zh", "压缩保留近期");
        l
    });
    m.insert("ui.memory_auto_recall", {
        let mut l = HashMap::new();
        l.insert("en", "Memory Auto Recall");
        l.insert("zh", "记忆自动召回");
        l
    });
    m.insert("ui.memory_auto_capture", {
        let mut l = HashMap::new();
        l.insert("en", "Memory Auto Capture");
        l.insert("zh", "记忆自动捕获");
        l
    });
    m.insert("ui.version", {
        let mut l = HashMap::new();
        l.insert("en", "Version");
        l.insert("zh", "版本");
        l
    });
    m.insert("ui.project_path", {
        let mut l = HashMap::new();
        l.insert("en", "Project Path");
        l.insert("zh", "项目路径");
        l
    });
    m.insert("ui.lsp_servers", {
        let mut l = HashMap::new();
        l.insert("en", "LSP Servers");
        l.insert("zh", "LSP 服务器");
        l
    });
    m.insert("ui.cancel", {
        let mut l = HashMap::new();
        l.insert("en", "Cancel");
        l.insert("zh", "取消");
        l
    });
    m.insert("ui.save", {
        let mut l = HashMap::new();
        l.insert("en", "Save");
        l.insert("zh", "保存");
        l
    });
    m.insert("ui.delete", {
        let mut l = HashMap::new();
        l.insert("en", "Delete");
        l.insert("zh", "删除");
        l
    });
    m.insert("ui.test", {
        let mut l = HashMap::new();
        l.insert("en", "Test");
        l.insert("zh", "测试");
        l
    });
    m.insert("ui.enable", {
        let mut l = HashMap::new();
        l.insert("en", "Enable");
        l.insert("zh", "启用");
        l
    });
    m.insert("ui.disable", {
        let mut l = HashMap::new();
        l.insert("en", "Disable");
        l.insert("zh", "禁用");
        l
    });
    m.insert("ui.params", {
        let mut l = HashMap::new();
        l.insert("en", "Params");
        l.insert("zh", "参数");
        l
    });
    m.insert("ui.name", {
        let mut l = HashMap::new();
        l.insert("en", "Name");
        l.insert("zh", "名称");
        l
    });
    m.insert("ui.protocol", {
        let mut l = HashMap::new();
        l.insert("en", "Protocol");
        l.insert("zh", "协议");
        l
    });
    m.insert("ui.base_url", {
        let mut l = HashMap::new();
        l.insert("en", "Base URL");
        l.insert("zh", "基础 URL");
        l
    });
    m.insert("ui.api_key", {
        let mut l = HashMap::new();
        l.insert("en", "API Key");
        l.insert("zh", "API 密钥");
        l
    });
    m.insert("ui.models", {
        let mut l = HashMap::new();
        l.insert("en", "Models");
        l.insert("zh", "模型");
        l
    });
    m.insert("ui.add", {
        let mut l = HashMap::new();
        l.insert("en", "Add");
        l.insert("zh", "添加");
        l
    });
    m.insert("ui.close", {
        let mut l = HashMap::new();
        l.insert("en", "Close");
        l.insert("zh", "关闭");
        l
    });
    m.insert("ui.back", {
        let mut l = HashMap::new();
        l.insert("en", "Back");
        l.insert("zh", "返回");
        l
    });
    m.insert("ui.thinking_depth", {
        let mut l = HashMap::new();
        l.insert("en", "Thinking Depth");
        l.insert("zh", "思考深度");
        l
    });
    m.insert("ui.handoff", {
        let mut l = HashMap::new();
        l.insert("en", "Handoff");
        l.insert("zh", "交接");
        l
    });
    m.insert("ui.handoff_back", {
        let mut l = HashMap::new();
        l.insert("en", "Handoff Back");
        l.insert("zh", "回传");
        l
    });
    m.insert("ui.active", {
        let mut l = HashMap::new();
        l.insert("en", "active");
        l.insert("zh", "活跃");
        l
    });
    m.insert("ui.total", {
        let mut l = HashMap::new();
        l.insert("en", "total");
        l.insert("zh", "总计");
        l
    });
    m.insert("ui.idle", {
        let mut l = HashMap::new();
        l.insert("en", "idle");
        l.insert("zh", "空闲");
        l
    });
    m.insert("ui.msg", {
        let mut l = HashMap::new();
        l.insert("en", "msg");
        l.insert("zh", "条");
        l
    });
    m.insert("ui.env_var_hint", {
        let mut l = HashMap::new();
        l.insert("en", "Supports ${ENV_VAR} syntax");
        l.insert("zh", "支持 ${ENV_VAR} 环境变量语法");
        l
    });
    m.insert("ui.comma_separated", {
        let mut l = HashMap::new();
        l.insert("en", "comma-separated");
        l.insert("zh", "逗号分隔");
        l
    });
    m.insert("ui.commands", {
        let mut l = HashMap::new();
        l.insert("en", "Commands");
        l.insert("zh", "命令");
        l
    });
    m.insert("ui.shortcuts", {
        let mut l = HashMap::new();
        l.insert("en", "Shortcuts");
        l.insert("zh", "快捷键");
        l
    });
    m.insert("ui.waiting_input", {
        let mut l = HashMap::new();
        l.insert("en", "waiting for input");
        l.insert("zh", "等待输入");
        l
    });
    m.insert("ui.answered", {
        let mut l = HashMap::new();
        l.insert("en", "answered");
        l.insert("zh", "已回答");
        l
    });
    m.insert("ui.waiting_confirm", {
        let mut l = HashMap::new();
        l.insert("en", "waiting for confirmation");
        l.insert("zh", "等待确认");
        l
    });
    m.insert("ui.approved", {
        let mut l = HashMap::new();
        l.insert("en", "approved");
        l.insert("zh", "已通过");
        l
    });
    m.insert("ui.always_approved", {
        let mut l = HashMap::new();
        l.insert("en", "always approved");
        l.insert("zh", "永久通过");
        l
    });
    m.insert("ui.denied", {
        let mut l = HashMap::new();
        l.insert("en", "denied");
        l.insert("zh", "已拒绝");
        l
    });
    m.insert("ui.navigate", {
        let mut l = HashMap::new();
        l.insert("en", "navigate");
        l.insert("zh", "导航");
        l
    });
    m.insert("ui.switch", {
        let mut l = HashMap::new();
        l.insert("en", "switch");
        l.insert("zh", "切换");
        l
    });
    m.insert("ui.select", {
        let mut l = HashMap::new();
        l.insert("en", "select");
        l.insert("zh", "选择");
        l
    });
    m.insert("ui.loading", {
        let mut l = HashMap::new();
        l.insert("en", "loading...");
        l.insert("zh", "加载中...");
        l
    });
    m.insert("ui.saving", {
        let mut l = HashMap::new();
        l.insert("en", "Saving...");
        l.insert("zh", "保存中...");
        l
    });
    m.insert("ui.adding", {
        let mut l = HashMap::new();
        l.insert("en", "Adding...");
        l.insert("zh", "添加中...");
        l
    });
    m.insert("ui.working", {
        let mut l = HashMap::new();
        l.insert("en", "working...");
        l.insert("zh", "处理中...");
        l
    });

    // ===== handoff prompt =====
    m.insert("handoff.doc_prompt_intro", {
        let mut l = HashMap::new();
        l.insert(
            "en",
            "You are a handoff document generator. Based on the conversation history below, generate a structured handoff Markdown document.",
        );
        l.insert(
            "zh",
            "你是一个交接文档生成器。根据以下对话历史，生成一份结构化的交接 Markdown 文档。",
        );
        l
    });
    m.insert("handoff.back_prompt_intro", {
        let mut l = HashMap::new();
        l.insert(
            "en",
            "You are a handoff back document generator. Based on the sub-agent's conversation history below, generate a concise handoff back Markdown document.",
        );
        l.insert(
            "zh",
            "你是一个回传文档生成器。根据以下子代理的对话历史，生成一份简洁的回传 Markdown 文档。",
        );
        l
    });

    m
}

/// 翻译函数
/// lang: "en" 或 "zh"，其他值 fallback 到 "en"
pub fn t(key: &'static str, lang: &str) -> &'static str {
    let lang = lang.to_ascii_lowercase();
    let translations = TRANSLATIONS.get_or_init(build_translations);
    if let Some(langs) = translations.get(key) {
        langs
            .get(lang.as_str())
            .or_else(|| langs.get("en"))
            .copied()
            .unwrap_or(key)
    } else {
        key
    }
}

/// 获取当前缓存的界面语言，不触发配置读盘。
pub fn current_lang() -> String {
    LANG_CACHE
        .get_or_init(|| RwLock::new("en".into()))
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 同步运行时界面语言缓存。
pub fn set_current_lang(lang: &str) {
    let lang = if lang.eq_ignore_ascii_case("zh") {
        "zh"
    } else {
        "en"
    };
    *LANG_CACHE
        .get_or_init(|| RwLock::new("en".into()))
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = lang.into();
}
