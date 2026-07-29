import type { CapacitorConfig } from '@capacitor/cli';

// 设计文档 §8.6.2: Capacitor 配置
// 打包成 Android/iOS，web 前端复用 TUI 逻辑层
const config: CapacitorConfig = {
  appId: 'com.mcoder.mobile',
  appName: 'mcoder',
  webDir: 'dist',
  server: {
    // 允许混合内容（ws:// 在 https 上下文）
    cleartext: true,
  },
  android: {
    // 允许明文 HTTP（开发期连本地 server）
    allowMixedContent: true,
  },
  ios: {
    // 允许明文 HTTP（开发期连本地 server）
    contentInset: 'always',
  },
  plugins: {
    Keyboard: {
      resize: 'body',
      style: 'DARK',
      resizeOnFullScreen: true,
    },
    Network: {
      // 弱网检测
    },
  },
};

export default config;
