# Operator Dashboards Remediation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Basis:** the verified assessments —
[AUDIT-ASSESSMENT-2026-08-08.md](./AUDIT-ASSESSMENT-2026-08-08.md) (index),
[AUDIT-ASSESSMENT-FMAN-2026-08-08.md](./AUDIT-ASSESSMENT-FMAN-2026-08-08.md),
[AUDIT-ASSESSMENT-FLIP-2026-08-08.md](./AUDIT-ASSESSMENT-FLIP-2026-08-08.md).
Plan snapshot: `9b8e12b6`; re-reviewed at `362f04ad` and `8082ab74` (the
latter shrank the biome debt to 5 errors and refactored
`check-css-dupes.mjs` — A2 is written as dynamic for exactly this reason;
the A5 coverage baseline must be measured at execution head).
All actions in workflow snippets are SHA-pinned to match the existing
`selfci.yml` style — verify pins against the marketplace before first use.

**Goal:** clear the verified P1/P2 findings so both operator dashboards can pass
a real-daemon release gate.

**Architecture:** four tracks. Track A (repo) wires the existing, already-written
quality gates into CI and creates Rust-produced contract fixtures — it unblocks
and protects everything else. Tracks B (FLIP) and C (FMan) are per-team frontend
fixes, ordered so each lands independently. Track D holds work gated on a
backend decision; each entry names the exact question and owner, so answers can
be chased in parallel instead of discovered mid-implementation.

**Tech stack:** React 19 + React Query + Vite + Vitest + Playwright
(`operator-ui/`), Rust axum daemons (`crates/`), Nix/selfci CI.

## Relationship to `tasks/fman-dashboard.md` (FMan plan of record)

That document stays authoritative for **product decisions and scope** (PM/maan
answers, the payout-sweep question, done-means); this plan is the **execution
plan** for remediation. Assessed 2026-08-08 for conflicts; resolutions, decided
on current code with the backend as source of truth:

1. **FMan live-data freshness** — its requirements section is *adopted verbatim*
   as Task C7 below. Verified still true at head: only
   `useOnboarding`/`useAuthorizationWatch` poll; `SEATS_KEY`,
   `seatStatusKey`, `PAYMENT_FEDERATIONS_KEY`, `guardianFeesKey` have no
   `refetchInterval`; `queryClient` is a bare `new QueryClient()`. Its
   intervals are cost-derived (per-seat fan-out) and stand as written.
2. **Overview (C3) vs the PM sign-off** — no real conflict: the PM signed off
   the *earnings presentation* (one gross figure + footnote). C3 is
   constrained to add loading/error branches only; the signed-off copy,
   figure, and caveat structure must not change.
3. **Design pass ordering** — the plan of record runs the Fedi Design System
   pass (its Phase 3) deliberately last. C4 (a11y) is *semantics*, not visual
   design, and proceeds before it; C6's token cleanup overlaps Phase 3 and
   should be folded into or sequenced with it, not raced ahead of it.
4. **Setup-state verb** — its §2 note "the setup gate needs no new verb today"
   is true only against the mock. On a real daemon the wizard's trigger
   string-matches a socket-only error, so the verb (its daemon ask #2, maan's
   preference, and D1 here) is load-bearing, not a nicety. D1 stands.
5. **Daemon asks** — its asks #1–#2 are D1 here (cross-referenced; keep
   `MVP-SPEC.md` in step per its own rule). Asks #3 (available-slots verb) and
   #4 (the three payout pieces) are *feature* asks, not remediation — they stay
   owned by the plan of record and are not duplicated into Track D. Per its
   open decision #1 default: MVP ships the ecash-token path; the sweep is
   post-MVP unless the daemon side lands in time. This plan's release criteria
   deliberately exclude the sweep.
6. **FLIP setup-gate placement** — its note that FLIP gates *inside* its shell
   (nav visible during setup, unlike FMan) is adopted as Task B9.
7. **Stale items in the plan of record** (`tasks/` is protected — operator UI developers update
   it, not agents): `90773ab7` **has** merged to master via `a0480018`, so §7's
   "A PR" bullet is satisfied; §6's harness invocation remains valid for
   feature slices but remediation tasks here are small enough to land without
   the harness.

## Global constraints

- All frontend work follows `operator-ui/CLAUDE.md`: lambda components with a
  declared `XProps` interface, one React unit per file, kebab-case component
  folders with `__tests__/`, absolute `@/` imports, no Tailwind strings in TSX,
  tokens only (no hardcoded values), Vitest `it("should …")`.
- TDD: each behavior change starts with a failing test committed together with
  the fix.
- BE is the source of truth: TS types and mocks must match `admin.rs` /
  `admin_http.rs`, never the other way around.
- Do not modify protected dirs (see `harness.config.json`).
- Commands below run from `operator-ui/` unless the path says otherwise.

---

## Track A — repo-level (owner: whoever lands first; blocks nothing, protects everything)

### Task A1: Wire the existing frontend checks into CI

**Policy gate: ✅ satisfied.** `.github` is in `operator-ui/harness.config.json`
`protectedDirs` (mode: block); operator UI developers approved creating
`.github/workflows/operator-ui.yml` on 2026-08-08 (option (a)). Restate this approval in the PR description so the reviewer sees
the protected-dir change was authorized. The selfci alternative (b) is
retired.

**Files:**
- Create: `.github/workflows/operator-ui.yml` (repo root; approval required —
  see above)

**Interfaces:**
- Consumes: existing `operator-ui/package.json` scripts — `typecheck`, `lint`,
  `lint:compiler`, `lint:boundaries`, `lint:css-dupes`, `test`, `build`,
  `test:e2e:ci`. This is a **pnpm workspace** (`pnpm-lock.yaml`; scripts
  invoke `pnpm -r`) — never `npm ci`.
- Produces: a PR check named `operator-ui` that **always reports** (required
  checks must not depend on a workflow-level path filter, or PRs that don't
  touch the paths wait forever on a check that never starts).

- [ ] **Step 1: Confirm the scripts run locally** (lint is known-failing until
  A2/A3 land — fine; the workflow must exist first so the fixes are provable):

