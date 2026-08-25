# Current argument

## Argument

**L1 (`enum` + `code`) — automatic submission IDs are locally random.** The sole
production submission call path is `advance_stability_deposit` ->
`StabilityPoolBackend::submit_deposit_to_provide` ->
`FedimintStabilityPoolBackend::submit_deposit_to_provide` ->
`deposit_to_provide_with_operation_id`. `StabilityDepositOperationId` has a private
field. The automatic path creates it only through `OperationId::new_random`;
the automatic persisted string is parsed and rehydrated only after the complete
`submitting` tuple is classified; malformed IDs and zero amounts fail closed as
forced by `malformed_and_zero_persisted_tuples_fail_closed_after_decoding`.
`target_recovery` is the separate operator constructor/writer: bind parses the
operator-selected ID, exact inspection verifies an existing deposit, and
`bind_and_resume` stores it without calling submission. Public request data
cannot reach the automatic constructor.

**L2 (`enum` + `code`) — one complete bounded request is durable before effects.**
The production `step_json` writers are allocation-store step update/CAS paths and
target-recovery bind. The automatic branch computes an amount no larger than `C`,
generates `(id, amount, fee)`, and exact-step-CASes the complete tuple plus
`submitting` before its first backend call. A zero-row CAS returns without calling
the backend. A winner thereafter reloads the tuple; it never regenerates one
member. Before backend use, semantic validation requires amount `C`, fee no
greater than one billion ppb, and lossless checked sat-to-msat conversion;
failure enters `action_required`. `complete_submission_tuple_round_trips_without_regeneration` tests the
serde boundary, while `stability_pool_allocation_completes_with_deposit_evidence`
tests one ordinary bounded submission. CAS-loss ordering remains a code rung.

**L3 (`enum` + `code` + `test`) — every automatic exit preserves or terminates the
fence.** A backend error propagates without clearing the tuple. Success changes
only status to `initiated`; later passes observe its ID. An incomplete
`submitting` tuple enters `action_required` before any backend call, covered by
`legacy_incomplete_submission_requires_operator_action`. Startup recovery merely
reloads active rows. Operator retry rejects unsafe wallet states; an accepted retry resets status
while preserving `step_json`, so the next worker uses the tuple. Cancellation
rejects unsafe wallet states; accepted cancellation and abandon terminalize the
item, so the active-item query cannot submit it. The enclosing SQLite write
transaction and worker step CAS order concurrent winners under A1; `cancel_item_tx`
has no independent CAS. Bind is accepted
only for `action_required`, records a verified existing operation, and resumes at
observation rather than submission; its tests cover bind replacement, validation,
and resume. A concurrent worker or operator winner is ordered by the enclosing
SQLite transaction and the worker step CAS.

**L4 (`code` + A1-A3) — exact lookup closes every ambiguous-result window.** Before
calling Fedi, the concrete backend asks the global operation log for the persisted
ID. A matching stability-provider entry with the exact version, role, item ID,
amount, and fee commitment returns without another operation. A mismatching entry
moves the item to `action_required` with its fence retained. Absence alone calls
the caller-ID API. By A3, pre-commit failure creates neither receipt nor network
submission; commit creates one receipt and one transaction before the executor can
submit it. A crash or lost response after commit leaves the receipt, so the next
serialized pass returns without calling Fedi. Thus absence can lead to at most one
committed operation for the ID. The request tuple never changes, and A2 plus the
official-writer enumeration associates that ID with this automatic request.

**Conclusion.** L1 keeps automatic identity out of request/operator input, L2
publishes one amount-bounded immutable request, L3 preserves or terminalizes it
through every official competing path, and L4 converges retries on one committed
client operation. Therefore the claimed duplicate or excess committed operation
cannot occur under A1-A4. ∎

## Residual windows

- A legacy or corrupt incomplete tuple fails closed and requires operator
  reconciliation; this claim does not promise automatic recovery for it.
- Reusing one operation ID for different work, direct database rollback, restored
  stale backups, target-client data loss, external clients, and random collisions
  violate A2-A4.
- The claim is about committed target-client operations. Federation acceptance,
  executor retry behavior, terminal outcomes, and recovery of stranded e-cash are
  separate properties.

## Weakest links

1. **A3 (axiom):** the guarantee bottoms out in the exact Fedi/Fedimint transaction
   ordering and must be rechecked when either pin changes.
2. **L3 (`code`):** manual-operation and worker CAS interactions lack one combined
   concurrency test.
3. **L1/L2 (`enum`):** constructor, submission-call, and writer enumerations must be
   regenerated on scoped changes.
