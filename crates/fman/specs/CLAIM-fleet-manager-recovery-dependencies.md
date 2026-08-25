# CLAIM-fleet-manager-recovery-dependencies: Guardian recovery dependencies hold

This is an external recovery-input assumption, not a software-readiness result:
Fedimint peers retain enough correct consensus state for guardian recovery, and
the pinned Fedimint client, mint cryptography, iroh, SQLite, RocksDB,
filesystem, operating-system process isolation, and Nostr primitives satisfy
their documented contracts or expose a contract-visible failure to FMan, its
caller, or its operator.

It promises neither that the peer state exists nor that recovery completes,
persists, or meets a deadline after a dependency failure.

## Status

Unverified. The property now explicitly separates external recovery inputs and
visible failure contracts from FMan software readiness.

## Assumptions

- Fedimint peers retain enough correct consensus state for guardian recovery.
- The pinned Fedimint client, mint cryptography, and iroh satisfy their
  documented contracts or expose a contract-visible failure to FMan, its caller,
  or its operator.
- SQLite, RocksDB, and filesystem satisfy their documented contracts or expose
  a contract-visible failure to FMan, its caller, or its operator.
- Operating-system process isolation satisfies its documented contract or
  exposes a contract-visible failure to FMan, its caller, or its operator.
- Nostr primitives satisfy their documented contracts or expose a
  contract-visible failure to FMan, its caller, or its operator.