```bash
cd operator-ui
corepack enable
pnpm install --frozen-lockfile
pnpm run typecheck
pnpm run lint
pnpm run lint:compiler
pnpm run lint:boundaries
pnpm run lint:css-dupes
pnpm run test
pnpm run build
```

(Run each separately so a failure is attributable; `lint` failing here is the
expected pre-A2 state, the rest should pass.)

- [ ] **Step 2: Create the workflow** — no `paths:` filter on the trigger;
  a change-detection job gates the heavy job and reports success when
  `operator-ui/**` (and the contract-relevant Rust) is untouched:

```yaml
name: operator-ui
on:
  pull_request:
  push:
    branches: [master]
jobs:
  changes:
    runs-on: ubuntu-latest
    outputs: { ui: ${{ steps.filter.outputs.ui }} }
    steps:
      - uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1
      - uses: dorny/paths-filter@de90cc6fb38fc0963ad72b210f1f284cd68cea36 # v3.0.2
        id: filter
        with:
          filters: |
            ui:
              - 'operator-ui/**'
              - 'crates/service-liquidity-manager/**'
              - 'crates/liquidity-manager-daemon/**'
              - 'crates/fman/**'
              - 'crates/operator-ui-auth/**'
              - 'crates/domain/**'
              - 'packages/fleet-manager/**'
              - 'flake.nix'
  checks:
    needs: changes
    if: needs.changes.outputs.ui == 'true'
    runs-on: ubuntu-latest
    defaults: { run: { working-directory: operator-ui } }
    steps:
      - uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1
      # pnpm must exist BEFORE setup-node resolves `cache: pnpm` — a plain
      # `corepack enable` step is not reliable here because setup-node
      # switches the Node installation afterwards and may lose the shim.
      - uses: pnpm/action-setup@fe02b34f77f8bc703788d5817da081398fad5dd2 # v4.0.0
        with: { version: 11.9.0 } # match the workspace's local pnpm
      - uses: actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444 # v4.1.0
        with: { node-version: 22, cache: pnpm, cache-dependency-path: operator-ui/pnpm-lock.yaml }
      - run: pnpm install --frozen-lockfile
      - run: pnpm run typecheck
      - run: pnpm run lint
      - run: pnpm run lint:compiler
      - run: pnpm run lint:boundaries
      - run: pnpm run lint:css-dupes
      - run: pnpm run test
      - run: pnpm run build
      - run: pnpm run test:e2e:ci
```

  (With `changes` always running, mark **`changes`** plus `checks` as the
  required contexts; a skipped `checks` behind a false filter output
  satisfies branch protection as "skipped-but-reported" — verify this on the
  draft PR in Step 3, and if the org's branch-protection treats skipped as
  missing, fold the filter into a single always-running job that exits early
  instead.)

- [ ] **Step 3: Verify on a draft PR** — one commit touching only the
  workflow file (must trigger, because there is no workflow-level path
  filter), one touching `operator-ui/`, one touching neither; confirm all
  three report a check.
- [ ] **Step 4: Commit** — `ci: run operator-ui checks on pull requests`
- [ ] **Step 5: After A2 + A3 merge, mark the check(s) required** in repo
  settings (or ask an admin) so the gate is enforced, not advisory.

### Task A2: Clear the biome debt

The debt is a moving target on an ungated branch (15 errors at `362f04ad`,
5 errors / 1 warning / 8 infos at `8082ab74`) — so this task is **dynamic**:
the authoritative list is whatever `pnpm exec biome ci .` reports at
execution head, not any count written here.

**Files:**
- Modify: whatever `pnpm exec biome ci .` reports at execution head. At
  `8082ab74` that is: `useWithdrawForm.test.tsx` (noUselessFragments),
  `scripts/apply-css-utilities.mjs` and `scripts/check-css-dupes.mjs`
  (useNodejsImportProtocol, useTemplate, noUnusedVariables) — all FIXABLE —
  plus one **manual** fix: `noAssignInExpressions` at
  `scripts/check-css-dupes.mjs:71` (the regex-exec loop; not auto-fixable).

- [ ] **Step 1:** `pnpm exec biome ci .` — capture the fresh list.
- [ ] **Step 2:** `pnpm exec biome check --write .`; re-run
  `pnpm exec biome ci .` and hand-fix what remains, preserving behavior (no
  logic changes in a lint commit). The known manual case is the exec loop at
  `check-css-dupes.mjs:71` — refactor the assignment out of the condition:

```js
// before: while ((m = blockRe.exec(css)) !== null) { ... }
while (true) {
  const m = blockRe.exec(css);
  if (m === null) break;
  // ...
}
```
- [ ] **Step 3:** `pnpm exec biome ci .` → expect `Found 0 errors.`
- [ ] **Step 4:** `pnpm run test` and `pnpm run lint:css-dupes` (the write
  pass touches the css-dupes script itself — prove it still runs and still
  reports 8 groups).
- [ ] **Step 5: Commit** — `style: clear biome lint and format debt`

### Task A3: Fix the boundary violation

**Files:**
- Modify: `apps/liquidity-provider/src/features/settings/hooks/use-provider-config-form/useProviderConfigForm.ts:4`
- Create: `apps/liquidity-provider/src/shared/api/queryKeys.ts` (if no shared
  key module exists yet)
- Modify: `apps/liquidity-provider/src/features/advertisement/` — the file
  currently exporting `ADVERTISEMENT_KEY`

**Interfaces:**
- Produces: `export const ADVERTISEMENT_KEY = ['advertisement-state'] as const;`
  in shared — the exact existing literal from
  `features/advertisement/hooks/use-advertisement-state/useAdvertisementState.ts:5`;
  do not change its value.

- [ ] **Step 1:** move `ADVERTISEMENT_KEY` to
  `shared/api/queryKeys.ts`; re-export or import from shared in the
  advertisement feature (shared ← feature is the allowed direction).
- [ ] **Step 2:** update `useProviderConfigForm.ts:4` to import from
  `@/shared/api/queryKeys`.
- [ ] **Step 3:** `pnpm run lint:boundaries` → `0 problems`; `pnpm run test`.
- [ ] **Step 4: Commit** — `refactor(flip): lift ADVERTISEMENT_KEY to shared to fix the feature boundary`

### Task A4: Rust-produced contract fixtures

