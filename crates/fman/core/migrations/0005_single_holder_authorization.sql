-- An FMan presents one Holder authorization: the complete kind-37705 event
-- with the greatest signed issue time. Earlier builds kept one row per
-- credential digest; adopt only the newest of those that is within the
-- receiver bound (now plus FMAN_HOLDER_AUTHORIZATION_MAX_FUTURE_SKEW_SECS),
-- since startup would discard an out-of-bound one and leave none.
CREATE TABLE holder_authorization (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    authorization_issued_at BLOB NOT NULL
        CHECK (typeof(authorization_issued_at) = 'blob' AND length(authorization_issued_at) = 8),
    event_json TEXT NOT NULL
);

-- Big-endian issue times order bytewise as they do numerically.
INSERT INTO holder_authorization (id, authorization_issued_at, event_json)
SELECT 1, authorization_issued_at, event_json
FROM holder_authorization_events
WHERE authorization_issued_at <= unhex(printf('%016X', unixepoch() + 3600))
ORDER BY authorization_issued_at DESC, credential_digest
LIMIT 1;

-- holder_authorization_events is left in place, unread and unwritten, so the
-- authorizations this migration did not adopt remain recoverable.
