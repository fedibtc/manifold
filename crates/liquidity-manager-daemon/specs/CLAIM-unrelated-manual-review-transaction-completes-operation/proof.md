# Current argument

## What this record does not claim

**An operator can still complete a reviewed send FLIP could not verify.** The
claim below says this needs a deliberate second verb and leaves a durable record.
It does not say it cannot happen, and it does not make the operator's assertion
true.

**Nothing verifies the asserted txid on that path.** The value recorded in
`txid` after `complete_review_without_evidence` is an operator's claim, not
evidence. `tx_vout` stays unset to mark exactly that, and the audit row says in
so many words that FLIP did not verify the transaction pays this operation.

**The API preserves availability through an explicit override.** Requiring
evidence with no alternate path would make reviewed operations unresolvable
during a chain-observer outage, which is close to the situation that produces
them. An unverified completion therefore uses a distinct verb and cannot arrive
through the verb that represents verified completion.

This matches
[`failed-stability-allocation-strands-ecash`](../CLAIM-failed-stability-allocation-strands-ecash.md)
and
[`stability-deposit-rejection-releases-capacity`](../CLAIM-stability-deposit-rejection-releases-capacity.md)
: FLIP names a state it cannot prevent, requires a deliberate call to reach it,
and writes the choice down.

## Argument

### L1 — the in-place completion-transition enumeration is exact (`enum`)

Within one SQLite runtime generation, the production writers that can
transition an existing wallet operation to `completed` are:

1. `wallet::resolve_manual_review_tx` for
   `ManualReviewOutcome::Completed`;
2. `wallet::apply_sync_update` when its
   `WalletOperationSync.status` is `SyncedWalletStatus::Completed`; and
3. `wallet::claim_chain_evidence` after one output passes the persisted
   address, amount, txid, and vout filters and has enough confirmations.

`rg 'WalletOperationStatus::Completed|"completed"' crates/liquidity-manager-daemon/src`
regenerates the status-target portion of this list. `insert_wallet_operation_tx`
accepts an input status but creates a new row, so it is not a transition of an
existing reviewed row. `backup::commit_live_restore` replaces the data root,
and `DaemonShell` drops then rebuilds the runtime generation; it likewise is
not an in-place transition. The only other
`SyncedWalletStatus::Completed` construction is the local status choice in
`claim_chain_evidence`; `wallet` declares the sync input but its current
gateway backend returns no such updates. Test-only SQL seeds are not production
writers.

### L2 — the authenticated operator route reaches manual resolution (`code`)

`admin::app` installs `/admin/v1/resolve_manual_review` in its protected
router behind `require_auth`. Its handler delegates through
`DaemonContext`'s `OperatorAdminApi::resolve_manual_review` implementation to
`manual_ops::resolve_manual_review`. Thus the claim's input is an authenticated
operator request, not a direct database write.

### L3 — observer completion requires exact output evidence (`code`, `test`)

`claim_chain_evidence` rejects a candidate unless its destination equals the
persisted address and its amount equals the persisted nonzero amount, claims
one `(txid, vout)`, and only then selects `Completed`. The
[`unrelated-transaction-settled-operation`](../CLAIM-unrelated-transaction-settled-operation.md)
claim covers this chain-observer path. This proof does not import it: the
manual counterexample neither uses nor depends on observer reconciliation.

### L4 — a manual completed resolution has no exact-output guard (`code`, `test`)

`resolve_manual_review_with_database` accepts `Completed` for any nonempty
string and calls `resolve_manual_review_tx` while the row is
`manual_review_required`. Its `Completed` arm writes `status = completed` and
the supplied `txid`, does not write `tx_vout`, and neither reads the persisted
address/amount nor invokes a wallet, chain observer, or
`claim_chain_evidence`. The focused
`manual_ops::tests::completed_manual_review_accepts_txid_without_exact_output_evidence`
test pins a seeded operation's persisted destination and nonzero amount,
accepted completion, and retained null `tx_vout`, with no output evidence. The
null-vout result applies to this seeded counterexample, not every possible
reviewed row.

### L5 — the counterexample falsifies the claim (`test`, `code`)

Seed a reviewed operation with its persisted destination and amount. Let the
operator supply a nonempty canonical-length txid for a transaction which, by
A1, has no output matching those fields. The test's illustrative 64-hex-character
string does not establish a real transaction; it demonstrates that L4 requires
no output inspection or output claim. A1 substitutes the unrelated real
transaction for that representative input. L4 then changes the row to
`completed` without inspecting transaction outputs. This reaches the bad thing
in the claim, so the claim is false.

## Residual windows

- A manual `failed` or `safe_to_retry` resolution does not set the operation to
  `completed`; it is outside this exact bad thing.
- A backend sync completion is an independent wallet-backend assertion, and
  chain-observer completion is covered separately by
  `unrelated-transaction-settled-operation.md`. Neither makes the manual writer
  evidence-based.
- This record does not claim source provenance, target-side credit, reorg
  handling, duplicate submissions, or whether a completed operation later
  advances an allocation item.

## Weakest links

1. **L1 (`enum`)** — a new in-place production completion transition requires
   regenerating the writer list.
2. **L2 (`code`)** — route protection and delegation must keep the operator
   request on the authenticated Admin API path.
3. **L4 (`code`/`test`)** — the manual transition deliberately trusts an
   operator assertion.
4. **L5 (`test`/`code`)** — the reproduction pins the durable state transition;
   A1 supplies the transaction/output distinction.
5. **L3 (`code`/`test`)** — observer output attribution is a separate boundary.