The root-cause fix for the mock-vs-daemon divergence class (timestamps, backup
envelope, `relay_cursors`). Frontend adapts in B1; this task makes divergence
impossible to reintroduce silently.

**Files:**
- Create: `crates/service-liquidity-manager/tests/contract_fixtures.rs`
- Create: `operator-ui/packages/types/fixtures/` (generated JSON, committed)
- Create: `operator-ui/packages/types/src/__tests__/contractFixtures.test.ts`

**Interfaces:**
- Produces: one JSON file per admin response type consumed by TS (health,
  funds, advertisement state, attestations, backup manifest, paging), each
  serialized by the real serde impls with representative values
  (`Timestamp(1721476800)` etc.).

- [ ] **Step 1:** write a **dedicated generator binary** (not an ordinary test
  that writes across the repo): `crates/service-liquidity-manager/src/bin/gen_contract_fixtures.rs`
  — construct each response struct with fixed values,
  `serde_json::to_string_pretty`, write to
  `operator-ui/packages/types/fixtures/<name>.json`. Add a `just`
  recipe (`just gen-contract-fixtures`). The paired Rust *test*
  (`tests/contract_fixtures.rs`) only re-serializes and asserts equality with
  the committed files, so CI fails on drift without writing anything.
- [ ] **Step 2:** generate, eyeball (timestamps must be JSON numbers; backup
  manifest must have exactly the 7 real groups), commit the JSON.
- [ ] **Step 3:** TS-side validation — TypeScript types vanish at runtime, so
  a bare `JSON.parse` + cast proves nothing. Use both layers:
  - compile-time: import each fixture with resolveJsonModule and pin it —
    `import health from '../fixtures/health.json'; const _check = health satisfies SystemHealthResponse;`
    — so `tsc` (already in CI via A1) rejects shape drift;
  - runtime: a small hand-written guard per response (or zod if already a
    dependency — check before adding one) exercised in
    `contractFixtures.test.ts`, asserting e.g. `typeof fixture.ingested_at === 'number'`.
  The `satisfies` check must fail against `Timestamp = string` until B1 lands
  — land this step on the B1 branch.
