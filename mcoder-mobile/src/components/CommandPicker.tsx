// 命令选择面板（Mobile 端底部 sheet）
// 输入 / 时弹出，列出可用命令供选择

import { useState, useEffect } from 'react';
import { t } from '../i18n.js';

interface CommandEntry {
  name: string;
  type: string;
  description: string;
}

interface Props {
  client: any;
  filter: string;
  onSelect: (command: string) => void;
  onClose: () => void;
}

export function CommandPicker({ client, filter, onSelect, onClose }: Props) {
  const [commands, setCommands] = useState<CommandEntry[]>([]);

  useEffect(() => {
    (async () => {
      try {
        const result = await client.request('command.list');
        setCommands(result || []);
      } catch {}
    })();
  }, [client]);

  const filtered = commands.filter(c =>
    c.name.startsWith(filter.replace(/^\//, ''))
  );

  if (filtered.length === 0) return null;

  return (
    <div className="command-picker-overlay" onClick={onClose}>
      <div className="command-picker" onClick={e => e.stopPropagation()}>
        <div className="command-picker-header">
          <span>{t('ui.commands')}</span>
          <button onClick={onClose}>x</button>
        </div>
        <div className="command-picker-list">
          {filtered.map(c => (
            <button
              key={c.name}
              className="command-picker-item"
              onClick={() => onSelect('/' + c.name)}
            >
              <span className="command-picker-name">/{c.name}</span>
              <span className="command-picker-desc">{c.description}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
