# CLAIM-manifold-guardian-restore-adopts-authentic-consistent-state: Restore adopts only authentic, consistent state

A successful `OnboardFromBackup` never durably adopts either:

1. an identity, seat fact, payment record, guardian archive byte, or
   consensus-observed guard from a document that was not authentically produced
   under the recovered mnemonic's derived backup keys; or
2. an internally inconsistent set of authentic documents.

Subject to A4, relay bytes may be forged, reordered, duplicated, or replayed.
The daemon may crash at any statement or await boundary during recovery
and installation. The host, operator-supplied mnemonic, database, and seat data
root are otherwise honest and exclusively operated by this implementation.

The enumerated adoption boundary ends at the identity/seat transaction and the
four restored guardian-config files. A subsequently started `fedimintd` may
populate its consensus database from federation peers; that separate network
input is explicitly outside the artifact domain above and is owned by axiom A3
(see the adjacent-boundary section below).

Pure omission is not quantified as forged adoption: a relay that withholds
documents and thereby makes recovery fail or produces an incomplete fleet is an
availability failure, recorded below. Freshness is an assumed property of the
configured backup source, not a cryptographic guarantee. An older authentic
backup or stale local snapshot can resurrect a retired guardian; that rollback
is outside the claim, not prevented by publication readback.

## Status

Unverified.

## Assumptions

- **A1 cryptography:** HKDF domain separation behaves as a PRF; SHA-256 is
  collision and second-preimage resistant; and NIP-44 v2 decryption
  authenticates its ciphertext. In particular, without the mnemonic-derived
  backup secret an adversary cannot make `unseal` accept a new or altered self-addressed ciphertext.
- **A2 durable single-host execution:** SQLite transactions and constraints,
  Tokio filesystem operations, and process-crash semantics behave as specified;
  committed writes survive a crash. No other process or operator mutates the
  database or seat directories during onboarding.
- **A3 fedimintd peer catch-up:** when a restored seat's pinned `fedimintd`
  populates its consensus database from hosted-federation peers, it validates
  that peer-supplied history under hostile bytes; the guarantee is bounded by
  the hosted federation's threshold honesty. No lemma in this record uses A3:
  it exists to own the adjacent boundary below, restating the composition
  root's A-fedimintd-protocol for this input (root owner ratification).
- **A4 latest-backup recovery:** a successful recovery receives the latest
  published state for each recovered seat, including retirement/removal
  tombstones and their referenced material. The configured relay does not
  substitute older state or omit a newer tombstone. Operators never restart
  stale local database/data snapshots. There is no independent anti-rollback
  checkpoint enforcing this premise.
