# FLIP dashboard production-readiness audit

**Date:** 2026-08-08
**Branch assessed:** `feat/msw-mock-migration`
**Assessed commit:** `0539876b`
**Comparison point:** `origin/master` (`2c4da7…`)
**Owner scope:** Liquidity Provider dashboard, FLIP Rust admin integration,
financial and advertisement actions, tests, and release gates.

> **Status (2026-08-08):** severities and remediation order superseded by the
> verified assessment —
> [AUDIT-ASSESSMENT-FLIP-2026-08-08.md](./AUDIT-ASSESSMENT-FLIP-2026-08-08.md).
> Key corrections there: the Requests P1 is withdrawn (feature deliberately
> deleted in `e32a0a40`); the withdrawal and advertisement P1s are downgraded
> (the daemon enforces the invariants); the network-locality finding gains
> the OCI image-definition override and loses the "arbitrary default" framing.

## Verdict

The FLIP dashboard is **not production-ready**.

The frontend unit tests, typecheck, and production build pass, and focused Rust
admin tests are healthy. Real Rust/browser contracts still break backup/restore
and timestamp rendering. Financial and advertisement actions also lack required
safety behavior, and the Requests MVP is absent end to end.

There is no measured frontend or Rust coverage threshold enforced by CI.

## Findings and remedial actions

### P1 — Backup and restore use incompatible transport models

**Finding**

Rust creates a `.tar.gz` on the daemon filesystem and returns its path. Inspect
and restore also accept a filesystem path:

- `crates/liquidity-manager-daemon/src/backup.rs:42-61,170-191`

The dashboard downloads the returned path string as a text file:

- `operator-ui/apps/liquidity-provider/src/features/settings/components/backup-card/BackupCard.tsx:7-16`
- `operator-ui/apps/liquidity-provider/src/shared/utils/downloadTextFile.ts:1-10`

The restore screen asks the operator to paste "archive contents" and submits that
text as `archive`:

- `operator-ui/apps/liquidity-provider/src/pages/restore-console/RestoreConsolePage.tsx:20-35,86-100`

The downloaded file is therefore only the daemon-local pathname, not the backup
archive. Restore on a different host cannot work.

**Remedial action**

1. Define a single backup-transfer contract. For browsers, prefer an
   authenticated binary download with an explicit archive content type and
   filename.
2. Define restore as an authenticated streaming upload with explicit size limits.
3. Keep path-based operations as local/CLI APIs only if still needed.
4. Validate archive format and metadata before applying a restore. Make failure
   atomic and preserve current state.
5. Add a cross-host live test: create on instance A, transfer the bytes, restore
   instance B, and verify state.

**Acceptance criteria**

- The browser downloads a real, inspectable `.tar.gz`.
- The archive restores a different fresh host without a shared filesystem.
- Invalid, oversized, or partial uploads fail safely and visibly.

### P1 — Rust and TypeScript timestamp contracts disagree

**Finding**

Rust's transparent `Timestamp` is a `u64` containing Unix seconds:

- `crates/domain/src/lib.rs:64-67`

TypeScript declares timestamps as strings, and mock fixtures use strings:

- `operator-ui/packages/types/src/admin.ts:21`
- `operator-ui/packages/types/src/paging.ts:13-15`
- `operator-ui/packages/mock-fixtures/src/health.ts:6-24`

The attestation formatter calls `.slice()` and throws on a real numeric value:

- `operator-ui/apps/liquidity-provider/src/shared/utils/format.ts:13-14`
- `operator-ui/apps/liquidity-provider/src/features/attestations/components/attestation-panel/AttestationPanel.tsx:93-104`

Advertisement formatting uses `Date.parse`; numeric Unix seconds render as `—`:

- `operator-ui/apps/liquidity-provider/src/features/advertisement/services/format.ts:14-18,45-56`

TypeScript also omits Rust's `background_workers` health group and defines a
`relay_cursors` backup group that does not exist in the Rust enum.

**Remedial action**

1. Choose one canonical timestamp wire representation: numeric Unix seconds or a
   documented RFC 3339 string.
