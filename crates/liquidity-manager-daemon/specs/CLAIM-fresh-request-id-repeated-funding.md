# CLAIM-fresh-request-id-repeated-funding: One semantic request has one accepted allocation

For one authenticated FI key and one semantic liquidity intent, FLIP accepts
and creates at most one independent allocation, including when the operator
uses the documented live restore operation.

## Status

Falsified: after an accepted allocation is cancelled before funding and released
through the official Admin operation, replaying the same signed semantic request
creates another independent allocation
([evidence](CLAIM-fresh-request-id-repeated-funding/falsification-release-erases-identity.md)).

## Assumptions

- SQLite commits, checkpoints, close/reopen, filesystem moves, and the
  documented backup and live-restore operations complete as documented; one
  daemon owns the data root.
- A configured deployment has one correctly signed request that passes every
  acceptance gate when it is first accepted.
- The authenticated FI and Admin callers use only their documented interfaces,
  and canonical encoding, hashes, and Schnorr verification behave as documented.
