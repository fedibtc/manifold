# Current argument

## Argument

**R1 (`enum`) — one production submit path and one production observer path.**
`advance_stability_deposit` reaches `submit_deposit_to_provide`, and
`observe_stability_deposit` reaches `observe_deposit`. Every other
implementation of either is a `#[cfg(test)]` fake. The antecedent is also
established by the `submitting` CAS: `begin_submission` writes the status, the
caller-owned operation id, the amount, and the fee rate in one committed step,
so an item at `submitting` already carries `O`. `bind_and_resume`
(`target_recovery.rs`) writes `initiated` on the operator recovery path.

**R2 (`code`) — a responsive drain returns a terminal state.**
`observe_deposit` drains `subscribe_deposit_operation`'s stream to its end under
`STREAM_DRAIN_BUDGET` and returns the furthest update seen. The generator always
ends at `Success`, `TxRejected`, or `PrimaryOutputError`, and
`caching_operation_update_stream` writes the outcome at stream end, so a later
invocation resolves from the cached outcome without a federation round trip. A
drain that ends therefore yields a terminal `StabilityDepositStatus`.

Two arms end predicate (1). `Success` sets `sp_deposit_status = success` and
commits it **before** `observe_stability_pool`, so no unbounded federation call
stands between the observation and the write. `Failed` reaches
`require_item_action`, whose `action_required` is outside the claim's own
definition of an active item. `Initiated` and `TxAccepted` record progress rather
than ending an observation.

That matters because each write is an await on more than an instant:
`update_item_step` waits for a pool connection, and `set_item_failure` opens a
`BEGIN IMMEDIATE` that queues on SQLite's single write lock behind any other
worker for up to the configured busy timeout. Under the wrapper those were the
windows an adversary delaying an earlier dependency could aim at, on every pass.
Under the deadline they are ordinary awaits inside a future nothing drops.

**One cancellation source remains, and it is not the budget.**
`run_interval_task` selects the pass against the shutdown token, so a shutdown
drops the whole pass at an arbitrary await. Both writes still run through
`*_beyond_cancellation` helpers, which execute the statement on a spawned task
and await it, so a dropped caller does not take the write with it. The
difference from the wrapper case is the quantifier: shutdown is one event per
process lifetime, resumed from durable state on the next start, and not
something a counterparty can aim repeatedly at a chosen write.

**R4 (`code`) — the report gate decides completion, not observation.** After R3,
`observe_stability_pool` still gates `complete_stability_pool_item` on the
provider account reporting at least the fulfilled amount. A hanging or
short report leaves the item active with `sp_deposit_status = success`, which
falsifies predicate (1)'s requirement of a nonterminal local status. The item
stays incomplete; it does not stay unobserved.

**R5 (`code`) — no writer restores predicate (1) for an operation id a
responsive invocation has carried past it.**

**The statement now holds by construction.** Every write of `sp_deposit_status`
goes through `StabilityPoolAllocationStep::advance_sp_deposit_status`
([`allocation_store.rs`](../src/allocation_store.rs)), which owns both that field
and `sp_deposit_operation_id` so the two cannot disagree, and which refuses a
status ranking below the one already recorded **for the same operation id**:
`submitting` < `initiated` < `tx_accepted` < `success`. The call sites are
`stability_allocation.rs`'s post-submit write and its `Initiated`, `TxAccepted`
and `Success` observation arms, and `target_recovery::bind_and_resume`.

**Three things the guard deliberately does not do.**

- **It is keyed on the operation id, not the item.** `bind_target_deposit` must
  be able to start a *new* deposit at `initiated` after a lost operation id, and
  that is a different `O`, so predicate (1) is about a different subject.
  Re-binding the id already recorded is refused with a `failed_precondition`
  rather than silently discarding a terminal observation.
- **It does not order unknown strings.** A status this build cannot rank never
  blocks a write, so a value a later build introduces cannot wedge an item.
- **`begin_submission` carries no such guard, and needs none.**
  `advance_stability_deposit_with` returns into `observe_stability_deposit`
  whenever `sp_deposit_operation_id` is already set, so the submission path is
  only ever reached with no id to walk backwards.

