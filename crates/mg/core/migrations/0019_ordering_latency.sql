-- The readiness ordering latency check's history: when the daemon last read
-- its running seats' consensus ordering latency counters, and the
-- bias-corrected 6-hour exponential moving average of the per-run median of
-- the seats' mean ordering latency, in seconds, with the weight the
-- intervals measured so far carry. The average and weight are NULL until an
-- interval has been measured. No row means no reading yet. The seats' own
-- counters are kept in memory only: a daemon restart starts each seat's
-- baseline again, while the average carries over.
CREATE TABLE ordering_latency (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    sampled_at_ms INTEGER NOT NULL CHECK (sampled_at_ms >= 0),
    average_6h_secs REAL,
    weight_6h REAL,
    CHECK (
        (average_6h_secs IS NULL AND weight_6h IS NULL)
        OR (average_6h_secs >= 0 AND weight_6h > 0)
    )
);
