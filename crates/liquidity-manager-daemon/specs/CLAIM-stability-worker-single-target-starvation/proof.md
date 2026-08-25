# Current argument

## Argument

**L1 (`code`) — the worker drops an item's future on budget, and continues.**
`process_with_item_budget` wraps each `process_stability_pool_item` in a
`tokio::time::timeout` of `STABILITY_ITEM_BUDGET` (30 s). The timeout arm logs
and continues the loop. Note the other arm: `if result? {` returns from the whole
pass on an item `Err`, so an error aborts the pass where a budget overrun does
not. Many `Err` sources reach that arm from one target — `ensure_client`,
`check_target`, `allocate_peg_in_address`, `validate_deposit_address`,
`observe_peg_in`, `recheck_peg_in`, `list_deposit_operations`, and more below
them — and **at least three are permanent**, aborting a pass on every pass for
the life of the process:

- an invite the endpoint policy refuses, which never becomes a cache hit because
  the check sits after the cached-client fast returns;
- a target whose config has the stability-pool module but no wallet module,
  which `get_first_module::<WalletClientModule>()?` errors on *after*
  `usable_target` has already answered `Usable`;
- a target whose wallet module consensus version is below 2.2, which
  `btc_tx_has_no_size_limit`'s `ensure!` errors on at the same point.

The last two are fixed properties of the config acceptance pinned by hash, and
nothing at acceptance checks the wallet module or its version — stage 2a checks
only the stability-pool kind. They are surfaced below, because by the trait's
own doc a fixed property of the target belongs in `PegInAllocation::Unusable`
rather than in an error.

`PegInAllocation::Unusable` itself is **not** an `Err` source: it is a value,
handled with `fail_item` and `Ok(true)`, and it leaves the queue after one pass.

**L2 (`code`) — a target-client open is owned by the pool, not by its caller.**
`TargetFedimintClients::create_or_load` does not await `create_or_load_client`.
It calls `open_slot`, which registers a `PendingOpen` and runs the open on a
spawned task; the caller then awaits a `tokio::sync::watch` receiver. A caller
dropped by L1 cancels only its own wait. One *new* await precedes the slot on
the caller's own future — `check_invite_endpoints`, which resolves the invite's
hosts; the cache reads, `make_room`, and the federation lock already did. It is backed by `spawn_blocking` rather than `block_in_place`, and an IP
literal short-circuits without a blocking task at all, so the caller stays
pollable and L4 still holds across it. Under A3 this is what keeps the drop
from detaching an unreclaimable lock holder: a *cancelled caller* never drops
the build, so its `TaskGroup` reaches the built client, which `evict` and
`shutdown_all` can shut down. This covers cancellation only — a build that
returns `Err` after the refresh task spawned still detaches it, which the
surfaced item below states.

**L3 (`code`) — a second caller attaches rather than starting its own.**
`open_slot` checks the installed clients and the in-flight slots under one
guard, returning the cached client or the existing receiver. Both checks are
needed: a finishing open installs its client before clearing its slot, so a
caller preempted between them would otherwise find neither.

At most one open per federation is in flight **except after `evict` or
`shutdown_all` drops a slot while its task is still building.** A later caller
then finds no slot and starts a second open, whose `open_rocksdb` blocks on the
file lock the first task still holds until that task finishes and shuts its
superseded client down. That block is inside a pool task, never on a caller, so
L4 carries the claim through it.

**L4 (`code`) — no *unbounded* uninterruptible section remains on the worker's
future.** The unbounded ones are `open_rocksdb`'s `block_in_place` and the
`flock` fallback beneath it (A3); `create_or_load_client` has exactly one call
site and it is inside `tokio::spawn`, so neither can land on a caller.

Bounded `block_in_place` sections do remain on the worker's future and must be
named rather than denied: every RocksDB transaction operation goes through one,
so each target-client call the worker makes after `create_or_load` returns —
`client.config()`, `subscribe_deposit`, the operation log, the module state
machines — contains them. They are bounded by local disk. A last-reference
`ClientHandle::drop` is also `block_in_place(block_on(shutdown_inner))`. Three
sites remove a `clients` entry and so can leave a caller holding the last
reference: `evict` and `shutdown_all`, both operator actions outside this
claim's adversary model, and `evict_if_idle`, which is requester-driven through
`make_room`. `evict_if_idle` cannot leave the drop on a caller: it gates on
`Arc::strong_count == 1` in the same critical section as the removal and moves
the handle into a spawned task.

