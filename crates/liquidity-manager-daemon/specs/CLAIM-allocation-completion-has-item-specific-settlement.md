# CLAIM-allocation-completion-has-item-specific-settlement: Allocation completion has item-specific settlement

No production allocation item can durably become `completed` unless its
reported fulfilled amount reached that item's persisted target through
source-specific settlement evidence:

- a gateway item must bind one exact on-chain output to the configured
  gateway's claim into that target federation; and
- a stability-pool item must bind one exact on-chain output to the target-client
  peg-in and that item's recorded `deposit_to_provide` operation.

One on-chain output cannot settle more than one funding operation. This claim
does not require the settlement contribution to originate from FLIP's provider
wallet and does not by itself assert that the provider wallet incurred a debit.

The adversary is a hostile FI with an accepted, endorsed federation, able to
schedule deliveries, target-federation credit, ordinary third-party deposits,
network responses, worker ticks, and crashes at every await. It cannot forge
an issuer/FMan, write FLIP's database, or compromise the provider wallet,
gateway, chain observer, or target federation's consensus responses.

## Assumptions

- **A1 — SQLite/process integrity.** Committed SQLite transactions are atomic
  and durable across ordinary crashes; the official daemon is the sole writer.
- **A2 — honest dependency observations.** Wallet, chain, gateway, and target
  federation APIs return their documented truthful observations, but aggregate
  balances/reports do not by themselves identify their causal operation.
- **A3 — external-effect boundary.** A provider-wallet withdrawal and
  target-federation operations are irreversible external effects; their
  responses may be delayed, lost, or reordered relative to local persistence.
- **A4 — manual reconciliation.** When the operator marks a funding operation
  completed with an asserted transaction id, that transaction uniquely carries
  the operation's intended output and is not assigned to another operation.
