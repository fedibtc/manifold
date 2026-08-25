# Operator UI remediation QA review 1

- **Date:** 2026-08-09
- **Reviewer role:** Independent, adversarial QA
- **Worktree reviewed:** `/Users/kc/Projects/decentralized-federations-remediation`
- **Branch:** `feat/operator-ui-remediation`
- **Range:** `77a049c7...3885fa65`
- **Commit count:** 29

## Verdict: FAIL

The branch fails one required gate. Several ledger tasks marked complete satisfy only part of their acceptance criteria.

## Defects

### P1: The new CI gate fails on every relevant change

`pnpm run lint:css-dupes` exits 1 with eight duplicate groups. The workflow runs that command at:

`/Users/kc/Projects/decentralized-federations-remediation/.github/workflows/operator-ui.yml:55`

C6 owns the duplicate cleanup, but the execution deliberately deferred C6. The branch therefore wires a known-failing command into A1's required check.

### P2: B7 and C2 do not propagate auth failures from non-boot queries

Both apps derive their gate state from one boot query:

- FLIP reads `setup.error` at `/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/liquidity-provider/src/features/boot/hooks/use-boot-status/useBootStatus.ts:29-36`.
- FMan reads `onboarding.error` at `/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/fleet-manager/src/features/boot/hooks/use-boot-status/useBootStatus.ts:17-34`.

A 401 from Funds, Advertisement, ListSeats, or another privileged query remains local until the boot query polls again. Both boot queries use a 60-second healthy interval. A route-specific FLIP 403 also stays local instead of opening the access-denied screen.

Rust defines the intended distinction:

- Missing or invalid bearer token returns 401 at `/Users/kc/Projects/decentralized-federations-remediation/crates/liquidity-manager-daemon/src/admin_http.rs:607-630`.
- An authenticated `permission_denied` service error maps to 403 at `/Users/kc/Projects/decentralized-federations-remediation/crates/liquidity-manager-daemon/src/admin_http.rs:676-684`.

The clients classify 401 and 403 correctly. They do not propagate auth errors from the API/query boundary into the app gate.

### P2: B5 does not satisfy the poll-failure contract

The polling contract requires backoff, retained last-good data, and a stale banner after a poll failure:

`/Users/kc/Projects/decentralized-federations/docs/operator-dashboards/liquidity-manager/02-requirements-baseline.md:108-116`

Current behavior:

- Advertisement replaces cached data with an error whenever `isError` is true at `/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/liquidity-provider/src/pages/advertisement/AdvertisementPage.tsx:51-71`.
- Allocation rows disappear on a failed refetch at `/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/liquidity-provider/src/features/allocations/components/allocations-table/AllocationsTable.tsx:26-36`.
- Allocation detail disappears at `/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/liquidity-provider/src/features/allocations/components/timeline-panel/TimelinePanel.tsx:14-25`.
- The new hooks return constant intervals after errors, so they add no backoff.
- Wallet-operation polling keeps local 5-second and 30-second constants at `/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/liquidity-provider/src/features/funds/api/hooks/use-wallet-operations/useWalletOperations.ts:10-14`, outside the claimed central policy.

Funds implements the required last-good behavior. The other polled views do not.

### P2: C3 omits dependent GuardianFees loading

`useOverviewEarnings` mounts one GuardianFees query per live seat at:

`/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/fleet-manager/src/features/overview/hooks/use-overview-earnings/useOverviewEarnings.ts:18-25`

Its `isLoading` result includes only seats and payment federations:

`/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/fleet-manager/src/features/overview/hooks/use-overview-earnings/useOverviewEarnings.ts:31-37`

After `ListSeats` resolves, the fee queries mount in a pending state. During that window, `guardianFees` is empty and Overview renders zero earnings as populated data. The existing loading test holds all requests pending together and does not cover this dependent-query transition.

The final populated copy and caveat structure remain unchanged from base.

### P2: A4 leaves the backup MSW contract hand-authored

The generator and committed JSON fixtures match. The backup mock still copies its manifest instead of importing `backup_manifest.json`:

`/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/liquidity-provider/src/mocks/world/verbs.ts:471-506`

The mock can drift on `created_at` or the seven state groups while the Rust fixture test remains green. This violates A4 Step 4, which requires MSW fixtures to consume the generated JSON.

### P2: A6 omits the required advertisement live-write cycle

The FLIP live spec only completes setup and round-trips a settings value:

