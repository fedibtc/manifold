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
  before returning, and a durable lookup by operation id returns that operation
  after restart.
- The pinned Fedimint client records a payout under a predictable operation id:
  a mint v1 or v2 send under the caller's id, refused in the transaction that
  takes the notes when that id already exists; a Lightning v2 send of a new
  invoice under the id derived from the invoice and attempt 0; and the first
  Lightning v1 payment of a payment hash under the id derived from that hash and
  attempt 1.
- No request was left started but not linked to its operation by a release
  before the ledger named payout operation ids.
