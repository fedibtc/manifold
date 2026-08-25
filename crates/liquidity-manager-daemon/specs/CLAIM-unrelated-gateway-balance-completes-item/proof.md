# Current argument

## Argument

### L1 — completion-writer and evidence enumeration (`enum`, `code`, `type`) — fails

There is one production SQL writer capable of the bad transition:
`allocation_store::complete_item` unconditionally updates the selected
`allocation_items` row to `completed`, writes `fulfilled_amount_sats` and
serialized evidence, recomputes roll-ups, and commits
(`allocation_store.rs:278-301`). Its two production callers are the gateway
caller at `gateway.rs:296-310` and the stability-pool caller at
`stability_allocation.rs:387-401`; only the former constructs
`CompletionEvidence::Gateway`. The other `complete_item` occurrence and the
other gateway-evidence constructors are tests. There is no Admin/manual gateway
item-completion writer.

The `GatewayCompletionEvidence` type has exactly `gateway_id`,
`gateway_api`, `fulfilled_amount`, `observed_gateway_balance`, `observed_at`, optional
`withdrawal_txid`, and optional `wallet_operation_id`
(`service-liquidity-manager/src/public.rs:472-505`). The type rung ensures these
shapes serialize, but no field names the federation, initial baseline/delta, a
deposit address, Bitcoin output index/value, Fedimint peg-in operation, or
claimed target outpoint. At completion, code re-reads the
wallet row only to copy its `txid`; it does not compare that row's address or
txid with any gateway/Fedimint claim (`gateway.rs:295-308`). Thus the
evidence durably co-locates two facts but does not bind them.

### L2 — `FederationInfo.balance_msat` is an aggregate ecash balance (`code`, `enum`, axiom A2) — fails

The dependency is not inferred from a fake. `flake.nix` supplies
Fedimint tag `v0.11.1-fedi15`; `flake.lock` pins it to GitHub revision
`4c70c0e54f2f6a25df518c5082ac5a81d7a46d70`; root `Cargo.toml` declares
Fedimint `0.11.1` workspace dependencies and patches registry and Fedimint git
crates to that source, while the daemon inherits the gateway crates with
`{ workspace = true }`. Their path-package entries in `Cargo.lock` resolve
through that patch.
The workspace Fedi stability-pool source is exactly
`https://github.com/fedixyz/fedi` revision
`2f35ea4e3b2516d35b8ed315455718cd3b336758`; it is not on this gateway
completion route.

At the pinned Fedimint revision, `FederationInfo.balance_msat` is only an
`Amount` beside `federation_id` and config
(`gateway/fedimint-gateway-common/src/lib.rs:168-176`). Gatewayd's `get_info`
calls `federation_info_all_federations`, which calls each federation client's
`get_balance_for_btc` and places that result directly in `balance_msat`
(`gateway/fedimint-gateway-server/src/lib.rs:1916-1967`;
`federation_manager.rs:254-291`). `get_balance_for_btc` delegates to the primary
Bitcoin module's `get_balance` (`fedimint-client/src/client.rs:978-994`). Mint v1
returns the total amount of all spendable notes, and mint v2 sums every local
denomination/count (`modules/fedimint-mint-client/src/lib.rs:1080-1087`;
`modules/fedimint-mintv2-client/src/lib.rs:510-519`). It is therefore the entire
gateway client's spendable ecash balance in that federation—not the on-chain
balance of one allocated address, not an inbound-LN capacity measurement, and
not an item/txid/outpoint-specific receipt.

FLIP's `gateway_snapshot_from_info` and `connect_federation` merely divide that
amount by 1000, rounding down to sats (`gateway.rs:90-113,157-180`). The
enumerated target balance reads are: initial `gateway_info` in
`process_gateway_item`; completion's `observe_federation_balance`, whose default
implementation calls `gateway_info`; and the separate observation task's
`gateway_info` used only to UPSERT monitoring rows
(`gateway.rs:43-46,74-107,261-286,328-357`; `gateway.rs:52-63`). None
returns deposit provenance.

### L3 — baseline, address, schema, and concurrency enumeration (`enum`, `schema`, `code`) — fails

