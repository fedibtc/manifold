# Guardian metrics privacy inventory

Status: exact implemented collector policy baseline, 2026-08-31. The pinned
source can still emit raw JSON-RPC and Iroh method labels, but both telemetry
boundaries retain only the compiled canonical core-method set or `unknown`.

## Release boundary

FMan and the collector compile one default-deny source policy, so release review
must inventory the actual `fedimintd` source before changing that policy. This baseline was
read from Manifold's pinned `fedibtc/fedimint/v0.11.1-fedi18` source at
`5703f543f76746369f0a11e0d1635ac395b2efac`. The machine-checked
[`fedimint-metrics-v0.11.1-fedi18.tsv`](./fedimint-metrics-v0.11.1-fedi18.tsv)
enumerates every registration in the complete pinned Fedimint Rust source and
its admission disposition. It fails the Nix check when either the lock pin or
that source registration set drifts. The
release-pin recheck found no metric registration changes from fedi10 through
fedi15. This
telemetry stack includes the Fedi SPv2 server source at
`2f35ea4e3b2516d35b8ed315455718cd3b336758`. Re-run the inventory against the
exact combined integration tip whenever either pin or the attached module set
changes.

The Fedi SPv2 and legacy stability-pool server modules currently register no
Prometheus metric families of their own. "Including SPv2" therefore means this
absence is checked from the exact Cargo-selected `fedixyz/fedi` stability-pool server
source, not that it can be omitted from future review.

The Fedimint release tag is `0.11.1-fedi18`, while `fedimintd` emits its
upstream Cargo package version `0.11.1` in `app_start_ts{version=...}`. That
reviewed baseline selects the current safe shapes; it is not a collector
target-version requirement. The collector accepts bounded release metadata from
older and newer sources when the marker is otherwise valid, and still admits
other independently valid families when it is absent or invalid.

## Current explicit families

All names below receive the registry's `fm_` prefix.

