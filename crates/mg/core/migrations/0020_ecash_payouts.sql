-- Ecash payouts. A payout job now records how it leaves the wallet: the
-- existing Lightning jobs keep their destination snapshot, and an ecash job
-- has none, because its bearer token goes to the operator instead. The token
-- itself is not stored here: the Fedimint wallet database already persists it
-- in the spend operation's metadata, in the same commit that removes the notes.
-- An ecash job records only the operator's set-once confirmation that the
-- token was redeemed.
--
-- SQLite cannot relax `destination NOT NULL` or add a cross-column CHECK in
-- place, so the table is rebuilt with every existing row copied as Lightning.

DROP TRIGGER payout_jobs_identity_immutable;
DROP TRIGGER payout_jobs_commit_monotone;
DROP TRIGGER payout_jobs_no_delete;

CREATE TABLE payout_jobs_v2 (
    request_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(request_id) BETWEEN 1 AND 128
    ),
    scope_kind TEXT NOT NULL CHECK (
        scope_kind IN ('payment_federation', 'guardian_fee')
    ),
    federation_id TEXT NOT NULL,
    seat_id TEXT,
    invite_code TEXT,
    method TEXT NOT NULL CHECK (method IN ('lightning', 'ecash')),
    destination TEXT CHECK (
        destination IS NULL OR length(destination) BETWEEN 1 AND 1024
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
    confirmed_at_ms INTEGER CHECK (
        confirmed_at_ms IS NULL OR 0 <= confirmed_at_ms
    ),
    CHECK (
        (scope_kind = 'payment_federation' AND seat_id IS NULL AND invite_code IS NULL)
        OR (
            scope_kind = 'guardian_fee'
            AND seat_id IS NOT NULL
            AND invite_code IS NOT NULL
        )
    ),
    CHECK (
        (operation_id IS NULL AND amount_msat IS NULL AND committed_at_ms IS NULL)
        OR (operation_id IS NOT NULL AND amount_msat IS NOT NULL AND committed_at_ms IS NOT NULL)
    ),
    CHECK (
        (method = 'lightning' AND destination IS NOT NULL AND confirmed_at_ms IS NULL)
        OR (
            method = 'ecash'
            AND destination IS NULL
            AND cap_maximum_msat IS NULL
            AND cap_remaining_msat IS NULL
        )
    ),
    CHECK (confirmed_at_ms IS NULL OR operation_id IS NOT NULL)
);

INSERT INTO payout_jobs_v2 (
    request_id, scope_kind, federation_id, seat_id, invite_code, method,
    destination, operation_id, amount_msat, created_at_ms, committed_at_ms,
    cap_maximum_msat, cap_remaining_msat, confirmed_at_ms
)
SELECT
    request_id, scope_kind, federation_id, seat_id, invite_code, 'lightning',
    destination, operation_id, amount_msat, created_at_ms, committed_at_ms,
    cap_maximum_msat, cap_remaining_msat, NULL
FROM payout_jobs;

DROP TABLE payout_jobs;
ALTER TABLE payout_jobs_v2 RENAME TO payout_jobs;

CREATE TRIGGER payout_jobs_identity_immutable
BEFORE UPDATE OF
    request_id, scope_kind, federation_id, seat_id, invite_code, method,
    destination, created_at_ms
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

CREATE TRIGGER payout_jobs_confirmation_set_once
BEFORE UPDATE OF confirmed_at_ms
ON payout_jobs
WHEN OLD.confirmed_at_ms IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'payout job confirmation is immutable');
END;

CREATE TRIGGER payout_jobs_no_delete
BEFORE DELETE ON payout_jobs
BEGIN
    SELECT RAISE(ABORT, 'payout jobs are durable');
END;