2. Update Rust serialization, TypeScript types, formatters, and mocks together.
3. Generate TypeScript wire types from Rust/OpenAPI, or check contract fixtures
   produced by the real Rust serializer.
4. Add boundary tests for zero, current, far-future, malformed, and absent values.
5. Reconcile every health and backup field against the Rust response types.

**Acceptance criteria**

- A Rust-serialized response is consumed directly by frontend contract tests.
- Attestation and advertisement views render real daemon timestamps correctly.
- Checked schemas prevent silent Rust/TypeScript drift.

### P1 — Required Requests workflow is absent

**Finding**

The Requests MVP requires list, detail, filter, status, and action behavior:

- `docs/operator-dashboards/liquidity-manager/mvp/user-stories/06-requests.md:9-63`

No Requests route exists in the SPA, and Rust's admin HTTP surface lacks list and
get request endpoints:

- `operator-ui/apps/liquidity-provider/src/app/App.tsx:23-28`
- `crates/liquidity-manager-daemon/src/admin_http.rs:94-100`

**Remedial action**

1. Confirm whether Requests is a release requirement or explicitly defer it in
   the active MVP scope.
2. If required, add Rust list/detail endpoints with pagination and stable status
   values, then implement the frontend route and state handling.
3. Add Rust contract tests and live browser coverage for populated and empty
   request states.

**Acceptance criteria**

- The active MVP specification and delivered routes/API agree.
- Required request operations work against a real daemon.

### P1 — Withdrawal input can submit invalid or empty values

**Finding**

`FundsActions` converts input with `Number(value) || 0` and can submit an empty
address and zero amount:

- `operator-ui/apps/liquidity-provider/src/features/funds/components/funds-actions/FundsActions.tsx:20-27,52-57`

Its success test submits blank fields:

- `operator-ui/apps/liquidity-provider/src/features/funds/components/funds-actions/FundsActions.test.tsx:76-83`

This misses address/network, positive whole amount, balance, and fee validation.

**Remedial action**

1. Add explicit client validation for address, network, positive whole-unit
   amount, available balance, and fee constraints.
2. Enforce the same invariants in Rust; frontend validation is only feedback.
3. Disable submission while invalid or pending and preserve backend error detail.
4. Add table-driven invalid cases and a controlled live successful-withdrawal
   test.

**Acceptance criteria**

- Invalid values cannot reach the frontend mutation.
- Rust independently rejects the same invalid values.
- Tests prove both rejection paths and one real successful path.

### P1 — Advertisement actions bypass readiness and confirmation

**Finding**

Republish always sends `force: true`:

- `operator-ui/apps/liquidity-provider/src/features/advertisement/api/hooks/useRepublishAdvertisement.ts:12-16`

The page enables republish regardless of readiness and withdraws immediately even
though its copy promises confirmation:

- `operator-ui/apps/liquidity-provider/src/pages/advertisement/AdvertisementPage.tsx:41-43,100-116`

No live browser test covers advertisement withdrawal.

**Remedial action**

1. Default routine publication to `force: false`; reserve force for a separate,
   explicitly labelled recovery action.
2. Disable publication when readiness preconditions fail and show the reasons.
3. Require deliberate confirmation before withdrawal.
4. Add frontend, Rust authorization/effect, and live tests for normal publish,
   blocked publish, forced recovery, cancellation, and confirmed withdrawal.

**Acceptance criteria**

- Routine UI paths cannot silently force publication.
- Destructive withdrawal requires confirmation.
- Both UI and Rust enforce readiness and authorization rules.

### P2 — Mid-session authentication expiry can be hidden by cached data

**Finding**

The boot-status hook only routes to authentication failure when no cached data
exists. A later 401/403 can leave stale privileged data on screen:

- `operator-ui/apps/liquidity-provider/src/features/boot/hooks/use-boot-status/useBootStatus.ts:20-36`

**Remedial action**

1. Handle authentication failure independently of cache presence.
2. Centralize 401/403 handling at the API/query boundary.
3. Clear or quarantine privileged cached data on expiry.
4. Test expiry after a successful fetch and during a mutation.

**Acceptance criteria**

- An expired session always returns the operator to authentication without
  presenting cached privileged data as current.

### P2 — Polling and stale-data behavior do not meet the contract

