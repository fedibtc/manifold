# Current argument

## Argument

**L1 (`schema`) — durable identities exclude duplicates.** `allocations` makes
`federation_id` its primary key; `allocation_items` has a unique index on
`(federation_id, source_type)`. These constraints cover every writer, including
racing transactions ([`20260716000000_initial_schema.sql`](../migrations/20260716000000_initial_schema.sql)).

**L2 (`enum` + `code`) — there is one production creator and it creates only the
planned sources.** Regenerating production inserts finds
`allocation_store::insert_allocation` as the sole creator. It inserts one parent
with `INSERT OR IGNORE`, returns `false` before inserting items on either the
federation or details-hash conflict, and otherwise inserts exactly the
`PlannedItem` list in the caller's transaction. `plan_allocation` constructs at
most one gateway item and at most one stability-pool item from nonzero requested
minimums ([`allocation_store.rs`](../src/allocation_store.rs),
[`public.rs`](../src/public.rs)).

**L3 (`code` + `test`) — all delivery races converge.** `accept_or_reject_request`
checks an existing allocation first. If the later insert loses a concurrent
race, it rolls back and returns the winner's current accepted status or a
conflict; it never attempts another item insert. The focused concurrency test
`concurrent_duplicate_request_creates_one_allocation` exercises
the contender path. A1 and A2 extend that reasoning across commits and restart.

## Residual windows

- This is allocation cardinality, not a per-requester quota: distinct endorsed
  federations can each have their own allocation.
- Rejected requests are intentionally stateless and may be retried; they create
  no allocation.
- Admin/database mutation outside the public writer domain is excluded by A1.

## Weakest links

1. **L2 (`enum`/`code`)** — new writers or sources must renew the inventory.
2. **L3 (`code`/`test`)** — conflict handling is application logic.
3. **L1 (`schema`)** — durable cardinality is database-enforced.
4. **A1–A2 (`axiom`)** — SQLite and crash semantics are trusted.
