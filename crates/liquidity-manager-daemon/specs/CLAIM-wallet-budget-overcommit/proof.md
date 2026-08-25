# Current argument

## Argument

**L1 (enum + schema) — durable liability representation and writers.** The
schema has no budget-total row, reconciliation watermark, non-negative/check
constraint on the monetary columns, or trigger enforcing either inequality.
Its relevant uniqueness constraints prevent duplicate request/item/operation
identities, not aggregate overcommit. Regenerating all production `INSERT` and
`UPDATE` statements over `allocation_items`, `wallet_operations`,
`wallet_balance_observations`, and setup config plus restore yields T1–T8
above. Test-only raw fixture inserts are not official-daemon executions. There is no separate U
column or table.

T1 is the only normal creator of positive active allocation reservations. T2
is the only creator of active operator withdrawals. T3 is the only intentional
production retry of failed allocation items/operations; T4's unconditional
status writers can also resurrect a cancelled operation in the L7 race. T4–T5
move liabilities between active and terminal classifications. T6 is the only B
writer, T7 is the only F/C writer, and T8 replaces the whole local snapshot.
Allocation workers create item-attached funding operations through
`insert_wallet_operation_tx`, but R remains active throughout those rows'
active send states. Recovery only queries active rows; it neither checks
nor repairs the invariant. Backup checkpoints and copies the database; restore
validates setup/dependencies but performs no budget reconciliation.

**L2 (code) — acceptance alone takes a serialized SQLite writer snapshot, but
does not establish the claim.** T1 opens one transaction and its first SQL
statement inserts the request row, acquiring writer serialization before it
reads B, R, and W. It computes available balance as
`B.saturating_sub(W).saturating_sub(R).saturating_sub(F)`, checks the new
reservation, checks `old R + new R <= C` in explicit-cap mode, inserts all
reservation rows, and commits. Concurrent accepted requests therefore
serialize or one fails; they do not both commit from the same old R. This fact
is specific to T1's insert-before-read order: a general SQLite deferred
read-then-write path may instead fail with `BUSY_SNAPSHOT`. Neither behavior
protects against T2's pre-transaction check, T3's
unchecked reactivation, absent U, a stale T6 overwrite, or T7 changing F/C.
T6's complete call-site list is wallet sync, `get_funds_with_wallet`, and
`request_withdrawal_with_wallet`.

**L3 (code + concrete counterexample) — T2 has a check/insert race.**
`request_withdrawal_with_wallet` fetches gatewayd balance, persists it, queries
accounting, and checks the requested amount before it calls
`prepare_withdrawal` and before opening the transaction which inserts the
pending withdrawal. The transaction performs no budget recheck.

Starting with `B = 100`, `R = W = U = F = 0`, run two concurrent authorized
withdrawals of 60. Both observe 100 and both pass the pre-transaction check.
After preparation, SQLite serializes their inserts; serialization does not
re-run the check, so both commits succeed. Immediately after the second T2
commit, `W = 120 > B = 100`. The same trace works with one withdrawal and one
T1 acceptance: the withdrawal passes against the old state, T1 commits (say)
R=60, then T2 commits W=60 without rechecking, yielding 120 > 100. A crash
after either insert preserves the violating state on restart. This directly
falsifies the claim.

**L4 (code + concrete counterexample) — T3 reactivates reservations without
capacity admission.** Manual retry loads failed items and failed wallet
operations in one transaction, verifies only that wallet status is `failed`
or `pending` with no txid, resets the operation and item to `pending`,
recomputes target status, and commits. It reads no B, R, W, F, or C.

Reach a failed item retaining `reserved_amount_sats = 60`; terminal status
means it contributes zero to R. With `B = 100`, `F = 0`, another request may
legitimately commit R=60. Retrying the failed item then commits R=120. In
available-funds mode `120 > 100`; with `C = 100`, it also violates `R <= C`.
No concurrency or faulty external dependency is required. If retry races
acceptance, SQLite serialization again cannot help because retry never checks the budget.

**L5 (enum + code + concrete counterexample) — possibly-spent terminal
transitions have no U/watermark handoff.** Before an allocation funding send,
the worker marks its operation `in_doubt`; while the item stays active R covers
the amount. Operator withdrawals similarly remain in W through `in_doubt`.
However, `apply_sync_update` can mark an operation `completed`, and item
completion/failure/cancellation can remove R, without atomically recording a
new wallet observation or a durable fact that the current B includes the
debit. `apply_sync_update` accepts the supplied status without a transition
guard. The generic clean-submission-failure arm marks the operation failed and
item failed in separate transactions, but the production `GatewaydFundsWallet` maps
all actual `send_onchain` errors to `InDoubt`; this record does not rely on the
post-send `Failed` arm being production-reachable. Successful sends and later
sync/manual paths still require the U handoff when coverage is removed.

Concrete trace: persist B=100; submit an 80-sat operator withdrawal; retain a
stale-but-accurate pre-debit wallet response of 100; observe the transaction on
chain and T4 marks the withdrawal completed. W falls from 80 to zero while the
possibly spent 80 belongs in U and B has no watermark proving otherwise. T6
may then persist the delayed observation of 100 (its UPSERT has no monotonic
`observed_at` guard). T1 sees B=100, R=W=0 and, because code has no U, accepts
R=80. The actual durable predicate is `R + U = 160 > B = 100`. Allocation
completion produces the analogous handoff from R to U. Crash/restart preserves
these rows; recovery does not synthesize U or invalidate B.

**L6 (code + concrete counterexample) — T6, T7, and T8 are not monotone budget
updates.** T6 unconditionally overwrites the singleton observation, including
`observed_at`; an older call completing later can replace a newer lower
balance. There is no send/reconciliation watermark, age limit, or compare-and-
set. Independently, T7 validates and atomically persists configuration but
does not inspect active liabilities. With active R=80, an operator can lower C
from 100 to 50; with `R + W + U = 80`, the operator can raise F from 0 to 30
against B=100. Both accepted configuration commits violate the corresponding
bound. Subsequent readiness changes do not repair the already committed state.
`apply_setup_config` and `update_provider_config` are the exact T7 writers; their
validation does not query active liabilities. For T8, the backup contract omits
gatewayd wallet state. Restoring an older archive after the external wallet has
sent funds can roll back B plus operation/liability history without rolling back
the wallet, and restore validation performs no budget reconciliation.

**L7 (enum + code) — cancel/fail/complete, sync, crash/restart, and restore do
not close the counterexamples.** Cancel rejects wallet states other than
pending/failed/cancelled, which avoids intentionally cancelling a known
broadcast/in-doubt send; retry rejects txid-bearing and non-failed active
operations. However, a worker can load a pending item/operation, pause, then
cancel commits both rows as cancelled and removes R. The stale worker resumes:
`submit_funding_withdrawal` unconditionally marks the operation `in_doubt`,
sends, and unconditionally marks it broadcast by id, while the item remains
cancelled. Neither R nor W covers this allocation-attached send, so it enters U.
These local guards therefore neither perform the missing budget check nor
provide the required U handoff. Sync fetches a balance before applying operation
and chain updates, so even its normal order provides no “after all terminal
updates” watermark. Startup gates fresh public work on recovery, but recovery
only enumerates the violating rows. Restore replaces local state from a
checkpoint while gatewayd is outside the archive, then validates
configuration/dependencies but not the budget. Thus
every required lifecycle surface is covered and none invalidates L3–L6.

## Residual windows

The in-claim schedules in L3–L6 are not residuals.

## Weakest links

Ranked weakest first:
