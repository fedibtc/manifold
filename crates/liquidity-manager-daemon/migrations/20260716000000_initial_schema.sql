-- Initial schema baseline.
--
-- Collapses the incremental pre-launch migration history (phases 0-11B)
-- into one migration; the daemon never went live, so no deployed database
-- carries the old versions.

CREATE TABLE daemon_metadata (
  key TEXT PRIMARY KEY NOT NULL,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

INSERT INTO daemon_metadata (key, value, updated_at)
VALUES ('schema_family', 'flip-liquidity-manager', unixepoch());

CREATE TABLE secret_records (
  name TEXT PRIMARY KEY NOT NULL,
  version INTEGER NOT NULL,
  algorithm TEXT NOT NULL,
  key_id TEXT NOT NULL,
  nonce BLOB NOT NULL,
  ciphertext BLOB NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE setup_state (
  id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
  status TEXT NOT NULL,
  config_view_json TEXT,
  latest_validation_json TEXT,
  revision INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE provider_identity (
  id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
  provider_pubkey TEXT NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE attestation_state (
  key TEXT PRIMARY KEY NOT NULL,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE attestation_payloads (
  id TEXT PRIMARY KEY NOT NULL,
  kind TEXT NOT NULL,
  issuer TEXT,
  subject_json TEXT NOT NULL,
  payload BLOB NOT NULL,
  valid INTEGER NOT NULL DEFAULT 0,
  ingested_at INTEGER NOT NULL DEFAULT (unixepoch())
);

-- An accepted allocation is the only durable request outcome. The
-- federation is the allocation's identity: at most one allocation ever
-- exists per federation. Rejections are not persisted (they re-evaluate on
-- retry) and repeats are answered from current state, so there is no
-- request lifecycle beside the allocation itself.
CREATE TABLE allocations (
  federation_id TEXT PRIMARY KEY NOT NULL,
  requester_pubkey TEXT NOT NULL,
  provider_pubkey TEXT NOT NULL,
  network TEXT NOT NULL,
  details_payload_hash BLOB NOT NULL,
  request_json TEXT NOT NULL,
  verification_json TEXT NOT NULL,
  target_json TEXT NOT NULL,
  committed_amount_sats INTEGER NOT NULL DEFAULT 0,
  reserved_amount_sats INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

-- The details commitment binds the requester and the federation details, so
-- it is one-to-one with an allocation; status polls look up by it.
CREATE UNIQUE INDEX idx_allocations_details_hash
  ON allocations(details_payload_hash);

CREATE TABLE allocation_items (
  item_id TEXT PRIMARY KEY NOT NULL,
  federation_id TEXT NOT NULL,
  source_type TEXT NOT NULL,
  status TEXT NOT NULL,
  committed_amount_sats INTEGER NOT NULL DEFAULT 0,
  item_json TEXT,
  failure_json TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  reserved_amount_sats INTEGER NOT NULL DEFAULT 0,
  step_json TEXT,
  fulfilled_amount_sats INTEGER,
  completion_evidence_json TEXT,
  FOREIGN KEY (federation_id) REFERENCES allocations(federation_id)
);

CREATE UNIQUE INDEX idx_allocation_items_federation_source
  ON allocation_items(federation_id, source_type);

-- The status list restates ACTIVE_ITEM_STATUSES (allocation_store.rs); SQL
-- cannot reference the Rust constant. The two are pinned in step by
-- `partial_indexes_list_exactly_the_statuses_their_rust_constants_do`
-- (tests/database.rs), which fails if either side gains a status alone.
CREATE INDEX idx_allocation_items_active
  ON allocation_items(status, updated_at)
  WHERE status IN ('pending', 'running');

CREATE TABLE wallet_operations (
  operation_id TEXT PRIMARY KEY NOT NULL,
  operation_type TEXT NOT NULL,
  status TEXT NOT NULL,
  operation_json TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  external_operation_id TEXT,
  federation_id TEXT,
  item_id TEXT,
  amount_sats INTEGER NOT NULL DEFAULT 0,
  address TEXT,
  txid TEXT,
  tx_vout INTEGER CHECK (tx_vout IS NULL OR tx_vout >= 0),
  confirmation_count INTEGER,
  failure_json TEXT,
  label TEXT,
  fee_rate_sat_per_vbyte INTEGER,
  submitted_at INTEGER,
  completed_at INTEGER,
  sync_after INTEGER,
  sync_metadata_json TEXT,
  withdrawal_intent_id TEXT,

  -- The tick current when this operation settled. Until a persisted
  -- observation's read began after that tick, the debit is still subtracted
  -- from available capacity: settling is not proof that the balance on record
  -- already includes it.
  settled_tick INTEGER,

  -- The tick current when this operation first became visible to the capacity
  -- query, for operations excluded from it while their allocation item still
  -- reserved.
  --
  -- Without it the settle stamp expires unobserved: the query excludes any
  -- operation whose item is in a reserving status, so ticks run on while the
  -- row is invisible and the debit leaves the reserved term and the unsettled
  -- term in the same instant.
  released_tick INTEGER
);

CREATE UNIQUE INDEX idx_wallet_operations_external_operation_id
  ON wallet_operations(external_operation_id)
  WHERE external_operation_id IS NOT NULL;

CREATE UNIQUE INDEX idx_wallet_operations_item_type
  ON wallet_operations(operation_type, item_id)
  WHERE item_id IS NOT NULL;

-- Admin auth identifies the installation rather than an individual operator,
-- so operator withdrawal intents are unique across this installation.
CREATE UNIQUE INDEX idx_wallet_operations_withdrawal_intent
  ON wallet_operations(withdrawal_intent_id)
  WHERE withdrawal_intent_id IS NOT NULL;

-- The status list restates PENDING_SETTLEMENT_STATUSES (wallet.rs); SQL
-- cannot reference the Rust constant. The two are pinned in step by
-- `partial_indexes_list_exactly_the_statuses_their_rust_constants_do`
-- (tests/database.rs), which fails if either side gains a status alone.
CREATE INDEX idx_wallet_operations_active
  ON wallet_operations(status, updated_at)
  WHERE status IN ('pending', 'broadcast', 'confirmed', 'in_doubt', 'manual_review_required');

CREATE INDEX idx_wallet_operations_list
  ON wallet_operations(created_at DESC, operation_id DESC);

CREATE INDEX idx_wallet_operations_status_list
  ON wallet_operations(status, created_at DESC, operation_id DESC);

-- A chain output is settlement evidence for at most one wallet operation.
CREATE UNIQUE INDEX idx_wallet_operations_outpoint
  ON wallet_operations(txid, tx_vout)
  WHERE txid IS NOT NULL AND tx_vout IS NOT NULL;

-- A monotonic tick that orders balance reads against wallet settlements.
--
-- Every balance read takes a tick before it asks the backend, and every
-- settlement stamps the tick current when it settled. A read that begins after
-- a settlement therefore holds a strictly greater tick than that settlement,
-- and a read that began before holds a smaller one. That is the whole ordering
-- fact `active_wallet_withdrawal_amount_tx` needs, and counting persisted
-- observations could not supply it: two events falling between the same pair of
-- writes are indistinguishable by a write counter.
CREATE TABLE wallet_observation_ticks (
  id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
  tick INTEGER NOT NULL DEFAULT 0
);

INSERT INTO wallet_observation_ticks (id, tick) VALUES (1, 0);

-- The provider wallet balance FLIP last persisted, and the tick at which the
-- read that produced it began.
--
-- The balance is read from the backend and persisted after an await, so a slow
-- reply can persist a balance that was read before a settlement. Releasing a
-- settled withdrawal on arrival order would then stop subtracting a debit the
-- persisted balance does not include, and the next admission could spend that
-- money a second time. So a settled operation is released only when the
-- persisted observation's `read_tick` is strictly greater than the tick that
-- operation stamped.
--
-- The sync worker is serial with its own reads, so for it read order and
-- arrival order agree. `get_funds_with_wallet` and
-- `request_withdrawal_with_wallet` are not, and they are why the distinction
-- has to be recorded rather than assumed.
CREATE TABLE wallet_balance_observations (
  id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
  network TEXT NOT NULL,
  spendable_sats INTEGER NOT NULL DEFAULT 0,
  source_json TEXT,
  observed_at INTEGER NOT NULL,
  read_tick INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE recovery_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at INTEGER NOT NULL,
  completed_at INTEGER NOT NULL,
  active_allocation_item_count INTEGER NOT NULL,
  active_wallet_operation_count INTEGER NOT NULL,
  summary_json TEXT NOT NULL
);

CREATE TABLE gateway_observations (
  observation_key TEXT PRIMARY KEY NOT NULL,
  gateway_id TEXT NOT NULL,
  federation_id TEXT,
  status TEXT NOT NULL,
  observed_balance_sats INTEGER,
  observation_json TEXT,
  failure_json TEXT,
  observed_at INTEGER NOT NULL
);

CREATE INDEX idx_gateway_observations_gateway
  ON gateway_observations(gateway_id, observed_at DESC);

CREATE TABLE stability_pool_observations (
  observation_key TEXT PRIMARY KEY NOT NULL,
  federation_id TEXT NOT NULL,
  status TEXT NOT NULL,
  observed_provided_amount_sats INTEGER,
  liquidity_stats_json TEXT,
  observation_json TEXT,
  failure_json TEXT,
  observed_at INTEGER NOT NULL
);

CREATE INDEX idx_stability_pool_observations_federation
  ON stability_pool_observations(federation_id, observed_at DESC);

CREATE TABLE provider_advertisements (
  id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
  status TEXT NOT NULL,
  advertisement_hash BLOB,
  signed_advertisement_json TEXT,
  readiness_json TEXT,
  config_revision INTEGER,
  issued_at INTEGER,
  expires_at INTEGER,
  last_published_at INTEGER,
  withdrawn_at INTEGER,
  last_error TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE relay_publications (
  relay_url TEXT PRIMARY KEY NOT NULL,
  status TEXT NOT NULL,
  advertisement_hash BLOB,
  event_id TEXT,
  last_error TEXT,
  last_seen_at INTEGER,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_relay_publications_status
  ON relay_publications(status, updated_at);

CREATE TABLE audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  action TEXT NOT NULL,
  detail_json TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Holder-published kind-37705 authorizations this provider has ingested from
-- its configured relays.
--
-- The complete signed event is retained, not the extracted envelope, so every
-- read re-runs the same verification the ingest ran. A row is therefore never
-- a trusted assertion on its own, and a database an attacker can write does
-- not become an admission path.
--
-- The credential digest is the identity: one Holder authorizing this provider
-- for one badge has one row, and a re-published authorization for that badge
-- replaces it. `authorization_issued_at` is the signed statement timestamp,
-- stored big-endian so SQLite's byte comparison is unsigned numeric order over
-- the full u64 range. The merge replaces a row only on a strictly greater
-- value, which is what makes a replayed older authorization inert.
--
-- Mirrors FMan's `holder_authorization_events` deliberately: the two services
-- run the same enrollment flow and their durable shapes should not diverge.
CREATE TABLE holder_authorization_events (
    credential_digest BLOB PRIMARY KEY NOT NULL
        CHECK (typeof(credential_digest) = 'blob' AND length(credential_digest) = 32),
    authorization_issued_at BLOB NOT NULL
        CHECK (typeof(authorization_issued_at) = 'blob' AND length(authorization_issued_at) = 8),
    event_json TEXT NOT NULL,
    ingested_at INTEGER NOT NULL DEFAULT (unixepoch())
);
