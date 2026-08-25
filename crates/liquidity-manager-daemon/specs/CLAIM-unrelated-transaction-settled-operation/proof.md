# Current argument

## Argument

### L1 — observers preserve exact output identity and integer value (`type`, `test`)

`ChainOutputEvidence` requires txid, vout, destination address, script
identity, integer satoshis, and confirmations. Esplora maps integer `value`
fields from full transaction responses. Bitcoin Core maps `vout.n`,
`scriptPubKey`, and enables serde_json `arbitrary_precision` so JSON BTC decimal
tokens are parsed to satoshis without an `f64` conversion. The focused adapter tests
`esplora_observer_reads_health_tx_and_address_evidence`,
`bitcoind_observer_reads_rpc_evidence_and_errors`, and
`bitcoind_btc_amounts_are_parsed_exactly` pin those mappings.

### L2 — both discovery paths validate output identity (`code`, `test`)

`funds_admin::evidence_for_operation` fetches full outputs for a persisted
txid and address-filtered outputs when no txid is known. Both paths pass all
outputs to `wallet::claim_chain_evidence`; neither converts transaction
existence or confirmations directly into status. The store re-reads the row
inside its write transaction and filters by persisted address, positive amount,
expected amount when nonzero, persisted txid when present, and persisted vout
when present. The tests `chain_evidence_requires_the_exact_expected_amount`
and `known_txid_still_requires_its_exact_output` pin the two paths.

### L3 — ambiguity cannot select an arbitrary output (`code`, `test`)

After exact matching and exclusion of outpoints owned by other operations,
zero candidates returns `NoMatch`; more than one returns `Ambiguous`.
Neither result writes status or evidence. Exactly one candidate is the only
branch reaching the update. The test
`multiple_exact_chain_outputs_remain_nonterminal` pins this cardinality
guard. Deposits use the same guard, so their zero initial amount admits one
positive output, not an arbitrary first output.

### L4 — one atomic transaction owns selection and update (`code`, `schema`, `test`)

`claim_chain_evidence` begins with `Database::begin_write`
(`BEGIN IMMEDIATE`), reads existing claims, selects one unclaimed candidate,
and writes txid, vout, confirmations, status, and the observed top-up amount
before committing. The partial unique index
`idx_wallet_operations_outpoint` independently rejects duplicate non-null
outpoints. Therefore A1 serializes concurrent claim attempts and the second
operation observes or conflicts with the first claim. Tests
`one_outpoint_cannot_settle_two_operations_concurrently` and
`distinct_outputs_settle_distinct_operations` pin exclusivity without
forbidding distinct outputs of one transaction.

### L5 — later confirmation updates remain bound to the claim (`code`)

Once claimed, the operation persists both txid and vout. Subsequent known-txid
queries return full outputs, and L2 restricts candidates to that same vout,
address, and amount. The backend-sync and delayed-submission-response writers
also preserve an existing txid whenever `tx_vout` is non-null; guarded manual
retry clears both fields only on retry-safe operations without a txid. Thus
confirmation growth or a delayed writer cannot swap in another output.

## Residual windows

- Exact address and amount do not prove that gatewayd created the payment;
  source provenance and target-side credit are separate checks.
- The observer may later report a reorg; rollback/deep-reorg policy is separate
  work.
- Multiple unclaimed exact outputs deliberately leave the operation nonterminal
  until the separate manual-review writer/resolution feature exists.
- A top-up intentionally assumes one positive output per fresh requested
  address; multiple positive outputs are ambiguous rather than summed.
- Backend wallet sync is a distinct evidence source and is not asserted by this
  chain-observer claim.

## Weakest links

1. **L2 (`code`)** — exact matching is a local runtime predicate.
2. **L3 (`code`/`test`)** — ambiguity behavior depends on candidate
   enumeration.
3. **L4 (`schema`/`code`)** — exclusivity combines the unique index with
   the write-transaction boundary.
4. **L1 (`type`/`test`)** — external JSON adapters must preserve fields and
   exact amounts.
5. **L5 (`code`)** — repeat reconciliation must retain vout filtering.
