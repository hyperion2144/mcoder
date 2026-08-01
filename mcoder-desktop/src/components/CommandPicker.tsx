// 命令选择面板：输入 / 时在输入框上方弹出，支持鼠标 hover/点击 + 键盘上下键导航
// 通过 command.list RPC 获取命令列表（description 已按当前语言翻译）

import { useState, useEffect, useRef, forwardRef, useImperativeHandle } from 'react';

interface CommandEntry {
  name: string;
  type: string;
  description: string;
  argument_hint?: string;
}

interface Props {
  client: any;
  filter: string;  // 用户输入的内容（如 "/ha"）
  onSelect: (command: string) => void;
  onClose: () => void;
  /** 权限审批 pending 时为 true：当前组件是被动面板，主要由父级决定是否渲染 */
  pendingPermission?: boolean;
}

export interface CommandPickerHandle {
  moveCursor: (dir: number) => void;
  selectCurrent: () => void;
}

export const CommandPicker = forwardRef<CommandPickerHandle, Props>(function CommandPicker(
  { client, filter, onSelect, onClose, pendingPermission },
  ref,
) {
  const [commands, setCommands] = useState<CommandEntry[]>([]);
  const [cursor, setCursor] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const result = await client.request('command.list');
        if (!cancelled) setCommands(result || []);
      } catch {}
    })();
    return () => { cancelled = true; };
  }, [client]);

  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [onClose]);

  const filtered = commands.filter(c =>
    c.name.startsWith(filter.replace(/^\//, ''))
  );

  useEffect(() => { setCursor(0); }, [filter]);

  useImperativeHandle(ref, () => ({
    moveCursor: (dir: number) => {
      setCursor(prev => {
        if (filtered.length === 0) return 0;
        let next = prev + dir;
        if (next < 0) next = filtered.length - 1;
        if (next >= filtered.length) next = 0;
        return next;
      });
    },
    selectCurrent: () => {
      const idx = Math.min(cursor, filtered.length - 1);
      const cmd = filtered[idx];
      if (cmd) onSelect('/' + cmd.name);
    },
  }), [filtered, cursor, onSelect]);

  if (filtered.length === 0) return null;

  return (
    <div ref={containerRef} className="command-picker">
      {filtered.map((c, i) => (
        <div
          key={c.name}
          className={`command-picker-item ${i === cursor ? 'active' : ''}`}
          onMouseEnter={() => setCursor(i)}
          onClick={() => onSelect('/' + c.name)}
        >
          <span className="command-picker-name">/{c.name}</span>
          <span className="command-picker-desc">{c.description}</span>
        </div>
      ))}
    </div>
  );
});
