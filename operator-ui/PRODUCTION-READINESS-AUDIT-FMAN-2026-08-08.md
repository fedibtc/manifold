# FMan dashboard production-readiness audit

**Date:** 2026-08-08
**Branch assessed:** `feat/msw-mock-migration`
**Assessed commit:** `0539876b`
**Comparison point:** `origin/master` (`2c4da7…`)
**Owner scope:** Fleet Manager dashboard, its shared UI usage, FMan Rust admin
integration, packaging, tests, and release gates.

> **Status (2026-08-08):** severities and remediation order superseded by the
> verified assessment —
> [AUDIT-ASSESSMENT-FMAN-2026-08-08.md](./AUDIT-ASSESSMENT-FMAN-2026-08-08.md).
> Key corrections there: the onboarding fix additionally requires a
> setup-state verb and packaging changes; the offer-form error finding is
> upgraded to P1 (destructive save); the boot.spec structure complaint is
> withdrawn.

## Verdict

The FMan dashboard is **not production-ready**.

Its unit tests, typecheck, and production build pass. The release blocker is at
the real Rust/browser boundary: a fresh host offers only Unix-socket onboarding,
while the browser wizard needs an HTTP endpoint that starts only after
onboarding. The live test begins with an already-onboarded daemon, so it cannot
detect this failure.

There is also no measured frontend or Rust coverage threshold enforced by CI.

## Findings and remedial actions

### P1 — Fresh-install browser onboarding is unreachable

**Finding**

On a host without an identity, FMan waits for onboarding through a Unix socket:

- `crates/fman/bin/src/main.rs:373-395`
- `crates/fman/core/src/onboarding.rs:60-91`

The HTTP admin service used by the dashboard starts only after onboarding and
runtime initialization:

- `crates/fman/bin/src/main.rs:495-549`

The browser wizard sends `OnboardAsNew` and `OnboardFromBackup` to `/api/admin`:

- `operator-ui/apps/fleet-manager/src/features/setup/api/hooks/use-onboard-as-new/useOnboardAsNew.ts:5-11`

The production entrypoint does not pass HTTP-admin options, and the live test
starts from an already-onboarded daemon:

- `packages/fleet-manager/entrypoint.sh:14-21`
- `operator-ui/e2e/fman/live-daemon.spec.ts:4-24`

This does not satisfy the first-run workflow in
`docs/operator-dashboards/fleet-manager/mvp/MVP-SPEC.md:71-79,116-120`.

**Remedial action**

1. Agree one supported onboarding transport for production.
2. If browser onboarding remains required, start a tightly restricted bootstrap
   HTTP service before identity creation. Expose only the minimum onboarding
   commands and replace it with the normal authenticated admin service after
   onboarding.
3. Update packaging and deployment documentation to expose the chosen endpoint
   safely.
4. Add a live browser test that starts with an empty data directory, creates or
   restores identity through the UI, and reaches an authenticated operational
   screen.

**Acceptance criteria**

- A packaged, fresh FMan can complete browser onboarding without a separate CLI
  or Unix-socket step.
- The pre-identity endpoint exposes no normal privileged admin operations.
- A live test proves the complete journey from an empty data directory.

### P2 — Mid-session authentication expiry can be hidden by cached data

**Finding**

The boot-status hook only routes to the authentication error state when no cached
data exists. A later 401/403 can therefore leave stale privileged data on screen:

- `operator-ui/apps/fleet-manager/src/features/boot/hooks/use-boot-status/useBootStatus.ts:29-38`

**Remedial action**

1. Handle authentication failure independently of cache presence.
2. Centralize 401/403 handling at the API/query boundary.
3. Clear or quarantine privileged cached data when a session expires.
4. Test expiry after a successful fetch and during a mutation.

**Acceptance criteria**

- Any expired session returns the operator to authentication without presenting
  cached privileged data as current.

### P2 — Query failures are rendered as valid business states

**Finding**

Several FMan views collapse loading or failure into ordinary domain states:

- setup authorization reports "Waiting":
  `operator-ui/apps/fleet-manager/src/features/setup/components/setup-authorization/SetupAuthorization.tsx:13`
