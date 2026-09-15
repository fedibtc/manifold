# Proof for CLAIM-allocation-completion-has-item-specific-settlement

## Scope

This is an implementation-grounded proof of
[CLAIM-allocation-completion-has-item-specific-settlement](../CLAIM-allocation-completion-has-item-specific-settlement.md).
It covers the two production allocation workers, their shared wallet-operation
and chain-evidence path, and the only allocation-item completion writer.

## Model and quantifiers

For every production allocation item that durably enters `completed`, the
settlement contribution must identify the item's persisted destination and
amount, must be exclusively claimed by its funding operation, and must be
observed through that source's target-side settlement path.

The contribution may come from a third party. Provider-wallet provenance,
whether a provider-wallet debit occurred, and the accounting or recovery of any
separate provider-wallet effect are outside this local implication.

## Assumptions

The immediate assumptions are listed in
[the claim record](../CLAIM-allocation-completion-has-item-specific-settlement.md).
They grant database integrity, truthful dependency observations, the
external-effect boundary, and correct non-reuse when manual reconciliation has
only a transaction id rather than an exact output index.

## Argument

1. **`enum + code` — there is one completion writer and two callers.**
   `allocation_store::complete_item` is the only production writer of
   `ItemAllocationStatus::Completed`. Its callers are
   `gateway_allocation::complete_if_gateway_funded` and the successful
   stability path in `stability_allocation::complete_stability_pool_item`.
2. **`schema + code` — exact chain evidence belongs to at most one funding
   operation.** `wallet::claim_chain_evidence` selects one output matching the
   operation's persisted address, amount, and any existing txid/output index.
   It performs selection and assignment under SQLite's write lock, rejects
   multiple candidates, and respects the unique `(txid, tx_vout)` ownership
   constraint. An output already owned by another operation is not eligible.
3. **`code` — gateway completion requires the exact contribution to be claimed
   into the intended federation.** The gateway worker waits for the funding
   operation to complete, then requires a configured-gateway deposit claim
   matching its txid, its output index when known, and at least the item's
   committed amount. It records the intended federation's observed balance and
   only then completes that item with its wallet-operation id and settlement
   txid.
4. **`code` — stability completion follows the item's exact target operation
   lineage.** The stability worker observes the funding operation's exact output
   through the item's persisted peg-in operation, requires the claimed target
   amount, submits the item's persisted exact-amount `deposit_to_provide`
   operation id, and completes only after that operation reports `Success` and
   the intended provider account reports the committed amount.

Together these steps establish item-specific target settlement without relying
on who supplied the on-chain contribution.

## External-contribution scenario

If the original gateway funding call fails before broadcast and leaves a
txid-less `in_doubt` operation, a third party can pay the exact persisted
address and amount. Chain reconciliation may exclusively bind that output to
the operation, and the configured gateway may claim it into the intended
federation. Completion is valid under this claim: the intended recipient
received the required liquidity and the contribution cannot also settle
another operation.

That scenario does not show a provider-wallet debit or loss. The worker does
not automatically resubmit an `in_doubt` or completed operation. Capacity
accounting conservatively charges the operation until a later provider-wallet
balance read; because this scenario caused no debit, that later unchanged
balance correctly makes the contribution consume no provider-wallet capacity.
A distinct already-running invocation that sends after terminalization is
instead the counterexample recorded by
[CLAIM-post-cancellation-effect](../CLAIM-post-cancellation-effect.md), and
provider-wallet capacity accounting remains governed by its own claims.

## Residuals

- Exact output evidence proves item-specific contribution, not provider-wallet
  provenance.
- A provider withdrawal that remains `in_doubt` or an item in
  `action_required` is intentionally nonterminal; this claim does not promise
  automatic recovery or liveness.

## Weakest links

1. A manually completed wallet operation may have no output index, so gateway
   completion then binds the gateway claim by txid and amount. Assumption A4,
   rather than a schema constraint, prevents reuse in that path.
2. The target APIs and chain observer are trusted to report the observations
   the claim consumes.