There is exactly one production baseline writer. On the first processing of an
item whose deserialized step has no baseline, it takes the matching
federation-wide balance from one `gateway_info` snapshot (or zero if the
federation is absent), adds an observation timestamp, and persists the whole
step JSON (`gateway.rs:96-109`;
`allocation_store.rs:58-65,256-269,577-598`). A restart reloads that value; it
does not re-baseline or reserve a portion of the aggregate for the item. There
is a second independent failure when the federation is absent from this first
snapshot: code persists zero, then connects with `recover: Some(true)` but does
not replace the baseline with the connect response's balance
(`gateway.rs:96-112`; `gateway.rs:96-113`). Honest pre-existing
ecash recovered by gatewayd can therefore satisfy the later threshold without
any post-baseline credit from the item.

There is one target gateway address-allocation call and one recheck call.
`ConfiguredGatewayClient::deposit_address` sends only `federation_id` to
gatewayd and validates the returned address's network; the item persists the
string before creating its wallet operation
(`gateway.rs:116-130`; `gateway.rs:152-194`). At the pinned source,
gatewayd selects that federation client and calls
`allocate_deposit_address_expert_only` (`fedimint-gateway-server/src/lib.rs:1326-1346`),
which durably derives an address, tweak index, and its own Fedimint operation id
(`modules/fedimint-wallet-client/src/lib.rs:798-826,1029-1050`). FLIP discards
the returned upstream operation id because the gateway API returns only the
address.

When the aggregate is absent or below threshold, FLIP's sole recheck sends the
persisted address and federation id (`gateway.rs:272-294,314-326`;
`gateway.rs:133-153`). Pinned gatewayd schedules that particular wallet-module
address for an immediate check, but returns only success; wallet-v2 makes this
verb a no-op (`fedimint-gateway-server/src/lib.rs:1442-1460`;
`modules/fedimint-wallet-client/src/lib.rs:1588-1625`). Critically, recheck is
not called when an unrelated aggregate increase already passes the inequality,
and even a successful recheck response is not evidence that this address found
or claimed an output.

The schema's only item uniqueness is `(funding_target_id, source_type)` and its
only wallet/item uniqueness is `(operation_type, item_id)`; neither address,
federation id, txid, nor output is unique or claimed
(`20260716000000_initial_schema.sql:135-192`). Different requests obtain
different funding targets and can name the same federation: `target_json`
contains the federation id, but the funding-target uniqueness key is
`(provider_pubkey, request_id, details_payload_hash)`
(`public.rs:500-572`; migration lines 94-115). The schema rung therefore
permits multiple active gateway items for the same configured gateway and
federation, and `active_gateway_items` loads all of them ordered by update time
without a federation lock (`allocation_store.rs:180-198`).

### L4 — wallet terminality is necessary but not target attribution (`enum`, `code`) — fails

For each item, `ensure_wallet_operation` persists one pending
`gateway_funding` row containing its item id, target id, amount, and already
persisted address; the unique item/type index prevents a second row
(`allocation_funding.rs:51-94`). Submission pre-marks it `in_doubt`, sends to
that address, then records a returned txid as `broadcast`
(`allocation_funding.rs:96-139`). This correctly associates the provider-wallet
operation with the item and address.

The gateway worker allows target completion only in the
`WalletOperationStatus::Completed` match arm; pending submits,
broadcast and confirmed wait, in-doubt/manual-review wait, and failed/cancelled
fail the item (`gateway.rs:189-239`). The wallet-status writer
enumeration is: insertion as pending;
`mark_operation_in_doubt`; `mark_withdrawal_broadcast`;
`mark_operation_failed`; `apply_sync_update`; and the manual retry/cancel SQL
writers that reset only safe pending/failed work to pending or set it cancelled
(`wallet.rs:61-218`; `manual_ops.rs:495-583`). Only
`apply_sync_update` can write `Completed`. It is called for backend updates and
chain evidence (`funds_admin.rs:74-88,101-133`), but the concrete
`GatewaydFundsWallet::sync_operations` returns an empty vector
(`wallet.rs:211-227`), so normal gateway-funding terminality comes from
the chain-evidence call. These are useful source-wallet terminal
prerequisites. They still do not say that gatewayd's
target Fedimint client claimed that output. This target-side claim is therefore
independent of the existing unrelated-Bitcoin-transaction settlement claim:
even granting that the operation's exact persisted output settled correctly,
the target completion guard can consume somebody else's target credit.

### L5 — the completion inequality admits an ordinary counterexample (`code`, `test`, axiom A1-A3) — fails

`complete_if_gateway_funded` reads the current aggregate `Bnow`, computes
`Binitial + item.committed_amount`, and completes on `Bnow >=` that sum
(`gateway.rs:261-310`). There is no critical section spanning the
baseline, other deposits, and this read; no per-address/outpoint query; and no
subtraction or claim of balance delta among items.

