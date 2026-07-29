// 设计文档 §8.6.2: 配对界面
// 首次启动时输入配对串或扫描二维码连接主机 server
// P2-2: 支持二维码扫描

import React, { useState, useRef } from 'react';
import { parsePairingString } from '@mcoder/shared/utils/pairing.js';

interface Props {
  onConnect: (pairingStr: string) => void;
}

export function PairingScreen({ onConnect }: Props) {
  const [input, setInput] = useState('');
  const [error, setError] = useState('');
  const [scanning, setScanning] = useState(false);
  const videoRef = useRef<HTMLVideoElement>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const rafRef = useRef<number | null>(null);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = input.trim();
    if (!trimmed) {
      setError('Please enter pairing string');
      return;
    }
    const parsed = parsePairingString(trimmed);
    if (!parsed) {
      setError('Invalid format. Expected: mcoder://<token>@<host>:<port>?tls=<auto|on|off>');
      return;
    }
    setError('');
    onConnect(trimmed);
  };

  // P2-2: 二维码扫描（使用浏览器原生 BarcodeDetector API）
  const startScan = async () => {
    setError('');
    setScanning(true);

    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        video: { facingMode: 'environment' },
      });
      streamRef.current = stream;

      if (videoRef.current) {
        videoRef.current.srcObject = stream;
        await videoRef.current.play();
      }

      // 使用原生 BarcodeDetector（Chrome/Android 支持），否则 fallback 到手动输入
      if ('BarcodeDetector' in window) {
        const AnyBarcodeDetector = (window as any).BarcodeDetector;
        const detector = new AnyBarcodeDetector({
          formats: ['qr_code'],
        });

        const detect = async () => {
          if (!videoRef.current || !scanning) return;
          try {
            const barcodes = await detector.detect(videoRef.current);
            if (barcodes && barcodes.length > 0) {
              const value = barcodes[0].rawValue;
              stopScan();
              const parsed = parsePairingString(value);
              if (parsed) {
                setInput(value);
                onConnect(value);
              } else {
                setError('Scanned code is not a valid mcoder pairing string');
              }
              return;
            }
          } catch {}
          rafRef.current = requestAnimationFrame(detect);
        };
        detect();
      } else {
        setError('Barcode detection not supported on this device. Please enter pairing string manually.');
        stopScan();
      }
    } catch (e: any) {
      setError(`Camera access failed: ${e.message}`);
      setScanning(false);
    }
  };

  const stopScan = () => {
    setScanning(false);
    if (streamRef.current) {
      streamRef.current.getTracks().forEach((t) => t.stop());
      streamRef.current = null;
    }
    if (rafRef.current) {
      cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
  };

  return (
    <div className="pairing-screen">
      <div className="pairing-logo">mcoder</div>
      <div className="pairing-title">Connect to Server</div>
      <div className="pairing-hint">
        Run <code>mcoder pair</code> on your computer to get the pairing string.
      </div>
      <form onSubmit={handleSubmit} className="pairing-form">
        <input
          type="text"
          className="pairing-input"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="mcoder://token@host:port?tls=auto"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
        />
        <button type="submit" className="pairing-button">Connect</button>
      </form>

      {!scanning ? (
        <button className="scan-button" onClick={startScan}>
          Scan QR Code
        </button>
      ) : (
        <>
          <div className="scan-container">
            <video ref={videoRef} className="scan-video" playsInline muted />
            <div className="scan-overlay" />
          </div>
          <button className="scan-button cancel" onClick={stopScan}>
            Cancel Scan
          </button>
        </>
      )}

      {error && <div className="pairing-error">{error}</div>}
    </div>
  );
}