| Area | Metric families | Type | Exact source labels / privacy notes |
| --- | --- | --- | --- |
| process | `app_start_ts` | gauge | `version`, `version_hash`, each 1–128 ASCII alphanumeric or `.`, `_`, `-`, `+`, `:` bytes; bounded release metadata, no user id |
| backups | `stored_backups_count`, `total_backup_size` | gauge | none; aggregate count/size, no backup key |
| backups / active-wallet proxy | `backup_counts` | gauge | `timeframe`, restricted to `1d`, `1w`, `1m`, `3m`, `all_time` |
| backups | `backup_write_size_bytes` | histogram | none; aggregate size distribution |
| consensus | `consensus_session_count` | gauge | none |
| consensus | `consensus_tx_processed_inputs`, `consensus_tx_processed_outputs`, `consensus_ordering_latency_seconds` | histogram | none; transaction shape and timing |
| consensus | `consensus_items_processed_total` | counter | `peer_id`, an at-most-five-byte value parseable as `u16`; bounded producer-owned operational dimension, not collector-verified config membership |
| consensus | `consensus_item_processing_duration_seconds` | histogram | `peer_id`, an at-most-five-byte value parseable as `u16`; bounded producer-owned operational dimension, not collector-verified config membership |
| consensus | `consensus_item_processing_module_audit_duration_seconds` | histogram | `module_id` is exactly `65535`; `module_kind` is exactly one of `lnv2`, `meta`, `mintv2`, `walletv2`, `multi_sig_stability_pool` |
| consensus | `consensus_peer_contribution_session_idx` | gauge | `self_id`, `peer_id`, each an at-most-five-byte value parseable as `u16`; bounded producer-owned operational dimensions, not collector-verified config membership |
| peer transport | `peer_connect_total`, `peer_messages_total` | counter | `self_id`, `peer_id`, each an at-most-five-byte value parseable as `u16`; bounded producer-owned operational dimensions, not collector-verified config membership; `direction` is exactly `incoming` or `outgoing` |
| peer transport | `peer_disconnect_total` | counter | `self_id`, `peer_id`, each an at-most-five-byte value parseable as `u16`; bounded producer-owned operational dimensions, not collector-verified config membership |
| APIs | `iroh_api_connections_active` | gauge | none |
| APIs | `iroh_api_connection_duration_seconds` | histogram | none |
| APIs | `iroh_api_request_duration_seconds` | histogram | `method` is exactly `unknown` or a source-coded core API method; an unrecognized value discards the family |
| APIs | `jsonrpc_api_request_duration_seconds` | histogram | `method` is exactly `unknown` or a source-coded core API method; an unrecognized value discards the family |
| APIs | `jsonrpc_api_request_response_code_total` | counter | `method` is exactly `unknown` or a source-coded core API method; `code` is `0`, `400`, `401`, `404`, `500`, or `-32700` through `-32603`; `type` is `subscription`, `batch`, or `default` |
| API client | `client_api_request_duration_seconds`, `client_api_requests_total` | histogram, counter | denied intentionally: FMan's API-announcement and guardian-metadata tasks instantiate `DynGlobalApi`, which records raw request `method` labels |
| connectors | `connector_connection_duration_seconds`, `connector_connection_attempts_total` | histogram, counter | denied intentionally: routine client/connector output remains outside this minimum telemetry surface |
| non-FMan Fedimint components | `bitcoind_rpc_request_duration_seconds`, `bitcoind_rpc_requests_total`, `gateway_htlc_handling_duration_seconds`, `gateway_htlc_lnv1_attempt_duration_seconds`, `gateway_htlc_lnv2_attempt_duration_seconds`, `ln_rpc_request_duration_seconds`, `ln_rpc_requests_total` | histogram, counter | denied: the exhaustive pinned-source scan includes these registrations, but the bundled FMan does not expose them |
| Bitcoin RPC | `server_bitcoin_rpc_request_duration_seconds` | histogram | `method` is exactly one of `get_block_count`, `get_block_hash`, `get_block`, `get_feerate`, `submit_transaction`, `get_sync_progress`, `get_chain_id`; `name` is exactly `server` |
| Bitcoin RPC | `server_bitcoin_rpc_requests_total` | counter | the same exact `method`/`name`; `result` is exactly `success` or `error` |
| mint | `mint_inout_sats`, `mint_inout_fees_sats` | histogram | `direction`, exactly `incoming` or `outgoing`; aggregate amount distribution |
| mint | deprecated `mint_redeemed_ecash_sats`, `mint_redeemed_ecash_fees_sats`, `mint_issued_ecash_sats`, `mint_issued_ecash_fees_sats` | histogram | none; aggregate amount distribution |
| mint v2 | `mintv2_inout_sats`, `mintv2_inout_fees_sats` | histogram | `direction`, exactly `incoming` or `outgoing`; aggregate amount distribution |
| mint v2 | deprecated `mintv2_redeemed_ecash_sats`, `mintv2_redeemed_ecash_fees_sats`, `mintv2_issued_ecash_sats`, `mintv2_issued_ecash_fees_sats` | histogram | none; aggregate amount distribution |
| Lightning | `ln_incoming_offer_total`, `ln_canceled_outgoing_contract_total` | counter | none |
| Lightning | `ln_funded_contract_sats` | histogram | `direction`, exactly `incoming` or `outgoing`; aggregate amount distribution |
| Lightning v2 | `lnv2_outgoing_contract_settled_total` | counter | `outcome`, exactly `claim`, `refund`, or `cancel` |
| Lightning v2 | `lnv2_funded_contract_sats` | histogram | `direction`, exactly `incoming` or `outgoing`; aggregate amount distribution |
| wallet | `wallet_block_count` | gauge | none |
| wallet | `wallet_inout_sats`, `wallet_inout_fees_sats` | histogram | `direction`, exactly `incoming` or `outgoing`; aggregate amount distribution |
| wallet | deprecated `wallet_pegin_sats`, `wallet_pegin_fees_sats`, `wallet_pegout_sats`, `wallet_pegout_fees_sats` | histogram | none; aggregate amount distribution |
| wallet v2 | `walletv2_block_count` | gauge | none; registration is eager so the height gauge is present even before a block-count update |
| wallet v2 | `walletv2_inout_sats`, `walletv2_inout_fees_sats` | histogram | `direction`, exactly `incoming` or `outgoing`; aggregate amount distribution |
| wallet v2 | deprecated `walletv2_pegin_sats`, `walletv2_pegin_fees_sats`, `walletv2_pegout_sats`, `walletv2_pegout_fees_sats` | histogram | none; aggregate amount distribution |

This table and the checked source-registration manifest are the complete
reviewed base-family inventory; rows marked denied
are reviewed source families whose complete sample families are discarded before
admission. Unknown or unclassified families are also discarded. Every
admitted histogram has
exactly `_bucket`, `_sum`, and `_count` series. `_bucket` adds only `le`, from
the family's bucket set below plus `+Inf`; `_sum` and `_count` add no label.
There are no admitted summaries or `quantile` labels. Counters use the table's
exact `_total` name. `_created`, process/runtime, generated-suffix variants not
belonging to a classified histogram, and every other unlisted family are
discarded until named and reviewed here.

The checked bucket sets are:

- amount histograms:
  `0`, `0.1`, `1`, `10`, `100`, `1000`, `10000`, `100000`, `1000000`,
  `10000000`, `100000000`;
- `consensus_tx_processed_inputs` and `consensus_tx_processed_outputs`:
  `1`, `2`, `5`, `10`, `20`, `50`, `100`, `200`, `500`, `1000`, `2000`,
  `5000`;
- `backup_write_size_bytes`:
  `1`, `10`, `100`, `1000`, `5000`, `10000`, `50000`, `100000`,
  `1000000`; and
