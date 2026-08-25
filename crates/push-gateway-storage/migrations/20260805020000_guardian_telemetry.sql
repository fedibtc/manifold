CREATE TABLE guardian_telemetry_targets (
    federation_id TEXT NOT NULL,
    peer_id TEXT NOT NULL,
    fman_pubkey TEXT NOT NULL,
    seat_id TEXT NOT NULL,
    iroh_endpoint_id TEXT NOT NULL,
    target_revision TEXT NOT NULL,
    secret_nonce BYTEA NOT NULL,
    secret_ciphertext BYTEA NOT NULL,
    created_at BIGINT NOT NULL CHECK (created_at >= 0),
    updated_at BIGINT NOT NULL CHECK (updated_at >= 0),
    PRIMARY KEY (federation_id, peer_id)
);

CREATE INDEX guardian_telemetry_targets_fman_idx
ON guardian_telemetry_targets (fman_pubkey);

CREATE TABLE observer_federation_outbox (
    federation_id TEXT PRIMARY KEY NOT NULL,
    invite_revision TEXT NOT NULL,
    invite_nonce BYTEA NOT NULL,
    invite_ciphertext BYTEA NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'retrying', 'succeeded')),
    attempts BIGINT NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at BIGINT NOT NULL CHECK (next_attempt_at >= 0),
    last_error TEXT,
    created_at BIGINT NOT NULL CHECK (created_at >= 0),
    updated_at BIGINT NOT NULL CHECK (updated_at >= 0)
);

CREATE INDEX observer_federation_outbox_due_idx
ON observer_federation_outbox (status, next_attempt_at, created_at);
