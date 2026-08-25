# Current argument

## Argument

**L1 (`code`) — unauthorised callers can form an authenticated probe.** Public
request authentication only requires a Schnorr signature made by the
request's self-declared `requester_pubkey` and the configured provider key;
the public Iroh router supplies no further caller authorization. The adversary
therefore signs a request under its own fresh key, names the known target
federation id, and supplies an arbitrary 32-byte `details_payload_hash`; no
allocation requester credential, correct commitment, or FMan endorsement is
required before the first allocation lookup.

**L2 (`code`) — the federation-only lookup precedes privacy-relevant gates.**
After public-signature verification, loading setup, and snapshotting
readiness, `accept_or_reject_request` calls `respond_from_existing_allocation`
before expiry, commitment, readiness, or endorsement validation. That helper
queries `allocations` by `federation_id` alone. If a row exists, an arbitrary
hash either differs and returns signed `request_conflict`, or coincides and
returns signed `accepted`; it never compares `requester_pubkey`. Neither case
requires the probe to pass prevalidation.

**L3 (`code` + concrete execution) — the response distinguishes the row.** For
an existing allocation, L2 returns either `request_conflict` or `accepted`,
both revealing the row. For an absent allocation, an intentionally
noncanonical arbitrary hash reaches signed `invalid_details_payload`; the only
production `request_conflict` constructor is the existing-row branch. By A1
the caller distinguishes the cases without needing to guess the stored hash.
This falsifies the claim.

## Residual windows

Both disclosures below are open to any holder of a valid unrevoked FMan
endorsement for the target federation and lie outside this claim's
unauthenticated adversary.

Accepting these costs nothing further, because the endorsement already buys the
allocation itself. `SPEC-flip-rpc` records that at
most one allocation exists per federation and one item per source type, so a
disclosed endorsement is worth a single federation's allocation rather than
repeated draw-down; learning that this allocation exists is strictly less than
taking it. The decision also fixes the boundary: the endorsement is a bearer
object naming a federation and never a requester. Closing these channels would
require changing that bearer-authorization contract, not merely rearranging
the existence lookup.

The channels exist for valid endorsement holders. If possession stops being
authorization, they enter the claim's adversary model and become
counterexamples.

## Weakest links

1. **L2 (`code`)** — fast-path ordering and query predicate must be regenerated
   after public idempotency changes.
2. **L1 (`code`)** — public signature authentication must remain
   self-declared-key verification.
3. **A1–A2 (`axiom`)** — observable response codes and known federation ids.