**`reset_item_tx` is outside this lemma, and that is the point of the
restatement.** It returns an `action_required` item to `pending` while leaving
`step_json` untouched, so it can make (1) true again with a nonterminal status.
It is an operator retry, not a step inside a responsive invocation, and the
worker then observes `O` and advances it. The claim quantifies over the
responsive invocation rather than asserting monotonicity over every writer.

**R6 (`test`) — `a_terminal_deposit_is_recorded_even_when_the_report_hangs`.**
Seeds an active item at `initiated` with a responsive deposit stream and a
provider report that never returns, drives one pass under a 2 s budget, and
asserts the durable status is `success`. The budget is deliberately generous: it
only has to outlast the item's real work and cut the report. The 2 s budget
accommodates suite contention. Removing R3's write makes the assertion report
`initiated`, so the test distinguishes the required persistence.

By R1-R4, one responsive invocation **commits** the end of
`LocalNonterminal(I,O)`, so no sequence containing unboundedly many responsive
invocations can leave predicate (1) true. With A6 supplying their recurrence,
the claim holds.

Predicate (3) asks whether the end of `LocalNonterminal(I,O)` is durable at the
instant a responsive invocation returns. Under the deadline model the answer is
yes on every path except one, and the exception is nameable rather than hedged:

- **Ordinary passes, budget spent or not.** Nothing drops the item's future, so
  the awaited write completes before the invocation returns. The strict
  at-the-instant-of-return reading holds.
- **Shutdown.** `run_interval_task`'s `select!` can drop the pass at the write's
  await. The `*_beyond_cancellation` helper has already spawned the statement, so
  the write lands shortly after the invocation returns: scheduled, not complete.
  Predicate (3) fails at that instant and holds immediately after.

The claim quantifies over an unbounded sequence of responsive invocations, and
shutdown contributes at most one member per process lifetime, so the exception
cannot make the antecedent true. The claim excludes strict at-return durability
for the shutdown-cancelled invocation without weakening the
unbounded-invocation conclusion.

## Residual windows

- **No-operation-id submission ambiguity:** a crash after the client creates an
  operation but before SQLite records its id may lead to duplicate submission.
  That is the separately recorded `duplicate-stability-deposit` failure. This
  claim starts with an exact durable `O`, so the L6 witness does not use or
  conflate that window.
- **Antecedent reachability:** the current real peg-in observer has the same
  first-update defect, so clean-start production reachability of a stored
  deposit `O` is not established. This is not a direct-database-edit witness:
  the assigned claim is explicitly conditional on that durable state, and L6
  begins there. An end-to-end claim from authenticated setup would currently
  pass this suffix only vacuously and needs a separate peg-in argument.
- **Fedimint `NonRetryableError`:** `TxSubmissionStates` includes this
  terminal state, but `await_tx_accepted` ignores it and waits forever. It is
  outside the requested `TxAccepted`/rejection/output terminal domain and is
  not used as a counterexample.
- **Pre-terminal dependency outage:** an operation that has not reached a
  durable terminal predicate may legitimately remain initiated. It is outside
  condition (2); L6 instead uses durable accepted success.
- **Explicit operator cancellation/retry:** authenticated operator actions are
  trusted and can terminate or requeue the item. The claim asks whether fair
  successful automatic processing can remain nonterminal, not whether a human
  can intervene.
- **Hard-crash lock cleanup and permanent process outage:** A1/A4 grant restart
  and fair ticks. An execution with no future processing is outside condition
  (3), and is not needed by the counterexample.
- **Post-completion pool balance changes:** retention of provided liquidity is
  outside this observation property; no out-of-band wallet activity is used.

## Weakest links

Weakest to strongest:

1. **A1/A4 (axiom):** durable replay and fair future ticks bottom out in the
   pinned Fedimint store/executor and deployment scheduler.
2. **L1/L2/L4/L5/L7 (enum):** call, variant, exit, writer, and executor
   exhaustiveness must be regenerated on any scoped change.
3. **L8 (test):** the named fake-backed test demonstrates only local transition
   handling, not the production dependency behavior.
4. **L1/L3/L5/L6 (code):** the one-element consumption and unconditional-first
   ordering are direct readings of local and pinned code.
5. **L4 (schema):** the migration visibly has no constraint capable of forcing
   the missing transition.
