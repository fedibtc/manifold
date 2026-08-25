# Current argument

## Argument

**L1 (`enum` + `code`) — the only stability completion path is the
operation-success branch.** The sole production caller that constructs
`CompletionEvidence::StabilityPool` is `complete_stability_pool_item`, reached
from `observe_stability_deposit` only when `observe_deposit` returns `Success`.
It writes the operation id and fulfilled amount from this item's step. The
other generic `complete_item` caller is the source-separated gateway worker.

**L2 (`code`) — each item persists the exact call lineage before completion.**
After the target peg-in has been claimed, `advance_stability_deposit` calculates
only zero or `item.committed_amount`, persists that amount and a `submitting`
marker using a compare-and-set, calls `submit_deposit_to_provide` with the
persisted amount, then saves the returned operation id. Later observations use
that saved id. Thus the completed item's operation id originates from its own
exact-amount call, rather than from the account-wide report.

**L3 (`code`) — an ambiguous crash never falls back to the aggregate.** A crash
between the external deposit submission and persistence of its returned id
leaves `submitting` with no id. The active worker's first branch makes that
item `action_required` and returns; it neither reads `report` nor calls
`complete_item`. This deliberately preserves the reservation rather than
attributing a sibling's deposit to an ambiguous item.

**L4 (`code` + A2–A3) — success is operation-specific target
fulfillment.** The production backend passes the saved id to
`subscribe_deposit_operation`. In the pinned client, `deposit_to_provide`
submits `DepositToProvide` for `our_account(Provider)` with the passed amount;
its stream emits `Success` only after that transaction is accepted and any
primary change outputs resolve. Therefore the `Success` prerequisite in L1 is
independent evidence that this item caused its exact provider deposit.

**L5 (`code`) — the aggregate report only corroborates the already-attributed
operation.** On `Success`, FLIP records the account-wide provided amount and
requires it to be at least the item amount. That report loses per-deposit
identity, so an unrelated deposit can satisfy this *second* predicate. But it
cannot substitute for L4: without the item's own saved operation id reporting
`Success`, no stability completion call is reachable. An unrelated deposit may
make completion earlier after L4, but cannot complete a zero-call or failed
item.

L1–L5 establish the claim.

## Residual windows

- An item can remain running when its successful deposit is not yet visible in
  the aggregate report. This is liveness, not false completion.
- The no-id crash branch intentionally needs trusted Admin reconciliation. Its
  lack of automatic recovery is covered separately by
  `found-bugs/ambiguous-stability-deposit-has-no-official-recovery-path.md`.
- The completed row stores operation id in JSON rather than a schema foreign
  key. Under A1, production control flow still supplies the binding; direct DB
  edits are outside the model.

## Weakest links

1. **L4 (`code`, A2–A3)** — upstream operation-stream semantics and
   their correspondence to the target federation are the main dependency.
2. **L1–L3 (`enum`/`code`)** — completion and ambiguous-crash path enumeration
   must be redone if another worker or recovery writer is added.
3. **L5 (`code`)** — report remains aggregate-only and must stay a conjunction
   with operation-specific success.