- all admitted duration histograms:
  `0.005`, `0.01`, `0.025`, `0.05`, `0.1`, `0.25`, `0.5`, `1`, `2.5`,
   `5`, `10`.

`app_start_ts` is optional release metadata rather than a compatibility gate.
Within each admitted family the collector rejects duplicate series and requires
each present histogram labelset to contain exactly one `_sum`, one `_count`, and
every reviewed bucket. A missing, duplicate, or invalid marker discards that
family only; it cannot reject another independently valid family or prevent
safe-journal collection.

A real release-binary scrape must confirm these families, types, labels, values,
and buckets before production use.

The collector uses an exact checked policy derived from the reviewed Fedimint
pin and Manifold module set. It validates each family's type, exact labels,
bounded label values, and generated suffixes; it does not accept a future family
or label through a wildcard. Pin or module changes require re-inventory before
expanding or changing that policy, not before contacting targets on other
releases: the collector keeps every independently valid family in the current
intersection.
The four routine API-client/connector families are deliberately reviewed-deny,
so the collector discards each complete family without inspecting, retaining,
forwarding, or exposing its labels or values. The same rule applies to every
row marked denied, including the raw-method families. Exact family and generated
histogram suffix matching prevents a lookalike or unknown suffix from entering
this path; an unknown family is discarded without suppressing an unrelated
valid family. Operators
must not interpret this inventory as a promise that every producer registration
is exposed.

## MVP disposition

The admitted labels contain no wallet/client public keys, full invite codes,
transaction ids, account ids, IP addresses, hostnames, or free-form errors. Peer
ids, release hashes, activity counts, timing, and value histograms are still
federation operational data and must remain behind the Fedi operator boundary.
`backup_counts{timeframe="1d"}` is the intended new/active-wallet proxy;
`stored_backups_count`/`all_time` are total-backup proxies and must not be
described as unique humans.

`peer_id` and `self_id` are producer-owned operational dimensions. The collector
bounds each to an at-most-five-byte value parseable as `u16`, but does not attest
that it belongs to the seat's current federation configuration. Consumers must
use `fman_id`, `guardian_seat_id`, and the asserted `federation_id` for
operational source identity and must not interpret
these producer labels as configuration or federation membership proofs.

The API families are admitted independently of FMan or Fedimint release only
when `method` is `unknown` or one of the compiled `CORE_API_METHODS`. The FMan
and collector both apply that exact allowlist, so raw caller-controlled
`ApiMethod::Core(String)` or `ApiMethod::Module(_, String)` values cannot create
new label values or series: an unrecognized method taints and discards that
family. This keeps the cardinality and privacy boundary closed even when a
producer emits a raw method label. The fixed `code` and `type` projections
remain required for `jsonrpc_api_request_response_code_total`.

Every admitted guardian series receives exactly these collector labels:

- `fman_id`: lowercase hexadecimal canonical FMan Nostr public key verified by
  registration; this is the series identity;
- `fman_name`: the deterministic bounded `FmanName` derived from `fman_id`, for
  display only; collisions must never merge series; and
- `guardian_seat_id`: the canonical seat id returned by that authenticated
  FMan, stable within its FMan identity; and
- `federation_id`: the lowercase 64-hex id derived from the formed invite that
  the authenticated FMan asserted for that exact seat.

The federation id has one value per asserted federation and is intentionally
groupable across seats. It is authenticated FMan-asserted attribution, not an
independent guardian-membership or child-config proof, and is insufficient for
authorization, billing, or dispute resolution.
These labels are operational identifiers in the private backend. The collector
must not add capabilities, endpoints, invites, journal identifiers, source
incarnations or cursors, caller-provided names, raw unverified identifiers, or
other caller-controlled or unbounded values.

Do not add user/account/transaction identifiers, URLs, free-form method/error
strings, or unbounded labels to a source metric without a new privacy review.
If a future source metric is unacceptable, change or disable it at the producer
or discard it through the compiled source policy. FMan enforces the projection
before transport; the collector repeats it so old raw-response FMans remain
compatible during rollout and a faulty FMan cannot bypass collector admission.

## Collector resource and lifecycle bounds

One seat response may contain at most 20,000 samples, including discarded
reviewed-deny samples, and produces at most 2 MiB of admitted canonical text.
The collector stages each admitted family until its exact labels, values,
duplicates, suffixes, and histogram completeness have been checked. It discards
only the affected family when that local validation fails. Invalid UTF-8 or an
unisolatable family boundary, and global line, sample, family, output, or
deadline exhaustion reject the complete response because a safe bounded
projection cannot be established. A release-marker failure remains family-local.
Durable state across all active targets is capped at 32 MiB and
100,000 samples, and the private listener admits one aggregate scrape at a time.
Changing the inventory revision atomically clears incompatible latest snapshots
and durable attempt deadlines. Quarantine and expiry suppress and delete
snapshots; renewal after expiry cannot resurrect them.
