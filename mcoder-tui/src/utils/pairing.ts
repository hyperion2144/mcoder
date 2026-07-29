// 设计文档 §6.12: utils/pairing.ts - 配对串解析 + QR 码生成
// 平台无关（Tauri/Capacitor 可复用）

/// 解析 mcoder://<token>@<host>:<port>?tls=<auto|on|off> 配对串
export function parsePairingString(s: string): { url: string; token: string; tls: 'auto' | 'on' | 'off' } | null {
  const m = s.match(/^mcoder:\/\/([^@]+)@([^:]+):(\d+)(\?[^]*)?$/);
  if (!m) return null;
  const token = m[1];
  const host = m[2];
  const port = m[3];
  const query = m[4] || '';
  const tlsMode = query.includes('tls=on') ? 'on'
    : query.includes('tls=off') ? 'off'
    : 'auto';
  const useTls = tlsMode === 'on' || (tlsMode === 'auto' && !isLocalhost(host));
  const proto = useTls ? 'wss' : 'ws';
  return { url: `${proto}://${host}:${port}`, token, tls: tlsMode };
}

export function isLocalhost(host: string): boolean {
  return ['127.0.0.1', 'localhost', '0.0.0.0', '::', '::1'].includes(host);
}

/// 生成配对串
export function buildPairingString(token: string, host: string, port: number, tls: 'auto' | 'on' | 'off' = 'auto'): string {
  return `mcoder://${token}@${host}:${port}?tls=${tls}`;
}

/// 生成终端 QR 码（用 qrcode-terminal 库）
// 平台无关：Node 环境用 qrcode-terminal，浏览器/Tauri 环境无此库则返回空
export function generateQrCode(text: string): string {
  try {
    const g: any = (typeof globalThis !== 'undefined' ? globalThis : undefined) as any;
    if (typeof g?.require === 'undefined') return '';
    const qrcode = g.require('qrcode-terminal');
    let result = '';
    qrcode.generate(text, { small: true }, (qrcode: string) => {
      result = qrcode;
    });
    return result;
  } catch {
    return '';
  }
}
