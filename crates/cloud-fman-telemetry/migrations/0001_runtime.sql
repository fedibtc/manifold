CREATE TABLE service_metadata (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    trust_profile TEXT UNIQUE NOT NULL,
    key_id TEXT NOT NULL,
    target_secret_format INTEGER NOT NULL CHECK (target_secret_format = 2),
    key_sentinel_nonce BLOB NOT NULL,
    key_sentinel_ciphertext BLOB NOT NULL
);
CREATE TABLE targets (
    fman_pubkey TEXT PRIMARY KEY NOT NULL,
    target_id TEXT UNIQUE NOT NULL,
    fman_name TEXT NOT NULL,
    key_id TEXT NOT NULL,
    secret_nonce BLOB NOT NULL,
    secret_ciphertext BLOB NOT NULL,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    auth_created_at INTEGER NOT NULL,
    lease_until INTEGER NOT NULL,
    registration_revision INTEGER NOT NULL CHECK (registration_revision > 0),
    status TEXT NOT NULL CHECK (status IN ('active', 'quarantined')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE auth_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX auth_events_expiry ON auth_events(expires_at);
-- Future journal workers own these rows. Admission deliberately cannot write cursors.
CREATE TABLE journal_streams (
    stream_id TEXT PRIMARY KEY NOT NULL,
    target_id TEXT NOT NULL REFERENCES targets(target_id),
    journal_selector BLOB NOT NULL,
    source_incarnation BLOB,
    cursor_segment INTEGER,
    cursor_offset INTEGER,
    observed_generation INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'pending'
);
