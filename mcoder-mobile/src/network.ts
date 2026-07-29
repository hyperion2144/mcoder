// 设计文档 §8.6.2: 弱网监控
// Capacitor 环境用 @capacitor/network，浏览器环境用 navigator.onLine + online/offline 事件

type StatusCallback = (status: 'online' | 'offline') => void;

export class NetworkMonitor {
  private callback: StatusCallback | null = null;
  private capListener: any = null;

  constructor() {
    this.init();
  }

  private async init() {
    // 尝试用 Capacitor Network plugin
    try {
      const cap = (await import('@capacitor/network')).Network;
      const status = await cap.getStatus();
      this.callback?.(status.connected ? 'online' : 'offline');
      this.capListener = await cap.addListener('networkStatusChange', (s: any) => {
        this.callback?.(s.connected ? 'online' : 'offline');
      });
      return;
    } catch {
      // 非 Capacitor 环境，fallback 到浏览器 API
    }

    // 浏览器 fallback
    this.callback?.(navigator.onLine ? 'online' : 'offline');
    window.addEventListener('online', this.handleOnline);
    window.addEventListener('offline', this.handleOffline);
  }

  private handleOnline = () => this.callback?.('online');
  private handleOffline = () => this.callback?.('offline');

  onStatusChange(cb: StatusCallback) {
    this.callback = cb;
  }

  destroy() {
    this.callback = null;
    if (this.capListener) {
      this.capListener.remove?.();
    }
    window.removeEventListener('online', this.handleOnline);
    window.removeEventListener('offline', this.handleOffline);
  }
}