`/Users/kc/Projects/decentralized-federations-remediation/operator-ui/e2e/live-writes.spec.ts:16-44`

It does not publish an advertisement, verify publication, confirm withdrawal, or verify that the advertisement becomes hidden. The plan requires that cycle after B3. This omission exceeds the disclosed A6 deferrals for live execution and CI wiring.

### P2: New structure enforcement misses committed violations

`AccessDenied` sits loose under `features/boot/components/` and has no colocated unit test:

`/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/liquidity-provider/src/features/boot/components/AccessDenied.tsx:1`

The folder rule requires a kebab-case component folder and a colocated test:

`/Users/kc/Projects/decentralized-federations-remediation/operator-ui/.claude/rules/folder-structure.md:20-29`

`check-structure.mjs` reports no violations because it scans only unstaged, staged, and untracked files:

`/Users/kc/Projects/decentralized-federations-remediation/operator-ui/scripts/check-structure.mjs:55-69`

The new CI workflow does not invoke this checker.

### P3: C4 leaves FormField accessibility wiring inert

The plan requires `htmlFor`, a `describedBy` render-prop value, and migrated call sites. The implementation makes `htmlFor` optional:

`/Users/kc/Projects/decentralized-federations-remediation/operator-ui/packages/shared-ui/src/components/form-field/FormField.tsx:4-16`

Production callers omit it and ignore `describedBy`. Their labels, hints, and errors remain unassociated. TextInput, SelectField, and Stepper implement their specified semantics.

### P3: B3 introduces a focus-management regression

Clicking the withdrawal button removes the focused trigger:

`/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/liquidity-provider/src/pages/advertisement/AdvertisementPage.tsx:131-149`

The replacement panel is a plain `div` with no focus transfer or group/dialog semantics:

`/Users/kc/Projects/decentralized-federations-remediation/operator-ui/apps/liquidity-provider/src/features/advertisement/components/withdraw-confirm/WithdrawConfirm.tsx:19-40`

This violates the repository's keyboard and focus requirement at `operator-ui/docs/clean-code.md:219`.

## Gate results

| Command | Result |
| --- | --- |
| `pnpm install --frozen-lockfile` | PASS. Dependencies already matched the lockfile. |
| `pnpm run typecheck` | PASS, exit 0. |
| `pnpm run lint` | PASS, 566 files checked. |
| `pnpm run lint:compiler` | PASS, exit 0. |
| `pnpm run lint:boundaries` | PASS, exit 0. |
| `pnpm run lint:css-dupes` | **FAIL, exit 1, eight duplicate groups.** |
| `pnpm run test` | PASS, 157 test files and 655 tests. |
| `pnpm run build` | PASS, both applications built. |
| `pnpm exec playwright test` | PASS, FLIP 20/20. |
| `E2E_APP=fman pnpm exec playwright test` | PASS, FMan 35/35. |

The first unit-test invocation hit a sandbox `EPERM` while Vite created temporary config bundles. The same command passed after the runner allowed temporary worktree writes. Polling tests emitted React `act(...)` warnings.

### A5 coverage collector

`pnpm run test:coverage` passed. The independent run reproduced 80.95% weighted statement coverage.

| Package | Statements |
| --- | ---: |
| Fleet Manager | 84.08% |
| Liquidity Provider | 79.33% |
| shared-ui | 81.30% |
| mock-devtools | 90.00% |
| types | 0.00% |

The types package contains almost entirely erased type declarations. Thresholds remain absent by design.

## Contract-fixture regeneration

The host did not provide `just`, so QA ran the exact command from the `gen-contract-fixtures` recipe in a disposable clone:

```bash
nix develop --command cargo run \
  --package fedi-decentralized-service-liquidity-manager \
  --bin gen_contract_fixtures
```

The generator wrote all six fixtures. A recursive byte comparison against head returned exit 0 with no differences.

The Rust drift test also passed:

```text
3 passed; 0 failed; 0 ignored
```

B1 matches the backend contract:

- Rust defines `Timestamp(pub u64)` at `/Users/kc/Projects/decentralized-federations-remediation/crates/domain/src/lib.rs:64-67`.
- TypeScript defines `Timestamp = number` at `/Users/kc/Projects/decentralized-federations-remediation/operator-ui/packages/types/src/admin.ts:21`.
- The timestamp formatters convert Unix seconds with `ts * 1000`.
- The fixture and mock sweep found numeric values rather than numeric strings or ISO wire literals.

