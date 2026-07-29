// 设计文档 §8.6.2: 消息列表
// 触摸友好的消息展示，支持文本、工具调用、工具结果
// 工具结果超长可折叠/展开

import React, { useEffect, useRef, useState } from 'react';
import type { Message } from '@mcoder/shared/rpc/types.js';

interface Props {
  messages: Message[];
  streaming: boolean;
  error: string | null;
  pendingCount: number;
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

// 工具结果折叠组件
function ToolResult({ output }: { output: any }) {
  const [expanded, setExpanded] = useState(false);
  const outputStr = typeof output === 'string' ? output : JSON.stringify(output, null, 2);
  const isLong = outputStr.length > 200;
  const preview = isLong ? outputStr.slice(0, 200) : outputStr;
  return (
    <div className="msg-tool-result">
      <pre className="tool-output">
        {expanded ? outputStr : preview}
        {isLong && !expanded && <span className="tool-output-ellipsis">…</span>}
      </pre>
      {isLong && (
        <button className="tool-output-toggle" onClick={() => setExpanded(!expanded)}>
          {expanded ? '收起' : `展开 (${outputStr.length})`}
        </button>
      )}
    </div>
  );
}

export function MessageList({ messages, streaming, error, pendingCount }: Props) {
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
      return (
        <div key={i} className="msg-tool-use">
          <span className="tool-name">{block.name}</span>
        </div>
      );
    }
    if (block.type === 'tool_result') {
      return <ToolResult key={i} output={block.output} />;
    }
    return null;
  };

  return (
    <div className="message-list">
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
