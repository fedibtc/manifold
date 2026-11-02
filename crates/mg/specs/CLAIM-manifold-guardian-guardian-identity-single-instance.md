# CLAIM-manifold-guardian-guardian-identity-single-instance: A guardian identity runs in only one Manifold Guardian instance

No two live Manifold Guardian instances with different data roots host the same
formed guardian identity, despite arbitrary daemon crashes and restarts,
concurrent onboarding on the same or different hosts, child crashes
and respawns, and restore racing an old host.

An **instance** is identified by its data root. “Guardian identity” means the
whole formed guardian seat: its consensus key material is the immutable
archive produced by DKG, together with its Iroh endpoint keys. Native endpoint
keys derive from the root mnemonic and seat id; a transferred guardian
preserves the source's consensus material and Iroh endpoint keys rather than
deriving replacements from the destination mnemonic. API authentication
(`api_auth`) is local to the hosting MG and is not part of the identity. Merely
running an MG with no formed seat is not a live guardian.

## Status

Unverified.

## Assumptions

- **A-valid-history:** each data root begins empty or was produced by this
  implementation. No non-daemon process edits an identity database or seat
  directory, starts another fedimintd from it, or bypasses the admin API.
  SQLite commits and constraints are durable and faithfully loaded.
- **A-key-generation:** fresh BIP-39 generation does not repeat an existing
  root mnemonic, and the documented mnemonic/seat derivation does not collide
  for distinct inputs. This covers the deterministic endpoint/authentication
  credentials and backup discovery keys, not fedimint's archived DKG shares.
- **A-operator:** the operator globally serializes one root mnemonic's
  ownership across data roots. They never supply, copy, or restore the same
  mnemonic into two instances whose daemon or guardian-child lifetimes can
  overlap; before acknowledging restore, they have permanently retired every
  prior instance and they do not concurrently restore another successor. This
  is the human constraint accepted by
  [SPEC-nostr-backup-restore](SPEC-nostr-backup-restore.md): the daemon
  requires the assertion but cannot observe or enforce its global truth.
- **A-nonconflicting-FA:** the FA does not authorize the same guardian to
  conflicting destinations whose guardian lifetimes can overlap. The source
  deliberately does not pin its first grant to one destination; custody
  selection and this serialization belong to the FA.
- **A-latest-backup:** recovery uses the latest published seat state, including
  transfer retirement/removal tombstones, and operators never restart stale
  local snapshots. A source's old authentic backup can otherwise resurrect its
  transferred guardian; publication confirmation does not prevent that replay.
  This is the same external premise as A4 in
  [CLAIM-manifold-guardian-restore-adopts-authentic-consistent-state](CLAIM-manifold-guardian-restore-adopts-authentic-consistent-state.md).
