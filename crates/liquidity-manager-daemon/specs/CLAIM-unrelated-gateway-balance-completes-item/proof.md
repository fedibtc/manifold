# Current argument

## Argument

### L1 — the sole gateway completion writer requires claim attribution (`enum`, `code`)

The only production writer that sets a gateway item to `completed` is
`allocation_store::complete_item`, called from `complete_if_gateway_funded`
(`gateway_allocation.rs`). Before completing, the worker reads the item's own
`gateway_funding` wallet operation and requires a `deposit-confirmed` entry
from the configured gateway whose `txid` equals the operation's recorded txid,
whose `out_idx` equals the operation's recorded `tx_vout` when chain
observation has recorded one, and whose amount covers the committed amount. A
transaction can pay two items' deposit addresses in separate outputs, so the
output index is what separates them. A `deposit-confirmed` entry is
logged by gatewayd's federation client exactly when it observes and claims a
confirmed deposit (`fedimint-wallet-client`, `pegin_monitor.rs`,
`claim_peg_in_inner`), and it carries the Bitcoin txid, output index, and
amount of that deposit. The guard therefore holds an
item-funding-output-to-target-claim identity; the aggregate federation
balance is read afterwards as evidence observation and never satisfies the
completion condition.

### L2 — the payment-log read returns per-deposit identity (`code`, pinned source)

`ConfiguredGatewayClient::deposit_claims` calls gatewayd's
`/payment_log` endpoint for the target federation filtered to the
`deposit-confirmed` event kind. At the pinned gatewayd source
(`fedimint-gateway-server/src/lib.rs`, `handle_payment_log_msg`) this reads
the federation client's event log and returns `PersistedLogEntry` payloads;
the `deposit-confirmed` payload deserializes as
`fedimint_wallet_client::events::DepositConfirmed { txid, out_idx, amount }`.
The event is emitted inside the claim transaction for that deposit's tweak
index (`pegin_monitor.rs`, `claim_peg_in_inner`), so an entry is target-side
claim evidence for the exact output it names, not an account aggregate.

### L3 — the falsifying counterexample no longer completes the item (`test`)

`unrelated_target_credit_does_not_complete_gateway_item` seeds an item whose
funding operation carries `txid-1`, then presents the gateway with a raised
federation balance and a claim for a different txid. The item stays
`running`. After the gateway also reports a claim for `txid-1`, the next pass
completes it. `claimed_output_index_gates_gateway_completion` settles the
funding operation through `claim_chain_evidence` at output index 1 and then
claims index 0 of the same txid; the item stays `running` until the gateway
claims index 1. The happy-path test
`completed_wallet_operation_persists_gateway_completion_evidence` covers the
claiming side, and `gateway_allocation_in_doubt_is_not_resubmitted` still
pins that a wallet operation without a txid never reaches completion.

### L4 — no aggregate balance is a completion input (`enum`, `code`)

The federation balance has one use in the completion path.
`complete_if_gateway_funded` reads it only after the claim guard has passed,
records it as a `GatewayObservation` row, and stores it in
`GatewayCompletionEvidence.observed_gateway_balance`. No allocation step
persists a balance baseline and no code path compares a balance against one,
so a raised aggregate is evidence of nothing on its own and cannot satisfy a
completion condition. A gateway that reports no balance for the target
federation leaves the item running rather than recording a substitute value.

## Residual windows

- **Settlement-by-address txid provenance.** `claim_chain_evidence`
  (`wallet.rs`) is a second production writer of the funding operation's
  `txid` and `tx_vout`: it settles an operation lacking a txid from the first
  confirmed output paying the operation's persisted address and amount, and
  the guard then matches that output exactly. A deposit to
  that address from outside the provider wallet therefore completes the item
  on a txid the provider wallet never broadcast. This satisfies the claim's
  attribution definition — the credit derives from the item's own persisted
  operation and address — and the address is persisted only in FLIP's
  `step_json` and gatewayd's database, never in a requester- or Admin-facing
  response, so the modeled adversary cannot target it. A deployment that
  leaks allocation addresses to third parties widens this window.
- **Operator-asserted settlement.** A manual review resolved as `Completed`
  writes the operator's txid and leaves `tx_vout` unset
  ([CLAIM-unrelated-manual-review-transaction-completes-operation](../CLAIM-unrelated-manual-review-transaction-completes-operation.md)),
  so for that operation the guard has no output index to match and accepts
  any claimed output of the asserted transaction. The operator's action is
  trusted in this claim's adversary model, and the unset `tx_vout` is the
  durable marker that FLIP did not verify the output.
- **Dishonest inputs and privileged corruption.** Malicious Admin behavior,
  direct database mutation, malicious configuration, and forged
  gateway/Fedimint or chain-observer responses remain outside the claim's
  adversary model. A forged `deposit-confirmed` log entry is a forged backend
  response and outside A2.
- **Liveness.** A claimed deposit whose log entry falls outside the bounded
  payment-log page delays completion rather than falsifying it; the
  payment-log read is newest-first and the page far exceeds the concurrent
  item ceiling.

## Weakest links

1. **L2 (`code`, pinned source)** — the semantic hinge is that
   `deposit-confirmed` entries name the claimed deposit's txid and are
   emitted per claim. Recheck when the Fedimint flake pin or Cargo patching
   changes.
2. **L1 (`enum`)** — a new gateway completion writer requires regenerating
   this argument.
3. **L3 (`test`)** — the counterexample pairings are pinned by unit tests
   against a fake gateway; a live-test variant under
   `integration_live_liquidity` would strengthen them.