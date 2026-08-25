# Cursor QA Review 1 — Operator-UI Remediation

**Verdict: PASS-WITH-FINDINGS**

| Field | Value |
| --- | --- |
| Reviewer | Independent adversarial QA (no implementer role) |
| Worktree | `/Users/kc/Projects/decentralized-federations-remediation` |
| Branch | `feat/operator-ui-remediation` |
| HEAD | `3885fa65` |
| Base | `77a049c7` (`feat/msw-mock-migration`, not `master`) |
| Commits | 29 |
| Date | 2026-08-09 |
| Plan of record | `operator-ui/REMEDIATION-PLAN-2026-08-08.md` |
| Ledger | `.superpowers/sdd/REMEDIATION-PLAN-2026-08-08/progress.md` |
| Assessment basis | `AUDIT-ASSESSMENT-2026-08-08.md` (+ FMAN/FLIP siblings) |

Priority behaviour fixes at HEAD hold under code review and freshly run gates. The branch does **not** deliver a green operator-ui CI gate, and several assessment items remain open by design or omission. Ledger/self-reports were not trusted; claims were checked against code and commands.

---

## Defects

### 1. High — A1 CI cannot pass at HEAD (`lint:css-dupes`)

- **Command:** `pnpm run lint:css-dupes` → **exit 1**, 8 duplicate `@apply` groups (same failure class as at base `77a049c7`).
- **Evidence:** Workflow `.github/workflows/operator-ui.yml` runs `pnpm run lint:css-dupes` as a hard step.
- **Context:** C6 (the planned clear-up of those 8 groups) was deliberately deferred.
- **Impact:** The P1 “wire frontend checks into CI” lands a check that fails on every UI PR until C6. Ledger “BRANCH COMPLETE” overstates release readiness.

### 2. Medium — Dead Overview “Requests” quick action (assessment / B8)

- **Evidence:** `apps/liquidity-provider/src/features/overview/components/quick-actions/QuickActions.tsx` still links to `/requests`.
- **Evidence:** `App.tsx` has no requests route; no `pages/requests` directory.
- **Context:** Assessment closed the “Requests workflow absent” audit P1 as stale-spec/doc; plan B8 is gated on D5.
- **Impact:** UI still ships a broken nav affordance. Not half-implemented — fully deferred; defect remains.

### 3. Low — B5 centralisation incomplete vs module contract

- **Evidence:** `apps/liquidity-provider/src/shared/api/pollingIntervals.ts` states all polling hooks must use the module.
- **Still local:**
  - `useWalletOperations.ts` — local `FAST_POLL_MS` / `IDLE_POLL_MS` (values match §6).
  - `useAttestationList.ts` — hardcoded `60_000` (not in the §6 table).
- **Context:** Named B5 hooks (allocations / health / funds / advertisement / setup) are correct.
- **Impact:** Claim of full centralisation is not met; interval *values* for the contract table are fine.

---

## Observations

- **Deferrals intact:** No B8 / C6 / Track D half-landings in the remediation diff. `flake.nix` still has `FLIP_ADMIN_BIND_ADDRESS=0.0.0.0:8173` (D3). FMan browser onboarding still D1. Backup remains path/honesty (B2), not byte transport (D2).
- **Stacking:** Remediation = 29 commits on `77a049c7`. That base is ~230 commits ahead of `master` (`feat/msw-mock-migration`). A PR to `master` is not remediation-only.
- **Out-of-`operator-ui/` files in the delta:** `.github/workflows/operator-ui.yml`, `.typos.toml` (hygiene `a16ebc3a`), contract-fixture Rust under `crates/service-liquidity-manager/`, `justfile.custom.just` (gen fixtures + rust-coverage) — consistent with A1/A4/A5, not drive-by.
- **A4 residuals:** MSW ready path uses generated JSON; some tests / alternate scenarios / backup MANIFEST in verbs remain hand-authored copies.
- **`act(…)` warnings** in polling hook tests (FMan seat status/reports, FLIP health/funds) — noise; tests still pass.
- **A5/A6 partial-by-design** matches ledger: thresholds proposal-only; `@live` not run against a daemon; firefox/webkit projects configured, not executed in this review.

---

## Priority correctness (verified at HEAD)

