// 设计文档 §8.6.2: 消息列表
// 触摸友好的消息展示，支持文本、工具调用、工具结果
// 工具结果超长可折叠/展开
// ask_user 工具：内联渲染 AskCard（移动端触摸友好）

import React, { useEffect, useRef, useState } from 'react';
import { Capacitor } from '@capacitor/core';
import type { Message } from '@mcoder/shared/rpc/types.js';
import { AskCard } from '@mcoder/shared/ask/index.js';
import { ASK_USER_TOOL } from '@mcoder/shared/ask/types.js';
import { ToolCard } from '@mcoder/shared/toolCard/ToolCardHtml.js';
import { formatUsageDelta } from '@mcoder/shared/utils/format.js';

interface Props {
  messages: Message[];
  streaming: boolean;
  error: string | null;
  pendingCount: number;
  client: any | null;
  currentSessionId: string | null;
  onError?: (m: string) => void;
  /** 按 tool_use id 索引的 result（来自消息流全局配对） */
  resultsById?: Map<string, any>;
  /** TopInfo: mcoder 版本号 */
  version?: string;
  /** TopInfo: 当前模型名 */
  model?: string;
  /** TopInfo: 项目路径 */
  projectPath?: string;
  /** TopInfo: 已启动的 LSP 服务器列表 */
  lspServers?: string[];
}

// 简单 markdown 行内渲染：`code` → <code>，**bold** → <strong>
function renderInline(text: string): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  const regex = /(`[^`]+`|\*\*[^*]+\*\*)/g;
  let last = 0;
  let match: RegExpExecArray | null;
  let key = 0;
  while ((match = regex.exec(text)) !== null) {
    if (match.index > last) {
      nodes.push(<span key={key++}>{text.slice(last, match.index)}</span>);
    }
    const token = match[0];
    if (token.startsWith('`')) {
      nodes.push(<code key={key++} className="md-code">{token.slice(1, -1)}</code>);
    } else {
      nodes.push(<strong key={key++}>{token.slice(2, -2)}</strong>);
    }
    last = match.index + token.length;
  }
  if (last < text.length) {
    nodes.push(<span key={key++}>{text.slice(last)}</span>);
  }
  return nodes;
}

export function MessageList({ messages, streaming, error, pendingCount, client, currentSessionId, onError, resultsById, version, model, projectPath, lspServers }: Props) {
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages.length, streaming, error]);

  const renderBlock = (block: any, i: number) => {
    if (block.type === 'text') {
      return (
        <div key={i} className="msg-text">
          {block.text.split('\n').map((line: string, k: number) => (
            <div key={k}>{renderInline(line)}</div>
          ))}
        </div>
      );
    }
    if (block.type === 'tool_use') {
      if (block.name === ASK_USER_TOOL && client && currentSessionId) {
        return (
          <AskCard
            key={i}
            ask_id={block.id || ''}
            tool_call_id={block.id || ''}
            session_id={currentSessionId}
            client={client}
            onError={onError}
          />
        );
      }
      const result = block.id && resultsById ? resultsById.get(block.id) || null : null;
      return (
        <ToolCard
          key={i}
          block={block}
          resultBlock={result}
        />
      );
    }
    if (block.type === 'tool_result') {
      // tool_result 由 ToolCard 内联显示，不单独渲染
      return null;
    }
    if (block.type === 'image' && block.path) {
      const src = block.path.startsWith('data:') || block.path.startsWith('http')
        ? block.path
        : Capacitor.convertFileSrc(block.path);
      return (
        <div key={i} className="msg-image-wrap">
          <img src={src} className="msg-image" alt={block.path} />
        </div>
      );
    }
    return null;
  };

  return (
    <div className="message-list">
      {/* TopInfo: mcoder 版本 / model / project / lsp，随消息滚动 */}
      <div className="top-info">
        <div className="top-info-title">mcoder v{version || '?'}</div>
        <div className="top-info-meta">
          model: {model || '-'}  project: {projectPath || '-'}
        </div>
        {lspServers && lspServers.length > 0 && (
          <div className="top-info-lsp">lsp: {lspServers.join(', ')}</div>
        )}
      </div>
      {messages.length === 0 && !streaming && !error && (
        <div className="message-empty">
          <div className="message-empty-text">No messages yet</div>
        </div>
      )}
      {messages.map((msg, i) => (
        <div key={i} className={`message message-${msg.role}`}>
          <div className="message-role">{msg.role}</div>
          <div className="message-body">
            {msg.content.map((block: any, j: number) => renderBlock(block, j))}
            {msg.role === 'assistant' && msg.usage && (
              <div className="message-usage">↳ {formatUsageDelta(msg.usage)}</div>
            )}
          </div>
        </div>
      ))}
      {streaming && (
        <div className="message message-assistant">
          <div className="message-role">assistant</div>
          <div className="message-body">
            <div className="streaming-indicator">
              <span className="dot" />
              <span className="dot" />
              <span className="dot" />
            </div>
          </div>
        </div>
      )}
      {pendingCount > 0 && (
        <div className="pending-notice">
          {pendingCount} message(s) queued, will send when online
        </div>
      )}
      {error && <div className="message-error">{error}</div>}
      <div ref={endRef} />
    </div>
  );
}
