# Current argument

## Argument

**L1 (`enum` + `code`) — the target effect precedes every durable FLIP
item-bound reference.** The one production stability allocation branch is
`process_stability_pool_item` in `stability_allocation.rs`. For an active item
whose `peg_in_address` or `peg_in_operation_id` is absent, its ordered effects
and writes are:

1. `mark_item_running` has already made the item locally active;
2. `StabilityPoolBackend::allocate_peg_in_address` returns a target operation
   and address;
3. `validate_deposit_address` checks that returned address;
4. the worker assigns the operation id, address, and waiting status only to its
   in-memory `item.step`;
5. `allocation_store::update_item_step` serializes that step and executes the
   SQLite `UPDATE`; and
6. only its successful SQLite commit makes the operation/address part of
   `allocation_items.step_json`.

The production backend reaches the FLIP-owned target client and calls
`WalletClientModule::allocate_deposit_address_expert_only`; it supplies no
allocation-item idempotency key. `allocation_store` has no independent
operation/address row, unique key, or transaction spanning that target call and
the later SQLite update.

**L2 (`code`) — crash preserves the allocation but not its local name.** By A1,
let the first call in L1(2) create target operation/address `P1`. Crash after
that return and before L1(6) commits (including during address validation,
in-memory assignment, JSON serialization, SQLite execution, or a failed SQLite
write). By A2 the target client retains `P1`, while `I.step_json` retains neither
its operation id nor its address. No target recovery path can recover an
unrecorded peg-in: `target_recovery` reads only an item step and operates on a
post-claimed recorded operation; it does not list or bind target peg-in
allocations.

**L3 (`enum` + `code` + `test`) — normal retry calls the allocator again.**
`ACTIVE_ITEM_STATUSES` keeps `I` eligible after L2; startup recovery inventories
that active row but does not itself dispatch it. Independently, the
`stability_pool_allocation` interval invokes
`process_stability_pool_allocations_with` every ten seconds. The reloaded step
still satisfies the same missing-address-or-operation guard in L1, so that
periodic retry calls `allocate_peg_in_address` again rather than querying `P1`.
Under A1 it may create distinct durable `P2`.

`crash_after_target_peg_in_allocation_reuses_the_same_target_operation` deterministically
models exactly this return-before-write crash point with a non-production backend:
it retains the first allocation externally, leaves the SQLite step unchanged,
then drives the ordinary retry only until it has persisted `P2`. It observes
`P1`, `P2`, and a persisted step naming only `P2`, while a test-only
pre-submission wallet pause proves zero provider-wallet submissions and zero
`deposit_to_provide` calls.

**L4 (`concrete execution`) — one item has two target operations.** Seed one
active item with a target client opened but no peg-in operation/address. Take
the L2 crash after `P1`, restart, and let L3 create `P2`. The same durable item
therefore allocated both `P1` and `P2`, contradicting the claim. Repeating the
same crash after each new allocation yields additional target operations for
that one item, but adds no different bad thing.

## Residual windows

- This record does not claim that either allocated address receives a provider
  wallet payment. The counterexample ends before provider-wallet funding and
  `deposit_to_provide`; duplicate deposits are a separate bad thing.
- A target implementation that durably reuses an allocation for repeated calls
  falls outside A1. The current call boundary supplies no such guarantee.
- A crash after the SQLite step commits is outside this witness because retry
  then sees the recorded operation/address.

## Weakest links

1. **A1 (`axiom`)** — the target client may make each allocation call a distinct
   durable operation/address.
2. **L1 (`enum` + `code`)** — every production allocation effect/write ordering
   and every durable FLIP item-bound reference.
3. **L3 (`enum` + `code` + `test`)** — all active-item retry triggers and the
   missing-step guard.
