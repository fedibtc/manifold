-- Seats from each MG's latest successful seat listing, so every listed seat
-- can report its own freshness, including a seat with no observation yet.
CREATE TABLE metric_listed_seats (
    target_id TEXT NOT NULL REFERENCES targets(target_id),
    guardian_seat_id TEXT NOT NULL,
    PRIMARY KEY (target_id, guardian_seat_id)
);
