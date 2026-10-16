-- Target freshness now means the MG answered its seat listing; each listed
-- seat reports its own freshness from its snapshot. A former complete poll
-- also listed seats, so existing values keep their meaning.
ALTER TABLE metric_poll_state RENAME COLUMN last_complete_at TO last_listed_at;

CREATE TABLE metric_listed_seats (
    target_id TEXT NOT NULL REFERENCES targets(target_id),
    guardian_seat_id TEXT NOT NULL,
    PRIMARY KEY (target_id, guardian_seat_id)
);
