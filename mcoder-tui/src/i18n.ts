//! 前端 i18n 模块
//! 用法: import { t, setLang, getLang } from './i18n.js';
//! t('ui.settings') -> "Settings" (en) or "设置" (zh)

type Lang = 'en' | 'zh';

let currentLang: Lang = 'en';
let listeners: Array<() => void> = [];

const translations: Record<string, Record<Lang, string>> = {
  // ===== 通用 UI =====
  'ui.connected': { en: 'connected', zh: '已连接' },
  'ui.disconnected': { en: 'disconnected', zh: '未连接' },
  'ui.running': { en: 'running', zh: '运行中' },
  'ui.settings': { en: 'Settings', zh: '设置' },
  'ui.providers': { en: 'Providers', zh: '供应商' },
  'ui.subagents': { en: 'Subagents', zh: '子代理' },
  'ui.no_messages': { en: 'No messages yet', zh: '暂无消息' },
  'ui.no_session': { en: 'No session selected', zh: '未选择会话' },
  'ui.no_session_hint': { en: 'No session selected. Press + to create one.', zh: '未选择会话。按 + 创建。' },
  'ui.send_message': { en: 'type a message or /help for commands', zh: '输入消息或 /help 查看命令' },
  'ui.send_message_shift': { en: 'type a message or /help for commands (Shift+Enter for newline)', zh: '输入消息或 /help 查看命令 (Shift+Enter 换行)' },
  'ui.enter_to_send': { en: 'Enter to send', zh: 'Enter 发送' },
  'ui.shift_newline': { en: 'Shift+Enter for newline', zh: 'Shift+Enter 换行' },
  'ui.new_session': { en: 'New Session', zh: '新建会话' },
  'ui.add_provider': { en: 'Add Provider', zh: '添加供应商' },
  'ui.no_providers': { en: 'No providers configured', zh: '未配置供应商' },
  'ui.no_providers_hint': { en: 'Click "Add Provider" to set up OpenAI / Anthropic / Ollama / etc.', zh: '点击"添加供应商"配置 OpenAI / Anthropic / Ollama 等' },
  'ui.language': { en: 'Language', zh: '语言' },
  'ui.general': { en: 'General', zh: '通用' },
  'ui.server_connection': { en: 'Server Connection', zh: '服务器连接' },
  'ui.remote_server': { en: 'Remote Server', zh: '远程服务器' },
  'ui.remote_server_desc': { en: 'Connect to a remote mcoder server', zh: '连接到远程 mcoder 服务器' },
  'ui.connect': { en: 'Connect', zh: '连接' },
  'ui.model': { en: 'Model', zh: '模型' },
  'ui.model_desc': { en: 'LLM model for this session', zh: '本会话使用的 LLM 模型' },
  'ui.role': { en: 'Role', zh: '角色' },
  'ui.role_desc': { en: 'Agent role / mode', zh: '代理角色 / 模式' },
  'ui.max_iterations': { en: 'Max Iterations', zh: '最大迭代次数' },
  'ui.max_iterations_desc': { en: 'Max agent loop iterations', zh: '代理循环最大迭代次数' },
  'ui.compact_threshold': { en: 'Compact Threshold', zh: '压缩阈值' },
  'ui.compact_threshold_desc': { en: 'Context fill ratio (0-1) to trigger compaction', zh: '上下文填充比例 (0-1) 触发压缩' },
  'ui.compact_keep_recent': { en: 'Compact Keep Recent', zh: '压缩保留近期' },
  'ui.compact_keep_recent_desc': { en: 'Messages to keep after compaction', zh: '压缩后保留的消息数' },
  'ui.memory_auto_recall': { en: 'Memory Auto Recall', zh: '记忆自动召回' },
  'ui.memory_auto_recall_desc': { en: 'Automatically recall relevant memories', zh: '自动召回相关记忆' },
  'ui.memory_auto_capture': { en: 'Memory Auto Capture', zh: '记忆自动捕获' },
  'ui.memory_auto_capture_desc': { en: 'Automatically capture memories from conversations', zh: '自动从对话中捕获记忆' },
  'ui.version': { en: 'Version', zh: '版本' },
  'ui.project_path': { en: 'Project Path', zh: '项目路径' },
  'ui.lsp_servers': { en: 'LSP Servers', zh: 'LSP 服务器' },
  'ui.cancel': { en: 'Cancel', zh: '取消' },
  'ui.save': { en: 'Save', zh: '保存' },
  'ui.delete': { en: 'Delete', zh: '删除' },
  'ui.test': { en: 'Test', zh: '测试' },
  'ui.enable': { en: 'Enable', zh: '启用' },
  'ui.disable': { en: 'Disable', zh: '禁用' },
  'ui.params': { en: 'Params', zh: '参数' },
  'ui.name': { en: 'Name', zh: '名称' },
  'ui.protocol': { en: 'Protocol', zh: '协议' },
  'ui.base_url': { en: 'Base URL', zh: '基础 URL' },
  'ui.api_key': { en: 'API Key', zh: 'API 密钥' },
  'ui.models': { en: 'Models', zh: '模型' },
  'ui.add': { en: 'Add', zh: '添加' },
  'ui.close': { en: 'Close', zh: '关闭' },
  'ui.back': { en: 'Back', zh: '返回' },
  'ui.thinking_depth': { en: 'Thinking Depth', zh: '思考深度' },
  'ui.handoff': { en: 'Handoff', zh: '交接' },
  'ui.handoff_back': { en: 'Handoff Back', zh: '回传' },
  'ui.active': { en: 'active', zh: '活跃' },
  'ui.total': { en: 'total', zh: '总计' },
  'ui.idle': { en: 'idle', zh: '空闲' },
  'ui.msg': { en: 'msg', zh: '条' },
  'ui.env_var_hint': { en: 'Supports ${ENV_VAR} syntax', zh: '支持 ${ENV_VAR} 环境变量语法' },
  'ui.comma_separated': { en: 'comma-separated', zh: '逗号分隔' },
  'ui.commands': { en: 'Commands', zh: '命令' },
  'ui.shortcuts': { en: 'Shortcuts', zh: '快捷键' },
  'ui.waiting_input': { en: 'waiting for input', zh: '等待输入' },
  'ui.answered': { en: 'answered', zh: '已回答' },
  'ui.waiting_confirm': { en: 'waiting for confirmation', zh: '等待确认' },
  'ui.approved': { en: 'approved', zh: '已通过' },
  'ui.always_approved': { en: 'always approved', zh: '永久通过' },
  'ui.denied': { en: 'denied', zh: '已拒绝' },
  'ui.navigate': { en: 'navigate', zh: '导航' },
  'ui.switch': { en: 'switch', zh: '切换' },
  'ui.select': { en: 'select', zh: '选择' },
  'ui.loading': { en: 'loading...', zh: '加载中...' },
  'ui.saving': { en: 'Saving...', zh: '保存中...' },
  'ui.adding': { en: 'Adding...', zh: '添加中...' },
  'ui.working': { en: 'working...', zh: '处理中...' },
  'ui.attach_image': { en: 'Attach image', zh: '附加图片' },
  'ui.image': { en: 'Image', zh: '图片' },
  'ui.graph': { en: 'Graph', zh: '图谱' },
  'ui.diff': { en: 'Diff', zh: '差异' },
  'ui.tree': { en: 'Tree', zh: '树' },
  'ui.select_graph_hint': { en: 'Select Graph, Diff, or click a file', zh: '选择图谱、差异或点击文件' },
  'ui.no_graph_data': { en: 'No graph data. Run graph_index first.', zh: '无图谱数据。请先运行 graph_index。' },
  'ui.git_diff': { en: 'Git Diff', zh: 'Git 差异' },
  'ui.no_changes': { en: 'No changes', zh: '无变更' },
  'ui.message_tree': { en: 'Message Tree', zh: '消息树' },
  'ui.no_messages_session': { en: 'No messages in this session', zh: '此会话暂无消息' },
  'ui.collapse_all': { en: 'Collapse all', zh: '全部折叠' },
  'ui.no_files': { en: 'No files', zh: '无文件' },
  'ui.no_sessions_yet': { en: 'No sessions yet. Create one above.', zh: '暂无会话。请在上方创建。' },
  'ui.close_tab': { en: 'Close tab', zh: '关闭标签' },
  'ui.new_session_project': { en: 'New session in this project', zh: '在此项目中新建会话' },
  'ui.plan_pending': { en: 'Plan pending approval', zh: '计划待审批' },
  'ui.offline': { en: 'offline...', zh: '离线...' },
  'ui.set_default': { en: 'Set as default', zh: '设为默认' },
  'ui.edit_params': { en: 'Edit params', zh: '编辑参数' },
  'ui.key_set': { en: 'key set', zh: '已设密钥' },
  'ui.no_key': { en: 'no key', zh: '无密钥' },
  'ui.disabled': { en: 'disabled', zh: '已禁用' },
  'ui.not_set': { en: '(not set)', zh: '(未设置)' },
  'ui.default': { en: '(default)', zh: '(默认)' },
  'ui.off': { en: 'Off', zh: '关闭' },
  'ui.usage': { en: 'Usage: /handoff <task description>', zh: '用法: /handoff <任务描述>' },
  'ui.no_active_session': { en: 'no active session', zh: '无活跃会话' },
  'ui.handoff_to': { en: 'Handoff ->', zh: '交接 ->' },
  'ui.handoff_back_to': { en: 'Handoff back to', zh: '回传到' },
  'ui.suggested_skills': { en: 'Suggested Skills', zh: '建议技能' },
  'ui.navigate_hint': { en: 'navigate', zh: '导航' },
  'ui.switch_hint': { en: 'switch', zh: '切换' },
  'ui.back_hint': { en: 'back', zh: '返回' },
  // ===== 命令选择面板 =====
  'ui.no_commands': { en: 'No commands found', zh: '未找到命令' },
  'ui.background_tasks': { en: 'Background Tasks', zh: '后台任务' },
  'ui.empty': { en: 'empty', zh: '空' },
  'ui.interrupted': { en: 'interrupted', zh: '已中断' },
  'ui.args': { en: 'args', zh: '参数' },
  'ui.error_label': { en: 'error', zh: '错误' },
  'ui.switching': { en: 'switching...', zh: '切换中...' },
  'ui.submitting': { en: 'submitting...', zh: '提交中...' },
  'ui.protocol_field': { en: 'protocol', zh: '协议' },
  'ui.cannot_get_schema': { en: 'Cannot get protocol fields', zh: '无法获取协议字段' },
  'ui.no_providers_add': { en: 'No providers. Press a to add.', zh: '无供应商。按 a 添加。' },
  // ===== 思考深度描述 =====
  'thinking.none': { en: 'None', zh: '关闭' },
  'thinking.low': { en: 'Low', zh: '低' },
  'thinking.medium': { en: 'Medium', zh: '中' },
  'thinking.high': { en: 'High', zh: '高' },
  'thinking.max': { en: 'Max', zh: '最高' },
  'thinking.none.desc': { en: 'No thinking', zh: '不启用思考' },
  'thinking.low.desc': { en: 'Light thinking', zh: '浅度思考' },
  'thinking.medium.desc': { en: 'Medium thinking', zh: '中度思考' },
  'thinking.high.desc': { en: 'Deep thinking', zh: '深度思考' },
  'thinking.max.desc': { en: 'Max thinking', zh: '最大思考' },
  // ===== 复合提示 =====
  'hint.provider_add_form': { en: 'Tab: next field · ↑↓: protocol switch · Enter: next/submit · Esc: cancel', zh: 'Tab: 下一字段 · ↑↓: 协议切换 · Enter: 下一字段/提交 · Esc: 取消' },
  'hint.provider_params': { en: '↑↓/Tab: switch field · ←/->: enum switch · M: switch model · Enter: save · Esc: cancel', zh: '↑↓/Tab: 切换字段 · ←/->: 枚举切换 · M: 切换模型 · Enter: 保存 · Esc: 取消' },
  'hint.esc_close': { en: 'Esc: close', zh: 'Esc: 关闭' },
  // ===== 错误提示 =====
  'error.answer_all': { en: 'Please answer all questions (number keys to select + optional note)', zh: '请回答所有问题（数字键选择 + 可选 note）' },
  // ===== 命令 =====
  'cmd.lang_usage': { en: 'Usage: /lang <en|zh>', zh: '用法: /lang <en|zh>' },
  'cmd.lang_set': { en: 'Language set to', zh: '语言已设置为' },
  'cmd.lang_current': { en: 'Current language:', zh: '当前语言:' },
};

export function setLang(lang: Lang) {
  currentLang = lang;
  listeners.forEach((listener) => listener());
}

export function onLangChange(cb: () => void): () => void {
  listeners.push(cb);
  return () => {
    listeners = listeners.filter((listener) => listener !== cb);
  };
}

export function getLang(): Lang {
  return currentLang;
}

export function t(key: string): string {
  const entry = translations[key];
  if (!entry) return key;
  return entry[currentLang] || entry.en || key;
}

/// 从后端加载语言设置
export async function loadLang(client: { request: (method: string, params?: any) => Promise<any> }): Promise<Lang> {
  try {
    const result = await client.request('config.get_language');
    const lang = result?.language === 'zh' ? 'zh' : 'en';
    setLang(lang);
    return lang;
  } catch {
    return 'en';
  }
}
