# Current argument

## What this record does not claim

**An operator can still write off provider value with one authenticated call.**
The exact property is that no automatic path writes it off and no write-off is
silent. The claim does not make an operator write-off reversible or assert that
the value remains safe afterward.

The operator route back is
[`liquidity-manager-recovery-runbook.md`](../../../../docs/liquidity-manager/liquidity-manager-recovery-runbook.md),
which reaches the notes with `fedimint-cli` against the database FLIP retains.
Its own `## Verification status` section records that **a live rehearsal has never
been run**. With this record narrowed, that rehearsal is the only thing standing
between a claimed peg-in and lost provider value.

The capacity half of the same mechanism is
[`stability-deposit-rejection-releases-capacity`](../CLAIM-stability-deposit-rejection-releases-capacity.md).
The accepted limitation, its severity, and its operator impact are recorded in
[`liquidity-manager-open-items.md`](../../../../docs/liquidity-manager/liquidity-manager-open-items.md).

## Argument

**L1 (`enum`) — post-claim syntax and production reachability differ.**
Regenerating every exit from `advance_after_withdrawal_completed` through
`observe_stability_deposit` (`stability_allocation.rs:180-346`) gives this
abstract backend matrix:

1. a persisted `submitting` marker without an operation id reads the aggregate
   provider report; at least `C` completes the item, while less than `C`
   clears the marker for a later tick;
2. target-balance, submit, observe, and report `Err` values return
   `unavailable` and leave the item active (submit `Err` first clears the
   marker);
3. `committed_deposit_amount` returns zero if either claimed amount or current
   target balance is below `C`, and that exit calls `fail_item`;
4. a stored operation reported as `Initiated` or `TxAccepted` stays active;
5. `Success` stays active until the aggregate provider report reaches the
   submitted amount, then completes; and
6. abstract `StabilityDepositStatus::Failed` calls `fail_item`.

The sole production observer narrows that matrix.  It creates
`subscribe_deposit_operation(...).into_stream()`, takes exactly one
`next()`, and drops the stream (`stability_pool.rs:173-202`).  The pinned
Fedi subscription unconditionally yields `Initiated` first and only afterward
awaits and yields `TxAccepted`, `TxRejected`, `Success`, or
`PrimaryOutputError` (Fedi `client/src/lib.rs:761-800`).  Every FLIP tick
therefore recreates the stream and observes `Initiated`; a stored operation
cannot reach abstract exits 4's `TxAccepted` or 5/6 through the official
binary.

Pinned Fedimint outcome caching does not rescue it:
`outcome_or_updates` stores only the last update after its update stream
finishes normally
(`fedimint-client/src/oplog.rs:203-224,349-377`,
`fedimint-client-module/src/oplog.rs:194-207`).  Dropping after the first
yield neither exhausts the stream nor caches a terminal outcome.  No other
official target-client subscriber exists.  This kills the simpler direct
`TxRejected -> fail_item` trace and exposes an adjacent permanent-`Initiated`
bug, but it does not kill the no-id crash trace below.

**L2 (`enum` + `code`) — an unrecorded real deposit can reject and refund.**
There is one production allocation submission: the worker call at
`stability_allocation.rs:288-303`, whose only production implementation calls
the pinned `deposit_to_provide` at `stability_pool.rs:164-175`.  Trait and
fake calls are not production submissions.

At Fedi revision A3, `deposit_to_provide` builds a `DepositToProvide` output,
chooses a fresh operation id, and invokes `finalize_and_submit_transaction`
(`client/src/lib.rs:738-758,1096-1114`).  Fedimint transaction finalization
balances the output with primary-mint inputs and durably installs transaction
and input state machines before the method returns
(`fedimint-client/src/client.rs:592-671,766-824`).  FLIP, however, chooses no
id in advance: it writes `submitting`, awaits that method, and writes the
returned id only afterward (`stability_allocation.rs:284-307`).  The client
RocksDB commit and later SQLite step update are separate commits, so A1 permits
a hard crash preserving the former and losing the latter.

The pinned stability server can ordinarily reject a correct provide with
`NoCycle` before any staged-provider write
(`server/src/lib.rs:1105-1145`).  On transaction rejection the pinned mint's
`CreatedBundle` transition constructs refund inputs for the same selected
spendable notes
(`modules/fedimint-mint-client/src/input.rs:207-308`).  Accepted refund outputs
become durable spendable notes counted by `get_balance_for_btc`
(`modules/fedimint-mint-client/src/lib.rs:1080-1087`,
`fedimint-client/src/client.rs:983-993`).  A4 chooses a delayed rejection and
refund schedule, with enough reserve to retain `C`.  The operation is real
and attributable to `I` even though FLIP crashed before recording its id.

