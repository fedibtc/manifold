-- A formed seat records which guardian it is, not an invite: the federation
-- id and its own peer id. An invite is only a way to dial the guardian, and
-- MG builds it from the seat's installed fedimintd config whenever it hands
-- one out (ARCH-manifold-guardian-storage).
--
-- SQLite cannot decode an invite, so `Db::open` decodes every stored
-- `formed_seats.federation_invite` into `temp.formed_invite_peers` on the
-- migrating connection first, and fails before this runs if one does not
-- name exactly one guardian. The join below is the backstop: a stored invite
-- without its decoded row inserts a NULL federation id, the NOT NULL
-- constraint aborts this migration's transaction, and the database stays
-- as it was. The IF NOT EXISTS keeps this file runnable on an empty
-- database by tools that do not decode, such as sqlx-cli.
CREATE TEMP TABLE IF NOT EXISTS formed_invite_peers (
    federation_invite TEXT PRIMARY KEY NOT NULL,
    federation_id TEXT NOT NULL,
    peer_id INTEGER NOT NULL
);

CREATE TABLE formed_seats_0011 (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    federation_id TEXT NOT NULL CHECK (
        length(federation_id) = 64
        AND federation_id NOT GLOB '*[^0-9a-f]*'
    ),
    peer_id INTEGER NOT NULL CHECK (peer_id BETWEEN 0 AND 65535),
    formed_at_ms INTEGER NOT NULL
);

INSERT INTO formed_seats_0011 (quote_id, federation_id, peer_id, formed_at_ms)
SELECT f.quote_id, p.federation_id, p.peer_id, f.formed_at_ms
FROM formed_seats f
LEFT JOIN temp.formed_invite_peers p ON p.federation_invite = f.federation_invite;

DROP TABLE formed_seats;
ALTER TABLE formed_seats_0011 RENAME TO formed_seats;
DROP TABLE temp.formed_invite_peers;

CREATE TRIGGER formed_seats_immutable_update
BEFORE UPDATE ON formed_seats BEGIN
    SELECT RAISE(ABORT, 'formed seat records are immutable');
END;

CREATE TRIGGER formed_seats_immutable_delete
BEFORE DELETE ON formed_seats BEGIN
    SELECT RAISE(ABORT, 'formed seat records are immutable');
END;

-- A guardian-fee payout job is scoped by its federation id and seat. Its
-- invite was a copy of the seat's; the worker now builds it from the seat's
-- installed config when it has to join the fee wallet. Payment-federation
-- jobs never stored one.
CREATE TABLE payout_jobs_0011 (
    request_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(request_id) BETWEEN 1 AND 128
    ),
    scope_kind TEXT NOT NULL CHECK (
        scope_kind IN ('payment_federation', 'guardian_fee')
    ),
    federation_id TEXT NOT NULL,
    seat_id TEXT,
    destination TEXT NOT NULL CHECK (
        length(destination) BETWEEN 1 AND 1024
    ),
    operation_id TEXT CHECK (
        operation_id IS NULL OR (
            length(operation_id) = 64
            AND operation_id NOT GLOB '*[^0-9a-f]*'
        )
    ),
    amount_msat INTEGER CHECK (amount_msat IS NULL OR 0 <= amount_msat),
    created_at_ms INTEGER NOT NULL CHECK (0 <= created_at_ms),
    committed_at_ms INTEGER CHECK (
        committed_at_ms IS NULL OR 0 <= committed_at_ms
    ),
    cap_maximum_msat INTEGER CHECK (
        cap_maximum_msat IS NULL OR 0 <= cap_maximum_msat
    ),
    cap_remaining_msat INTEGER CHECK (
        cap_remaining_msat IS NULL OR 0 <= cap_remaining_msat
    ),
    CHECK (
        (scope_kind = 'payment_federation' AND seat_id IS NULL)
        OR (scope_kind = 'guardian_fee' AND seat_id IS NOT NULL)
    ),
    CHECK (
        (operation_id IS NULL AND amount_msat IS NULL AND committed_at_ms IS NULL)
        OR (operation_id IS NOT NULL AND amount_msat IS NOT NULL AND committed_at_ms IS NOT NULL)
    )
);

INSERT INTO payout_jobs_0011 (
    request_id, scope_kind, federation_id, seat_id, destination, operation_id,
    amount_msat, created_at_ms, committed_at_ms, cap_maximum_msat, cap_remaining_msat
)
SELECT
    request_id, scope_kind, federation_id, seat_id, destination, operation_id,
    amount_msat, created_at_ms, committed_at_ms, cap_maximum_msat, cap_remaining_msat
FROM payout_jobs;

DROP TABLE payout_jobs;
ALTER TABLE payout_jobs_0011 RENAME TO payout_jobs;

CREATE TRIGGER payout_jobs_identity_immutable
BEFORE UPDATE OF request_id, scope_kind, federation_id, seat_id, destination, created_at_ms
ON payout_jobs
BEGIN
    SELECT RAISE(ABORT, 'payout job identity is immutable');
END;

CREATE TRIGGER payout_jobs_commit_monotone
BEFORE UPDATE OF
    operation_id, amount_msat, committed_at_ms, cap_maximum_msat, cap_remaining_msat
ON payout_jobs
WHEN OLD.operation_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'payout job operation is immutable');
END;

CREATE TRIGGER payout_jobs_no_delete
BEFORE DELETE ON payout_jobs
BEGIN
    SELECT RAISE(ABORT, 'payout jobs are durable');
END;
