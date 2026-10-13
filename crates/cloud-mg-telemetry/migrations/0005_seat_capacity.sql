CREATE TABLE seat_capacity (
    target_id TEXT PRIMARY KEY NOT NULL REFERENCES targets(target_id),
    observed_at_ms INTEGER NOT NULL,
    max_seats INTEGER NOT NULL,
    available_seats INTEGER NOT NULL,
    accepting_seats INTEGER NOT NULL
);
