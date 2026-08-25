# CLAIM-stability-worker-single-target-starvation: Stability worker single target starvation

One accepted stability-pool target whose client operation never resolves cannot
prevent FLIP from processing a later active stability-pool allocation.

The network may delay all responses from one accepted target. The FI cannot
write FLIP storage, forge an endorsement, or operate the provider wallet.

## Status

Unverified.

## Assumptions

- **A1.** An awaited target-Fedimint client operation may remain pending
  indefinitely when its network peer or transport makes no progress.
- **A2.** A later active allocation is independently processable if the worker
  reaches it.
- **A3 — pinned client cancellation semantics.** `ClientBuilder::build_stopped`
  creates its own `TaskGroup`, spawns a config-refresh task holding a clone of
  the client database into it, and that group has no `Drop`. Dropping a build
  therefore detaches a task holding the client's RocksDB file lock, with no
  handle left to reclaim it. `fedimint-db-locked` exposes no non-blocking open,
  so a later open of the same database blocks on `flock`.