**Finding**

Allocations do not poll, while funds and advertisement poll every 60 seconds
rather than the required cadence:

- `operator-ui/apps/liquidity-provider/src/features/allocations/api/hooks/useAllocations.ts:16-20`

The funds page replaces last-good data with an error instead of marking it stale.

**Remedial action**

1. Put refresh intervals in a shared, documented policy.
2. Preserve last-good data on transient failure, label it stale, show its fetch
   time, and offer retry.
3. Add fake-timer tests for cadence, pause/resume, stale transition, and recovery.

**Acceptance criteria**

- Every live view meets its specified freshness target.
- Current, loading, stale, and failed data are visibly distinct.

### P2 — Admin network locality is not enforced

**Finding**

The daemon accepts an arbitrary `SocketAddr` and serves bearer-token
authentication over plaintext HTTP:

- `crates/liquidity-manager-daemon/src/config.rs:232-239,303-326`
- `crates/liquidity-manager-daemon/src/admin_http.rs:170-195,607-630`

`specs/SPEC-flip-admin-api.md:27-29` requires local/private binding unless a
separate access-control layer exists.

**Remedial action**

1. Reject non-loopback/non-private binds by default.
2. Require an explicit unsafe override or separately configured TLS/reverse-proxy
   boundary for wider exposure.
3. Fail startup with a clear diagnostic when locality is violated.
4. Test accepted and rejected binds, forwarded-header assumptions, token
   handling, and log redaction.

**Acceptance criteria**

- A default installation cannot expose bearer credentials over public plaintext
  HTTP.
- Wider exposure is deliberate, documented, and protected.

### P2 — FLIP release coverage is not measured or enforced

**Finding**

No Vitest or Rust coverage threshold is configured. Operator UI checks are not an
enforced root release gate. The live FLIP tier stops at a setup-gate read and does
not cover writes, binary backup transfer, restore, or numeric timestamps.

**Remedial action**

1. Collect statement, branch, function, and line coverage for FLIP and its shared
   packages.
2. Collect `cargo llvm-cov` results for the FLIP admin/API/domain release surface.
3. Measure the baseline, then enforce non-regression and agreed minimums.
4. Require FLIP typecheck, unit tests, lint/boundary checks, production build,
   mocked browser tests, focused live tests, and Rust checks in CI.
5. Add Rust-produced contract fixtures for every response used by TypeScript.
6. Add live coverage for backup/restore, timestamp rendering, withdrawal,
   publication, request management, and authentication expiry.

**Acceptance criteria**

- CI publishes FLIP frontend and Rust coverage and fails below agreed thresholds.
- Every P1 integration regression in this report is detected by a contract or
  live test.
- The release decision uses branch coverage and workflow risk, not test count.

### P3 — FLIP quality gates are not clean

**Finding**

Biome reported FLIP import/formatting failures, the boundary check found one
cross-feature import in `useProviderConfigForm.ts:4`, and the CSS duplication
check reported shared duplicate groups.

**Remedial action**

1. Fix formatting and import-order failures.
2. Move the provider-config dependency behind an allowed public feature boundary.
3. Resolve or narrowly document relevant CSS duplication.
4. Make all checks required in CI.

**Acceptance criteria**

- FLIP lint, format, boundary, and CSS checks pass in CI.

## FLIP verification evidence

| Check | Result | Notes |
| --- | --- | --- |
| FLIP unit tests | Pass | 306 tests. |
| Shared UI unit tests | Pass | 15 tests. |
| Typecheck | Pass | FLIP and shared packages. |
| Production build | Pass | 460.66 kB raw / 146.97 kB gzip JavaScript. |
| React compiler lint | Pass | No reported compiler-lint failures. |
| Mocked Playwright | Pass | 18/18. |
| FLIP live Playwright | Inadequate | Shallow setup/read coverage only. |
| `operator-ui-auth` Rust tests | Pass | 3/3. |
| FLIP daemon admin HTTP tests | Pass | 9/9 focused tests. |
| Service liquidity-manager tests | Pass | 13/13 library tests. |
| Measured coverage | Not available | No configured collector, report, or threshold. |

## Recommended FLIP remediation order

