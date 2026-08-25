# Operator dashboard production-readiness audit index

**Date:** 2026-08-08
**Branch assessed:** `feat/msw-mock-migration`
**Assessed commit:** `0539876b`
**Comparison point:** `origin/master` (`2c4da7…`)

> **Status (2026-08-08):** severity rankings and remediation orders in this
> audit set are superseded by the independently verified assessments —
> see [AUDIT-ASSESSMENT-2026-08-08.md](./AUDIT-ASSESSMENT-2026-08-08.md).
> Use the assessments as the remediation basis; this set remains the source
> of the raw findings and Rust-team question banks.

The audit is split by engineering ownership so FMan and FLIP developers can work
from separate finding, remediation, Rust-question, and release-exit lists.

## FMan dashboard

See [PRODUCTION-READINESS-AUDIT-FMAN-2026-08-08.md](./PRODUCTION-READINESS-AUDIT-FMAN-2026-08-08.md).

**Verdict:** Not production-ready.

Primary blocker: a packaged fresh host cannot complete the browser onboarding
flow because FMan exposes only Unix-socket onboarding before starting its HTTP
admin service.

## FLIP dashboard

See [PRODUCTION-READINESS-AUDIT-FLIP-2026-08-08.md](./PRODUCTION-READINESS-AUDIT-FLIP-2026-08-08.md).

**Verdict:** Not production-ready.

Primary blockers include incompatible browser/Rust backup transport, timestamp
contract drift, absent Requests functionality, and unsafe withdrawal and
advertisement action behavior.

## Shared release decision

Neither dashboard has measured frontend or Rust coverage thresholds enforced by
CI. Passing unit tests alone therefore cannot support a production-ready coverage
claim.

Shared-package work should be assigned explicitly during planning. The FMan audit
currently owns the detailed shared form/stepper accessibility findings because
those controls are central to its setup workflow. Each product audit owns its own
coverage, live-integration, and release criteria.
