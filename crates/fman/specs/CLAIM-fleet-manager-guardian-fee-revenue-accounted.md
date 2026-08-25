# CLAIM-fleet-manager-guardian-fee-revenue-accounted: Guardian-fee revenue is completely accounted

For a current Manifold federation, this daemon never silently represents or
votes for a guardian-fee policy that the pinned Fedi payer will reject, and it
never knowingly votes for a payer-valid policy that violates the authenticated
MVP split:

1. a federation cannot pass `StartDKG` unless every guardian code is a
   canonical Manifold envelope whose upstream Iroh setup code, FMan identity,
   and full guardian-fee account are bound by the setup API endpoint key;
2. every whole-metadata object this daemon submits either contains no
   guardian-fee keys, or contains both keys, a `0..=210,000`-ppm rate, and the
   canonical payer-parseable `FI=4`, every guardian `=1`, Fedi `=1` recipient
   vector derived from the endpoint-verified DKG transcript, consensus seat
   directory, and environment profile;
3. a completed policy read reports `pays_us = true` only when the payer parser
   accepts both keys, the rate is nonzero, and the canonical recipient list
   names this seat's mnemonic-derived account with a positive share; and
4. accepted setup-payment claims, guardian-fee collection, and payment- or
   guardian-fee payout cannot substitute an adversarial destination. A payout
   uses the operator-configured destination durably bound to its request before
   any native Lightning operation starts, including across replay and recovery.

Every recipient account must be unique. An FI account colliding with a guardian
or Fedi account, a guardian colliding with Fedi, or a purported combined
FI-and-guardian weight-five entry fails closed. Setup payments and every fee
stream other than the ongoing per-transaction federation guardian fee remain
outside the split.

The adversary may send arbitrary public inputs, control relays, control an FI,
control hosted-federation peers through threshold, crash and restart the daemon,
and race verbs. The local operator, mnemonic, owner-only admin surface, official
binary, and data root are trusted. A hostile threshold can adopt a different
metadata value without this guardian's vote; the claim promises that this daemon
exposes a later non-paying read and does not copy the hostile fee policy forward
in a subsequent unrelated whole-object vote. It does not promise that one
guardian can prevent threshold replacement, force a payer to remit, or guarantee
Lightning settlement.

## Status

Falsified by three independent current-source counterexamples:

- [`StartDKG` accepts bare setup codes without the claimed account-bearing
  envelope](CLAIM-fleet-manager-guardian-fee-revenue-accounted/falsification-bare-dkg-codes.md).
- [A jointly hostile directory and matching fee vector are copied forward in an
  unrelated vote](CLAIM-fleet-manager-guardian-fee-revenue-accounted/falsification-hostile-directory-copy-forward.md).
- [The configured LNURL string does not bind the final Lightning
  payee](CLAIM-fleet-manager-guardian-fee-revenue-accounted/falsification-lnurl-payee-substitution.md).

## Assumptions

- Accepted setup-payment claims and guardian-fee collection use ordinary client,
  module, account, and operation authority, never the guarded seat's
  guardian/admin authority or direct child database access; this imports only
  that part of
  [CLAIM-fleet-manager-value-moves-use-client-authority](CLAIM-fleet-manager-value-moves-use-client-authority.md).
- Payment- and guardian-fee payout, including native Lightning start, replay,
  observation, refund/reclaim state machines, and lazy recovery, uses ordinary
  payment- or guardian-scoped Fedimint client authority rather than the guarded
  seat's guardian/admin authority or direct child database access.
- **A1 language, storage, and local-operator integrity:** safe Rust, async
  capture, database transactions, and serialization behave as written; SQLite
  and the Fedimint wallet database survive acknowledged commits; the operator
  controls the payout destination and the binary/data root are not replaced
  underneath the daemon.
- **A2 cryptographic derivation and signatures:** BIP-39/HKDF, secp256k1,
  Ed25519/Iroh signatures, canonical JSON signing, and hash commitments have
  their stated collision and unforgeability properties.
- **A3 payer contract:** the deployed Fedi payer reads
  `fedi:guardian_fee_send_ppm` and
  `fedi:guardian_fee_remittance_account` together, retains its generic
  210,000-ppm compatibility ceiling, and accepts the predecessor single-account
  form or strict version-1 weighted list. Each v1 entry contains a validated
  single-sig `BtcDepositor` `account`, its matching `account_id`, and positive
  `weight`; ids are unique and strictly sorted, total weight does not overflow,
  the list contains 1--32 entries, and unknown fields/versions are refused.
  This outside-repository interface is pinned to fedi#11816 commit
  `995a80f367` and cross-repository canonical-vector tests.
- **A4 pinned client semantics:** stability-pool, mint, and native Lightning
  clients enforce authenticated ownership, consensus, and their documented
  operation-log/state-machine contracts. A successful payout start commits its
  metadata before returning; operation enumeration returns committed metadata
  after restart. Collection and payout may incur declared fees, balancing
  dust/rounding, consolidation, and availability-dependent refund/reclaim
  behavior, but those state machines do not redirect principal to a different
  caller-selected destination.
