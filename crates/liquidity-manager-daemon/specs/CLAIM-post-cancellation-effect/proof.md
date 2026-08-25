# Current argument

## Argument

**Allocation-item writers — twelve production statements in twelve functions**,
ten in `allocation_store.rs` and two in `target_recovery.rs`:

| Writer | Site | Production call sites |
| --- | --- | --- |
| `insert_allocation` (INSERT) | `allocation_store.rs:295` | 2 — `public.rs:301` and `:316` |
| `release_allocation_tx` (DELETE) | `allocation_store.rs:184` | 2 — `manual_ops.rs:85`, `public.rs:492` |
| `mark_item_running` | `:608` | 2 (1 gateway, 1 stability) |
| `update_item_step_json` | `:631` | 1 (stability) |
| `update_item_step` | `:650` | 18 (6 gateway, 12 stability) |
| `compare_and_set_item_step` | `:670` | **2** (stability) |
| `complete_item` | `:694` | 2 (1 gateway, 1 stability) |
| `set_item_failure` (private) | `:760` | 2 — via `fail_item` `:718` and `require_item_action` `:735` |
| `reset_item_tx` | `:785` | 1 — `manual_ops.rs:791` |
| `cancel_item_tx` | `:807` | 1 — `manual_ops.rs:923` |
| `bind_and_resume` | `target_recovery.rs:206` | admin verb `bind_target_deposit` |
| `abandon_target_client_value` | `target_recovery.rs:312` | admin verb of the same name |

`fail_item` has 4 production call sites (1 gateway, 3 stability).
`require_item_action` has **16** (6 gateway, 9 stability, and
`allocation_funding.rs:196`). Both funnel into the private `set_item_failure`, so
a lemma about failure writes must cover both entry points.

1. `insert_wallet_operation_tx` (`:101`) — the sole INSERT. Reached from
   allocation `ensure_wallet_operation`, admin deposit-address creation, and
   admin withdrawal.
2. `bind_operator_withdrawal_intent_tx` (`:140`) — admin withdrawal.
3. `mark_withdrawal_broadcast` (`:208`)
4. `mark_operation_in_doubt` (`:239`)
5. `escalate_in_doubt_to_manual_review` (`:275`) — 1 production caller,
   `funds_admin.rs:156`.
6. `resolve_manual_review_tx` (`:330`, `:346`, `:368`) — three statements, one
   function; `manual_ops.rs`, admin verb `resolve_manual_review`.
7. `mark_operation_failed` (`:408`)
8. `reset_wallet_operation_tx` (`:434`) — manual retry.
9. `cancel_wallet_operation_tx` (`:464`) — manual cancel.
10. `apply_sync_update` (`:502`) — backend loop and chain evidence.
11. `claim_chain_evidence` (`:619`) — 1 production caller, `funds_admin.rs:131`.
12. `stamp_released_item_operations_tx` (`:902`) — 1 production caller,
    `wallet.rs:1234`, inside `active_wallet_withdrawal_amount_tx`.
13. `claim_funding_submission` (`allocation_funding.rs:169`) — the only
    production wallet writer outside `wallet.rs`.

The only item executors are `process_gateway_item` and
`process_stability_pool_item`; their periodic task loops await one pass, but
either pass may overlap admin and sync tasks. Admin verbs that write
`wallet_operations`: five — `retry_funding_step`, `cancel_allocation`,
`resolve_manual_review`, `create_deposit_address`, `request_withdrawal`. Three
write `allocation_items`: `bind_target_deposit`, `abandon_target_client_value`,
and `release_federation_allocation`. `restore_backup` replaces the whole SQLite
file, but `backup.rs` is outside this record's `Scope:`.

**E2 (`schema` + `code`) — cancellation is atomic; the schema does not make
terminal state monotonic.** `cancel_allocation_with_database` loads active items
and attached wallet operations, accepts only pending/running items whose wallet
operations are pending/failed/cancelled, and updates them to `cancelled` in one
SQLite transaction before replying. That establishes a durable cancellation point
under A1. The schema constrains row identity and one operation per
`(operation_type, item_id)`, but has no status-domain check, transition trigger,
terminal-row immutability trigger, or generation column. **So nothing in the
schema rejects a stale writer, and every lemma below has to come from the
statements.**

**E3 (`code`) — every status-moving item writer excludes terminal rows in its
own `WHERE`.** `mark_item_running`, `update_item_step`, `update_item_step_json`,
`compare_and_set_item_step`, `complete_item`, and the private `set_item_failure`
behind both `fail_item` and `require_item_action` all predicate
`item_id = ? AND status IN (?, ?)` bound to `Pending` and `Running`. `Cancelled`,
`Failed`, `Completed` and `ActionRequired` are outside that set — `ActionRequired`
deliberately, which is why `abandon_target_client_value` carries its own
statement rather than reusing `set_item_failure`. Read the bound values, not the
function name: a `status IN (?, ?)` says nothing until the binds are read.

