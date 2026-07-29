// 设计文档 §8.6.1: 桌面客户端入口
// 复用 TUI 的 rpc/store/commands/utils 逻辑层（通过相对路径 import）
// 桌面专属 UI：图谱可视化、diff viewer、文件树

import React from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App.js';
import './styles.css';

const container = document.getElementById('root');
if (!container) throw new Error('root element not found');
const root = createRoot(container);
root.render(React.createElement(App));
