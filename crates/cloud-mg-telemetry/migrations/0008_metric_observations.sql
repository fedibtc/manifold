-- Each seat's admitted observations for the last seven days, at most one per
-- 15-minute bucket, with the samples JSON zstd-compressed. A seat's latest
-- snapshot is its newest row, and only that row keeps the seat invite.
-- `samples_json_bytes` is the uncompressed length, so the cap on latest
-- snapshots counts the same bytes as before.
ALTER TABLE metric_snapshots RENAME TO metric_snapshots_uncompressed;

CREATE TABLE metric_observations (
    target_id TEXT NOT NULL REFERENCES targets(target_id),
    guardian_seat_id TEXT NOT NULL,
    observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
    federation_id TEXT NOT NULL
        CHECK (length(federation_id) = 64)
        CHECK (federation_id NOT GLOB '*[^0-9a-f]*'),
    samples_zstd BLOB NOT NULL CHECK (length(samples_zstd) <= 4194304),
    samples_json_bytes INTEGER NOT NULL
        CHECK (samples_json_bytes >= 0 AND samples_json_bytes <= 4194304),
    sample_count INTEGER NOT NULL CHECK (sample_count >= 0),
    invite_nonce BLOB,
    invite_ciphertext BLOB,
    PRIMARY KEY (target_id, guardian_seat_id, observed_at_ms)
);

CREATE UNIQUE INDEX metric_observation_bucket
    ON metric_observations(target_id, guardian_seat_id, observed_at_ms / 900000);

CREATE INDEX metric_observation_time ON metric_observations(observed_at_ms);

CREATE VIEW metric_snapshots AS
SELECT o.* FROM (
    SELECT target_id, guardian_seat_id, MAX(observed_at_ms) AS observed_at_ms
    FROM metric_observations GROUP BY target_id, guardian_seat_id
) latest JOIN metric_observations o
    ON o.target_id = latest.target_id
   AND o.guardian_seat_id = latest.guardian_seat_id
   AND o.observed_at_ms = latest.observed_at_ms;

-- Store::open compresses the rows of metric_snapshots_uncompressed into
-- metric_observations and drops it in one transaction; SQL has no zstd.
