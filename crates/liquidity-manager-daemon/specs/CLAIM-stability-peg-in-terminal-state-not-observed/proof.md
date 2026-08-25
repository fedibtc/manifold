# Current argument

## Argument

**L1 (enum + code) — the FLIP call path has one production observer.** A fresh
search for `subscribe_deposit(` finds one FLIP production call, in
`FedimintStabilityPoolBackend::observe_peg_in`
(`stability_pool.rs:121-150`). Each call recreates the subscription for `O`,
converts it with `into_stream`, applies a 500 ms timeout to exactly one
`stream.next()`, maps that one result, and drops the stream. The stability
worker is spawned once by official daemon wiring; `run_interval_task` awaits a
whole tick before the next interval branch, and
`process_stability_pool_allocations_with` serially awaits each active item
(`lib.rs:65-88`, `stability_allocation.rs:43-79`). Restarts recreate the same
path from the durable operation id: `target_fedimint.rs:39-80,170-261` opens
the same `{federations_dir}/{federation_id}/client.db`. No stability-pool or
Fedi downstream API is
reached until this observer returns `Claimed`.

**L2 (enum + code) — all subscription exits.** Before a stream exists,
`observe_peg_in` can return an error from client creation/loading, module
lookup, operation-id parsing, or `subscribe_deposit`; upstream subscription
errors comprise missing operation, non-wallet operation, non-deposit metadata,
address/network decoding, and legacy no-`tweak_idx` operations lacking a
terminal V1 outcome (plus malformed persisted metadata under A1's exclusion).
Those errors become `Unavailable` and are not successful invocations.

For a current deposit operation, `outcome_or_updates` has exactly two branches.
A cached outcome becomes a one-item stream, so `Claimed` or `Failed` is returned
immediately. Without a cached outcome it creates the update stream. The local
match exhausts `Ok(Some(...))` for `WaitingForTransaction`,
`WaitingForConfirmation`, `Confirmed`, `Claimed`, and `Failed`; timeout `Err`
maps to `WaitingForTransaction`; and normal end `Ok(None)` also maps to
`WaitingForTransaction`. The uncached upstream generator's first action is an
unconditional `yield WaitingForTransaction`, before its retrying watch/history
RPC, claim-key wait, output wait, and later variants
(`fedimint-wallet-client/src/lib.rs:1420-1480`). Consequently its first poll is
ready with `WaitingForTransaction`; dependency delay after that yield cannot
move the one-item FLIP poll to a later state. Panic or process termination is
not a successful exit and is already covered by the crash adversary.

**L3 (code) — durable claim state is not a durable cached outcome.** The
independent peg-in monitor claims the input and atomically inserts
`ClaimedPegInKey`/`ClaimedPegInData`; the subscription later waits for that key
and for all primary outputs before yielding `DepositStateV2::Claimed`
(`pegin_monitor.rs:360-420,540-583`; `lib.rs:1462-1478`). That monitor does not
write the operation-log outcome. `OperationLog::outcome_or_updates` consults
only the operation-log outcome. Its caching wrapper records an outcome only
after the generated stream has yielded its last item, is polled again to
`None`, and reaches `optimistically_set_operation_outcome`
(`fedimint-client/src/oplog.rs:207-224,349-377`). Dropping immediately after the
first item executes none of that tail. Thus the Claim's durable physical
`Claimed` predicate and available e-cash can coexist indefinitely with a
missing cached outcome.

**L4 (enum + code + schema) — every local peg-in-status writer accepts the
stale result.** Production assignments to the stability step's
`peg_in_status` are exhausted by: (a) address allocation writes
`waiting_for_transaction` (`stability_allocation.rs:102-112`); and (b) the five
observer arms write `waiting_for_transaction`, `waiting_for_confirmation`,
`confirmed`, `claimed`, or fail the item (`stability_allocation.rs:180-228`).
`StabilityPoolStepExt::set_peg_in_status` overwrites the string and preserves an
old amount when passed `None`. All successful nonterminal arms call the sole
generic `update_item_step`, which unconditionally replaces `step_json`; neither
it nor the `allocation_items` schema imposes a monotonic-state constraint or a
compare-and-swap guard. Other stability step writers alter other fields but do
not assign `peg_in_status`; gateway callers load source-disjoint rows. Item
completion/failure changes status and is downstream of a non-waiting arm.

**L5 (test) — present tests do not exercise this backend contract.** The named
unit test `stability_pool_allocation_completes_with_deposit_evidence` injects
`FakeStabilityPoolBackend`, whose `observe_peg_in` returns an in-memory enum set
directly to `Claimed`; it does not call `subscribe_deposit`, `into_stream`, or
the operation log. The other stability allocation tests use the same fake. In
the live harness setup at `integration_live_liquidity.rs:1197-1219`,
`stability_min_amount` is zero, so its real Fedimint exercises gateway rather
than stability allocation. The open-items document explicitly
leaves a Fedi-flavor live stability-pool harness undone. Thus no named test
catches this one-item resubscription behavior; the `test` rung supports only
the fake orchestration mapping, not the production liveness claim. The pinned
upstream happy-path wallet test (`fedimint-wallet-tests/tests/tests.rs:272-335`)
keeps one subscription alive through every update and polls its terminal update
through `None`; it does not cover repeated fresh one-item subscriptions.

**L6 (counterexample) — every counted invocation succeeds and rewrites the
initial update.** Begin after predicates 1-3 in the Claim hold; this is a normal
state when the independent peg-in monitor has durably claimed the exact output
and made its notes available but no consumer has drained `O`'s subscription.
On invocation `T1`, FLIP sees no cached outcome, constructs a fresh generator,
polls its unconditional initial `WaitingForTransaction`, persists
`waiting_for_transaction`, returns `Ok`, and drops the stream before it can
inspect the already-present claim key. Repeat for `T2, T3, ...`. Every call is a
successful processing invocation by definition; every fresh generator starts
at the same yield; none writes an outcome cache; and every local write reports
the earlier waiting state. For every proposed finite `N`, the first `N`
invocations in this execution avoid `claimed`. This disproves fair bounded
progress without a crash, dependency failure, concurrent worker, malicious
actor, or wall-clock assumption. Crashes/restarts between invocations merely
recreate the same uncached path, finite dependency delays occur after the
consumed initial yield, and A2 serialization does not help.

## Residual windows

- If another consumer fully drains the same operation stream, or if the
  operation log already durably contains `DepositStateV2::Claimed`, the next
  invocation sees the one-item cached outcome and advances. This is outside
  predicate 3 and does not rescue the quantified uncached execution.
- Failed/interrupted ticks, permanently unavailable dependencies, shutdown,
  and an item skipped because an earlier item aborts the outer loop are outside
  the explicitly counted successful invocations. They are not needed for the
  counterexample.
- What happens after FLIP commits local `claimed`—wallet-balance checking,
  `deposit_to_provide`, stability observation, and completion—is outside the
  core bad thing by definition. The cold check found that the downstream
  observer has the analogous one-item defect: FLIP `stability_pool.rs:173-203`
  drops after one update, while the exactly pinned
  `fedixyz/fedi@2f35ea4e3b2516d35b8ed315455718cd3b336758`
  `crates/modules/stability-pool/client/src/lib.rs:761-801` starts an uncached
  stream at `Initiated`. That adjacent property is not imported into this one.
- Legacy pre-0.4 deposit operations and corrupt/maliciously edited stores are
  outside the current-operation and trusted-store predicates.

## Weakest links

1. **Axiom:** A1/A3 bottom out database durability, identity, consensus output
   acceptance, and spendability; no local claim record proves those external
   platform/protocol facts.
2. **Enum:** L1/L2/L4 can stale if a call, exit, or writer is added anywhere in
   Scope; they must be regenerated rather than trusted as listed.
3. **Code:** L3 depends on the subtle poll-after-final-yield placement of the
   optimistic outcome write and on the unconditional initial update.
4. **Schema:** L4's absence of monotonicity is visible in the unconstrained
   nullable `step_json`; schema changes could remove this counterexample.
5. **Test:** L5 is deliberately weak: fake-backed tests prove the local branch
   mapping but provide no regression protection for real stream behavior.
