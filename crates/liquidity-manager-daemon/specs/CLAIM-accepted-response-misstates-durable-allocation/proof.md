# Current argument

## Argument

**L1 (`code`) — validation defines the request domain.**
`validate_amount_bounds` requires at least one positive minimum, rejects a
maximum for an unrequested source, and rejects `max < min`; supported-source
validation rejects an unavailable requested source ([`public.rs`](../src/public.rs)).

**L2 (`enum` + `code`) — planning is exact and bounded.** The two `SourceType`
cases add one item only for each positive gateway/stability minimum and set its
amount exactly to that minimum. Thus each amount is at least its minimum and no
larger than its supplied maximum ([`public.rs`](../src/public.rs)).

**L3 (`code`) — durable insert precedes the first response.**
`insert_allocation` writes the parent, then every planned item, in the caller's
transaction; acceptance signs `plan.initial_status` only after `tx.commit()`.
`recovery` inventories pending/running rows after restart
([`allocation_store.rs`](../src/allocation_store.rs),
[`public.rs`](../src/public.rs), [`recovery.rs`](../src/recovery.rs)).

**L4 (`schema`) — row identities prevent a second source item.** The allocation
primary key and `(federation_id, source_type)` unique index prevent a committed
extra item of either supported source in this creator's transaction.

## Residual windows

- A later idempotent response reports current durable status, not the original
  initial status; this claim concerns the first response, as specified by the
  semantic-idempotency rule in `SPEC-flip-rpc`.
- Recoverability means the durable target, amount, source, and pending state are
  present for official recovery, not that an external dependency will eventually
  succeed.

## Weakest links

1. **L2 (`enum`/`code`)** — adding a source or changing zero/minimum semantics.
2. **L3 (`code`)** — response ordering and recovery inventory.
3. **L4 (`schema`)** — durable cardinality.
4. **A1–A2 (`axiom`)** — SQLite and protocol source semantics.
