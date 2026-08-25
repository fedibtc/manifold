# Current argument

## What this record does not claim

**The value is not recovered.** After this operation the e-cash remains with the
target federation client, and FLIP has no operation that returns it: there is no
target-client send, no stability withdrawal, and no operation reconciliation on
the Admin surface, and startup recovery does not reopen a failed item to try. The
failure text and the audit row both say so in as many words.

## Argument

**L1 (`code`) — a rejected deposit lands the item in `action_required`, still
reserving.** The worker submits provider funds before it observes the peg-in as
`Claimed`, and only then submits and records `deposit_to_provide`.
`observe_stability_deposit` maps `StabilityDepositStatus::Failed` to
`require_item_action` (`stability_allocation.rs`), because the peg-in is claimed
and the e-cash sits with the target client unprovided. `ActionRequired` is a
member of `RESERVING_ITEM_STATUSES` (`allocation_store.rs`), so the item goes on
holding its reservation. This is the state the operation below exists to resolve.

**L2 (`code`) — `abandon_target_client_value` moves the item out of that status
under a compare-and-set.** The Admin verb refuses an empty reason, refuses an
unknown federation, refuses an item that is not `ActionRequired`, and refuses an
item whose value never reached the target client — that last guard requires
either a corrupt step record or `peg_in_status == "claimed"`, so `retry` and
`cancel` remain the answer while nothing has been sent. It then updates the item
to `Failed` with `WHERE item_id = ? AND status = ?` bound to `ActionRequired`,
and returns `failed_precondition` when `rows_affected() != 1`
(`target_recovery.rs`). A concurrent status change therefore loses rather than
double-releasing.

**Three terms read `RESERVING_ITEM_STATUSES`**, which is exactly
`[Pending, Running, ActionRequired]`, and `Failed` is not a member. Two of them
fall when L2's row update commits:

1. `wallet_accounting_sums`'s `in_flight_allocations` — `SUM(reserved_amount_sats)`
   over items in that set.
2. `active_reserved_amount_tx` — the same sum, read inside the admission
   transaction (`wallet.rs`).

**The third moves the other way at the same instant.**
`active_wallet_withdrawal_amount_tx` charges an outgoing wallet operation whose
debit is not yet known to be in the observed balance, and it *excludes* any
operation whose `item_id` is in a reserving item — because
`active_reserved_amount_tx` already subtracts that item's reservation, and
counting both would refuse capacity the provider has. When the item leaves the
reserving set, that exclusion lifts, so **the item's own funding send starts
being charged** and stays charged for about one balance-observation cycle, until
a persisted observation whose read tick follows the send's
`released_tick`/`settled_tick` watermark releases it.

Net capacity is released and stays released, but not every accounting term
drops at the same commit.

**So the claim is an outcome:** from the abandon commit onward, one rejecting
federation cannot permanently consume provider capacity. It is not the stronger
statement that every capacity term drops at that commit.

Making the third term drop at the same commit would change the exclusion logic
in the settlement-watermark path. Existing serialization constraints require
the current transient: the mechanism keeps the debit charged until a persisted
balance observation covers it rather than releasing capacity prematurely.

No separate release write exists or is needed for the first two terms, and that
is the reason this holds rather than an accident of it.

**L4 (`code`) — the abandoned amount is durably recorded twice.** The write
transaction stores the operator's reason and the amount in the item's
`failure_json` as a `StabilityPoolFailed` failure whose text states the value
remains with the target federation and needs recovery outside FLIP, and inserts
an `audit_log` row carrying `federation_id`, `reason`, `abandoned_amount`, and
outcome. Both are in the same transaction as the status update, so capacity
cannot be released without the amount being recorded.

L1 to L4 together give the claim: a rejected provide after a claimed peg-in
leaves an `action_required` item holding a reservation, one authenticated Admin
call fails it under a compare-and-set, the reservation leaves both capacity terms
because the new status is outside the reserving set, and the amount left behind
is durably recorded in the same transaction.

## Residual windows

- The FI does not receive the e-cash; the loss is a provider operational lockup.
- Extracting FLIP's target-client secret and operating an external client is
  outside the official remediation surface.
- The operation is operator-driven. Nothing releases the capacity automatically,
  so an unattended deployment holds the reservation until someone calls the verb.
- `abandon_target_client_value` reaches no `operator-ui` feature and no
  `packages/types` entry, so today the operator route to it is `curl`.

## Weakest links

## Tests

`target_recovery.rs` covers each part: `abandoning_releases_the_capacity_the_item_held`,
`abandoning_records_the_value_left_behind`,
`abandoning_is_refused_before_value_reaches_the_client`,
`abandoning_requires_a_reason`, and
`abandoning_is_refused_for_an_item_not_awaiting_action`.