**L3 (`code`) — no-id recovery can terminally fail while that operation is
pending.** On restart the item still has `sp_deposit_status = submitting` and
no operation id.  The production no-id branch reads only aggregate active
provider deposits.  Before the unrecorded transaction is accepted, the report
is below `C`, so it clears `sp_deposit_status` and returns
(`stability_allocation.rs:248-259`).  It neither searches the durable target
operation log nor waits for primary-mint input recovery.

On the next tick the item again observes its already claimed peg-in, enters the
deposit path with no stored deposit id, and reads the target wallet balance.
The unrecorded transaction's selected mint notes are absent from spendable
balance while its input state machine awaits acceptance/rejection.  Schedule
that balance below `C`.  `committed_deposit_amount` returns zero, and lines
271-281 call `fail_item`.  Only after that SQLite commit need the correct
federation reject the unrecorded transaction and the pinned mint refund finish,
restoring at least `C` of the same selected value under A4.

**L4 (`schema` + `code`) — all local durable predicates then coexist.** The
schema stores item status, wallet status, and `step_json` separately and has
no constraint coupling a failed item to target-client operations or value
(`migrations/20260716000000_initial_schema.sql:135-179`).  The worker persists
peg-in id/address and `peg_in_status = claimed` before entering the deposit
path (`stability_allocation.rs:94-103,213-216`), and reaches it only after
the attached wallet row is `Completed` (lines 125-168).  `fail_item`
unconditionally commits `failed` and recomputes the roll-up without changing
the completed wallet row, claimed step, or target client
(`allocation_store.rs:298-343`).  Later mint refund changes only target-client
RocksDB.  The schema rung guarantees durable separation; it supplies no desired
recovery invariant.

**L5 (`enum`) — terminal writers, selectors, and restart cannot resume it.**
Production allocation-item status writers are accepted-request insert,
`mark_item_running`, `complete_item`, `fail_item`, and manual reset/cancel.
Wallet status writers funnel through `wallet` plus manual reset/cancel.
The source-filtered stability selector admits only `pending` or `running`
items (`allocation_store.rs:204-219`).  Startup recovery likewise loads only
`pending`/`running` items and five nonterminal wallet statuses, records a
snapshot, and seeds dedupe; it does not open or reconcile a failed item's
target operation (`recovery.rs:289-321,417-470`).  The daemon then starts the
same periodic worker (`daemon.rs:394-414`).

If the daemon remains up, the already-open target client's executor completes
the delayed refund after `I` fails.  If a hard crash instead occurs mid-refund,
a failed item alone will not reopen that target client; that execution is not
needed for the witness.  Crashes before the item-failure commit merely postpone
the trace, and a crash after both item failure and completed refund preserves
the complete stranded conjunction.

**L6 (`enum` + `code`) — authenticated remediation is fenced out.**
`retry_funding_step` resets a failed item only if every attached wallet row is
retry-safe.  `retry_safe_wallet_status` accepts only `pending` or `failed`
with no txid; `completed` is false
(`manual_ops.rs:171-243,495-504`).  Predicate 2 therefore makes retry reject.
`cancel_allocation` selects only `pending`/`running` items, so a failed item
has no cancellable active work (`manual_ops.rs:288-358`).

Regenerating the Admin router and `OperatorAdminApi` yields only
provider-wallet funds/deposit-address/withdrawal/listing verbs;
allocation/request reads; retry/cancel; and setup, attestation, backup,
advertisement, and health verbs (`admin.rs:31-85`,
`admin.rs:136-246`, `service-liquidity-manager/src/service.rs:71-228`).
Provider-wallet money verbs use configured gatewayd, not the target client
(`funds_admin.rs:42-78`).  The exhaustive target-client surface is open,
peg-in allocation/observation, balance, deposit submission/observation, and
provider report (`stability_pool.rs:79-104,106-230`).  There is no official
target e-cash send/return, stability withdrawal, operation-log reconciliation,
or failed-item deposit resume.  Backup/restore preserves the client but adds no
online mutation; external secret use is excluded by A2.

