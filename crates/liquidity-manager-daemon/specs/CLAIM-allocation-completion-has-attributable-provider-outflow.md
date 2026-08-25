# CLAIM-allocation-completion-has-attributable-provider-outflow: Allocation completion has attributable provider outflow

No production allocation item can durably become `completed` unless its
reported fulfilled amount is attributable to value that FLIP caused to leave
its provider wallet for that item's persisted target. Attribution is
source-specific:

- a gateway item must bind its provider-wallet operation's exact on-chain
  output to the configured gateway's claim into that target federation; and
- a stability-pool item must bind its provider-wallet operation's exact output,
  the target-client peg-in, and that item's recorded `deposit_to_provide`
  operation.

The adversary is a hostile FI with an accepted, endorsed federation, able to
schedule deliveries, target-federation credit, ordinary third-party deposits,
network responses, worker ticks, and crashes at every await. It cannot forge
an issuer/FMan, write FLIP's database, or compromise the provider wallet,
gateway, chain observer, or target federation's consensus responses.

## Status

Unverified.

## Assumptions

- **A1 — SQLite/process integrity.** Committed SQLite transactions are atomic
  and durable across ordinary crashes; the official daemon is the sole writer.
- **A2 — honest dependency observations.** Wallet, chain, gateway, and target
  federation APIs return their documented truthful observations, but aggregate
  balances/reports do not by themselves identify their causal operation.
- **A3 — external-effect boundary.** A provider-wallet withdrawal and
  target-federation operations are irreversible external effects; their
  responses may be delayed, lost, or reordered relative to local persistence.