1. Agree canonical backup, timestamp, and admin-exposure contracts.
2. Fix timestamp/schema generation and add Rust-produced contract fixtures.
3. Implement binary backup download and restore upload, then prove cross-host
   restore.
4. Fix withdrawal and advertisement safety in both UI and Rust.
5. Reconcile Requests scope and implement or formally defer it.
6. Fix authentication expiry, polling, and stale-data behavior.
7. Clean FLIP lint/boundary failures and add measured coverage gates.
8. Run the packaged live browser matrix before release approval.

## Open questions for the FLIP Rust/backend team

### Backup and restore

1. Is the canonical admin API for local filesystem automation, remote browser
   operation, or both? Should those be separate endpoints?
2. Should archive bytes stream through the daemon, or should Rust issue a
   short-lived capability to another storage layer?
3. What archive size, timeout, and concurrency limits are safe?
4. Which data is sensitive, and should backups be encrypted at rest or protected
   with a user-supplied recovery secret?
5. Which compatibility metadata must Rust verify before restore: version,
   network, instance identity, checksum, or signature?
6. What is the atomicity model if restore fails halfway?
7. Must restore happen at startup/on an offline daemon, or can it be live?
8. Are path-based APIs still needed for CLI/container workflows?

### Timestamp and generated contracts

1. Should the wire representation remain Unix seconds as a JSON number, or change
   to RFC 3339?
2. If numeric seconds remain, does any consumer need millisecond precision or
   values beyond JavaScript's safe integer range?
3. Is seconds-based `Timestamp` consistent across all admin responses and paging
   cursors?
4. Which mechanism should be authoritative for TypeScript: OpenAPI, `ts-rs`, JSON
   Schema, or checked serializer fixtures?
5. Are `background_workers` and `relay_cursors` deliberate version skew, stale UI
   fields, or missing Rust fields?
6. Does changing the wire format require compatibility with existing clients or
   recorded fixtures?

### Requests API

1. Is Requests part of this release's committed backend scope?
2. What are the authoritative request statuses and allowed transitions?
3. What pagination, filtering, ordering, and retention behavior must the API
   guarantee?
4. Which request actions are privileged or irreversible, and how are they made
   idempotent?

### Withdrawal and advertisement invariants

1. Which address networks and formats must Rust accept, and which library is the
   validation source of truth?
2. Are withdrawal amounts always whole satoshis, and where are fee and dust limits
   enforced?
3. Must the service reserve balance between validation and broadcast to prevent
   concurrent overspending?
4. Which readiness conditions block normal advertisement publication?
5. When is `force: true` permitted, and should Rust require a distinct capability
   or command?
6. Is advertisement withdrawal idempotent, and what response represents "nothing
   published"?

### Admin API exposure and authentication

1. Is RFC1918 binding sufficient, or must the default be loopback/Unix socket?
2. What component terminates TLS for non-loopback deployments?
3. Should Rust reject public binds unconditionally or allow an explicit unsafe
   override?
4. Are bearer tokens rotated and revoked, and what exact response signals expiry?
5. Which proxy headers may the daemon trust, and how is spoofing prevented?

### Rust coverage and release policy

1. Which FLIP crates and features constitute the dashboard release surface?
2. Should the first gate use absolute line/branch thresholds, non-regression from
   a measured baseline, or both?
3. Which real-daemon tests run on each pull request versus the pre-release tier?
4. Who owns the cross-language contract suite when Rust response shapes change?

## FLIP production release exit criteria

- Every P1 finding is fixed or explicitly removed from active scope with
  product/backend agreement.
- Rust-produced payloads pass frontend contract tests without hand-authored type
  translation.
- Cross-host backup/restore passes against packaged real daemons.
- Financial and advertisement actions have backend-enforced invariants,
  confirmation where appropriate, and live coverage.
- Requests scope, Rust API, and delivered UI agree.
- Authentication expiry and stale data are handled explicitly.
- All FLIP lint, typecheck, unit, build, mocked E2E, live E2E, and Rust checks pass.
- FLIP frontend and Rust coverage is measured, published, and enforced.
- Chromium, Firefox, and WebKit pre-release runs pass.
