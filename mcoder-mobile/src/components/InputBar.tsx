// 设计文档 §8.6.2: 输入栏
// 触摸友好的输入框，支持斜杠命令和图片附件

import React, { useState, useRef, useEffect } from 'react';
import { t } from '../i18n.js';

export interface PendingImage {
  data: string;
  media_type: string;
  preview: string;
}

interface Props {
  value?: string;
  onValueChange?: (value: string) => void;
  /** 输入变更回调（与 onValueChange 类似，但通常用于父组件做副作用，如展开 / 命令面板） */
  onChange?: (value: string) => void;
  onSubmit: (value: string, images: PendingImage[]) => void;
  onCancel?: () => void;
  streaming: boolean;
  disabled: boolean;
}

export function InputBar({ value: valueProp, onValueChange, onChange, onSubmit, onCancel, streaming, disabled }: Props) {
  const [internalValue, setInternalValue] = useState('');
  const value = valueProp ?? internalValue;
  const [pendingImages, setPendingImages] = useState<PendingImage[]>([]);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // 自适应高度
  useEffect(() => {
    const ta = textareaRef.current;
    if (!ta) return;
    ta.style.height = 'auto';
    ta.style.height = Math.min(ta.scrollHeight, 120) + 'px';
  }, [value]);

  const handleSubmit = () => {
    const trimmed = value.trim();
    if ((!trimmed && pendingImages.length === 0) || disabled || streaming) return;
    onSubmit(trimmed, pendingImages);
    if (onValueChange) onValueChange('');
    else setInternalValue('');
    setPendingImages([]);
  };

  const handleChange = (val: string) => {
    if (onValueChange) onValueChange(val);
    else setInternalValue(val);
    if (onChange) onChange(val);
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

  const handleImageSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files;
    if (!files) return;
    Array.from(files).forEach(file => {
      const reader = new FileReader();
      reader.onload = () => {
        const result = reader.result as string;
        const base64 = result.split(',')[1] || '';
        const media_type = file.type || 'image/png';
        setPendingImages(prev => [...prev, { data: base64, media_type, preview: result }]);
      };
      reader.readAsDataURL(file);
    });
    e.target.value = '';
  };

  const isCommand = value.startsWith('/');

  return (
    <div className="input-bar">
      {pendingImages.length > 0 && (
        <div className="pending-images">
          {pendingImages.map((img, i) => (
            <div key={i} className="pending-image-item">
              <img src={img.preview} alt="" />
              <button
                className="pending-image-remove"
                onClick={() => setPendingImages(prev => prev.filter((_, idx) => idx !== i))}
              >x</button>
            </div>
          ))}
        </div>
      )}
      <div className="input-row">
        <input
          ref={fileInputRef}
          type="file"
          accept="image/*"
          multiple
          style={{ display: 'none' }}
          onChange={handleImageSelect}
        />
        <button
          className="attach-btn"
          onClick={() => fileInputRef.current?.click()}
          disabled={disabled || streaming}
          aria-label={t('ui.attach_image')}
        >+</button>
        <textarea
          ref={textareaRef}
          className={`input-textarea ${isCommand ? 'input-command' : ''}`}
          value={value}
          onChange={(e) => handleChange(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={disabled ? t('ui.offline') : t('ui.send_message')}
          rows={1}
          disabled={disabled}
          autoCapitalize="none"
          autoCorrect="off"
        />
        <button
          className={`send-button ${streaming ? 'cancel-button' : ''}`}
          onClick={streaming ? handleCancel : handleSubmit}
          // streaming 时按钮始终可用（允许取消）；非 streaming 时要求有内容且未离线
          disabled={streaming ? false : ((!value.trim() && pendingImages.length === 0) || disabled)}
          aria-label={streaming ? t('ui.stop') : t('ui.send')}
        >
          {streaming ? t('ui.stop') : t('ui.send')}
        </button>
      </div>
    </div>
  );
}
