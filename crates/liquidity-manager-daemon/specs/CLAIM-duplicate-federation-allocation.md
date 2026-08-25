# CLAIM-duplicate-federation-allocation: Duplicate federation allocation

For any federation id, production public deliveries can durably create at most
one `allocations` row and that allocation can durably contain at most one
`allocation_items` row for each source type. The result holds for concurrent
identical or conflicting signed `RequestLiquidity` deliveries, response loss
and retries, and crashes before or after every SQLite statement or commit.

## Status

Unverified.

## Assumptions

- **A1 — SQLite integrity.** Primary/unique constraints and one transaction's
  atomicity/durability have SQLite's documented semantics; the official daemon
  is the sole database writer.
- **A2 — process model.** A crash preserves commits and discards uncommitted
  transaction work; post-restart requests use the same database.
