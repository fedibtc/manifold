# CLAIM-stability-deposit-rejection-releases-capacity: Stability deposit rejection releases capacity

If a claimed stability-pool peg-in's recorded `deposit_to_provide` operation is
rejected, FLIP has an official Admin operation that fails that item, releases the
provider capacity its reservation held, and durably records the amount left
behind in the target client.

The claim is about **capacity**, not about the value. It says the provider can
keep allocating after a federation refuses provision. It does not say the e-cash
comes back; see `## What this record does not claim`.

## Status

Unverified.

## Assumptions

- **A1.** A claimed Fedimint wallet peg-in makes e-cash spendable in FLIP's
  target client; a normal stability-pool deposit can be rejected.
