# Falsification: Claim worker obtains a database seat ID

## Counterexample

Grant every assumption of
[CLAIM-fleet-manager-selects-only-owned-seat-authority](../CLAIM-fleet-manager-selects-only-owned-seat-authority.md).
After a paid seat has been accepted, the background ecash claim worker reads
`pending_ecash_claims` through the shared Fleet Manager database, obtains the
persisted payment record's `seat_id`, and passes that ID to
`record_claim_outcome`.

This contradicts the claim's explicit statement that payment settlement
continuations obtain no Fleet Manager database seat ID. It does not show that
the worker can select a `Seat`, operate its local control plane, or use an ID
unrelated to the accepted payment record.

## Current source

[`fman-fedimint`'s `claim_worker`](../../fedimint/src/claim_worker.rs) reads the
pending claim records and carries `PaymentRecord.seat_id` into outcome
recording. [The Fleet Manager database](../../core/src/db/seats.rs) materializes
that field from `pending_ecash_claims` and updates the payment row by the same
ID.

The counterexample remains current while the worker obtains and uses that
database seat ID or until the claim is deliberately narrowed to distinguish a
payment-row key from local seat authority.
