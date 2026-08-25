# CLAIM-fleet-manager-payment-claim-preserves-principal: Payment claims preserve operator principal

Successfully claiming a valid paid seat does not reduce wallet principal that
the operator held before receiving that payment.

## Status

Falsified: the mint-v1 claim path can consolidate older operator notes and charge
their input fees while recording the payment claim as successful. See
[the current counterexample](CLAIM-fleet-manager-payment-claim-preserves-principal/falsification-mint-v1-consolidation.md).

## Assumptions

- The accepted payment is valid and all counterparties behave honestly.
- The pinned Fedimint mint-v1 finalizer follows its documented fee and note
  consolidation rules.
