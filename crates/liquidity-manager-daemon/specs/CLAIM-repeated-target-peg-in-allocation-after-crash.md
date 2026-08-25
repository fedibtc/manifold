# CLAIM-repeated-target-peg-in-allocation-after-crash: Repeated target peg in allocation after crash

For one durable, active stability-pool allocation item `I`, repeated ordinary
crashes cannot make FLIP allocate more than one target-Fedimint peg-in
operation/address for `I`.

The adversary may hard-crash the one official daemon after any external return,
local assignment, SQLite statement, or SQLite commit; restart it serially; and
repeat that schedule. It cannot edit or roll back SQLite or the target-client
store, run a second daemon, alter setup, or make the target federation
malicious. This claim concerns allocation of the target peg-in
operation/address only. It ends before provider-wallet funding and before
`deposit_to_provide`.

## Status

Unverified.

## Assumptions

- **A1 — target allocation durability.** Each successful
  `allocate_peg_in_address` call may create a distinct durable target-Fedimint
  peg-in operation/address, even when its `FundingTargetRecord` is the same as
  an earlier call. A later call need not discover or reuse the earlier operation.
- **A2 — ordinary durable stores.** A successful target-client allocation remains
  durable across an ordinary daemon crash, while a SQLite write that did not
  commit leaves the old row durable. A restart can reopen both stores. Hardware
  loss, store corruption, and rollback are not used.
- **A3 — one official process.** One daemon owns the data directory and its
  target-client root. Its startup recovery and periodic worker run normally
  after each restart.