- offer loading is exposed but offer error is discarded:
  `operator-ui/apps/fleet-manager/src/features/offer/hooks/use-offer-form/useOfferForm.ts:51`
- backup renders `—` for loading and failure:
  `operator-ui/apps/fleet-manager/src/pages/backup/BackupPage.tsx:25`
- overview derives zero totals while queries settle or fail:
  `operator-ui/apps/fleet-manager/src/pages/overview/OverviewPage.tsx:15`

An integration outage can therefore look like healthy zero, blank, or waiting
data. This violates the documented `REACT-009` state model.

**Remedial action**

1. Model loading, error, empty, stale, and populated states explicitly.
2. Preserve structured query errors through feature hooks.
3. Add component tests for every state and a browser test for backend
   unavailability.

**Acceptance criteria**

- No backend failure is presented as zero, empty, or waiting without a visible
  error or stale-data indication.

### P2 — FMan release coverage is not measured or enforced

**Finding**

No Vitest or Rust coverage threshold is configured. Operator UI checks are not an
enforced root release gate. The live FMan tier covers only one already-onboarded
read path; it does not cover fresh onboarding, restore, live writes, or
mid-session expiry.

**Remedial action**

1. Collect statement, branch, function, and line coverage for the FMan app and
   the shared packages it consumes.
2. Collect `cargo llvm-cov` results for the FMan admin/API/domain release surface.
3. Measure the initial baseline, then enforce non-regression and agreed minimum
   thresholds. Do not choose a percentage before measuring.
4. Require FMan typecheck, unit tests, lint/boundary checks, production build,
   mocked browser tests, focused live tests, and Rust checks in CI.
5. Add live coverage for first-run onboarding, restore, at least one write path,
   backend failure, and authentication expiry.

**Acceptance criteria**

- CI publishes FMan frontend and Rust coverage and fails below agreed thresholds.
- Required FMan workflows are verified against a packaged real daemon.
- The release decision uses branch coverage and workflow risk, not test count.

### P2 — Browser tests show isolation risk

**Finding**

The mocked FMan suite initially passed 33 of 34 cases. The failing
`wallet-not-receivable` case displayed data from the previous `seats-mixed` case
and passed alone in clean CI mode. A later full clean run was blocked by an
existing process on port 8787.

**Remedial action**

1. Give every test isolated browser storage, mock state, server state, and ports.
2. Parameterize data-driven cases rather than looping inside one ordinary test.
3. Make server startup fail clearly and clean up only resources owned by the
   suite.
4. Run Chromium on every change and Firefox/WebKit in the pre-release gate.

**Acceptance criteria**

- The full suite is repeatable and independent of case order or prior runs.
- Pre-release browser coverage passes on Chromium, Firefox, and WebKit.

### P3 — Shared form and stepper accessibility needs correction

**Finding**

Form primitives used by FMan render help/error text without
`aria-describedby`, `aria-invalid`, or a live status association:

- `operator-ui/packages/shared-ui/src/components/text-input/TextInput.tsx:39`
- `operator-ui/packages/shared-ui/src/components/select-field/SelectField.tsx:40`
- `operator-ui/packages/shared-ui/src/components/form-field/FormField.tsx:17`

The shared stepper lacks list/progress semantics and `aria-current="step"`.
Completed/upcoming state relies on green versus grey:

- `operator-ui/packages/shared-ui/src/components/stepper/Stepper.tsx:20`
- `operator-ui/packages/shared-ui/src/components/stepper/Stepper.module.css:15`

**Remedial action**

1. Give hint/error elements stable IDs and associate them with their controls.
2. Set `aria-invalid` and announce newly displayed validation errors.
3. Represent steps as an ordered list, mark the current step, and add a non-color
   status signal.
4. Add automated accessibility assertions and keyboard/screen-reader checks.

**Acceptance criteria**

- Form errors are programmatically associated with inputs and announced.
- Step identity and state remain understandable without color.

### P3 — Styling, duplication, and test structure are not clean

**Finding**

Hard-coded design values bypass shared tokens, the boot E2E loops over cases
inside one test, and setup-price behavior duplicates offer-price behavior:

