// 设计文档 §8.6.2: 输入栏
// 触摸友好的输入框，支持斜杠命令

import React, { useState, useRef, useEffect } from 'react';

interface Props {
  onSubmit: (value: string) => void;
  onCancel?: () => void;
  streaming: boolean;
  disabled: boolean;
}

export function InputBar({ onSubmit, onCancel, streaming, disabled }: Props) {
  const [value, setValue] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // 自适应高度
  useEffect(() => {
    const ta = textareaRef.current;
    if (!ta) return;
    ta.style.height = 'auto';
    ta.style.height = Math.min(ta.scrollHeight, 120) + 'px';
  }, [value]);

  const handleSubmit = () => {
    const trimmed = value.trim();
    if (!trimmed || disabled || streaming) return;
    onSubmit(trimmed);
    setValue('');
  };

  // P1-4: 流式响应时按钮变为取消，可点击
  const handleCancel = () => {
    if (onCancel) onCancel();
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    // 移动端 Enter 直接发送（Shift+Enter 换行在桌面端，移动端用回车键）
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    }
  };

  const isCommand = value.startsWith('/');

  return (
    <div className="input-bar">
      <textarea
        ref={textareaRef}
        className={`input-textarea ${isCommand ? 'input-command' : ''}`}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={disabled ? 'offline...' : 'message or /help'}
        rows={1}
        disabled={disabled}
        autoCapitalize="none"
        autoCorrect="off"
      />
      <button
        className={`send-button ${streaming ? 'cancel-button' : ''}`}
        onClick={streaming ? handleCancel : handleSubmit}
        disabled={!streaming && (!value.trim() || disabled)}
        aria-label={streaming ? 'cancel' : 'send'}
      >
        {streaming ? 'Stop' : 'Send'}
      </button>
    </div>
  );
}
