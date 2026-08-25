# Current argument

## Argument

### Derivation DAG

```diagram
                         R: completed item has attributable provider outflow
                                              │
        ┌─────────────────────────────────────┼─────────────────────────────────────┐
        ▼                                     ▼                                     ▼
 A: one durable operation                B: exact wallet output              C: source-specific target
    is bound to the item                    is settled once                       fulfillment observes it
        │                                     │                                     │
  ┌─────┴─────┐                         ┌─────┴─────┐                    ┌──────────┴──────────┐
  ▼           ▼                         ▼           ▼                    ▼                     ▼
A1 item/     A2 pre-send              B1 exact    B2 exclusive        Cg gateway             Cs stability pool
   operation    CAS fence                output      outpoint claim      claim attribution      operation lineage
   uniqueness   before send               match        across rows         (known failing leaf)   (current target leaf)
```

**A (`enum` + `schema` + `code`) — durable operation lineage.** The only
allocation funding-operation creator is `ensure_wallet_operation`; it binds a
source-specific `WalletOperationType`, `item_id`, federation id, amount, and
persisted destination. The per-item operation lookup/index and active-item
transaction check are the first proof leaf. This leaf must enumerate creation,
retry, cancel, and recovery paths.

**B (`enum` + `schema` + `code`) — an exact Bitcoin output can settle at most
one durable operation.** `claim_chain_evidence` selects an output matching the
persisted address, amount, existing txid/vout constraints, and confirmation
policy while holding the SQLite writer; the `(txid, tx_vout)` uniqueness
constraint prevents a second operation from claiming it. This is necessary but
not sufficient: address/amount matching does not prove that gatewayd originated
the output or that the target consumed it.

**C (`enum` + `code`) — completion has two irreducible source leaves.** The
only completion writer is `complete_item`, reached by the gateway and
stability-pool workers. Gateway completion currently combines its completed
wallet operation with a federation-wide gateway balance inequality. Stability
completion follows the item's recorded peg-in and stability deposit operation,
then corroborates it with the provider account report. Both paths must be
separately re-derived; neither can borrow attribution from the other.

**Cg (`claim`, pending regeneration) — gateway target attribution is the
current attack leaf.** The existing
[`unrelated-gateway-balance-completes-item`](../CLAIM-unrelated-gateway-balance-completes-item.md)
record describes a counterexample: an independent credit can satisfy the
aggregate balance inequality without identifying this item's output-to-claim
path. Its counterexample blocks the root conclusion. This DAG does not
import it as a lemma because falsified records cannot be imports; it makes the
required re-derivation boundary explicit.

**Cs (`claim`) — stability has a distinct operation lineage.**
The regenerated
[`unrelated-stability-balance-completes-item`](../CLAIM-unrelated-stability-balance-completes-item.md)
record derives that the no-id crash boundary fails closed and the only
completion branch requires the item's saved exact-amount `deposit_to_provide`
operation to report `Success`; the aggregate provider-account report is a
conjunction, not a substitute.

## Residual windows

- Exact output evidence alone is not a provenance proof: an independent party
  can pay the same target address and amount. That is in R's adversary model,
  not an accepted residual.
- A provider withdrawal that remains `in_doubt` or an item in
  `action_required` is intentionally nonterminal; this claim does not promise
  automatic recovery or liveness.
- The trusted operator may make separate Admin withdrawals; they are excluded
  unless they supply a causal input to an allocation completion, in which case
  they are an attribution counterexample rather than a residual.

## Weakest links

1. **Cg/Cs (`enum`/`code`)** — source completion must be regenerated against
   every worker/recovery path and pinned target API behavior.
2. **B (`schema`/`code`)** — exact-output selection is durable but not source
   provenance.
3. **A (`enum`/`schema`/`code`)** — item/operation binding crosses all lifecycle
   paths.
4. **A1–A3 (`axiom`)** — database and external-effect semantics bottom out
   outside this record.