- `operator-ui/apps/fleet-manager/src/features/setup/components/setup-authorization/SetupAuthorization.tsx:7`
- `operator-ui/packages/shared-ui/src/components/stepper/Stepper.module.css:8`
- `operator-ui/packages/shared-ui/styles/utilities.css:107`
- `operator-ui/e2e/fman/boot.spec.ts:23`

`check-styles` and `check-structure` passed, but `lint:css-dupes` reported eight
duplicate groups across the operator UI.

**Remedial action**

1. Replace hard-coded values with documented Fedi tokens.
2. Parameterize boot cases so failures and state are isolated.
3. Extract a single price-editor hook/presentation seam for setup and offers.
4. Resolve or narrowly document each relevant CSS duplication.

**Acceptance criteria**

- FMan and shared-package lint, format, boundary, and CSS checks pass in CI.

## FMan verification evidence

| Check | Result | Notes |
| --- | --- | --- |
| FMan unit tests | Pass | 203 tests. |
| Shared UI unit tests | Pass | 15 tests. |
| Typecheck | Pass | FMan and shared packages. |
| Production build | Pass | 411.20 kB raw / 131.58 kB gzip JavaScript. |
| React compiler lint | Pass | No reported compiler-lint failures. |
| Mocked Playwright | Unstable | 33/34 initially; failed case passed alone in clean mode. |
| FMan live Playwright | Inadequate | One already-onboarded read path only. |
| `operator-ui-auth` Rust tests | Pass | 3/3. |
| FMan core Rust tests | Flaky | 133/134 initially; supervisor timing failure passed on exact rerun. |
| Measured coverage | Not available | No configured collector, report, or threshold. |

## Recommended FMan remediation order

1. Agree the production onboarding transport and security boundary.
2. Make fresh-host browser onboarding reachable and prove it with a live test.
3. Fix authentication expiry and explicit async states.
4. Stabilize and isolate the mocked browser suite.
5. Correct shared form/stepper accessibility and FMan styling violations.
6. Add measured frontend/Rust coverage and required CI gates.
7. Run the packaged live browser matrix before release approval.

## Open questions for the FMan Rust/backend team

### Onboarding and packaging

1. Is browser onboarding on a completely fresh host a committed production
   requirement, or should the product specification require a local CLI or
   Unix-socket bootstrap?
2. If browser onboarding is required, what authentication or physical-presence
   mechanism protects the temporary pre-identity HTTP endpoint?
3. Which commands may exist before identity creation, and how will Rust guarantee
   that the rest of the admin API is unreachable in that phase?
4. Should the bootstrap listener be loopback-only, private-network capable, or
   available only behind a reverse proxy? Who owns that proxy configuration?
5. What is the intended packaged deployment topology? The current entrypoint does
   not expose the HTTP admin service.
6. How should interrupted onboarding recover without leaving a partial identity
   or an indefinitely exposed bootstrap endpoint?

### Authentication contract

1. What exact HTTP response identifies an expired versus invalid credential?
2. Are bearer tokens rotated or revoked, and should expiry clear all cached admin
   state?
3. Is there a server-provided session lifetime the UI should display or use for
   proactive re-authentication?

### Rust coverage and release policy

1. Which FMan crates and features constitute the dashboard release surface?
2. Should the first coverage gate use absolute line/branch thresholds,
   non-regression from a measured baseline, or both?
3. Which real-daemon tests belong on every pull request, and which belong in a
   slower pre-release tier?
4. Who owns cross-language contract fixtures when a Rust response changes?
5. Should the timing-sensitive supervisor test use an observable readiness signal
   rather than retry or timing tolerance?

## FMan production release exit criteria

- Fresh onboarding is either working through the packaged browser flow or
  explicitly removed from the active product scope with product/backend
  agreement.
- The fresh-host journey passes against a packaged real daemon.
- Authentication expiry and backend outages are distinct from valid empty,
  waiting, or zero states.
- Shared controls used by FMan meet accessibility requirements.
- All FMan lint, typecheck, unit, build, mocked E2E, live E2E, and Rust checks pass.
- FMan frontend and Rust coverage is measured, published, and enforced.
- Chromium, Firefox, and WebKit pre-release runs pass without shared-state or
  test-order failures.
