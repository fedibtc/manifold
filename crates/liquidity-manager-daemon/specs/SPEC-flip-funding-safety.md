# SPEC-flip-funding-safety: Provider wallet send-once and settlement contract

## Record justification

The send-once and settlement contract is enforced jointly by the wallet
store, chain-observer sync, both allocation workers, manual operations, and
startup recovery, so no single module can own it coherently.

## Irreversible submission

Gatewayd's admin API has no idempotency key for `send_onchain` and no
reliable sent-to-address query, so FLIP owns the guard locally. Before any
send, a wallet operation is atomically persisted with its local operation id,
destination address, amount, and fee policy (and the operator's
`withdrawal_intent_id` when operator-initiated). Immediately before
`send_onchain`, one SQLite write transaction moves that operation from
`pending` to `in_doubt` only while its allocation item is still active; if
the conditional update affects zero rows, no call is made. Cancellation that
commits first therefore prevents submission, and a submission fence that
commits first makes cancellation reject. Wallet-operation creation uses the
same transaction-time active-item check. Before `deposit_to_provide`, FLIP
atomically persists `submitting`, a locally generated random Fedimint operation
ID, the amount, and the minimum fee rate. The ID never comes from request data
or an allocation identifier. A losing exact-step CAS makes no call, and an
incomplete legacy or corrupt tuple fails closed to operator reconciliation.

Every attempt first performs an exact global operation-log lookup by the
persisted ID. An entry whose stability provider action and versioned `{role, item_id, amount_sats, min_fee_rate_ppb}` commitment exactly match the persisted request is the durable receipt and advances the item to
observation without another financial submission. Absence permits calling the
Fedi caller-ID API with the same immutable tuple. Any error preserves the fence,
so a crash, lost response, or ambiguous error returns through the same exact
lookup. History bounds and wall-clock ordering do not participate in recovery.

Operator withdrawals are idempotent by `withdrawal_intent_id`
(installation-global, since the bearer token identifies the installation): an
exact replay returns the existing operation; the same id with different
address, amount, or effective fee rate is a stable conflict; a fresh id may
request an identical payment.

## Settlement evidence

Chain-driven settlement binds to one exact Bitcoin output. A known txid is
not settlement evidence by itself: reconciliation fetches its outputs and
applies the destination and amount checks before claiming an output or
advancing status. Address discovery advances only when exactly one matching,
previously unclaimed output exists; no match waits, and multiple matches are
ambiguous and stay nonterminal. The output claim and status update run in one
SQLite write transaction, and a partial unique constraint on
`(txid, tx_vout)` prevents one output from settling two operations. A top-up
begins at amount zero and claims one positive output. `in_doubt` recovery
uses destination-address monitoring plus target-side evidence (gateway
liquidity, peg-in state, stability-pool progress); exact output matching
proves the payment shape, not gatewayd provenance — target-side completion is
a separate check.

## Manual review

An `in_doubt` send whose chain-observer and target-side evidence stays missing,
ambiguous, or conflicting past the operator-configured review threshold
(`funding_policy.in_doubt_review_after_secs`, measured from submission and
hot-reloadable) becomes `manual_review_required`. The threshold governs only
when to stop waiting; it never rewrites the settlement requirements of an
already-submitted operation, and evidence that settles a send still wins after
the threshold has passed.

Escalation exists because `in_doubt` rejects guarded retry and cancellation
both, so an operation nothing will ever resolve would otherwise have no route
to a terminal state. The operator resolves a reviewed operation as completed
with a supplied txid, failed, or safe to retry; each is guarded on the
operation still being under review, and each writes an audit row. A completed
resolution records the txid but claims no exact output — chain observation owns
output attribution. `safe_to_retry` returns the operation to `pending`, which
is what lifts the never-automatically-resubmitted rule: that rule holds for
`in_doubt` and `manual_review_required` precisely because nobody has
established what happened, and a resolution is somebody establishing it. The
Admin token authenticates that someone to the installation; it does not make the
out-of-band `safe_to_retry` conclusion evidence-based or infallible. A mistaken
resolution can therefore release an externally accepted send whose response and
observable evidence remain missing for another submission.