**L7 (`enum` + `code`) — crashes and interleavings preserve the witness.**
One periodic stability task loops active items serially, so no duplicate worker
is required.  The hard crash is specifically after the client transaction/state
machine commit and before FLIP's operation-id commit.  Dependency delay keeps
the unrecorded transaction pending through no-id clearing and the zero-balance
failure; ordinary rejection/refund occurs afterward.  An operator retry before
failure sees no failed work; after failure completed `W` fences it.
Cancellation before failure is blocked by completed `W`; afterward the item
is terminal.  The same-federation shared-client and multi-item case can create
additional fungible-balance interference, but the counterexample uses one item
and the exact notes selected by its own unrecorded operation.

**L8 (`test`, limited) — fake tests miss both real joints.**
`stability_pool_allocation_completes_with_deposit_evidence` covers a fake
`Claimed -> Success` response, and
`stability_pool_submitting_without_operation_id_retries_deposit_only` clears
the no-id marker with a fake that has no pending client transaction and still
reports balance `C`.  The fake has no mint-note selection, durable operation,
rejection/refund, target RocksDB, or always-first-`Initiated` stream
(`stability_allocation.rs:824-934`).  No named test drives the production
crash/balance/refund trace or `PrimaryOutputError`.

**L9 (`code`) — concrete stranded execution.** Choose a normal accepted item
with committed amount `C`, a completed provider-wallet operation and claimed
safe peg-in into a fresh target client, ordinary reserve sufficient for
transaction/refund fees, and a correct stability module whose transaction will
be delayed and rejected with `NoCycle`.

1. FLIP persists `claimed`, writes `submitting` with no deposit id, and calls
   `deposit_to_provide(C)`.
2. The target client commits fresh operation `O`, its transaction, selected
   primary-mint inputs, and input state machines.  Hard-crash FLIP before it
   commits `O` to SQLite.
3. Restart.  Before `O` resolves, report zero active provides.  The no-id
   branch clears `submitting`.
4. On the next tick, the notes selected for `O` remain absent from spendable
   balance, so balance is below `C`.  The zero guard durably fails `I`.
5. While the daemon stays up with that target client open, the federation
   rejects `O` with `NoCycle`; the durable mint input state machine refunds
   the selected notes, and the refund outputs finalize with at least `C`
   spendable in the same FLIP-owned client.  No provider deposit was accepted.
6. Every later tick/restart omits `I`; authenticated retry/cancel is fenced,
   and no official target-client recovery/return action exists.

This satisfies all five exact stranded predicates without malicious Admin,
federation, or configuration; direct edits; or out-of-band wallet activity.
The ordinary crash, restart, dependency delay, and client-worker interleaving
are explicitly in the adversary model.  Therefore the claim is false.

## Residual windows

- Without the client-commit/SQLite-id crash gap, production FLIP repeatedly
  observes only `Initiated`; a rejected transaction can refund e-cash while
  the item remains `running`.  That adjacent liveness bug fails stranded
  predicate 1 and is not the L9 witness.
- `PrimaryOutputError` is emitted only after transaction acceptance, so the
  stability output already entered the pool and predicate 4 is false.
- An immediate submission `Err` clears the marker and retries.  The witness
  instead requires the pinned client commit to succeed and the process to crash
  before the returned id reaches SQLite.
- If the automatic refund never succeeds or loses too much to fees, predicate 4
  may be false.  The universal claim is falsified by A4's one allowed delayed,
  successful, sufficiently funded refund.
- A permanently unavailable federation, corrupt/lost disk, or crash-left lock
  without host cleanup is outside A1/A2 and unnecessary.
- Extracting a backup's target-client secret and spending externally is an
  excluded out-of-band wallet action, not an official daemon remedy.

## Weakest links

Weakest to strongest:

1. **A4/A5 (`axiom`):** delayed ordinary rejection, successful refund,
   retained `C` after fees, exact note lineage, and balance meaning bottom out
   in dependency/deployment semantics.  The witness chooses sufficient reserve
   and one successful schedule rather than claiming every refund retains `C`.
2. **L1/L2/L5/L6 (`enum`):** exit, subscription, writer, selector, Admin, and
   external-state exhaustiveness must be regenerated on scoped changes.
3. **L8 (`test`, weak coverage):** fake-backed success/no-id tests omit both
   the real pending transaction's balance effect and always-first stream.
4. **L2/L3/L6/L7/L9 (`code`):** the cross-store ordering, no-id clear,
   zero-balance failure, retry fence, and absent recovery calls directly admit
   the witness.
5. **L4 (`schema`, failed mechanism):** durable stores preserve the bad
   conjunction but enforce no item/client-operation coupling or recovery duty.
