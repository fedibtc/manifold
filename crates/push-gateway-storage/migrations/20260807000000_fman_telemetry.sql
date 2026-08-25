CREATE TABLE guardian_telemetry_fmans (
    fman_pubkey TEXT PRIMARY KEY NOT NULL,
    secret_nonce BYTEA NOT NULL,
    secret_ciphertext BYTEA NOT NULL,
    created_at BIGINT NOT NULL CHECK (created_at >= 0),
    updated_at BIGINT NOT NULL CHECK (updated_at >= 0)
);

-- Per-seat targets cannot be converted to the new FMan-wide authority. The
-- periodic FMan registration repopulates the normalized table.
DROP TABLE observer_federation_outbox;
DROP TABLE guardian_telemetry_targets;
