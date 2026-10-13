DROP TABLE metric_snapshots;

CREATE TABLE metric_snapshots (
    target_id TEXT NOT NULL REFERENCES targets(target_id),
    guardian_seat_id TEXT NOT NULL,
    federation_id TEXT NOT NULL
        CHECK (length(federation_id) = 64)
        CHECK (federation_id NOT GLOB '*[^0-9a-f]*'),
    observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
    samples_json BLOB NOT NULL CHECK (length(samples_json) <= 4194304),
    sample_count INTEGER NOT NULL CHECK (sample_count >= 0),
    PRIMARY KEY (target_id, guardian_seat_id)
);