`shutdown_join_all`'s deadline bounds this, but not at 30 seconds flat:
`join_all` walks subgroups and tasks sequentially with a per-task timeout
floored at 10 ms and does not abort stragglers, so the bound is 30 seconds plus
10 ms per unfinished task and subgroup, and the file lock is not guaranteed
released when it returns.

This is the step that decides the claim: a *timeout cannot interrupt a
`block_in_place`*, so an unbounded one on the worker's future is exactly what
wedged the pass before L2, and bounded ones are what the budget is sized
against.

**L5 (`code` + `enum`) — the pass reaches every item in its snapshot.**
`process_with_item_budget` takes one `active_stability_pool_items` snapshot and
iterates it. Under the timeout arm every later item is attempted. Under the
`Err` arm the pass returns early, and reachability then rests on ordering:
`mark_item_running` stamps `updated_at = unixepoch()` when an item's work
begins, so an item that aborts a pass carries a *newer* stamp than the items
that pass never reached, and `active_item_rows` orders by
`updated_at ASC, item_id ASC`, so the next pass leads with those unreached
items. The stamp is second-granularity, so items marked inside one second fall
back to the `item_id` tiebreak; with N consecutively failing items the last is
reached after N passes. **This rotation is what carries the claim against the
permanently aborting items L1 names**, however many there are: an unreached
item's stamp is frozen while a reached item's is `unixepoch()`, so once wall
time leaves the frozen second every reached item sorts behind it and each pass
moves at least one item past the frozen key. A tie cannot outlive its own
second, whatever the pass rate.

Two premises this rests on, stated rather than smuggled. `unixepoch()` is
non-decreasing across passes: a backward clock step lets an aborting item's new
stamp fall at or below a frozen one, which cannot starve indefinitely unless the
clock is pinned there, and a pinned clock is outside the stated adversary. And
`run_interval_task` logs an `Err` pass and keeps ticking, so there *is* a next
pass for the rotation to use. The rotation also needs the 10-second tick to exceed
that one-second granularity, which it does. `ACTIVE_ITEM_STATUSES` is `[Pending, Running]` and
`mark_item_running` re-accepts `Running`, so a cut-off item stays in the queue.

**L6 (`test`) — `a_cancelled_open_stays_owned_by_the_pool`.** Cancels a caller
against an unroutable endpoint, asserts the pool still owns exactly one open,
then asserts a second caller returns rather than blocking and that the slot's
sequence is unchanged. Pins L2 and L3.

**L7 (`test`) — `a_stuck_target_does_not_stop_the_item_behind_it`.** Seeds a
stuck target ahead of a healthy one, drives a pass with a 50 ms budget, and
asserts the healthy item is funded and the stuck one is not `Failed`. Pins L1
and the pass-level conclusion.

By L1–L5, one unresponsive target costs at most one budget per pass and leaves
every later item in the snapshot reachable. With A2 the claim holds.

## Residual windows

- **Latency is not bounded.** A pass costs up to the number of active stability
  items times the budget. That count is bounded by the federations an FI can get
  endorsed, which is a different quantity from the one this claim quantifies
  over — the claim is about reaching a later allocation, not about when.
- Restarting the daemon is an operator intervention, not an in-process timeout.
- This is a liveness and availability property; it does not by itself concern
  provider-wallet outflow.

## Weakest links

1. **L2–L4 (`code`)** — pool ownership of the open. Nothing in the type system
   stops a later change reverting to an inline await; L6 is the only thing that
   would fail.
2. **A3 (`axiom`)** — pinned Fedimint's `TaskGroup` has no `Drop` and
   `fedimint-db-locked` exposes no non-blocking open. A pin bump can change
   either, in either direction.
3. **L5 (`code`)** — the `Err` arm's reachability rests entirely on the
   `updated_at` rotation, no test pins it, and the arm is permanently reachable
   from at least three distinct sources rather than rare. This ranks higher than
   it reads.
