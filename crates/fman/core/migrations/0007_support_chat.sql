-- The operator's NIP-17 chat with Fedi support (SPEC-fman-support-chat).
-- One row per private message, keyed by its rumor id so the copy a relay
-- serves again after a restart or reinstall is stored once.
CREATE TABLE support_messages (
    rumor_id TEXT PRIMARY KEY NOT NULL CHECK (length(rumor_id) = 64),
    from_fedi INTEGER NOT NULL CHECK (from_fedi IN (0, 1)),
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    -- Only Fedi messages are ever unread; it never goes back to 0.
    read INTEGER NOT NULL DEFAULT 0 CHECK (read IN (0, 1))
);

CREATE INDEX support_messages_by_time ON support_messages (created_at);
