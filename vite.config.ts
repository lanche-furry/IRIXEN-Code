import { defineConfig } from 'vite';

// IRIXEN Code — frontend build config.
// The webview payload is deliberately tiny: no framework, just TypeScript + CSS.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: 'es2022',
    outDir: 'dist',
    assetsDir: 'assets',
    sourcemap: false,
    chunkSizeWarningLimit: 2048,
  },
});