| Task | Result | Evidence |
| --- | --- | --- |
| **B1** timestamps | PASS | Rust `Timestamp(u64)` `crates/domain/src/lib.rs`; TS `Timestamp = number` in `packages/types`; FLIP formatters use seconds → `* 1000` for `Date` |
| **A4** fixtures | PASS | `cargo test --package fedi-decentralized-service-liquidity-manager --test contract_fixtures` → 3/3; TS `contractFixtures.test.ts` → 12/12 |
| **B7 / C2** auth | PASS | FLIP: `needsAuth` / `accessDenied` without `!data` gate; Rust bearer miss → 401 (`admin_http.rs`), policy `PermissionDenied` → 403; FMan 401 cache-independent re-auth |
| **C1 / B6** guards | PASS | Offer `canSubmit` + early return in `useOfferForm`; withdraw `isWithdrawValid` + early return in `FundsActions` (not button-only) |
| **B5 / C7** polling | PASS* | FLIP constants + setup wiring; FMan list/fees/formation intervals present (*centralisation residual in Defect 3) |
| **B9** gate | PASS | BootGate → SetupGate → AppShell; `RootLayout` keeps `MockPanelMount` above gates |
| **C3** overview | PASS | Loading/error branches added; populated gross earnings + caveats presentation unchanged vs base |

---

## Gates (from `operator-ui/` in worktree)

| Command | Result |
| --- | --- |
| `pnpm install --frozen-lockfile` | OK |
| `pnpm run typecheck` | exit 0 |
| `pnpm exec biome check .` / `pnpm run lint` | exit 0 (566 files, no issues) |
| `pnpm run lint:compiler` | exit 0 |
| `pnpm run lint:boundaries` | exit 0 |
| `pnpm run lint:css-dupes` | **exit 1** (8 groups) |
| `pnpm run test` | exit 0 — types 12, mock-devtools 27, shared-ui 24, fleet-manager 243, liquidity-provider 349 |
| `pnpm run build` | exit 0 (both apps) |
| `pnpm exec playwright test` | **20/20**, exit 0 |
| `E2E_APP=fman pnpm exec playwright test` | **35/35**, exit 0 |

---

## Gaps vs assessments (not covered by completed plan tasks)

| Finding | Status |
| --- | --- |
| No frontend CI (P1) | Workflow exists but **red** on css-dupes |
| Hand-authored mocks as contract | Largely fixed (A4); residuals remain |
| `docs/operator-dashboards/` untracked/stale | Still globally ignored; D5 |
| FMan fresh onboarding unreachable (P1) | Still D1 |
| FLIP backup byte transport (P1) | B2 honesty only; D2 |
| OCI admin bind `0.0.0.0` (P2) | Still `flake.nix`; D3 |
| Requests UI affordance | Dead link remains; B8/D5 |
| Coverage thresholds / pre-release CI job | A5/A6 partial as stated |
| Shared-UI a11y beyond C4 | C6 token/housekeeping deferred |

---

## Branch / worktree hygiene

| Check | Result |
| --- | --- |
| HEAD | `3885fa65`, 29 commits over `77a049c7` |
| Tree | Clean (`git status --porcelain` empty after verification) |
| Push | `git ls-remote origin refs/heads/feat/operator-ui-remediation` → empty (not pushed) |
| `--no-verify` | No markers in remediation commit messages; hygiene `a16ebc3a` is a normal commit (`.typos.toml` + package.json newline only) |
| File scope | Remediation delta scoped to plan surfaces + authorized hygiene; no silent Track D daemon work |

---

## Ledger claims not independently verified

1. **A1 Step 3** — draft-PR path-filter / required-check behaviour (no push/PR).
2. **A5 80.95% baseline number** — coverage dirs gitignored; `test:coverage` not re-run. Collectors/config presence accepted; figure not re-measured.
3. **A6 `@live` vs real daemon** — explicitly unfinished; not run.
4. **A6 firefox/webkit projects** — present in `playwright.config.ts`; only Chromium mock suites run here.
5. **Pre-commit actually executed** on each commit — only absence of skip evidence, not hook logs.
6. **C3 “byte-identical” populated block** — copy/structure match verified by diff/read; not a mechanical byte hash of the full function body.

---

## Bottom line

Do not treat this branch as CI-green or release-gated. Behaviour remediations for the executable track look real. Clear `lint:css-dupes` (or stop gating on it until C6) before calling the CI P1 done.
