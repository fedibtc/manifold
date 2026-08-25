CREATE TABLE metric_poll_state (
    target_id TEXT PRIMARY KEY NOT NULL REFERENCES targets(target_id),
    last_attempt_at INTEGER NOT NULL,
    next_due_at INTEGER NOT NULL,
    last_complete_at INTEGER
);

CREATE TABLE metric_snapshots (
    target_id TEXT NOT NULL REFERENCES targets(target_id),
    guardian_seat_id TEXT NOT NULL,
    observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
    samples_json BLOB NOT NULL CHECK (length(samples_json) <= 4194304),
    sample_count INTEGER NOT NULL CHECK (sample_count >= 0),
    PRIMARY KEY (target_id, guardian_seat_id)
);

CREATE TABLE metric_policy (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    fingerprint TEXT NOT NULL
);

CREATE TABLE metric_exposition_revision (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    revision INTEGER NOT NULL CHECK (revision >= 0)
);
INSERT INTO metric_exposition_revision(singleton, revision) VALUES (1, 0);
