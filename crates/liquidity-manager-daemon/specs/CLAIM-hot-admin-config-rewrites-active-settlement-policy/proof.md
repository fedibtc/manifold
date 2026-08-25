# Current argument

## Argument

**The implementation uses the claim's second route**: the update is
rejected until the affected work terminates. No per-item snapshot exists, so
every lemma below is about refusing writes rather than about capturing inputs.

**L1 (`code`) — one persisted carrier, one production writer.** Regenerated from
the schema and the write side rather than from any module's callers.
`setup_state.config_view_json` is the only column in the schema that carries a
`FundingPolicyConfig`; `setup_state` is a singleton (`id INTEGER PRIMARY KEY NOT
NULL CHECK (id = 1)`), and no `DELETE FROM setup_state` exists anywhere in the
crate. Every production write of that column goes through
`upsert_setup_state_tx` ([`setup_store.rs`](../src/setup_store.rs)), reached from
exactly four paths: `apply_setup_config`, `update_provider_config`,
`refresh_stored_bitcoind_password_flag`, and `adopt_local_iroh_endpoint_address`.
Remaining `setup_state` writers are test-only — `advertisement.rs:1086` and
`:1761`, `public.rs:2074` and `:2137`, and `test_support.rs:371`, the
last of which is `#[cfg(test)]` at `lib.rs:43`.

**L2 (`code`) — every one of those writes is a compare-and-set on the revision
its caller read.** `upsert_setup_state_tx` takes an `expected_revision`. Its
statement predicates the update arm on `WHERE setup_state.revision = ?` and
returns the resulting revision, and the write counts as landed only when that
value is `expected_revision + 1`. A refused write returns no row; an insert
against a non-zero expectation returns `1` and is likewise refused. So a write
built on a read that something superseded cannot land, whichever of the four
paths built it. This is what makes paths 3 and 4 sound without a policy
predicate of their own: they each move one field and write the whole view back,
so before this they could revert a policy change they never inspected. They still
do not inspect it. They can no longer revert it.

**L4 (`code`) — the revision fence does not subsume the guard, and both are
load-bearing.** Admitting an allocation item does not write `setup_state`, so it
does not move the revision. A config writer that reads revision `R`, has an item
admitted under `R` while it validates, and then commits at `R` passes L2's fence
untouched. What refuses it is the in-transaction count, and `Database::begin_write`
opens `BEGIN IMMEDIATE` ([`database.rs`](../src/database.rs)), so that count sees
every allocation committed before it and blocks any that would commit after.
Whichever of the two transactions loses is refused — the late allocation by this
guard, the late config change by the admission path's setup-revision fence at
`public.rs:242`.

**L5 (`code`) — the refusal route is required, because no worker reads a
snapshot.** Gateway dependencies use `ready_gateway_config` at worker time, which
returns the currently stored setup view rather than an allocation-time
funding-policy snapshot. The stability worker obtains its current `setup` through
`configured_wallet` and reads its funding policy before its target operation, and
the wallet sync path reads `confirmations` (`funds_admin.rs:125`) and
`in_doubt_review_after_secs` (`funds_admin.rs:155`) live for in-flight work
([`gateway_allocation.rs`](../src/gateway_allocation.rs),
[`stability_allocation.rs`](../src/stability_allocation.rs),
[`funds_admin.rs`](../src/funds_admin.rs)). Accepted items persist target data but
no policy revision. They still do not, and no longer need to: an input that cannot
change while the work runs does not have to be captured per item.

## Residual windows

## Weakest links

1. **L1 (`code`)** — the enumeration of persisted carriers and production
   writers. This is the shape that failed in every record this sweep refuted, and
   it is the first thing to regenerate.
2. **L2 (`code`)** — that the upsert statement is a genuine compare-and-set in
   all four of its cases, including the insert arm, which no `WHERE` reaches.
3. **L4 (`code`)** — that `BEGIN IMMEDIATE` orders the config write against the
   allocation commit, and that admission really does not move the setup revision.
4. **A1 (`axiom`)** — external value-effect meaning.
