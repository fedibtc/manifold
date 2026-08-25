# Proof: A quote settles to one outcome

> **Stale proof:** The Lean model still describes the former refund-ledger
> implementation. Fleet Manager now retains no refusal row, and the model-to-Rust
> correspondence has not been re-derived for the current settlement call paths.
> This evidence does not establish the claim at the current source state.

## Scope

The property concerns the quote decision, durable seat and payment rows, refusal
commitment, payment hand-off, exact-request replay, crashes, and concurrent
`CreateSeat` calls. The relevant implementation is in `fman-core`'s fleet,
database, and seat code, `fman-fedimint`'s payee code, and the locked-payment
service boundary.

The proof does not establish that a payment is valid or has sufficient value,
that a terminal payment status is eventually recorded, or that claiming cannot
consume unrelated wallet principal.

## Model

`lean/FMan/Settlement` contains a transition system for the older implementation.
Its durable state has a seat map and a refund-ledger map; its volatile state
mirrors accepted quotes; and its event log records acceptance responses, refund
responses, refund submissions, and payment claims. Crashes discard volatile
state and preserve durable state.

`FMan.Settlement.Claims` proves, for reachable model states:

1. `outcome_exclusive`, `seat_row_unique`, and `refund_row_unique`: one quote
   cannot have both modeled outcomes or duplicate either outcome;
2. `refund_matches_ledger` and `refund_canonical`: emitted refund bytes equal the
   modeled ledger entry;
3. `refund_committed_before_emission` and
   `refusal_response_backed_at`: the modeled refund row precedes the exits;
4. `claim_after_durable_acceptance`: a claim event follows the durable seat row;
   and
5. `accept_response_matches_row`: replayed acceptance identifies the durable
   seat.

`lean/FMan/Audit.lean` checks that the cited theorems do not depend on
`sorryAx`. The audit reports only Lean's standard `propext`, `Quot.sound`, and
`Classical.choice` axioms. This check says nothing about correspondence to Rust.

## Correspondence evidence

The retired correspondence argument mapped the older Rust implementation to the
transition system at points outside the allocation lock.
It enumerated the outcome writers and external exits, matched the allocation
lock's branch structure, treated failures before a modeled effect as stuttering,
and required every modeled exit's backing row to commit before the Rust exit.
An external exit meant an `EcashWallet` invocation or handing a signed
`CreateSeatResponse` to the caller, not successful network submission.

That argument was only hand-checked. It had no generated lockstep relation, and
its last current-callpath note acknowledged that the writer/exit table predates
the split into `handoff_stored`, `handoff_locked_v1`, `handoff_locked_v2`,
`await_claim`, and `submit_refund_transaction`. More importantly,
[SPEC-quote-settlement](../SPEC-quote-settlement.md) now specifies a
refund-less schema: a refusal has no durable row and monotone admission prevents
later acceptance. The old model therefore cannot transfer its refund-ledger
theorems to the current mechanism without a new correspondence argument.

The focused
`duplicate_snapshots_resolve_to_one_acceptance_without_a_refusal` database test
forces two duplicate requests to observe an absent seat and the same current
epoch before either enters the SQLite writer boundary. It then requires the
serialized decisions to produce one insert and one existing-seat replay, never
an epoch-mismatch refusal. This mechanically guards the current
existing-seat-before-epoch recheck, but it does not replace the missing complete
writer, exit, crash, and refund-canonicality correspondence.

## Residuals

- The Lean money theorems are not part of this claim. Their single-spend,
  spend-authority, quote-identity, and reissue-canonicality hypotheses do not
  establish the narrower Fleet Manager ordering property and do not protect
  operator principal.
- Fedimint operation recovery and terminal payment-status liveness begin after
  the durable acceptance required here.
- A network peer observing or accepting a transaction is beyond the external-exit
  boundary used by the model.

## Weakest links

The model-to-code correspondence is stale, and the current refund-less mechanism
has no machine-checked model. The focused duplicate-snapshot test covers the
reported replay/epoch interleaving but not the complete claim. A future
verification should model monotone admission directly, regenerate the complete
writer and exit enumeration, and check current replay and crash paths rather
than updating old source ranges.
