import { defineConfig, devices } from '@playwright/test';

// E2E_APP=flip (default) | fman — selects which operator app to drive.
// E2E_TARGET=mock (default) | daemon — both apps support both targets. In daemon
// mode a real backend is provisioned outside Playwright by the matching Rust
// runner (flip-ui-e2e-runner / fman-ui-e2e-runner) via defe.
const app = process.env.E2E_APP ?? 'flip';
const target = process.env.E2E_TARGET ?? 'mock';
const isMock = target === 'mock';

// Chromium is the only project that runs by default (`playwright test`, no
// `--project` flag) — that's the everyday mocked run and the daemon runs.
// Firefox and WebKit close the NFR-04 browser-matrix gap but only run when
// explicitly requested, e.g. `--project=firefox --project=webkit` from a
// pre-release job. Playwright's config file sees the raw CLI args, so this
// checks for `--project` itself rather than adding a separate env var: when
// it's present, Firefox/WebKit are declared and Playwright's own project
// filter narrows execution to the requested name(s); when it's absent, only
// the always-declared chromium project exists to run.
const hasProjectFilter = process.argv.some(
  (arg) => arg === '--project' || arg.startsWith('--project=')
);
const crossBrowserProjects = hasProjectFilter
  ? [
      { name: 'firefox', use: { ...devices['Desktop Firefox'] } },
      { name: 'webkit', use: { ...devices['Desktop Safari'] } }
    ]
  : [];

// The FLIP mock world lives in the browser (MSW + localStorage), so mock-target
// tests run serially against that single in-memory state — no Express server to
// boot. The Vite dev server is the only thing to wait on.
const mockWebServer = [
  {
    command: 'pnpm --filter flip dev',
    url: 'http://localhost:5173',
    reuseExistingServer: !process.env.CI,
    timeout: 60_000
  }
];

// In daemon mode the real backend is provisioned outside Playwright (by the
// flip-ui-e2e-runner via defe), so we only boot Vite. It inherits
// FLIP_ADMIN_PROXY_TARGET from the runner's environment and proxies /admin at it.
// VITE_MOCKS=off is the kill switch: without it MSW would intercept /admin/*
// itself and mock the very daemon this mode just stood up.
// reuseExistingServer is forced off here: Playwright's reuse check only probes
// the URL, not the env, so a still-running mocked dev server on :5173 would
// otherwise get reused and this "daemon" run would silently test against MSW.
const daemonWebServer = [
  {
    command: 'pnpm --filter flip dev',
    env: { VITE_MOCKS: 'off' },
    url: 'http://localhost:5173',
    reuseExistingServer: false,
    timeout: 60_000
  }
];

const flipConfig = defineConfig({
  testDir: './e2e',
  // fman specs live under e2e/fman and boot a different app/port — exclude them
  // from the flip run.
  testIgnore: '**/fman/**',
  // Both target modes run serially: mock mode has one in-memory MSW world to
  // share, and daemon mode has one defe-leased FLIP daemon shared by every
  // @live spec in the run. live-writes.spec.ts drives the setup wizard to
  // completion and mutates the daemon; live-daemon.spec.ts asserts the
  // daemon is still freshly unconfigured. Running those in parallel workers
  // races them nondeterministically, so daemon mode is pinned to the same
  // fullyParallel:false/workers:1 as mock mode rather than getting its own
  // parallel defaults. With workers:1 Playwright runs spec files in a stable
  // (alphabetical) order, so live-daemon.spec.ts always runs before
  // live-writes.spec.ts for both apps — confirmed with `--list`.
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: 1,
  reporter: 'html',
  use: {
    baseURL: 'http://localhost:5173',
    trace: 'on-first-retry'
  },
  metadata: { target },
  // @live specs need a real daemon: run them only in daemon mode, and never in
  // mock mode (a mock can't honestly exercise a write round-trip).
  grep: isMock ? undefined : /@live/,
  grepInvert: isMock ? /@live/ : undefined,
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }, ...crossBrowserProjects],
  webServer: isMock ? mockWebServer : daemonWebServer
});

// The fman mock world lives in the browser (MSW + localStorage), so mock-target
// tests run serially against that single in-memory state — no Express server to
// boot. The Vite dev server is the only thing to wait on.
const fmanMockWebServer = [
  {
    command: 'pnpm --filter fman dev',
    url: 'http://localhost:5174',
    reuseExistingServer: !process.env.CI,
    timeout: 60_000
  }
];

// In daemon mode a real Fleet Manager is provisioned outside Playwright (by the
// fman-ui-e2e-runner via defe), so we only boot Vite. It inherits
// FMAN_ADMIN_PROXY_TARGET from the runner's environment and proxies /api at it.
// VITE_MOCKS=off is the kill switch: without it MSW would intercept /api/*
// itself and mock the very daemon this mode just stood up.
// reuseExistingServer is forced off here: Playwright's reuse check only probes
// the URL, not the env, so a still-running mocked dev server on :5174 would
// otherwise get reused and this "daemon" run would silently test against MSW.
const fmanDaemonWebServer = [
  {
    command: 'pnpm --filter fman dev',
    env: { VITE_MOCKS: 'off' },
    url: 'http://localhost:5174',
    reuseExistingServer: false,
    timeout: 60_000
  }
];

const fmanConfig = defineConfig({
  testDir: './e2e/fman',
  // Serial in both target modes, same reasoning as flipConfig above: in
  // daemon mode this is the only thing stopping live-writes.spec.ts (which
  // mutates the leased FMan's offer price) from racing live-daemon.spec.ts
  // (which asserts a freshly unconfigured/empty fleet). workers:1 also
  // pins file execution to alphabetical order, so live-daemon.spec.ts runs
  // before live-writes.spec.ts — confirmed with `--list`.
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: 1,
  reporter: 'html',
  use: {
    baseURL: 'http://localhost:5174',
    trace: 'on-first-retry'
  },
  metadata: { app: 'fman', target },
  // @live specs need a real daemon: run them only in daemon mode, and never in
  // mock mode (a mock can't honestly exercise a write round-trip).
  grep: isMock ? undefined : /@live/,
  grepInvert: isMock ? /@live/ : undefined,
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }, ...crossBrowserProjects],
  webServer: isMock ? fmanMockWebServer : fmanDaemonWebServer
});

export default app === 'fman' ? fmanConfig : flipConfig;
