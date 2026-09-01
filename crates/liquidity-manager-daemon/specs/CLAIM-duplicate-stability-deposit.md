# CLAIM-duplicate-stability-deposit: Duplicate stability deposit

For every official stability-pool allocation item with committed amount `C`,
crashes, lost responses, restarts, periodic retries, public request retries, and
concurrent operator retry, cancel, bind, or abandon calls cannot cause FLIP's
automatic worker to create more than one committed target-client provider-deposit
operation for that item, and that operation's amount cannot exceed `C`.

A committed target-client provider-deposit operation means one global operation-log
entry and its transaction-submission state machines committed under an operation ID.
The supported topology has one singleton daemon and one sequential stability
worker. The adversary controls every public request and operator-API field, but
cannot modify SQLite, the target-client database, or process memory directly.

## Status

Unverified.

## Assumptions

- **A1 store semantics:** committed SQLite and Fedimint client transactions are
  atomic and durable; uncommitted transactions have no effect.
- **A2 random identity:** `OperationId::new_random()` uses a cryptographically
  secure generator for independent 256-bit values. Equality between independently
  generated IDs is negligible.
- **A3 pinned client contract:** Fedi revision
  `2f35ea4e3b2516d35b8ed315455718cd3b336758` accepts a caller operation ID.
  Its client resolves against separately pinned Fedimint `v0.11.1-fedi18`
  (`5703f543f76746369f0a11e0d1635ac395b2efac`). The fedi18 delta changes
  DKG/version configuration only, outside this claim's client state machines.
  `finalize_and_submit_transaction_dbtx` rejects an existing ID, writes transaction
  submission state machines and the global operation-log entry in one database
  transaction, and performs no federation network submission before that
  transaction commits. The executor can submit the one committed transaction at
  least once, but cannot turn one transaction ID into two distinct transactions.
- **A4 official execution:** one official daemon owns each data directory while
  its `DaemonLock` is held; operators use its APIs rather than editing databases.
