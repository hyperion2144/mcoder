// 设计文档 §8.6.1: 桌面端文件树组件
// 显示项目文件结构，点击文件在右栏预览内容

import React, { useState, useEffect, useCallback } from 'react';
import type { WsClient } from '@mcoder/shared/rpc/client.js';
import { ChevronDown, ChevronRight, ChevronUp } from './icons.js';

interface FileNode {
  name: string;
  path: string;
  is_dir: boolean;
  children?: FileNode[];
}

interface FileTreeProps {
  client: WsClient;
  onFileSelect?: (path: string, content: string) => void;
}

// 文件扩展名 → 语言（用于显示提示）
const LANG_HINT: Record<string, string> = {
  '.rs': 'Rust', '.ts': 'TypeScript', '.tsx': 'TSX', '.js': 'JavaScript',
  '.jsx': 'JSX', '.py': 'Python', '.go': 'Go', '.java': 'Java',
  '.json': 'JSON', '.md': 'Markdown', '.toml': 'TOML', '.yaml': 'YAML',
  '.yml': 'YAML', '.html': 'HTML', '.css': 'CSS', '.sh': 'Shell',
};

function langOf(name: string): string {
  const dot = name.lastIndexOf('.');
  if (dot < 0) return '';
  return LANG_HINT[name.slice(dot).toLowerCase()] || '';
}

export function FileTree({ client, onFileSelect }: FileTreeProps) {
  const [tree, setTree] = useState<FileNode[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    // P2-4: 使用项目自有的 list_files 工具（跨平台），避免 Windows 不存在 find 命令
    client.request('tool.call', {
      name: 'list_files',
      args: { path: '.', limit: 200, exclude: ['node_modules', '.git', 'target', 'dist'] },
    }).then((result: any) => {
      const files: any[] = result.files || result.result || (Array.isArray(result) ? result : []);
      const lines = files.map((f: any) =>
        typeof f === 'string' ? f : (f.path || f.name || '')
      ).filter(Boolean);
      setTree(buildTree(lines));
    }).catch(() => {
      // fallback: 如果 list_files 不存在，尝试用 bash + 跨平台命令
      client.request('tool.call', {
        name: 'bash',
        args: { cmd: 'ls -R . 2>/dev/null | head -200', timeout: 10 },
      }).then((result: any) => {
        const output = result.stdout || result.output || '';
        const lines = output.split('\n').filter(Boolean);
        setTree(buildTree(lines));
      }).catch(() => {});
    });
  }, []);

  const buildTree = (lines: string[]): FileNode[] => {
    const root: FileNode = { name: '', path: '', is_dir: true, children: [] };
    for (const line of lines) {
      const parts = line.split('/').filter(Boolean);
      let current = root;
      for (let i = 0; i < parts.length; i++) {
        const part = parts[i];
        const path = parts.slice(0, i + 1).join('/');
        const isDir = i < parts.length - 1;
        if (!current.children) current.children = [];
        let child = current.children.find(c => c.name === part);
        if (!child) {
          child = { name: part, path, is_dir: isDir, children: isDir ? [] : undefined };
          current.children.push(child);
        }
        current = child;
      }
    }
    return root.children || [];
  };

  const toggleExpand = (path: string) => {
    setExpanded(prev => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  // 点击文件 → 读取内容并回调
  const handleFileClick = useCallback(async (filePath: string) => {
    setSelectedFile(filePath);
    setLoading(true);
    try {
      const result = await client.request('tool.call', {
        name: 'read',
        args: { path: filePath, limit: 500 },
      });
      const content: string = result.content || result.text || result.result?.content || '';
      onFileSelect?.(filePath, content);
    } catch (e: any) {
      onFileSelect?.(filePath, `[Error reading ${filePath}: ${e.message}]`);
    } finally {
      setLoading(false);
    }
  }, [client, onFileSelect]);

  const renderNode = (node: FileNode, depth: number = 0): React.ReactNode => {
    if (node.is_dir) {
      const isExpanded = expanded.has(node.path);
      return (
        <div key={node.path}>
          <div
            className="file-tree-node file-tree-folder"
            style={{ paddingLeft: `${depth * 12 + 8}px` }}
            onClick={() => toggleExpand(node.path)}
          >
            <span className="file-tree-icon">{isExpanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}</span>
            <span className="file-tree-name">{node.name}/</span>
          </div>
          {isExpanded && node.children?.map(child => renderNode(child, depth + 1))}
        </div>
      );
    }
    const isActive = selectedFile === node.path;
    const lang = langOf(node.name);
    return (
      <div
        key={node.path}
        className={`file-tree-node file-tree-file ${isActive ? 'file-tree-file-active' : ''}`}
        style={{ paddingLeft: `${depth * 12 + 24}px` }}
        onClick={() => handleFileClick(node.path)}
        title={node.path}
      >
        <span className="file-tree-name">{node.name}</span>
        {lang && <span className="file-tree-lang">{lang}</span>}
        {isActive && loading && <span className="file-tree-spinner">...</span>}
      </div>
    );
  };

  return (
    <div className="file-tree">
      <div className="file-tree-header">
        <span>Files</span>
        <button
          className="file-tree-refresh"
          onClick={() => {
            setExpanded(new Set());
            setSelectedFile(null);
          }}
          title="Collapse all"
        >
          <ChevronUp size={12} />
        </button>
      </div>
      <div className="file-tree-content">
        {tree.length === 0 && (
          <div className="file-tree-empty">No files</div>
        )}
        {tree.map(node => renderNode(node))}
      </div>
    </div>
  );
}
