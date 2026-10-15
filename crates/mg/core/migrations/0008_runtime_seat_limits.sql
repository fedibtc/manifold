-- Seat limits (REQ-seat-capacity-default). Admission sells up to the lowest
-- of three limits:
--
-- - max_seats, the operator's. NULL is automatic: no operator cap.
-- - memory_max_seats, from sampled host memory. Until the first sample it
--   assumes 4 seats.
-- - policy_max_seats, from the setup-payment publication. Until the first
--   write it is 12, the default when the publication does not set it.
--
-- SQLite cannot make a column nullable in place, so the table is rebuilt.
--
-- The old onboarding form pre-filled a RAM recommendation capped at 8, so an
-- operator value from 1 to 8 most likely is that recommendation. It becomes
-- automatic, and pre_automatic_max_seats keeps it so it can be restored. A
-- value above 8 was typed by the operator, and 0 means not selling; both
-- stay.
--
-- The memory default can remove the last free slot, so the offer epoch is
-- replaced: a quote issued before the upgrade is refused with its refund
-- (SPEC-quote-settlement).
CREATE TABLE offer_state_0008 (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    max_seats INTEGER CHECK (max_seats BETWEEN 0 AND 4294967295),
    price_msats INTEGER CHECK (price_msats IS NULL OR price_msats >= 0),
    updated_at_ms INTEGER,
    offer_epoch BLOB NOT NULL CHECK (
        typeof(offer_epoch) = 'blob'
        AND length(offer_epoch) = 32
    ),
    ready_for_new_seats INTEGER NOT NULL DEFAULT 0
        CHECK (ready_for_new_seats IN (0, 1)),
    memory_max_seats INTEGER NOT NULL DEFAULT 4
        CHECK (memory_max_seats BETWEEN 0 AND 4294967295),
    policy_max_seats INTEGER NOT NULL DEFAULT 12
        CHECK (policy_max_seats BETWEEN 0 AND 4294967295),
    pre_automatic_max_seats INTEGER
        CHECK (pre_automatic_max_seats BETWEEN 1 AND 8)
);

INSERT INTO offer_state_0008 (
    id, max_seats, price_msats, updated_at_ms, offer_epoch, ready_for_new_seats,
    pre_automatic_max_seats
)
SELECT
    id,
    CASE WHEN max_seats BETWEEN 1 AND 8 THEN NULL ELSE max_seats END,
    price_msats,
    updated_at_ms,
    randomblob(32),
    ready_for_new_seats,
    CASE WHEN max_seats BETWEEN 1 AND 8 THEN max_seats END
FROM offer_state;

DROP TABLE offer_state;
ALTER TABLE offer_state_0008 RENAME TO offer_state;
