# CLAIM-manifold-guardian-payout-request-idempotent: Payout request IDs fence outgoing operations

For payment-federation and guardian-fee payouts, one MG data root cannot
commit more than one native outgoing operation for the same caller-generated
request ID, including across concurrent retries, process crashes, restarts, and
lost responses. Any operation committed before a crash remains discoverable by
that request ID after restart.

## Status

Unverified.

## Assumptions

- At most one MG process owns a data root at a time.
- SQLite and Fedimint wallet commits acknowledged by their database APIs survive
  process restart, and the fleet database and wallet databases are restored as
  one consistent data root.
- A successful pinned Fedimint payout start has durably committed its operation
  metadata before returning, and durable operation enumeration returns that
  metadata after restart.
