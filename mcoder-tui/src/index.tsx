#!/usr/bin/env node
import React from 'react';
import { render } from 'ink';
import { App } from './App.js';
import { WsClient } from './rpc/client.js';
import { useSessionStore } from './store/index.js';
import { parsePairingString } from './utils/pairing.js';

const DEFAULT_URL = 'ws://127.0.0.1:7654';

// 全屏模式：进入 alternate screen buffer
function enterFullscreen() {
  process.stdout.write('\x1b[?1049h'); // 切换到备用屏幕缓冲区
  process.stdout.write('\x1b[2J\x1b[H'); // 清屏 + 光标归位
  process.stdout.write('\x1b[?25l'); // 隐藏光标
}

// 退出全屏模式：恢复主屏幕缓冲区
function exitFullscreen() {
  process.stdout.write('\x1b[?25h'); // 显示光标
  process.stdout.write('\x1b[?1049l'); // 切换回主屏幕缓冲区
}

async function main() {
  const args = process.argv.slice(2);
  let url = DEFAULT_URL;
  let token = '';

  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--url' && args[i + 1]) {
      url = args[i + 1];
      i++;
    } else if (args[i] === '--token' && args[i + 1]) {
      token = args[i + 1];
      i++;
    } else if (args[i] === '--help' || args[i] === '-h') {
      console.log('Usage: mcoder-tui [--url ws://host:port] [--token <token>]');
      console.log('       mcoder-tui mcoder://<token>@<host>:<port>?tls=<auto|on|off>');
      process.exit(0);
    } else if (args[i].startsWith('mcoder://')) {
      // 设计文档 §5.1: 支持 mcoder:// 配对串作为唯一参数
      const parsed = parsePairingString(args[i]);
      if (parsed) {
        url = parsed.url;
        token = parsed.token;
      }
    }
  }

  if (!token) {
    console.error('Error: token required. Use --token <token> or pass mcoder:// pairing string.');
    console.error('Get pairing info from server: `mcoder pair`');
    process.exit(1);
  }

  const sessionStore = useSessionStore;
  const client = new WsClient(
    url,
    token,
    () => sessionStore.getState().setConnected(true),
    () => sessionStore.getState().setConnected(false),
  );

  try {
    await client.connect();
  } catch (e: any) {
    console.error(`Failed to connect to ${url}: ${e.message}`);
    console.error('Make sure mcoder server is running (mcoder server)');
    process.exit(1);
  }

  // Load sessions on connect
  try {
    const sessions = await client.request('sessions.list');
    sessionStore.getState().setSessions(sessions);
  } catch {}

  // 进入全屏模式
  enterFullscreen();

  // 确保退出时恢复终端
  const cleanup = () => {
    exitFullscreen();
    process.exit(0);
  };
  process.on('SIGINT', cleanup);
  process.on('SIGTERM', cleanup);
  process.on('exit', () => exitFullscreen());

  const instance = render(React.createElement(App, { client }));

  // 等待应用退出（Ctrl+C 或 exit()）
  try {
    await instance.waitUntilExit();
  } finally {
    exitFullscreen();
  }
}

main();
