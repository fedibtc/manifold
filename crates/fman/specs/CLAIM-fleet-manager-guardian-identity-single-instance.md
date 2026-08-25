# CLAIM-fleet-manager-guardian-identity-single-instance: A guardian identity runs in only one Fleet Manager instance

Under A-operator, no two live Fleet Manager instances with different data roots
host the same formed guardian identity, despite arbitrary daemon crashes and
restarts, concurrent onboarding on the same or different hosts, child crashes
and respawns, and restore racing an old host.

An **instance** is identified by its data root. “Guardian identity” means the
whole formed guardian seat: its consensus key material is the immutable
archive produced by DKG, while its Iroh endpoint keys and API authentication
derive from the root mnemonic and seat id. The mnemonic locates and decrypts
the archive; it does not derive the consensus shares. Merely running an FMan
with no formed seat is not a live guardian.

This claim deliberately assigns cross-data-root mnemonic ownership to the
human operator. The daemon enforces the restore acknowledgement and local
empty-install guards; it does not implement a distributed per-identity lock.

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
  is the human constraint accepted by `SPEC-nostr-backup-restore`: the daemon
  requires the assertion but cannot observe or enforce its global truth.
