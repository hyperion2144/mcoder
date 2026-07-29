import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

// 设计文档 §8.6.2: 移动客户端（Capacitor）
// React + Web 技术，打包成 Android/iOS
// 复用 TUI 的 rpc/store/commands/utils 逻辑层
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: {
      '@mcoder/shared': path.resolve(__dirname, '../mcoder-tui/src'),
    },
  },
  server: {
    port: 1430,
    strictPort: true,
  },
  build: {
    target: 'esnext',
    outDir: 'dist',
  },
});
