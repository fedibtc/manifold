# CLAIM-stability-target-client-retention-is-unbounded: Stability target client retention is unbounded

A hostile FI cannot make an official FLIP daemon retain an unbounded number of
**concurrently open** target Fedimint clients.

The FI can obtain valid endorsements for arbitrarily many distinct qualifying
federations and complete small stability allocations sequentially. Admin source
support and provider wallet operation are trusted.

## Status

Unverified.

## Assumptions

- **A1.** Each distinct accepted federation id can require a distinct target
  client/database to allocate a stability peg-in.
