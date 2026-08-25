# operator-ui

JS/TS workspace for the two operator dashboards (FLIP + FMan).

Spec: [`docs/operator-dashboards/README.md`](../docs/operator-dashboards/README.md).

## Layout

```text
packages/
  common-ui/       # shared components, tokens (no app routing)
  types/           # TS types mirroring service-* crates
  mock-fixtures/   # JSON fixtures + scenario generators
apps/
  flip/            # Vite + React SPA (+ mock-server/)
  fman/            # Vite + React SPA (+ mock-server/)
```

## Prerequisites

- Node >= 20
- pnpm >= 9 (`npm i -g pnpm` or `corepack enable pnpm`)

## Install

```bash
pnpm install
```

## Local development

```bash
pnpm flip:be       # :8787  FLIP Admin API mock (backend)
pnpm fman:be       # :8788  FMan operator API mock (backend)
pnpm flip:fe       # :5173  → proxy /admin (frontend)
pnpm fman:fe       # :5174  → proxy /fman (frontend)
```

## Test

```bash
pnpm test          # Vitest (unit)
pnpm test:e2e      # Playwright (E2E_TARGET=mock|daemon)
```
