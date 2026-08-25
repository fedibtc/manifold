/// <reference types="vitest/config" />
import { existsSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath, URL } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig, type Plugin, type ResolvedConfig } from 'vite';

// FMan operator API is served under /api/admin + /api/auth
// (crates/fman/core/src/admin_http.rs).
// Proxy to the local mock server in dev, or to a real daemon via FMAN_ADMIN_PROXY_TARGET
// (see dev/fman-stack/README.md).
const adminProxyTarget = process.env.FMAN_ADMIN_PROXY_TARGET ?? 'http://localhost:8788';

// public/mockServiceWorker.js is only needed in dev (see src/mocks/start.ts). Vite copies
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
    port: 5174,
    strictPort: true,
    proxy: {
      '/api': adminProxyTarget,
      '/__control': adminProxyTarget
    }
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./src/test-setup.ts'],
    coverage: {
      provider: 'v8',
      reporter: ['text-summary', 'json-summary', 'lcov'],
      include: ['src/**']
    }
  }
});
