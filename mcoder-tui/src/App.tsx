// 设计文档 §6.2 / §6.3: 主应用组件
// 布局：顶部信息（可滚动） + 消息区（可滚动） + 固定区（BottomStatus + 输入框）
// 设计文档 §6.7: 多视图切换（chat/sessions/todos/tasks/config/help）

import { useState, useEffect, useMemo, useRef } from 'react';
import { Box, Text, useInput, useApp } from 'ink';
import { useSessionStore, useMessagesStore, useUiStore } from './store/index.js';
import { useAskStore, type PendingAsk } from './ask/store.js';
import { usePermissionStore } from './permission/store.js';
import { ASK_USER_TOOL } from './ask/types.js';
import { hasToolUse } from './ask/messages.js';
/// 设计文档 §8.8: 权限审批占位 tool name（虚拟，仅用于 desktop/mobile 渲染识别）
const PERMISSION_TOOL_NAME = '__permission_pending__';
import { serializeSubmission } from './ask/validation.js';
import { dispatchSlashCommand } from './commands/index.js';
import { WsClient } from './rpc/client.js';
import type { Message } from './rpc/types.js';
import { hydrateSnapshot, type SessionSnapshot } from './rpc/sessionSnapshot.js';
import { computeResumeEntry as computeResumeEntryPure } from './resume/state.js';
import { clearSessionUiState } from './store/clearSessionUiState.js';
import {
  MessageList, PlanApproval,
  SessionList, TodoView, TodoSummaryBar, TaskMonitor, ConfigView, HelpView,
  InputBox, AskUserCard, AskUserSummary, ResumeBar, TreeView, ModelView,
  SettingView, ProviderView, ThinkingPicker, SubagentBar, CommandPicker,
} from './components/index.js';
import { formatContext, formatCost } from './utils/format.js';
import { parsePairingString } from './utils/pairing.js';
import { TUI_COLORS, PREFIX } from './theme.js';
import { ShimmerText } from './components/ShimmerText.js';
import { t, loadLang, onLangChange } from './i18n.js';

interface Props {
  client: WsClient;
}