## Ledger task disposition

| Task | QA disposition |
| --- | --- |
| A1 | **Failed.** The workflow invokes the red CSS duplicate gate. Draft-PR behavior remains unverified. |
| A2 | Verified. |
| A3 | Verified. |
| A4 | **Partial.** Generated fixtures match Rust; backup MSW still hand-copies its manifest. |
| A5 partial | JavaScript collector and 80.95% baseline verified. Thresholds remain deferred. |
| A6 partial | **Incomplete beyond the disclosed deferral.** The advertisement live-write cycle is absent. |
| B1 | Verified. |
| B2 | Verified as an interim honesty fix. |
| B3 | Functional acceptance verified; focus handling regressed. |
| B4 | Verified for Funds. |
| B5 | **Failed the full polling contract.** |
| B6 | Verified. The guard lives inside the handler. |
| B7 | **Partial.** HTTP classification is correct; app-wide propagation is missing. |
| B9 | Verified. Setup hides navigation and `MockPanelMount` stays above the gate. |
| C1 | Verified. The guard lives inside the hook handler. |
| C2 | **Partial.** Cache-independent gating works only when Onboarding receives the 401. |
| C3 | **Partial.** GuardianFees loading is omitted; the final presentation remains unchanged. |
| C4 | **Partial.** FormField production wiring is incomplete. |
| C5 | Verified. |
| C7 | Verified, including conditional per-seat polling and screen-scoped fee polling. |

## Ledger claims QA could not verify

- A1's three draft-PR trigger cases and required branch-protection contexts. The branch has no push or PR.
- A6 behavior against real daemons. Both `@live` specs remain unrun.
- Firefox and WebKit pre-release execution. The config declares the projects, but no CI job or recorded execution exists.
- Rust coverage output. The recipe exists and remains unrun by design.
- Failing-test-first history. Commits that contain both tests and implementation do not preserve the prior red result.
- Absolute absence of `--no-verify`. Git does not record commit invocation flags. The remediation ledger and reports contain no such evidence. Inherited pre-base planning documents mention `--no-verify`, but this branch did not modify those files.

## Assessment coverage and regressions

The plan assigns every P1/P2 assessment finding to a task or an explicit backend decision. The browser-isolation finding was addressed in the base branch; both mocked suites passed in this review.

The branch introduces or increases exposure to these behaviors outside the intended acceptance result:

- B5 starts periodic polling on views that blank cached data after a transient error.
- B3 removes the focused withdrawal trigger without placing focus in the confirmation panel.
- C7 makes the existing GuardianFees stale-data caveat mismatch easier to reach: a refetch error can retain cached fee data while the page says none was counted for that seat.
- B7 adds a component that violates the repository's folder and test rules while the checker misses committed files.

No other unrelated behavioral regression was found in `77a049c7...3885fa65`.

## Deferral sanity check

- B8 and D5 remain deferred. No stale Requests documentation changed.
- C6 remains deferred. `QR_SIZE = 192`, `min-w-[5rem]`, the price-form duplication, and eight CSS duplicate groups remain.
- Track D remains deferred. The branch does not change FLIP backup transport, OCI bind policy, `SECURITY.md`, the FMan packaged entrypoint, or backend onboarding transport.
- A5 remains partial as disclosed: collectors exist, thresholds and CI publication do not.
- A6 remains partial: browser projects and live specs exist, but no pre-release job or real-daemon run exists. The missing advertisement cycle is an additional defect.

## Branch and worktree hygiene

- `git status --short --branch` reported a clean tree at `3885fa65` after all checks.
- `git ls-remote --heads origin refs/heads/feat/operator-ui-remediation` returned no ref with exit 0. The branch is not pushed.
- Base `77a049c7` equals local `feat/msw-mock-migration` and is the merge-base with head.
- The remediation range contains 29 commits and 173 changed paths.
- `master..77a049c7` contains 230 stacked commits. Using `77a049c7...HEAD` excludes that pre-existing work from remediation attribution. A comparison against master would include 740 changed paths.
- Hygiene commit `a16ebc3a` touches only `.typos.toml` and `operator-ui/apps/liquidity-provider/package.json`.
- The remaining paths map to the remediation tasks, their tests, generated fixtures, mocks, coverage configuration, or gate configuration. QA found no unrelated path outside that scope.
