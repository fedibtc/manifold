-- The operator's NIP-17 chat with Fedi support (SPEC-fman-support-chat).
-- One row per private message, keyed by its rumor id so the copy a relay
-- serves again after a restart or reinstall is stored once.
CREATE TABLE support_messages (
    rumor_id TEXT PRIMARY KEY NOT NULL CHECK (length(rumor_id) = 64),
    from_fedi INTEGER NOT NULL CHECK (from_fedi IN (0, 1)),
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0)
);

CREATE INDEX support_messages_by_time ON support_messages (created_at);

-- Fedi messages created at or before `read_until` (Unix seconds) are read.
CREATE TABLE support_chat_state (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    read_until INTEGER NOT NULL CHECK (read_until >= 0)
);

INSERT INTO support_chat_state (id, read_until) VALUES (1, 0);
