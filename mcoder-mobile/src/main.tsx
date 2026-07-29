// 设计文档 §8.6.2: 移动客户端入口
// 复用 TUI 的 rpc/store/commands/utils 逻辑层
// 移动专属优化：弱网友好、触摸交互、简化视图

import React from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App.js';
import './styles.css';

const container = document.getElementById('root');
if (!container) throw new Error('root element not found');
const root = createRoot(container);
root.render(React.createElement(App));