The unit test
`completed_wallet_operation_persists_gateway_completion_evidence` is
fake-backed. It marks a wallet operation completed, independently calls
`FakeGateway::set_balance`, and observes completion
(`gateway.rs:436-510,753-835`). It accurately pins the local
aggregate-inequality behavior but cannot establish real gateway provenance.
The live tests launch pinned gatewayd/Fedimint and verify one operation's txid,
terminal status, aggregate-driven item completion, and restart durability
(`integration_live_liquidity.rs:480-527,936-995`). They exercise only one
gateway item and never introduce a same-federation competing credit or assert an
address/outpoint-to-Fedimint-claim identity, so their test rung does not catch
this counterexample.

Concrete execution, requiring neither false wallet settlement nor a dishonest
service:

1. With honest target aggregate balance `B`, two distinct valid requests create
   same-federation gateway items `I` and `J`, each for amount `A` (or let `J`'s
   amount be at least `I`'s). Capacity covers both. One or successive allocation
   ticks persist `B` as each item's baseline, obtain their distinct target
   deposit addresses, create their distinct persisted wallet operations, and
   honestly submit each operation.
2. Both own Bitcoin outputs settle and both persisted wallet operations honestly
   reach `completed`. This deliberately grants the full wallet-settlement
   premise. Due to ordinary gateway/Fedimint watcher and state-machine delay,
   only `J`'s deposit has yet been claimed into the gateway client's aggregate;
   `I`'s confirmed deposit remains uncredited. Gatewayd truthfully reports
   `B + A` (net amounts can be chosen so the observed increase meets `I`'s
   committed threshold).
3. When the worker processes `I`, `B + A >= B + A`. It skips recheck, re-reads
   `I`'s completed wallet row, and commits `I` as completed with `I`'s operation
   id/txid beside the balance caused by `J`. The durable bad predicate now
   holds. `I`'s own target credit may arrive later, but no code repairs or
   reattributes the already completed row.
4. A crash before step 2 merely preserves the baselines, operations, and active
   items. Startup recovery loads/counts pending/running items and active wallet
   operations without modifying their evidence, then respawns the independent
   wallet and allocation tasks (`recovery.rs:289-321,417-470`;
   `daemon.rs:394-412`). The same schedule resumes. A crash after step 3
   preserves the false completion; completed items are excluded from both
   `active_gateway_items` and startup active-item loading. Thus restart neither
   prevents nor heals the violation.

An ordinary operator or third party depositing to another address owned by the
same gateway/Fedimint client after `I`'s baseline supplies the same aggregate
delta. That variant is not needed for falsification and does not involve the
provider wallet, but enumerating it shows the flaw is aggregate attribution,
not merely cross-item ordering.

## Residual windows

- **Dishonest inputs and privileged corruption.** Malicious Admin behavior,
  direct database mutation, malicious configuration, forged gateway/Fedimint or
  chain-observer responses, and out-of-band provider-wallet activity are outside
  the claim's adversary model. None is used by the counterexample.
- **Pre-completion delay.** Dependency unavailability, an absent federation in
  `gateway_info`, a balance below threshold, and non-completed wallet states
  leave the item running. These executions do not satisfy the bad durable
  predicate and do not repair the in-model execution that does.
- **Later balance loss or delayed own credit.** The claim is about the identity
  of the increase at the completed write, not durable maintenance of liquidity
  or eventual arrival of the item's own deposit. Later target changes are
  outside that temporal predicate; completion is already false when written.

## Weakest links

1. **A2/A3 (axiom):** honest dependencies may expose two independently settled
   deposits at different times, and FLIP may observe between them. This is the
   ordinary asynchronous process model; removing it would require an external
   atomic-ordering guarantee absent from the pinned APIs.
2. **L2 (`code`, `enum`):** the external semantic hinge is that
   `FederationInfo.balance_msat` is the primary-module aggregate. It must be
   rechecked when the Fedimint flake pin or Cargo patching changes.
3. **L3/L5 (`schema`, `code`):** no per-federation serialization or durable
   address/output claim exists, and completion compares only two aggregate
   samples. These are local, line-level facts.
4. **L1 (`type`):** evidence types preserve identifiers but cannot express
   target claim identity; compiling them is not an attribution proof.
5. **L5 (`test`):** a fake-backed unit test pins the vulnerable mechanism and
   real live tests cover only the non-competing happy path. No named test rejects
   the counterexample.
