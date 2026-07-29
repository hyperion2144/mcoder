import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: {
      '@mcoder/shared': path.resolve(__dirname, '../mcoder-tui/src'),
    },
  },
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: 'esnext',
    outDir: 'dist',
  },
});
