# Current argument

## Argument

**L1 (`code`) — each active stability item opens and caches a client.** The
stability worker calls `ensure_client`, which reaches
`TargetFedimintClients::create_or_load`. On success it stores a
`ClientHandleArc` in `inner.clients`, keyed by the federation id, and creates a
RocksDB under `federations_dir/<federation_id>/client.db` when needed.

L1 creates two resources with the same key, and they no longer share an answer,
so the old single L2 is split. The cache half is bounded; the on-disk half is
not, and carries the falsification alone.

**L2a (`code`) — the open-client cache is bounded.** `TargetFedimintClients`
carries a configured ceiling (`--max-open-target-clients`, default
`DEFAULT_MAX_OPEN_TARGET_CLIENTS`, resolved in `config.rs` and passed at
`daemon.rs`), and before opening one more it closes idle clients least recently
used first. Its per-federation lock map — a second unbounded map the original
single lemma did not separate out — drops entries nothing holds. Retained open
clients are therefore bounded by that ceiling plus however many clients workers
hold concurrently: a finite bound, and one not set by requester input.

**L2b (`enum`) — no production path deletes a target-client database.** Nothing
removes `federations_dir/<federation_id>/`. Allocation completion, failure, and
cancellation, startup recovery, and the cache eviction in L2a all leave it on
disk. The omission is deliberate rather than missing: that database is what the
operator's manual e-cash recovery route reads after
`abandon_target_client_value` (`target_recovery.rs`), so deleting it needs the
target-client sweep decision first. Until a sweep exists, deleting the database
is how abandoned value stops being recoverable at all.

**L3 (`concrete execution`) — capacity reuse does not bound retained databases,
and that is now outside the claim.** For each fresh endorsed federation, request a
small stability allocation, let it complete, then repeat with a new federation.
Terminal items release their reservation, allowing subsequent requests, while L1's
RocksDB remains under L2b. For every finite `K`, this reaches at least `K` retained
client *databases*.

## Residual windows

- The trace requires valid endorsements and completed allocations; it is not an
  unauthenticated database-write attack.
- Process restart clears memory but not the accumulated on-disk client stores.

## Weakest links
