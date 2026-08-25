/// <reference types="vitest/config" />
import { existsSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath, URL } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig, type Plugin, type ResolvedConfig } from 'vite';

// FLIP Admin API is served under /admin/v1/*; the unauthenticated liveness probe
// is GET /health. Proxy both to the local mock server in dev, or to a real
// daemon via FLIP_ADMIN_PROXY_TARGET (see dev/flip-stack/README.md).
const adminProxyTarget = process.env.FLIP_ADMIN_PROXY_TARGET ?? 'http://localhost:8787';

// public/mockServiceWorker.js is only needed in dev (see src/app/App.tsx). Vite copies
// everything under public/ into dist/ verbatim (outside the rollup bundle, so generateBundle
// can't see it) — delete the copied file once the build finishes rather than removing the
// vendored source, which MSW's own CLI regenerates.
const excludeMockServiceWorkerFromBuild = (): Plugin => {
  let config: ResolvedConfig;
  return {
    name: 'exclude-mock-service-worker-from-build',
    apply: 'build',
    configResolved(resolvedConfig) {
      config = resolvedConfig;
    },
    closeBundle() {
      const emitted = resolve(config.root, config.build.outDir, 'mockServiceWorker.js');
      if (existsSync(emitted)) {
        rmSync(emitted);
      }
    }
  };
};

export default defineConfig({
  plugins: [
    react({ babel: { plugins: ['babel-plugin-react-compiler'] } }),
    excludeMockServiceWorkerFromBuild()
  ],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url))
    }
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      '/admin': adminProxyTarget,
      '/health': adminProxyTarget,
      '/__control': adminProxyTarget
    }
  },
  test: {
    globals: true,
    environment: 'jsdom',
    coverage: {
      provider: 'v8',
      reporter: ['text-summary', 'json-summary', 'lcov'],
      include: ['src/**']
    }
  }
});
