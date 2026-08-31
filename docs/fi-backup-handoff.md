# FI backup and restore handoff

Status: incomplete feature work on branch `fi-backup` as of 2026-08-31.

Prepared with OpenAI Codex agent `/root`. The independent identity-safety
review was performed by OpenAI Codex agent `Meitner`.

## Read first

- [`fi-nostr-backups.md`](fi-nostr-backups.md) — original design and current
  implementation status. Some older partial-formation and `ReservationId`
  sections are stale.
- [`ARCH-fi-client`](../crates/fi-client/specs/ARCH-fi-client.md) — current FI
  boundaries.
- [`fi-client/SECURITY.md`](../crates/fi-client/SECURITY.md) — identity,
  persistence, replay, and secret-handling rules.
- The current Fedi integration at
  `workspace/fedi/main/crates/bridge/src/fi_client.rs`.

## Decisions already made

- Portable backup begins only after federation formation fully completes.
- The payload contains only recovery facts that cannot be derived from the FI
  root or recovered from authoritative FMan, FLIP, federation, or wallet APIs.
- Fedi supplies `app_root.child_key(17)` as the stable FI-scoped root.
  `fi-client` derives the protocol, backup-author, and encryption keys.
- The protocol key must remain byte-for-byte compatible with existing Fedi
  staging users.
- Only encrypted backup and restore may remain public.
- `fi-cli` is a disposable development/test tool. Its identity format does not
  need compatibility work.
- Backup formats are pre-production and require no migrations yet.

## Branch history

- `f0fac6c` centralizes `base64` and `sha2` dependencies.
- `bf9dbb1` adds portable formation backup and restore.
- `a9c02c0` restricts backup to formed federations.
- `c745e0f` adds encrypted backup envelopes.
- The next commit changes identity ownership to the scoped-root design above.

## What currently works

- Formed-only portable export and empty-database restore.
- Versioned, compressed, bucket-padded XChaCha20-Poly1305 envelopes.
- Environment-separated backup author and content keys.
- `FiClient` now accepts a scoped `DerivableSecret`; the consumer signing trait
  is removed.
- A compatibility test proves the new protocol derivation equals Fedi's old
  child-17 derivation.

## Remaining work, in order

1. Replace `FiBackupPayload`'s direct `StoredFormation`, `StoredSeat`, and full
   liquidity rows with a purpose-built lean backup type.
2. Add a real backup-ready checkpoint after seat bindings and formation fee
   metadata finish. The current durable `Formed` marker is written earlier.
3. Remove public plaintext `FiBackup`, plaintext export/restore, and the
   redundant checksum/base64 inner envelope.
4. Make restore retry-safe and reconcile restored handles against FMan, FLIP,
   joined federation, and wallet state before allowing mutations.
5. Implement Nostr event signing, multi-relay publication, retry/refresh,
   seed-only lookup, candidate validation, newest-generation selection, and
   writer-conflict handling.
6. Update Fedi to pass child 17 directly, remove `BridgeFiIdentity`, update its
   Manifold pin, and add a fresh-device seed restore integration test.
7. Reconcile the stale backup-design sections and
   `SPEC-fi-post-formation-liquidity` ownership language with the final lean
   payload boundary.

## Verification completed

- `treefmt --ci`
- `cargo test -p fi-client -p fi-cli`: 287 fi-client tests and 83 fi-cli tests
  passed.
- Focused Clippy completed with existing unrelated warnings. Strict
  `-D warnings` is currently blocked by pre-existing warnings in the workspace.

The identity-safety review found no identity-continuity or secret-exposure
defect. Its stale-documentation blocker was fixed. The production-readiness
claims remain deliberately `Unverified`; no claim re-verification was run.
