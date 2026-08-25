# CLAIM-unendorsed-federation-allocation: Unendorsed federation allocation

No production `RequestLiquidity` delivery can newly commit an `allocations` row
unless, during that delivery before the commit, FLIP has accepted an FMan
endorsement for the exact federation later stored in that row. "Accepted" means
all of the following held at the admission point:

1. the endorsement's signed seat attestation verified;
2. the attestation's federation id equalled the federation id parsed from the
   request's invite code;
3. its FMan held a cryptographically valid badge from an issuer in FLIP's
   installed trust authorities, bound to that FMan identity; and
4. that authentic badge's trust level met the selected environment profile's
   schema-valid minimum; and
5. the required revocation lookup completed and did not report that badge
   revoked; and
6. FLIP repeated credential verification with a fresh local clock sample after
   that lookup completed.

The minimum predicate is governed by
[`REQ-fman-trusted-peer-badge`](../../../specs/REQ-fman-trusted-peer-badge.md),
[`SPEC-manifold-environment`](../../manifold-environment/specs/SPEC-manifold-environment.md),
and the shared deterministic policy described by
[`SPEC-holder-trust-envelope`](../../domain/specs/SPEC-holder-trust-envelope.md).

The adversary controls request fields, signatures for arbitrary self-generated
requester keys, endorsement replay/sharing, request delivery and concurrency,
and crashes at every await and SQLite commit boundary. It cannot forge a valid
attestation or credential, compromise a configured issuer/revocation source, or
write the daemon database directly.

This is intentionally a **bearer-capability admission** property. It does not
bind the requester or transport actor to the federation, give the endorsement
an expiry, or revoke work already accepted before a later revocation. Those are
outside this claim's bearer-endorsement authorization scope.

## Status

Unverified.

## Assumptions

- **A1 — cryptographic and canonical-verification soundness.** The pinned
  attestation, credential, and canonical-request verification routines accept
  only authentically signed values under their documented algorithms.
- **A2 — trust-source soundness.** Installed issuer authorities and a successful
  revocation lookup accurately state the configured issuer authority and the
  relevant credential's revocation state when that lookup responds.
- **A3 — SQLite/process integrity.** SQLite transactions are atomic and durable,
  the official daemon is the only writer of its data root, and a crash preserves
  committed transactions but not uncommitted writes.