## Target-client reconciliation

FLIP, not the stability module, owns the actual operation ID and immutable
request before effects. The target client's global operation log plus the validated versioned request commitment is the receipt namespace. A conflicting entry fails closed to operator reconciliation. Exact lookup makes recovery independent of bounded diagnostic
history, amounts shared by unrelated deposits, and non-monotonic clocks.

This caller contract requires globally unique IDs, one immutable request per ID,
and serialized same-ID attempts. The singleton daemon and sequential worker
provide serialization; the private ID type and complete persisted tuple enforce
the request association. Fedi owns its module transaction, submission state,
operation-log insertion, and commit. FLIP has no target-client write to compose
into that transaction, so the high-level caller-ID API preserves the correct
ownership boundary.

The actual Fedimint operation ID is the lookup identity. Its versioned metadata
commits the immutable request and validates the exact entry; it is not a secondary
recovery key and is never searched. A metadata-key design that scans history is a
separate alternative and is not part of this contract.

Legacy or corrupt `submitting` rows missing any tuple member fail closed with
the reservation held. Inspection remains available and its bounded history is
diagnostic only. Binding verifies an actual target deposit before resuming
observation, but rejects an unreadable whole-step record because reconstructing
its erased funding prerequisites could fund again. For such a record, audited
abandon is the sole terminal reservation-release escape; its status transition
and accepted audit entry commit atomically. Abandonment moves no money.

An operator then reconciles against the same records. FLIP reports the client's
spendable balance, its provider-account report, and the stability-pool deposits
the client records having made, alongside the operation id — if any — the item has recorded. A
deposit the client holds and the item does not name is the signature of the
interrupted window; a deposit the client never observed to completion reports no
outcome, which is a finding rather than a gap. Reporting drives no operation's
stream, so an operator asking what happened never waits on pending work.

The operator then binds one of those operations to the item. Binding is refused
unless the item is `action_required` and records no operation already, and
unless the target client really holds a deposit with that id for at least the
item's committed amount — an unverified id would attach the item to nothing and
let a later sibling deposit complete it. A bound item returns to the worker with
its deposit status reset to the earliest state, so observation re-derives
everything including the provider-report gate that completion depends on.
Binding moves no money and each outcome writes an audit row.

When the pool will never accept the deposit, none of that applies: the item
cannot complete, and after the peg-in is claimed the settled funding send makes
`cancel_allocation` and `retry_funding_step` refuse, so it would hold provider
capacity for good. The operator may then abandon it, with a required reason.
That fails the item and releases its reservation, and records the amount left at
the target client and that recovering it happens outside FLIP. Abandoning is
refused before the peg-in is claimed, where the funds have not reached the
client and the ordinary operations still apply.

Abandoning moves no money and recovers none. Returning target-client value to
the provider wallet is a peg-out and is not part of this contract.

## State monotonicity and reservation

Terminal item and wallet states are monotonic: delayed worker, sync, step, or
dependency-response writers must not replace them. Reservation is strict:
accepted allocations reserve the committed amount plus the configured fee
reserve, released only after terminal item state and wallet
settlement/accounting are reconciled; committed amounts are never
double-counted toward new requests. For stability-pool items FLIP may spend
the persisted reserved amount but reports completion only for the committed
amount, and only after observed provided liquidity covers it.

## Manual remediation guards

`retry_funding_step` requeues an `action_required` item only when every
attached wallet operation is retry-safe: failed operations without a `txid`
qualify; `broadcast`, `confirmed`, `completed`, `in_doubt`,
`manual_review_required`, `cancelled`, and failed-with-`txid` operations are
rejected, and a permanently `failed` item never reopens. A
`manual_review_required` operation is resolved through the manual-review
surface above rather than retried, which is what returns it to a state this
guard admits. `cancel_allocation`
cancels pending/running/action-required items and pending or failed wallet
operations before broadcast, and rejects when an active item has an operation
in any of those non-cancellable states. Operator cancellation is allowed only
before irreversible submission; a cancelled wallet operation is terminal.
