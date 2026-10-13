/// <reference types="vitest/config" />
import { existsSync, readFileSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath, URL } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig, type Plugin, type ResolvedConfig } from 'vite';

// MG operator API is served under /api/admin + /api/auth
// (crates/mg/core/src/admin_http.rs).
// Proxy to the local mock server in dev, or to a real daemon via MG_ADMIN_PROXY_TARGET
// (see dev/mg-stack/README.md).
const adminProxyTarget = process.env.MG_ADMIN_PROXY_TARGET ?? 'http://localhost:8788';

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

// A dashboard tab outlives the daemon that served it: after an upgrade it keeps
// running the previous release's code until reloaded (#154). Production builds
// carry the workspace version so the tab can notice the daemon moved on (see
// features/boot/utils/staleBundle.ts). Nix builds only the operator-ui tree,
// so it passes the version in MG_OPERATOR_UI_VERSION; a checkout build reads
// it from the workspace manifest. Dev and test servers carry none, because
// their mock daemons report fixture versions.
const workspaceVersion = (): string => {
  const manifest = readFileSync(
    fileURLToPath(new URL('../../../Cargo.toml', import.meta.url)),
    'utf8'
  );
  const version = /^\[workspace\.package\][^[]*?^version = "([^"]+)"/m.exec(manifest)?.[1];
  if (!version) {
    throw new Error('Cargo.toml has no [workspace.package] version');
  }
  return version;
};

export default defineConfig(({ command }) => ({
  define: {
    __MG_BUNDLE_VERSION__: JSON.stringify(
      command === 'build' ? (process.env.MG_OPERATOR_UI_VERSION ?? workspaceVersion()) : null
    )
  },
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
}));