- [ ] **Step 4:** point the MSW mock fixtures at the same JSON files (import,
  don't copy), so mocks cannot drift again.
- [ ] **Step 5: Commit** — `test: add Rust-generated contract fixtures shared by TS tests and mocks`

### Task A5: Measured coverage — collectors, baseline, thresholds (after A1 is required)

**Files:**
- Modify: `operator-ui/vitest.workspace.ts` or per-package vitest configs
  (`packages/shared-ui/vitest.config.ts`, `packages/mock-devtools/vitest.config.ts`;
  create configs for the two apps, which currently have none)
- Modify: `operator-ui/package.json` (add `test:coverage` script),
  per-package `package.json` (add `@vitest/coverage-v8` dev-dependency)
- Modify: `.github/workflows/operator-ui.yml` (coverage step + artifact)
- Create: `justfile` recipe `rust-coverage` (repo root)

- [ ] **Step 1:** add `@vitest/coverage-v8`; per-app/package config:

```ts
test: {
  coverage: {
    provider: 'v8',
    reporter: ['text-summary', 'json-summary', 'lcov'],
    include: ['src/**'],
  },
}
```

  and a root script `"test:coverage": "pnpm -r test -- --coverage"`.
- [ ] **Step 2: Measure the baseline** — run it, record per-package
  statement/branch/function/line numbers in this file (table below), and only
  then pick thresholds: non-regression floors = baseline rounded down to the
  nearest whole percent, plus a higher floor (agree with the team; suggest
  baseline+10 capped at 90) for `shared/api/**` and `features/*/api/**`
  (the adapter code where the P1s lived). **Do not invent numbers before
  measuring** — leave the `thresholds` block out of the config until the
  baseline table is filled in.

  | Package | Stmts | Branch | Funcs | Lines |
  | --- | --- | --- | --- | --- |
  | (fill on Step 2) | | | | |

- [ ] **Step 3:** encode the agreed floors via vitest `coverage.thresholds`
  per package; CI step `pnpm run test:coverage` + upload
  `coverage/` as an artifact (`actions/upload-artifact`, SHA-pinned like the
  other actions) and paste the
  `json-summary` into the job summary (`$GITHUB_STEP_SUMMARY`).
- [ ] **Step 4 (Rust):** `just rust-coverage` =
  `cargo llvm-cov --package fedi-decentralized-service-liquidity-manager --package operator-ui-auth --package fman-core --lcov --output-path target/llvm-cov/lcov.info`
  (adjust package list to the actual crate names of the dashboard release
  surface — confirm with D-track answer on scope). Record the baseline the
  same way; wire into selfci as a non-blocking report first, blocking
  non-regression after one week of stability.
- [ ] **Step 5: Commit** — `ci: measure and enforce operator-ui and Rust coverage baselines`

### Task A6: Live write-path smokes + pre-release browser matrix

**Files:**
- Modify: `operator-ui/playwright.config.ts` (both app configs)
- Create: `operator-ui/e2e/flip/live-writes.spec.ts`,
  `operator-ui/e2e/fman/live-writes.spec.ts`
- Modify: `.github/workflows/operator-ui.yml` (or a separate
  `operator-ui-prerelease.yml`, same approval gate as A1)

**Interfaces:**
- Consumes: the `@live` tag convention and the existing live stack recipes
  (`operator-ui/dev/flip-stack/`, `dev/fman-stack/up.sh`); defe for CI
  daemons.

- [ ] **Step 1: Add Firefox/WebKit projects** to both Playwright configs,
  tagged so they run only in the pre-release job (`--project=firefox
  --project=webkit`); Chromium stays on every change. This closes the
  documented NFR-04 gap (`07-test-plan.md:89`).
- [ ] **Step 2: FLIP live write smoke** (`@live`): against the flip-stack
  daemon — set a config value through the settings form and read it back;
  after B3 lands, extend with publish → verify status → confirmed withdraw →
  verify hidden. Each test owns its data and asserts on round-tripped daemon
  state, not UI echoes.
- [ ] **Step 3: FMan live write smoke** (`@live`): against the fman-stack
  daemon — the smallest available authenticated write (set the offer price
  via C1's fixed form, read it back). Fresh-onboarding live coverage stays
  gated on D1.
- [ ] **Step 4:** pre-release job runs: full mocked suite on all three
  browsers + the `@live` specs on Chromium against the packaged artifacts.
  Document the trigger (tag push / release branch / manual dispatch — agree
  with the team) in the workflow file.
- [ ] **Step 5: Commit** — `test: live write-path smokes and Firefox/WebKit pre-release matrix`

---

## Track B — FLIP (owner: FLIP dev)

Order within the track: B1 → B2 → B3 → B4 → B5 → B6 → B7. B1 first because the
attestations crash and the fixtures (A4) both hang off it.

### Task B1: Timestamp codec — adopt numeric Unix seconds

**Files:**
- Modify: `packages/types/src/admin.ts:21` (`Timestamp`), `packages/types/src/paging.ts:13-15`
- Modify: `apps/liquidity-provider/src/shared/utils/format.ts`
- Modify: `apps/liquidity-provider/src/features/advertisement/services/format.ts`
- Modify: `packages/mock-fixtures/src/health.ts`, `advertisement.ts`, `attestations.ts`
- Test: `apps/liquidity-provider/src/shared/utils/__tests__/format.test.ts`

**Interfaces:**
- Produces: `export type Timestamp = number; // Unix seconds (Rust: transparent u64)`
  and `export const timestampToDate = (ts: Timestamp): Date => new Date(ts * 1000);`
  in `shared/utils/format.ts`. Everything rendering a timestamp goes through it.

- [ ] **Step 1: Failing tests first** — in `format.test.ts`:

```ts
it('should format a numeric Unix-seconds timestamp as an ISO date', () => {
  expect(formatDate(1721476800)).toBe('2024-07-20'); // date -u -r 1721476800 → Sat Jul 20 12:00:00 UTC 2024
});
```

  and in the advertisement `format` tests: `formatRelative(1721476800, now)`
  returns a relative string, not `'—'`.
- [ ] **Step 2:** run them → FAIL (`.slice is not a function` / `'—'`).
- [ ] **Step 3:** implement:

```ts
export const formatDate = (timestamp: Timestamp): string =>
  timestampToDate(timestamp).toISOString().slice(0, 10);
```

  and in `advertisement/services/format.ts` replace `Date.parse(iso)` with
  `timestampToDate(ts).getTime()`.
- [ ] **Step 4:** flip the type declarations; let `tsc` enumerate every other
  consumer; fix each at the call site via the codec, not ad hoc.
- [ ] **Step 5:** convert the three mock fixture files to numeric seconds (or,
  once A4 exists, to imports of the shared fixtures). Delete the ISO-string
  test inputs.
- [ ] **Step 6:** collapse the third parser: make
  `features/overview/utils/time.ts` delegate to `timestampToDate` (keep its
  tests, retarget inputs to numbers).
- [ ] **Step 7:** `pnpm run test && pnpm run typecheck` → green. Run the FLIP mocked e2e.
- [ ] **Step 8:** also reconcile the two known type drifts while in
  `packages/types`: add `background_workers` to `HealthComponentName`
  (`health.ts`), delete `relay_cursors` from the backup groups (`admin.ts:291-299`)
  and from `mock-server/src/routes/admin/backup.ts:18-27`.
- [ ] **Step 9: Commit** — `fix(flip)!: adopt numeric Unix-second timestamps across types, formatters, and mocks`

### Task B2: Backup UI honesty (interim, pre-transport)

The real fix (byte transport) is Track D (D2). Until then the UI must stop
lying: today it downloads a daemon-local path as a `.txt` labelled as a backup.

**Files:**
- Modify: `apps/liquidity-provider/src/features/settings/components/backup-card/BackupCard.tsx`
- Test: `.../backup-card/__tests__/BackupCard.test.tsx`

- [ ] **Step 1: Failing test** — after a successful create-backup, the card
  shows the daemon path as copyable text with copy explaining it lives on the
  daemon host, and there is **no** download button:

```ts
it('should present the archive as a daemon-side path, not a download', async () => {
  // render, click create backup, await mutation
  expect(screen.getByText(/on the daemon host/i)).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /download/i })).not.toBeInTheDocument();
});
```

- [ ] **Step 2:** implement: remove the `downloadTextFile` call and the false
  "store the downloaded file somewhere safe" copy; render
  `response.archive` in a `<code>` block with copy along the lines of:
  "Backup written on the daemon host at the path below. Copy it for use with
  `inspect_backup`/`restore_backup` on that host. Browser download/restore is
  not yet supported."
- [ ] **Step 3:** delete `features/settings/utils/downloadTextFile.ts` and its
  test if nothing else imports it (`grep -r downloadTextFile apps/`).
- [ ] **Step 4:** mirror the honesty in `RestoreConsolePage.tsx`: label the
  textarea "Backup archive path (on this daemon's filesystem)" and update the
  helper copy; adjust its test.
- [ ] **Step 5:** tests green; commit —
  `fix(flip): stop presenting the daemon backup path as a downloadable archive`

### Task B3: Advertisement withdrawal confirmation

**Files:**
- Create: `apps/liquidity-provider/src/features/advertisement/components/withdraw-confirm/WithdrawConfirm.tsx` (+ `.module.css`, `__tests__/`)
- Modify: `apps/liquidity-provider/src/pages/advertisement/AdvertisementPage.tsx:41-43,100-120`

**Interfaces:**
- Produces:

```ts
interface WithdrawConfirmProps {
  onConfirm: (reason: string | null) => void;
  onCancel: () => void;
  isPending: boolean;
}
export const WithdrawConfirm = (props: WithdrawConfirmProps) => { ... };
```

  A two-step inline confirmation panel (not a modal — no dialog primitive
  exists in shared-ui; do not invent one for a single use): optional reason
  input + "Confirm withdrawal" + "Cancel", styled from existing tokens.

- [ ] **Step 1: Failing tests** — clicking "Withdraw" does **not** call the
  mutation; it reveals the confirm panel; "Confirm withdrawal" calls
  `withdraw.mutate(reason)` — the hook's existing signature takes
  `string | null` directly, not an object; keep that signature; "Cancel"
  hides the panel without mutating.
- [ ] **Step 2:** implement `WithdrawConfirm`; in the page, replace the direct
  `withdraw.mutate(null)` wiring with `showConfirm` state; pass the typed
  reason through as `withdraw.mutate(reason)` (the wire type already
  supports it — stop discarding it).
- [ ] **Step 3:** the promise in the copy at `AdvertisementPage.tsx:120`
  ("You'll be asked to confirm") is now true — leave it.
- [ ] **Step 4 (small, same commit):** disable the Republish button when
  `data.publication_status === 'not_ready'` and surface the blocking reasons
  the API already returns (missing-affordance P3 from the assessment).
- [ ] **Step 5:** tests green; mocked e2e for the advertisement page green;
  commit — `feat(flip): require confirmation before advertisement withdrawal`

### Task B4: FundsPage keeps last-good data

**Files:**
- Modify: `apps/liquidity-provider/src/pages/funds/FundsPage.tsx:34`
- Test: `apps/liquidity-provider/src/pages/funds/__tests__/FundsPage.test.tsx`

- [ ] **Step 1: Failing test** — with cached funds data and `isError: true`
  (simulate a failed refetch), the page still renders balances plus a stale
  indicator; it renders the error banner only when there is no data at all.
- [ ] **Step 2:** change the predicate and add the stale banner:

```tsx
if (!funds.data) {
  if (funds.isError) return <ErrorBanner ... />;
  return <Loading ... />;
}
// data present: render normally; if funds.isError, show a "showing last-known
// data — retrying" banner above the content (dataUpdatedAt for the stamp).
```

- [ ] **Step 3:** tests green; commit —
  `fix(flip): keep last-good funds data visible through transient poll failures`

### Task B5: Central polling policy

**Files:**
- Create: `apps/liquidity-provider/src/shared/api/pollingIntervals.ts`
- Modify: `features/allocations/api/hooks/use-allocations/useAllocations.ts`,
  `use-allocation/useAllocation.ts`,
  `features/funds/api/hooks/use-funds/useFunds.ts`,
  `features/advertisement/api/hooks/use-advertisement-state/useAdvertisementState.ts`,
  `features/health/.../use-system-health/useSystemHealth.ts`

**Interfaces:**
- Produces (values from `02-requirements-baseline.md:108-116`):

```ts
export const POLL_ACTIVE_MS = 5_000;   // allocations with non-terminal items
export const POLL_STANDARD_MS = 30_000; // health, funds, advertisement
export const POLL_SETUP_MS = 60_000;
```

- [ ] **Step 1: Failing fake-timer tests** per hook (pattern):

```ts
it('should refetch allocations every 5s while a non-terminal allocation exists', async () => {
  vi.useFakeTimers();
  // render hook with a mock returning one 'pending' allocation
  await vi.advanceTimersByTimeAsync(5_000);
  expect(fetchSpy).toHaveBeenCalledTimes(2);
});
```

- [ ] **Step 2:** implement: allocations list + detail get
  `refetchInterval: (query) => hasNonTerminal(query.state.data) ? POLL_ACTIVE_MS : false`,
  funds/advertisement/health change `60_000` → `POLL_STANDARD_MS`. Add the
  missing `retry: false` to `useAllocations` for consistency with siblings.
- [ ] **Step 3:** tests green (restore real timers in `afterEach`); commit —
  `fix(flip): centralize polling intervals per the documented contract`

### Task B6: Withdrawal client validation + honest test

**Files:**
- Modify: `features/funds/components/funds-actions/FundsActions.tsx:20-27,52-57`
- Test: `features/funds/components/funds-actions/__tests__/FundsActions.test.tsx`

- [ ] **Step 1: Replace the blank-submit "success" test** (lines 76-88) with
  table-driven failing tests: empty address → submit disabled; amount `0`,
  `-1`, `abc`, `1.5` → submit disabled with a visible message; valid address +
  positive integer → mutation called with `{ address, amount }` exactly.
- [ ] **Step 2:** implement: trim the address, parse with
  `const amount = Number(withdrawAmount);` and require
  `Number.isInteger(amount) && amount > 0 && address.length > 0`; disable the
  button and show the inline hint otherwise. (Address *format* validation
  stays server-side — the daemon's `prepare_withdrawal` is the source of
  truth; do not embed a bitcoin address library for MVP.)
- [ ] **Step 3:** tests green; commit —
  `fix(flip): block empty and non-positive withdrawal submissions client-side`

### Task B7: Access-denied state for 403

**Files:**
- Modify: `apps/liquidity-provider/src/shared/api/errors.ts` (add class),
  `apps/liquidity-provider/src/shared/api/adminCall.ts:51-57`
- Modify: `features/boot/hooks/use-boot-status/useBootStatus.ts`
- Test: colocated `__tests__/` for both

**Interfaces:**
- Produces: `export class AccessDeniedError extends Error {}` thrown when the
  ServiceError code is `permission_denied`; `useBootStatus` returns a new
  `status: 'access-denied'` the shell renders as a permission-error screen
  (NOT the re-auth gate — per `SPEC-flip-admin-api.md:31-33`, 403 is an
  authenticated permission failure).

- [ ] **Step 1: Failing tests** — `adminCall` throws `AccessDeniedError` for a
  403 + `{code:'permission_denied'}` body; boot hook maps it to
  `'access-denied'` even when cached data exists; 401 still maps to the
  re-auth gate.
- [ ] **Step 2:** implement (one added branch in `adminCall`, one in the hook,
  one shell screen reusing the existing error-screen component with
  permission copy).
- [ ] **Step 3:** while in the hook: fix the cache-masking for **401** too —
  drop the `!setup.data &&` guard so `AuthError` always routes to re-auth.
  Do **not** `queryClient.clear()` on that transition: the errored boot query
  is the state keeping the re-auth screen mounted, and clearing it triggers a
  refetch loop. Sequence instead: (1) the gate renders off the `AuthError`
  alone (durable via the query's error state); (2) privileged *other* queries
  are removed selectively — never compare key arrays by reference; use
  TanStack's key matching to *exclude* the boot query:
  `queryClient.removeQueries({ predicate: q => hashKey(q.queryKey) !== hashKey(SETUP_KEY) })`
  (`hashKey` from `@tanstack/react-query`), or equivalently enumerate and
  remove the known privileged keys (`FUNDS_KEY`, `ALLOCATIONS_KEY`,
  `ADVERTISEMENT_KEY`, …) explicitly — so stale data can't flash; (3) after
  successful re-login,
  `queryClient.invalidateQueries()` refreshes everything. Failing tests
  first: cached data + refetch `AuthError` → re-auth gate shown *and stays
  shown* across a render cycle (no loop); post-login → data refetched.
- [ ] **Step 4:** tests green; mocked e2e boot specs green; commit —
  `fix(flip): distinct access-denied state for 403 and cache-independent re-auth on 401`

### Task B9: Gate FLIP setup above the app shell (adopted from the plan of record)

FMan gates setup **above** `AppShell`, so the sidebar never renders during
setup; FLIP gates inside its shell, so its nav shows during an incomplete
setup. Align FLIP with the FMan pattern.

**Files:**
- Modify: `apps/liquidity-provider/src/app/App.tsx` (or the component
  mounting the setup gate — follow the FMan reference:
  `apps/fleet-manager/src/app/index.tsx` mounts `SetupGate` outside the shell)
- Test: the existing setup-gate/App tests plus the mocked setup e2e

- [ ] **Step 1: Failing test** — with the mock in a not-set-up scenario, the
  navigation landmark is absent while the setup screen shows.
- [ ] **Step 2:** move the gate above the shell, mirroring FMan's structure.
- [ ] **Step 3:** unit + mocked e2e green (update any spec that assumed nav
  exists during setup); commit —
  `fix(flip): gate setup above the app shell so the nav is hidden during setup`

### Task B8: Retire the stale Requests story — **gated on D5**

Do not schedule this until D5 (docs versioning) is decided: the
`docs/operator-dashboards/` tree is currently uncommittable from this
workstation, and an in-place edit of an unversioned file is not a deliverable.

- [ ] **Step 1 (after D5 resolves):** if the docs move into version control —
  prepend the banner to `06-requests.md` and fix the `MVP-SPEC.md` flow-3 row
  in the same commit: "Superseded 2026-08-08: the request domain was deleted
  in `e32a0a40` (federation is the allocation identity). Verification
  survives as `get_verification_summary`. Do not implement."
  If the docs are instead declared non-authoritative, record that in the
  repo (e.g. a note in `operator-ui/CLAUDE.md` or the specs index) and skip
  the banner.
- [ ] **Step 2: Commit** (only possible in the first branch of Step 1) —
  `docs: mark the Requests user story superseded by e32a0a40`

---

## Track C — FMan (owner: FMan dev)

Order: C1 first (smallest change, worst consequence), then C2, C3, C4, C5.

### Task C1: useOfferForm — surface load failure, block the destructive save

**Files:**
- Modify: `apps/fleet-manager/src/features/offer/hooks/use-offer-form/useOfferForm.ts:51-55`
- Modify: `apps/fleet-manager/src/pages/offer/OfferPage.tsx:50` (disable condition)
- Test: `features/offer/hooks/use-offer-form/__tests__/useOfferForm.test.ts`

- [ ] **Step 1: Failing tests** — when the offer query errors: the hook
  exposes the load error, and `canSubmit` is false (an empty field after a
  failed load must NOT be submittable as "stop selling seats"); when the
  query succeeds with a price, the field is seeded and submit works as today.
- [ ] **Step 2:** implement:

```ts
const loadError = offer.isError ? describeActionError(offer.error) : null;
return {
  ...,
  canSubmit: !offer.isError && !offer.isLoading,
  error: validationError
    ?? (setPrice.isError ? describeActionError(setPrice.error) : null)
    ?? loadError,
};
```

  and in `OfferPage` add `|| !form.canSubmit` to the Save-disable condition.
- [ ] **Step 3:** tests green; commit —
  `fix(fman): a failed offer load can no longer be saved as a blank price`

### Task C2: Cache-independent auth expiry

**Files:**
- Modify: `apps/fleet-manager/src/features/boot/hooks/use-boot-status/useBootStatus.ts:29-30`
- Test: colocated `__tests__/`

- [ ] **Step 1: Failing test** — cached onboarding data + a refetch that
  rejects with `AuthError` → `status: 'needs-auth'`.
- [ ] **Step 2:** implement: `const needsAuth = onboarding.error instanceof AuthError;`
  (drop `!onboarding.data &&`; update the hook's doc comment at lines 10-12,
  which currently documents the old behavior as intentional). Same cache rule
  as B7 Step 3: do **not** `queryClient.clear()` — the errored onboarding
  query is the state keeping the gate mounted, and clearing it causes a
  refetch loop. Preserve the onboarding query; selectively remove the other
  privileged queries (key-hash matching or explicit key list, per B7); after
  successful re-login, `queryClient.invalidateQueries()`.
- [ ] **Step 3:** tests + mocked boot e2e green; commit —
  `fix(fman): route to re-authentication on session expiry even with cached data`

(No 403 work in FMan: the daemon's admin surface is 401-only cookie auth
today. The client folds non-401 into `NetworkError` — acceptable until the
Rust team answers D4's 403 question.)

### Task C3: Overview must not render green on failure

**Files:**
- Modify: `apps/fleet-manager/src/pages/overview/OverviewPage.tsx:15-30`
- Modify: `apps/fleet-manager/src/features/overview/` — `deriveOverview.ts` and `useOverviewEarnings.ts` consumers as needed
- Test: `pages/overview/__tests__/OverviewPage.test.tsx`

**Constraint (from the FMan plan of record):** the earnings presentation is
PM-signed-off as-is — one gross headline figure with the asterisk/footnote
caveats. This task adds loading/error *branches*; it must not change the
signed-off copy, figure, or caveat structure in the populated state.

- [ ] **Step 1: Failing tests** — (a) while queries are loading, the page shows
  a loading state, not zero totals; (b) when seats/offer/payment queries
  error, the page shows an error banner, never the green "Advertised and
  healthy" banner or zero earnings presented as real; (c) the populated
  state renders the existing signed-off presentation unchanged (keep the
  current passing assertions).
- [ ] **Step 2:** implement: consume the already-computed
  `earnings.isLoading` (currently dead) plus an `isError` aggregate; branch
  before calling `deriveOverview` — do not feed defaulted `[]` into the
  success-tone derivation on error.
- [ ] **Step 3:** tests green; commit —
  `fix(fman): overview distinguishes loading and failure from healthy-zero`

### Task C4: Shared-UI accessibility

**Files:**
- Modify: `packages/shared-ui/src/components/text-input/TextInput.tsx`,
  `select-field/SelectField.tsx` (mechanical),
  `form-field/FormField.tsx` (interface change),
  `stepper/Stepper.tsx` + `Stepper.module.css`
- Test: each component's `__tests__/`

- [ ] **Step 1 (TextInput/SelectField, failing tests first):** the input has
  `aria-describedby` pointing at the hint/error ids and `aria-invalid` when
  errored. Implementation: both files already compute an id via `useId` —
  derive `hintId`/`errorId` from it, put them on the spans, join into
  `aria-describedby`, add `aria-invalid={Boolean(error)}`. Error spans get
  `role="alert"` so newly shown errors are announced.
- [ ] **Step 2 (FormField):** extend the interface —

```ts
interface FormFieldProps {
  htmlFor: string;
  label: string;
  hint?: string;
  error?: string;
  children: (ids: { describedBy?: string }) => ReactNode;
}
```

  (render-prop so arbitrary children can wire `aria-describedby`). Migrate the
  few call sites `tsc` finds. Failing test: the child receives the id of a
  rendered hint.
- [ ] **Step 3 (Stepper):** render `<ol>` with `<li>` steps,
  `aria-current="step"` on the active one, and a visually-hidden status word
  ("completed"/"current"/"upcoming") per step so completed-vs-upcoming no
  longer relies on green-vs-grey alone. Keep `data-state` for CSS.
- [ ] **Step 4:** all shared-ui tests green; both apps' snapshots/tests
  updated; commit — `fix(shared-ui): associate form errors and give the stepper list semantics`

### Task C5: Honest async states on Backup and SetupAuthorization

**Files:**
- Modify: `apps/fleet-manager/src/pages/backup/BackupPage.tsx:25-45`
- Modify: `apps/fleet-manager/src/features/setup/components/setup-authorization/SetupAuthorization.tsx:13-41`
- Tests: colocated

- [ ] **Step 1: Failing tests** — BackupPage: loading shows a loading state and
  a query error shows an error banner (never a bare `—` beside copy asserting
  keys are recoverable). SetupAuthorization: a query error shows an error
  message, not "Waiting for a holder to authorize this key."
- [ ] **Step 2:** implement the two branches (`isLoading` / `isError` before
  the data render), reusing the app's existing banner components.
- [ ] **Step 3:** tests green; commit —
  `fix(fman): backup and setup-authorization show loading and failure explicitly`

### Task C7: FMan live-data freshness (adopted from the plan of record)

Requirements copied from `tasks/fman-dashboard.md` "Live data freshness" —
re-verified at head 2026-08-08 (only onboarding/authorization poll today; the
`queryClient` is a bare `new QueryClient()`). The governing constraint:
`useSeatReports`/`useGuardianFees` fan out **per seat**, so polling cost is
`interval × seat count` — argue interval changes in seats, not milliseconds.

**Files:**
- Create: `apps/fleet-manager/src/shared/api/pollingIntervals.ts`
- Modify: `apps/fleet-manager/src/shared/api/queryClient.ts`;
  `features/seats/api/hooks/use-seat-status/useSeatStatus.ts` and
  `use-seat-reports/useSeatReports.ts` (whichever owns the per-seat fan-out);
  `shared/api/hooks/use-seats/useSeats.ts` (`SEATS_KEY`);
  `shared/api/hooks/use-payment-federations/usePaymentFederations.ts`
  (`PAYMENT_FEDERATIONS_KEY`);
  `features/overview/api/hooks/use-guardian-fees/useGuardianFees.ts`
- Test: colocated `__tests__/` per hook (fake timers, restored in `afterEach`)

**Interfaces:**
- Produces:

```ts
export const SEAT_FORMATION_POLL_MS = 5_000;  // only while a seat is non-terminal
export const LIST_POLL_MS = 30_000;           // ListSeats, ListPaymentFederations
export const FEES_POLL_MS = 60_000;           // GuardianFees, display screens only
```

- [ ] **Step 1: Failing fake-timer tests**, one per requirement:
  (1) `SeatStatus` refetches every 5s while any seat is in a non-terminal
  phase (`dkg_in_progress`/`code_generated`) and stops entirely once all are
  `running`/`decommissioned` — same conditional-interval shape as
  `useAuthorizationWatch.ts:23`;
  (2) `ListSeats` refetches at 30s;
  (3) `ListPaymentFederations` refetches at 30s;
  (4) `GuardianFees` refetches at 60s and only mounts its interval on screens
  displaying fees;
  (5) `ShowPlans`/`OFFER_KEY` does **not** poll (assert no interval —
  `useSetPrice` invalidation already covers it).
- [ ] **Step 2:** implement via the constants above;
  `refetchInterval: (query) => hasNonTerminalSeat(query.state.data) ? SEAT_FORMATION_POLL_MS : false`
  for (1).
- [ ] **Step 3:** set app-wide defaults deliberately in `queryClient.ts` — a
  modest `staleTime` (suggest 15s; hooks override) and leave
  `refetchIntervalInBackground` at its default `false`, stated in a comment
  (a hidden tab must stop polling; that default is load-bearing).
- [ ] **Step 4:** all tests green; mocked e2e green; commit —
  `feat(fman): poll seats, federations, and fees per the freshness contract`

### Task C6: P3 housekeeping (tokens, price-form dedup, CSS dupes)

Low priority — schedule after C1–C5. Sequencing note: the token cleanup
sub-items overlap the plan of record's Phase 3 design pass, which runs
deliberately last — fold them into that pass rather than racing ahead of it;
each sub-item is one small commit.

**Files:**
- Modify: `apps/fleet-manager/src/features/setup/components/setup-authorization/SetupAuthorization.tsx:7`
  (`QR_SIZE = 192` → a token or a documented component constant in the module CSS),
  `packages/shared-ui/src/components/stepper/Stepper.module.css:8` (`min-w-[5rem]` → preset token),
  `packages/shared-ui/styles/utilities.css:107-114` (`border-l-[3px]` ×3 → one token/utility)
- Create: `apps/fleet-manager/src/features/offer/hooks/use-price-form/usePriceForm.ts` (+ `__tests__/`)
- Modify: `features/offer/hooks/use-offer-form/useOfferForm.ts` and
  `features/setup/.../SetupPrice.tsx` to consume it
- Modify: the 8 `lint:css-dupes` groups (start with the within-file dupe in
  `ReviewStep.module.css`)

**Interfaces:**
- Produces:

```ts
interface UsePriceFormOptions {
  onSubmit: (priceSats: number | null) => void;
  /** Seed for the draft once async data arrives. The hook seeds the draft
   *  from this value exactly once — on the first render where it becomes
   *  defined and the user has not yet typed (dirty flag). It never
   *  overwrites user input on later refetches. `null` = "no price set". */
  initialPriceSats?: number | null;
}
export const usePriceForm = (options: UsePriceFormOptions) => {
  // returns { draft, error, isDirty, handleChange, handleSubmit }
};
```

  — the ~15 lines of draft/validate/clear-on-change/submit machinery
  currently duplicated between `useOfferForm.ts:21-49` and
  `SetupPrice.tsx:14-31`. The seed-once contract replicates the existing
  seeding block at `useOfferForm.ts:30-33`; `SetupPrice` simply passes no
  `initialPriceSats`. If this contract turns out not to cover both call
  sites cleanly, drop the extraction — it is P3 and not worth a leaky
  abstraction.

- [ ] **Step 1:** extract `usePriceForm` test-first: port the existing
  offer-form tests for draft/validation behavior **plus** two seed tests —
  (a) `initialPriceSats` arriving after mount seeds an untouched draft;
  (b) it never overwrites a dirty draft — then make setup and offer both
  consume it (behavior unchanged, tests prove it).
- [ ] **Step 2:** replace the three hard-coded values with preset tokens
  (check `packages/shared-ui/tailwind-preset.cjs` for an existing spacing/
  border token before adding one).
- [ ] **Step 3:** clear the CSS-dupe groups via the sharing ladder (component
  module → app `utilities.css` → shared-ui) until `pnpm run lint:css-dupes`
  reports 0; document any deliberately-kept duplicate inline.
- [ ] **Step 4:** `pnpm run test && pnpm run lint:css-dupes`; commit per sub-item.

---

## Track D — blocked on backend decisions (owner: Rust team + product; chase in parallel from day 1)

Each entry: the gating question → what it unblocks. These come from the
assessments' corrected question lists; full banks live in the audit docs.

### D1: FMan onboarding transport

**Questions:** Is fresh-host *browser* onboarding a committed requirement, or
is CLI/socket bootstrap acceptable? If browser: what protects the pre-identity
HTTP endpoint, and will the daemon add a **read-only setup-state verb** (the
UI currently string-matches `'has not been onboarded'`, which only the socket
path emits — a listener alone fixes nothing)? Will
`packages/fleet-manager/entrypoint.sh` enable `--admin-http-bind`/auth (today
the packaged install has no dashboard in any state)?
**Unblocks:** the FMan P1; a live e2e from an empty data dir.
**Do now regardless:** nothing frontend-side — the wizard is already built
and mock-tested.

### D2: FLIP backup byte transport

**Questions:** stream bytes through the daemon or issue a download capability?
Size/timeout limits? Encryption/recovery-secret? Restore atomicity + live vs
restore-mode-only? Are path APIs retained for CLI?
**Unblocks:** real download/upload UI replacing B2's interim honesty fix; the
cross-host live test.
**Note for the daemon team:** the UI-side contract preference (from the
assessment): authenticated binary download with content-type + filename;
restore as streaming upload with explicit limits.

### D3: OCI image bind topology

**Question:** the image definition sets `FLIP_ADMIN_BIND_ADDRESS=0.0.0.0`
(`flake.nix:545-551`) — necessary for Docker port-publishing to work at all,
but it means one careless `-p 8173:8173` exposes bearer-token plaintext admin.
Decide the supported topology (loopback-published ports as the documented
default? daemon-side bind validation with an explicit override? reverse-proxy
TLS?), then document it in `SECURITY.md` (currently silent on FLIP network
posture).
**Unblocks:** closing the network-locality P2 properly instead of a config
whack-a-mole.

### D4: Error contract confirmations

**Questions:** FLIP — confirm token expiry always surfaces as 401 (B7 assumes
it). FMan — does the admin API ever return 403? (Client folds non-401 into
`NetworkError`; a permission denial would render as "daemon unreachable".)
**Unblocks:** nothing today (B7/C2 proceed on the spec as written); answers
either confirm or add one small follow-up task.

### D5: Docs versioning

**Question:** `docs/operator-dashboards/` is untracked (ignored by this
workstation's global gitignore, not repo policy) and demonstrably stale
(Requests, `relay_cursors`, polling module). Either check the MVP docs into
the repo so they version with the code, or declare them non-authoritative.
**Unblocks:** B8 being a real commit; future audits not chasing deleted
features.

---

## Suggested sequencing across tracks

```
Week-0 parallel starts:
  A1 CI workflow (draft PR)      D1–D5 questions posted to Rust/product
  A2 biome  →  A3 boundary  →  A1 check made required
  C1 offer destructive save      B1 timestamps
Then:
  A4 fixtures (pairs with B1)    B2 backup honesty → B3 confirm → B4 funds
  C2 auth expiry → C3 overview   B5 polling → B6 withdrawal → B7 403/401
  C7 FMan freshness (after C2 —  B9 FLIP gate placement
  both touch the query layer)
Later (answer-dependent):
  D1 → onboarding implementation + live e2e
  D2 → real backup transport + cross-host live test
  D3 → image/bind change + SECURITY.md
  D5 → B8 requests-doc retirement
Finally: A5 coverage baselines → thresholds; A6 live smokes + browser matrix.
```

Coverage thresholds (A5) come **last** deliberately: measure the baseline after
the suite is CI-enforced, then set non-regression floors (the audit's own
advice — don't invent a percentage first). A6's live smokes land as soon as
the write paths they exercise are fixed (B3/B6/C1), independent of A5.
