# Current argument

## Argument

**L1 (`enum`) — `allocations` has exactly one production writer, and no
production update or delete.** `allocation_store::insert_allocation` issues the
only production statement against the table, an `INSERT OR IGNORE`. The three
other inserts are test-only: `recovery.rs` and `funds_admin.rs` both place
theirs below their `mod tests`, and `test_support.rs` is test support. No
production `UPDATE` or `DELETE` targets the table at all. Regenerate with:

```text
rg -n 'INTO allocations|UPDATE allocations|DELETE FROM allocations' crates/liquidity-manager-daemon/src
```

**L2 (`code`) — that writer has one caller, reachable only past the endorsement
gate.** `public.rs` holds the only call site. It runs after `verify`
returns no rejection, and stage 0 of the gate rejects a request carrying no FMan
endorsement. No other path reaches the insert.

**L3 (`code`) — one endorsement admits at most one federation id.** Three
comparisons force the same id end to end: the gate requires the attestation's
federation id to equal the invite's; `verification.rs` requires
`preview.federation_id == admitted_federation_id` and
`details.federation_id == preview.federation_id`; and `insert_allocation` binds
the primary key from that same declared field. Every comparison is a byte-exact
`String` equality, so a holder cannot point one endorsement at a second
federation.

**L4 (`schema`) — the primary key makes the per-federation bound mechanical
rather than argued.** `allocations.federation_id` is the table's primary key,
and the writer is `INSERT OR IGNORE`. A second admitted request for a federation
that already has a row therefore affects zero rows, including when two arrive
concurrently: the loser inserts nothing rather than erroring or duplicating.

**L5 (`code`) — admission has a second gate the token does not cover.**
`plan_allocation` must return a plan, or the request is refused
`InsufficientCapacity` with no durable write. Wallet-backed capacity for the
requested minimum plus fee reserve therefore gates every insert as well. This
does not weaken the claim — it is a further necessary condition, not an
alternative path — but "unless every increase first spends a token" should not
be read as "the token is the only gate".

**L6 (`code`) — fixture mode opens no second path.** `--trust-fixtures`
substitutes the invite-code preview provider only. The endorsement gate, the
installed-issuer check, the badge envelope check, and the fail-closed revocation
lookup all remain in place, so a fixture-backed deployment admits rows through
the same gate this argument walks.

**Conclusion.** By L1 the only way to add a row is that writer, by L2 it is
reachable only past the gate, and by L3 with L4 each distinct endorsement yields
at most one row while repeats add none. Reaching `K` retained rows therefore
requires `K` distinct admitted federations, each with its own endorsement, which
is exactly the token expenditure the claim demands. Under A5 that supply is
bounded, so the claim holds.

## Residual windows

## Weakest links

Ranked weakest first:

1. **A5 (`axiom`)** — it carries the whole implication. FLIP bounds rows per
   endorsement; nothing in this binary bounds how many endorsements exist. If
   A5 is false for a deployment, the claim says nothing useful about it.
2. **L1 (`enum`)** — writer completeness is regenerated per check and enforced
   by nothing. A second production writer, or any production `DELETE`, would
   land without this lemma reading false.
3. **L3 (`code`)** — the identity chain runs through three separate comparisons
   in two files. A relaxation at any one of them lets a single endorsement
   reach a second primary key.
4. **L5 (`code`)** — the capacity gate is stated so the token is not read as the
   only one; it is not load-bearing for the bound itself.
5. **L4 (`schema`)** — mechanically enforced, and the least likely to drift.