export function App({ client: initialClient }: Props) {
  const [client, setClient] = useState(initialClient);
  const [input, setInput] = useState('');
  // 输入 / 开头时弹出命令选择面板
  const [showCommandPicker, setShowCommandPicker] = useState(false);
  const [, setLangVersion] = useState(0);
  // S3 修复: 思考深度按 sessionId 分别存储（quick_thinking 不写盘也不触发 config_updated）
  const thinkingBySessionRef = useRef<Map<string, string>>(new Map());
  const getThinkingFor = (sid: string | null): string =>
    sid ? (thinkingBySessionRef.current.get(sid) || 'none') : 'none';
  const setThinkingFor = (sid: string | null, depth: string): void => {
    if (!sid) return;
    thinkingBySessionRef.current.set(sid, depth);
  };
  const uiStore = useUiStore();
  const sessionStore = useSessionStore();
  const msgStore = useMessagesStore();
  const askStore = useAskStore();
  const permissionStore = usePermissionStore();
  const { exit } = useApp();

  // 当前 session 的 pending ask
  const sid = sessionStore.currentSessionId;
  const pendingAsk: PendingAsk | null = useMemo(
    () => (sid ? askStore.pending[sid] || null : null),
    [sid, askStore.pending],
  );
  const askInputMode = !!(sid && askStore.askInputMode[sid]);

  // 通知处理（issue 6/9：Ask 通知只更新 store，消息历史由服务端真实 Message 事件负责；
  //             不再用 seenNotifications 持久去重遮挡）
  useEffect(() => {
    const handler = (notif: any) => {
      switch (notif.method) {
        case 'message':
          msgStore.addMessage(notif.params.message as Message);
          if (notif.params.message.role === 'assistant') {
            msgStore.setStreaming(false);
          }
          break;
        case 'tool_call_start':
          msgStore.setStreaming(true);
          break;
        case 'tool_call_done':
          break;
        case 'session_created':
          loadSessions();
          break;
        case 'session.mode_event':
          sessionStore.setRole(notif.params.role);
          break;
        case 'session.model_changed':
          sessionStore.setModel(notif.params.model);
          break;
        case 'session.plan_created':
          sessionStore.setPendingPlan(notif.params.plan);
          break;
        case 'session.todo_updated':
          sessionStore.setPendingTodos(notif.params.todos);
          break;
        case 'session.ask_pending': {
          // 服务端广播 ask pending：仅更新 store（issue 6/9）
          // 若消息流中已存在对应 tool_call_id 的 tool_use block（hasToolUse），
          // 不再重复追加；占位 tool_use 由服务端真实 Message 事件负责。
          const p = notif.params;
          if (p && p.ask_id && p.tool_call_id && p.request) {
            askStore.setPendingIdempotent({
              ask_id: p.ask_id,
              tool_call_id: p.tool_call_id,
              session_id: p.session_id,
              request: p.request,
              created_at: Date.now(),
            });
            if (!hasToolUse(msgStore.messages, p.tool_call_id)) {
              msgStore.addMessage({
                role: 'assistant',
                content: [{
                  type: 'tool_use',
                  id: p.tool_call_id,
                  name: ASK_USER_TOOL,
                  args: p.request,
                }],
              });
            }
          }
          break;
        }
        case 'session.ask_answered': {
          const p = notif.params;
          if (p && p.session_id && p.ask_id && p.tool_call_id) {
            const ok = askStore.setSubmissionIfMatch(
              p.session_id,
              p.ask_id,
              p.tool_call_id,
              p.submission,
            );
            // 仅当 store 真的写入终态、且消息流中未存在对应 tool_result 时才追加（issue 6/9）
            if (ok) {
              const haveResult = msgStore.messages.some((m) =>
                m.content.some(
                  (b) => b.type === 'tool_result' && b.id === p.tool_call_id,
                ),
              );
              if (!haveResult) {
                msgStore.addMessage({
                  role: 'tool',
                  content: [{
                    type: 'tool_result',
                    id: p.tool_call_id,
                    output: p.result,
                  }],
                });
              }
            }
          }
          break;
        }
        case 'session.ask_cancelled': {
          // 校验 ask_id + tool_call_id 后清空 pending（issue 8）
          const p = notif.params;
          if (p && p.session_id && p.ask_id && p.tool_call_id) {
            askStore.clearPendingByIds(p.session_id, p.ask_id, p.tool_call_id);
          }
          break;
        }
        case 'session.done':
          sessionStore.setLoopState('stopped', notif.params.reason);
          sessionStore.setCanResume(true);
          msgStore.setStreaming(false);
          break;
        case 'session.usage_updated': {
          const p = notif.params;
          if (p && p.cumulative) {
            // 用累计 usage 的总输入作为 contextUsed（真实占用）
            const used = (p.cumulative.prompt_tokens || 0)
              + (p.cumulative.cache_read_input_tokens || 0)
              + (p.cumulative.cache_creation_input_tokens || 0);
            sessionStore.setContextUsage(used, p.context_window || 0);
            sessionStore.setUsage(p.cumulative, sessionStore.sessionCost);
          }
          break;
        }
        case 'error':
          msgStore.setError(notif.params.message);
          msgStore.setStreaming(false);
          break;
        case 'permission.pending': {
          // 设计文档 §8.8: 权限审批 pending 通知
          const p = notif.params;
          if (p && p.request) {
            permissionStore.setPending(p.session_id, p.request);
            // 消息流占位：插入一条 assistant 消息含 tool_use block（虚拟工具名 PERMISSION_TOOL_NAME）
            if (!hasToolUse(msgStore.messages, p.request.tool_call_id)) {
              msgStore.addMessage({
                role: 'assistant',
                content: [{
                  type: 'tool_use',
                  id: p.request.tool_call_id,
                  name: PERMISSION_TOOL_NAME,
                  args: { real_tool_name: p.request.tool_name, ...p.request },
                }],
              });
            }
          }
          break;
        }
        case 'permission.resolved': {
          // 决议广播：清理 pending + 追加决议摘要消息
          const p = notif.params;
          if (p && p.request_id) {
            const decision = p.decision;
            permissionStore.setResolved(p.session_id, p.request_id, {
              type: decision.type === 'Allow' ? 'allow'
                : decision.type === 'AlwaysAllow' ? 'always_allow'
                : 'deny',
              reason: decision.reason,
            });
          }
          break;
        }
        case 'config_updated': {
          // 重新加载语言设置（后端 config.set_language 会广播此通知）
          loadLang(client).catch(() => {});
          break;
        }
      }
    };
    client.onNotification(handler);
    return () => {
      client.offNotification(handler);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client]);

  // Phase 2: 不再在 sid 变化后单独调 ask.pending -- pending_ask 由 session.attach 的
  // SessionSnapshot.pending_ask 字段提供，并由 hydrateSnapshot 一次性写入 store。
  // 此处保留 useEffect 钩子空实现仅为保留引用语义；如需扩展，加在 hydrate 路径上。
  useEffect(() => {
    // intentionally empty (Phase 2: pending_ask comes from snapshot)
  }, [sid]);

  // 语言变化时刷新所有使用 t() 的 TUI 文本
  useEffect(() => onLangChange(() => setLangVersion((version) => version + 1)), []);

  // 启动时从后端加载语言设置
  useEffect(() => {
    loadLang(client).catch(() => {});
  }, [client]);

  const loadSessions = async () => {
    try {
      const result = await client.request('sessions.list');
      sessionStore.setSessions(result);
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  };

  // 提交当前 ask 答案
  const submitAsk = async (submission: { cancelled: boolean; answers: Record<number, any> }) => {
    if (!sid || !pendingAsk) return;
    try {
      await client.request('ask.answer', {
        session_id: sid,
        ask_id: pendingAsk.ask_id,
        submission: serializeSubmission(submission as any),
      });
    } catch (e: any) {
      msgStore.setError(`ask.answer failed: ${e.message}`);
    }
  };

  // 取消当前 ask
  const cancelAsk = async () => {
    if (!sid || !pendingAsk) return;
    try {
      await client.request('ask.cancel', { session_id: sid });
    } catch (e: any) {
      msgStore.setError(`ask.cancel failed: ${e.message}`);
    }
  };

  // ask 模式下的输入解析：
  //   - 纯数字 1-4 → toggle 当前 focus question 的第 N 个 option
  //   - 纯空 → 无操作
  //   - 其他文本 → 作为当前 focus question 的 note（追加）
  //   - Enter on empty input → 提交
  //   - Esc → 取消
  const handleAskInputSubmit = (value: string) => {
    if (!pendingAsk) {
      setInput('');
      return;
    }
    const trimmed = value.trim();
    const focus = askStore.draftFocus[sid!] ?? 0;

    if (trimmed === '') {
      // 空提交：直接提交当前 draft
      const selections = askStore.draftSelections[sid!] || {};
      const notes = askStore.draftNotes[sid!] || {};
      const answers: Record<number, any> = {};
      let allFilled = true;
      for (let i = 0; i < pendingAsk.request.questions.length; i++) {
        const q = pendingAsk.request.questions[i];
        const isMulti = !!q.multi_select;
        const sel = selections[i] || [];
        const note = notes[i];
        if (isMulti) {
          if (sel.length === 0 && !note) {
            allFilled = false;
            continue;
          }
          answers[i] = { kind: 'multi', options: sel, ...(note ? { note } : {}) };
        } else {
          if (sel.length === 0 && !note) {
            allFilled = false;
            continue;
          }
          answers[i] = { kind: 'single', option: sel[0] || '', ...(note ? { note } : {}) };
        }
      }
      if (!allFilled) {
        msgStore.setError(t('error.answer_all'));
        return;
      }
      submitAsk({ cancelled: false, answers });
      setInput('');
      return;
    }

    // 数字 1-4
    const asNumber = Number(trimmed);
    if (Number.isInteger(asNumber) && asNumber >= 1 && asNumber <= 4) {
      const q = pendingAsk.request.questions[focus];
      if (q && asNumber <= q.options.length) {
        askStore.toggleSelection(sid!, focus, q.options[asNumber - 1].label);
        setInput('');
        return;
      }
    }

    // 纯数字焦点切换 "Q<num>"（如 "Q2"）
    const qMatch = /^Q(\d+)$/i.exec(trimmed);
    if (qMatch) {
      const idx = parseInt(qMatch[1], 10) - 1;
      if (idx >= 0 && idx < pendingAsk.request.questions.length) {
        askStore.setFocus(sid!, idx);
        setInput('');
        return;
      }
    }

    // 其他：作为当前 focus question 的 note（覆盖式）
    askStore.setNote(sid!, focus, trimmed);
    setInput('');
  };

  const sendMessage = async (content: string) => {
    let sid2 = sessionStore.currentSessionId;
    if (!sid2) {
      try {
        const result = await client.request('sessions.create', { title: t('ui.new_session') });
        sessionStore.setCurrentSession(result.session_id);
        client.setReconnectSession(result.session_id);
        // Phase 5c: 把 reconnect 回调注册成统一的 hydrate 入口
        // （singleton client，整个 App 共享同一份回调）
        registerReconnectHandler(client, hydrateFromSnapshot);
        sid2 = result.session_id;
      } catch (e: any) {
        msgStore.setError(e.message);
        return;
      }
    }

    // 解析 @image:/path/to/file 语法，提取图片文件
    const imagePaths: string[] = [];
    let textContent = content;
    const imageRegex = /@image:(\S+)/g;
    let match;
    while ((match = imageRegex.exec(content)) !== null) {
      imagePaths.push(match[1]);
    }
    textContent = content.replace(imageRegex, '').trim();

    // 读取图片文件为 base64
    const images: { data: string; media_type: string }[] = [];
    const pathToMediaType = new Map<string, string>();
    for (const imgPath of imagePaths) {
      try {
        const fs = await import('fs');
        const path = await import('path');
        const data = fs.readFileSync(imgPath);
        const base64 = data.toString('base64');
        const ext = path.extname(imgPath).toLowerCase();
        const media_type = ext === '.png' ? 'image/png'
          : ext === '.jpg' || ext === '.jpeg' ? 'image/jpeg'
          : ext === '.gif' ? 'image/gif'
          : ext === '.webp' ? 'image/webp'
          : ext === '.bmp' ? 'image/bmp'
          : 'image/png';
        images.push({ data: base64, media_type });
        pathToMediaType.set(imgPath, media_type);
      } catch (e: any) {
        msgStore.setError(`failed to read image ${imgPath}: ${e.message}`);
      }
    }

    // 构建乐观消息内容块
    const userBlocks: any[] = [];
    if (textContent) {
      userBlocks.push({ type: 'text', text: textContent });
    }
    for (const img of imagePaths) {
      userBlocks.push({ type: 'image', path: img, media_type: pathToMediaType.get(img) || 'image/png' });
    }
    msgStore.addMessage({ role: 'user', content: userBlocks.length > 0 ? userBlocks : [{ type: 'text', text: content }] });
    msgStore.setStreaming(true);
    setInput('');
    try {
      if (images.length > 0) {
        await client.request('sessions.send', { session_id: sid2, content: textContent, images });
      } else {
        // 图片读取失败时也要用 textContent（已去除 @image: 语法），避免泄漏到服务端
        await client.request('sessions.send', { session_id: sid2, content: textContent });
      }
    } catch (e: any) {
      msgStore.setError(e.message);
      msgStore.setStreaming(false);
    }
  };

  // Phase 5c: 统一的 hydrate 入口：reconnect 时拿到最新 snapshot
  // 走 hydrateSnapshot（带 currentMessageCount 走增量 append + 去重）
  const hydrateFromSnapshot = (snapshot: unknown) => {
    if (!snapshot) return;
    const cur = useMessagesStore.getState().messages.length;
    const sid = sessionStore.currentSessionId;
    if (!sid) return;
    try {
      hydrateSnapshot({
        sessionId: sid,
        snapshot: snapshot as SessionSnapshot,
        currentMessageCount: cur,
        store: buildHydrateStore(cur),
      });
    } catch (e: any) {
      msgStore.setError(`reconnect hydrate failed: ${e.message}`);
    }
  };

  const handleRemoteConnect = async (raw: string) => {
    let url = '';
    let token = '';

    if (raw.startsWith('mcoder://')) {
      const parsed = parsePairingString(raw);
      if (!parsed) {
        msgStore.setError('Invalid pairing string');
        return;
      }
      url = parsed.url;
      token = parsed.token;
    } else {
      // ws://host:port token
      const parts = raw.split(/\s+/);
      url = parts[0];
      token = parts[1] || '';
    }

    if (!url || !token) {
      msgStore.setError('Usage: /remote <mcoder://token@host:port> or /remote <ws://host:port> <token>');
      return;
    }

    // Close old connection
    client.close();

    // Reset stores
    sessionStore.reset();
    msgStore.clear();

    // Create new client
    const newClient = new WsClient(
      url,
      token,
      () => useSessionStore.getState().setConnected(true),
      () => useSessionStore.getState().setConnected(false),
    );

    setClient(newClient);

    try {
      await newClient.connect();
      const sessions = await newClient.request('sessions.list');
      sessionStore.setSessions(sessions);
      msgStore.addMessage({
        role: 'system',
        content: [{ type: 'text', text: 'Connected to ' + url }],
      });
    } catch (e: any) {
      msgStore.setError(`Failed to connect: ${e.message}`);
    }
  };

  const handleSlashCommand = async (cmd: string) => {
    try {
      const result = await dispatchSlashCommand(cmd, client);
      if (!result) {
        msgStore.setError(`command dispatch returned empty result for: ${cmd}`);
        return;
      }
      if (result.reconnect) {
        await handleRemoteConnect(result.reconnect);
        return;
      }
      if (result.error) msgStore.setError(result.error);
      else msgStore.setError(null);
      if (result.systemMessage) {
        msgStore.addMessage({
          role: 'system',
          content: [{ type: 'text', text: result.systemMessage }],
        });
      }
      if (result.switchView) uiStore.setView(result.switchView);
      if (result.loadSessions) await loadSessions();
      if (result.exit) exit();
    } catch (e: any) {
      msgStore.setError(e.message);
    }
  };

  // 输入变化时检测 / 开头以弹出/隐藏命令选择面板
  const handleInputChange = (value: string) => {
    setInput(value);
    if (value.startsWith('/') && !value.includes(' ')) {
      setShowCommandPicker(true);
    } else {
      setShowCommandPicker(false);
    }
  };

  const onSubmit = (value: string) => {
    // 命令选择面板打开时，Enter 由面板处理（选中命令），不提交
    if (showCommandPicker) return;
    if (value.startsWith('/')) {
      handleSlashCommand(value);
      setInput('');
      return;
    }
    if (askInputMode && pendingAsk) {
      // ask 模式下：不写普通历史（issue 9：note 数字键不应进上下箭头历史）
      handleAskInputSubmit(value);
      return;
    }
    sendMessage(value);
  };

  // 设计文档 §6.2 / §6.7 / §6.8: 全局快捷键
  useInput((inputChar: string, key: any) => {
    if (key.ctrl && inputChar === 'c') {
      exit();
      return;
    }
    // S2 修复: Ctrl+T (Ctrl+Shift+T 也行) 思考深度快捷切换（原 Ctrl+D 与 Emacs EOF 冲突）
    if (key.ctrl && (inputChar === 'T' || inputChar === 't')) {
      uiStore.setView('thinking');
      return;
    }
    // 设计文档 §8.8: 权限审批输入（pending 时 A/D/Y 决议；Esc 默认 Deny）
    const pendingPermission = sid ? permissionStore.pending[sid] : null;
    if (pendingPermission) {
      const submit = (decision: { type: string; reason?: string | null }) => {
        if (!sid) return;
        client.request('permission.submit', {
          session_id: sid,
          response: {
            request_id: pendingPermission.request_id,
            session_id: sid,
            decision,
          },
        }).catch((e: any) => {
          msgStore.setError(`permission.submit failed: ${e.message ?? e}`);
        });
      };
      if (key.escape || inputChar === 'd' || inputChar === 'D') {
        submit({ type: 'Deny', reason: 'denied by user' });
        return;
      }
      if (inputChar === 'a' || inputChar === 'A') {
        submit({ type: 'Allow' });
        return;
      }
      if (inputChar === 'y' || inputChar === 'Y') {
        submit({ type: 'AlwaysAllow' });
        return;
      }
    }
    // ask 模式下：Esc 取消 ask（issue 9）
    if (key.escape && askInputMode && pendingAsk) {
      cancelAsk();
      return;
    }
    if (key.escape) {
      // SettingView and ModelView handle their own Escape (e.g., exiting edit mode)
      if (uiStore.currentView !== 'setting' && uiStore.currentView !== 'model' && uiStore.currentView !== 'provider' && uiStore.currentView !== 'thinking') {
        uiStore.setView('chat');
      }
      return;
    }
    // Phase 3: Ctrl+R → session.resume
    if (key.ctrl && (inputChar === 'r' || inputChar === 'R')) {
      if (!sid) return;
      const entry = computeResumeEntryForApp();
      if (entry.kind === 'none') return;
      handleResume(entry.kind);
      return;
    }
    if (key.ctrl && inputChar === 's') {
      loadSessions();
      uiStore.setView('sessions');
      return;
    }
    if (key.ctrl && inputChar === 't') {
      uiStore.setView('todos');
      return;
    }
    if (key.ctrl && inputChar === 'k') {
      // Phase 5: 按 attached session 隔离（必须传 session_id）
      const sid2 = sid;
      if (!sid2) return;
      client.request('task.list', { session_id: sid2 }).then((tasks) => {
        sessionStore.setTaskCount(tasks.length);
        sessionStore.setBackgroundTasks(tasks);
        uiStore.setView('tasks');
      }).catch(() => {});
      return;
    }
    if (key.ctrl && inputChar === ',') {
      uiStore.setView('setting');
      return;
    }
    if (key.pageUp) {
      uiStore.scrollUp(10);
      return;
    }
    if (key.pageDown) {
      uiStore.scrollDown(10);
      return;
    }
  });

  const currentView = uiStore.currentView;
  const focusIdx = sid ? (askStore.draftFocus[sid] ?? 0) : 0;
  const selMap = sid ? (askStore.draftSelections[sid] || {}) : {};
  const noteMap = sid ? (askStore.draftNotes[sid] || {}) : {};
  const lastSub = sid ? askStore.lastSubmission[sid] : null;

  // Phase 3: Resume helpers (Ctrl+R)
  // Phase 5c: 加上 has_interrupted_tasks —— 与 Rust 5 参数完全一致
  const computeResumeEntryForApp = () => computeResumeEntryPure({
    loop_state: sessionStore.loopState,
    stop_reason: sessionStore.stopReason,
    has_unfinished_todo: ((sessionStore.pendingTodos ?? []) as any[]).some(
      (t) => t.status === 'pending' || t.status === 'in_progress',
    ),
    loop_running: !sessionStore.canResume,
    has_interrupted_tasks: ((sessionStore.backgroundTasks ?? []) as any[]).some(
      (t) => t.status === 'Interrupted' || t.status === 'interrupted',
    ),
  });
  const handleResume = async (kind: string) => {
    if (!sid) return;
    try {
      const result: any = await client.request('session.resume', { session_id: sid });
      if (result && result.started) {
        sessionStore.setLoopState('running', null);
        sessionStore.setCanResume(false);
        msgStore.setStreaming(true);
      } else if (result && result.requires_user_input) {
        // Phase 5c: 服务端给出明确 fallback reason（避免 UI 显示空 stop_reason）
        // 这里把 reason 写入 error 流，UI 提示即可
        if (result.reason) {
          msgStore.setError(result.reason);
        }
      } else if (result && result.waiting_for_user) {
        // 保留 ask 流程；不抢答
      }
    } catch (e: any) {
      msgStore.setError(`session.resume failed: ${e.message}`);
    }
    // unused `kind` retained for future extensibility (different UI per kind)
    void kind;
  };

  const ctxPctNum = sessionStore.contextWindow > 0
    ? (sessionStore.contextUsed / sessionStore.contextWindow * 100)
    : 0;
  const ctxStr = formatContext(sessionStore.contextUsed, sessionStore.contextWindow);
  const costStr = formatCost(sessionStore.sessionCost);

  return (
    <Box flexDirection="column" height="100%">
      {/* 消息区（可滚动）。header card + messages + tool cards 内联渲染 */}
      <MessageList
        askRenderState={
          pendingAsk
            ? { kind: 'pending', ask_id: pendingAsk.ask_id, tool_call_id: pendingAsk.tool_call_id, request: pendingAsk.request, selections: selMap, focusIndex: focusIdx, notes: noteMap }
            : lastSub
              ? { kind: 'submitted', ask_id: lastSub.ask_id, tool_call_id: lastSub.tool_call_id, submission: lastSub.submission }
              : null
        }
        sessionId={sid}
        version={sessionStore.version}
        lspServers={sessionStore.lspServers}
      />

      {/* Plan 审批（独立保留；ask 是另一套） */}
      <PlanApproval client={client} />

      {/* 覆盖层视图 */}
      {currentView === 'sessions' && <SessionList />}
      {currentView === 'todos' && <TodoView />}
      {currentView === 'tasks' && <TaskMonitor />}
      {currentView === 'config' && <ConfigView />}
      {currentView === 'help' && <HelpView client={client} />}
      {currentView === 'tree' && <TreeView client={client} />}
      {currentView === 'model' && <ModelView client={client} />}
      {currentView === 'setting' && <SettingView client={client} />}
      {currentView === 'provider' && <ProviderView client={client} onClose={() => uiStore.setView('chat')} pendingPermission={!!(sid && permissionStore.pending[sid])} />}
      {currentView === 'thinking' && (
        <ThinkingPicker
          client={client}
          sessionId={sid}
          currentDepth={getThinkingFor(sid)}
          onApplied={(depth) => setThinkingFor(sid, depth)}
          onClose={() => {
            uiStore.setView('chat');
            setShowCommandPicker(false);
          }}
          pendingPermission={!!(sid && permissionStore.pending[sid])}
        />
      )}

      {/* Todo 摘要条（消息区下方、输入框上方）；全部完成时隐藏 */}
      <TodoSummaryBar />

      {/* 子代理实时面板（无子代理时隐藏；Ctrl+A 切换焦点） */}
      <SubagentBar
        client={client}
        currentSessionId={sid}
        onSwitchSession={(targetSid) => {
          // 切换 session（复用 /sessions open 的逻辑）
          (async () => {
            try {
              // 清理旧 session UI 状态
              const oldSid = sessionStore.currentSessionId;
              if (oldSid && oldSid !== targetSid) {
                clearSessionUiState({ sessionId: oldSid });
              }
              const snapshot = await client.request('session.attach', { session_id: targetSid }) as SessionSnapshot;
              client.setReconnectSession(targetSid);
              hydrateSnapshot({
                sessionId: targetSid,
                snapshot,
                currentMessageCount: 0,
                store: buildHydrateStore(0),
              });
              // 补全 hydrateSnapshot 不覆盖的状态
              sessionStore.setLoopState(snapshot.session.loop_state, snapshot.session.stop_reason);
              sessionStore.setCanResume(snapshot.can_resume);
              sessionStore.setVersion(snapshot.session.version);
              sessionStore.setLspServers(snapshot.session.lsp_servers);
            } catch (e: any) {
              msgStore.setError(`switch session failed: ${e.message}`);
            }
          })();
        }}
        pendingPermission={!!(sid && permissionStore.pending[sid])}
        isActive={!showCommandPicker}
      />

      {/* Phase 3: Resume 入口（固定状态提示附近；非模态） */}
      <ResumeBar sessionId={sid} />

      {/* 命令选择面板（输入 / 时弹出，显示在输入框上方） */}
      {showCommandPicker && (
        <CommandPicker
          client={client}
          filter={input}
          onSelect={(cmd) => {
            setInput('');
            setShowCommandPicker(false);
            handleSlashCommand(cmd);
          }}
          onClose={() => setShowCommandPicker(false)}
          pendingPermission={!!(sid && permissionStore.pending[sid])}
        />
      )}

      {/* 输入框。ask 模式下显示 ask 提示 */}
      <InputBox
        value={input}
        onChange={handleInputChange}
        onSubmit={onSubmit}
        isActive={!showCommandPicker}
        placeholder={askInputMode && pendingAsk
          ? `ask Q${focusIdx + 1} ${PREFIX.sep} 1-${pendingAsk.request.questions[focusIdx]?.options.length || 0}`
          : undefined}
      />
    </Box>
  );
}

// Phase 5c: 注册 reconnect 回调（整个 App 共享同一份，避免重复挂载）
// 唯一性：每个 client 只能注册一次；重复注册会覆盖
let _registeredClient: WsClient | null = null;
function registerReconnectHandler(client: WsClient, handler: (snap: unknown) => void) {
  if (_registeredClient === client) return;
  _registeredClient = client;
  // 把 getCurrentMessageCount 也注入；doReconnect 用它生成 offset
  const opts: any = {
    sessionId: undefined, // 由 attach 时 setReconnectSession 设置
    onReconnect: (snapshot: unknown) => handler(snapshot),
    getCurrentMessageCount: () => useMessagesStore.getState().messages.length,
  };
  (client as any).reconnectOpts = opts;
}

// Phase 5c: buildHydrateStore — 构造 hydrateSnapshot 需要的 store 注入
// TUI 版本（同时是 Desktop / Mobile 的参考实现）
function buildHydrateStore(currentMessageCount: number) {
  return {
    setCurrentSessionId: (id: string) => useSessionStore.getState().setCurrentSession(id),
    setMessages: (m: Message[]) => useMessagesStore.getState().setMessages(m),
    appendMessages: (m: Message[]) => useMessagesStore.getState().appendMessages(m),
    getMessages: () => useMessagesStore.getState().messages,
    setRole: (r: string) => useSessionStore.getState().setRole(r),
    setModel: (m: string) => useSessionStore.getState().setModel(m),
    setProjectPath: (p: string) => useSessionStore.getState().setProjectPath(p),
    setContextUsage: (used: number, w: number) =>
      useSessionStore.getState().setContextUsage(used, w || useSessionStore.getState().contextWindow || 0),
    setUsage: (usage: any, cost: number) => useSessionStore.getState().setUsage(usage, cost),
    setPendingPlan: (p: any) => useSessionStore.getState().setPendingPlan(p),
    setPendingTodos: (t: any[]) => useSessionStore.getState().setPendingTodos(t),
    setBackgroundTasks: (t: any[]) => useSessionStore.getState().setBackgroundTasks(t),
    setPendingAskFromSnapshot: (ask: any) => {
      const askStore = useAskStore.getState();
      if (ask === null) {
        // 清空当前 session 的 pending（保留 submissions / lastSubmission）
        const cur = useSessionStore.getState().currentSessionId;
        if (cur) askStore.clearSession(cur);
        return;
      }
      askStore.setPendingAskFromSnapshot(ask);
    },
    clearAskSession: (sid: string) => useAskStore.getState().clearSession(sid),
    replaceTodosFromSnapshot: (_todos: any[]) => {
      // setPendingTodos 已替换全部，无需额外 replace
    },
  };
}
