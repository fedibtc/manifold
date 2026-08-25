# Current argument

## Argument

**L1 (`code`) — acceptance passes a detached setup snapshot through verification
and planning.** `request_liquidity` loads `StoredSetupState`; verification awaits
outside the write transaction; `plan_allocation` later uses the old `config`.
No allocation transaction reads `setup_state.revision` or rechecks readiness
([`public.rs`](../src/public.rs)).

**L2 (`code`) — Admin can atomically supersede that snapshot.**
`apply_setup_config` and `update_provider_config` update the singleton
`setup_state` row and increment `revision` before advertisement reconciliation
([`setup_store.rs`](../src/setup_store.rs), [`admin.rs`](../src/admin.rs)).

**L3 (`code`) — a stale verification commits.** Begin a request
under revision R, pause its verifier, commit Admin revision R+1 removing the
trust/policy/source that admitted it, then release verification. L1 opens its
transaction with R's config and commits the allocation after R+1. A1 makes that
allocation durable. The claim is false.

## Residual windows

- An existing allocation's idempotent response is excluded: it creates no new
  allocation, per `SPEC-flip-rpc`'s semantic-idempotency rule.
- This does not decide whether a later revision should cancel already accepted
  work; `SPEC-flip-admin-api` separately states the operational contract for
  in-flight accepted allocations. This record names only the earlier,
  new-allocation authorization fence.

## Weakest links

1. **L3 (`code`)** — delayed verifier/Admin race.
2. **L1–L2 (`code`)** — omitted revision comparison and update transaction.
3. **A1–A2 (`axiom`)** — SQLite and dependency scheduling.
