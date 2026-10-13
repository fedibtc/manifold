# CLAIM-manifold-guardian-paid-seat-payment-verified: Paid seats have their exact verified payment

Every seat newly allocated through the official production `CreateSeat` path
from a verified signed quote carrying payment terms is inserted atomically with
an `ecash_claims` row whose typed evidence Manifold Guardian verified as the exact
quote-bound payment for the quote's signed gross amount, after verifying the
requesting FA. Only Manifold Guardian controls the verified payment's spend keys.

## Status

Unverified.

## Assumptions

- Signature, hash, aggregate mint-signature, key-derivation, and random-secret
  schemes satisfy their stated security properties.
- The Manifold Guardian wallet root and its derived note secrets remain confidential.
- The configured Fedimint client supplies the joined federation's authentic mint
  keys and faithfully finalizes external mint-v1 and mint-v2 issuance.
- The official daemon wires `CreateSeat` to the production wallet verifier;
  alternate library embedders do not call the fleet or storage APIs directly.
- Committed SQLite writes preserve the paid seat's quote ID, FA identity, and
  typed claim evidence; the data-root lock excludes another writer.