**E4 (`code`) — every status-moving wallet writer does the same.**
`mark_withdrawal_broadcast` and `apply_sync_update` bind `Pending`, `InDoubt`,
`Broadcast`, `Confirmed`; `mark_operation_in_doubt` and `mark_operation_failed`
bind `Pending` and `InDoubt`; all three arms of `resolve_manual_review_tx`
predicate `status = ?` bound to `ManualReviewRequired` and the function returns
`rows_affected() > 0`. `Completed`, `Failed` and `Cancelled` are outside all of
them.

**E5 (`code`) — each irreversible call is preceded by a predicated statement
whose affected-row count the caller acts on.**

- **`send_onchain` on the allocation path.** `claim_funding_submission`
  predicates `operation_id = ? AND item_id = ? AND status = ? AND EXISTS (SELECT 1
  FROM allocation_items WHERE item_id = ? AND status IN (?, ?))` and returns
  `rows_affected() == 1`. One statement fences the send on the wallet row's status
  **and** on its item still being active.
- **`send_onchain` on the operator-withdrawal path.**
  `bind_operator_withdrawal_intent_tx` predicates `status = in_doubt` and refuses
  on `rows_affected() != 1`. **It provably cannot fire today** — its only caller
  inserts the row `in_doubt` inside the same `BEGIN IMMEDIATE` transaction.
  The predicate keeps all three irreversible call sites in one greppable shape
  rather than a carve-out whose membership must stay enumerated.
- **`deposit_to_provide`.** `compare_and_set_item_step` adds
  `AND step_json = ?` to the active-status predicate and returns
  `rows_affected() == 1`.

**E6 (`code`) — the writers that match on identity alone have no stale snapshot
to be stale against.** Four remain:

| Writer | Predicate | Why it is not a counterexample |
| --- | --- | --- |
| `reset_wallet_operation_tx` | `operation_id = ?` | manual retry; its checks and its write are in one `begin_write` |
| `cancel_wallet_operation_tx` | `operation_id = ?` | manual cancel; same |
| `cancel_item_tx` | `item_id = ?` | manual cancel; same |
| `release_allocation_tx` | `federation_id = ?` | admin release and admission takeover; same, and see below |

The first three are reached only from admin verbs that read the rows they will
write inside the same SQLite write transaction — `manual_ops.rs` opens
`begin_write` before its checks — and SQLite serialises write transactions, so
nothing interleaves between a check and its write.

**`release_allocation_tx` deletes rather than overwrites, and both clauses need
saying.** It is refused unless the allocation holds nothing: no item in
`RESERVING_ITEM_STATUSES`, no wallet operation in `PENDING_SETTLEMENT_STATUSES`,
and no `fulfilled_amount_sats`. So every item it removes is already terminal.
Against clause (2) it writes no outcome at all — the row ceases to exist rather
than acquiring a different terminal status — and the `audit_log` row it commits
in the same transaction is what records that the allocation existed. Against
clause (1) it is strictly protective: a deleted item makes every fence in E5
match zero rows, so an in-flight invocation holding an older snapshot is refused
rather than allowed. This is the least exercised writer in E1.

**Conclusion.** By E1 the writers are these and no others. By E3 and E4 no
status-moving statement matches a terminal row, so clause (2) holds for every
writer that names a status. By E6 the four that match on identity alone either
run inside the transaction that checked them or remove already-terminal rows. By
E5 each of the three irreversible calls is preceded by a predicated statement
whose zero-row outcome fences it, which is clause (1). E2 records that none of
this comes from the schema, so all of it has to be re-derived whenever a
statement changes — which is the maintenance cost this record has already paid
three times.

## Residual windows

**One accepted, and it is load-bearing.** The interval between the fence
committing and the remote call being observed by the counterparty is granted by
A2, not closed by any lemma. Every irreversible call in scope sits inside it: the
fence commits, then the process asks gatewayd or the target federation to act,
and a crash or a delay in between leaves a durable intent with no durable
outcome. Three verdicts on this record rely on that grant.

The window is **widest at the stability deposit site**, and that is not obvious
from the fence's placement: `stability_allocation.rs`'s compare-and-set precedes
`submit_deposit_to_provide`, which first awaits `self.client(target)` — an open
this record documents as able to hang with no terminal condition — and then reads
the operation log, before the actual `deposit_to_provide_with_operation_id` call.
The two `send_onchain` sites have a much narrower gap. See `## Weakest links`.

Otherwise nothing is accepted. Crashes around remote calls, delayed remote
responses, concurrent authorized admin verbs, sync races, and the single-daemon
process model are expressly inside the claim. Direct database tampering,
unsafe-memory corruption, and a replacement binary are excluded by A1/A3 rather
than filed as accepted residuals.

## Weakest links

Ranked weakest first:
