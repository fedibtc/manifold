-- The readiness pressure check's history of the MG's cgroup: the last
-- cumulative PSI `some` stall microseconds for CPU and IO and when they were
-- read, and the bias-corrected stall share averages over the intervals
-- measured since (6-hour and 3.5-day exponential moving averages, each
-- between 0 and 1) with the weight those intervals carry per time constant.
-- The averages and weights are all NULL until an interval has been measured.
-- No row means no reading yet.
CREATE TABLE host_pressure (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    sampled_at_ms INTEGER NOT NULL CHECK (sampled_at_ms >= 0),
    cpu_some_us INTEGER NOT NULL CHECK (cpu_some_us >= 0),
    io_some_us INTEGER NOT NULL CHECK (io_some_us >= 0),
    cpu_6h REAL,
    cpu_3_5d REAL,
    io_6h REAL,
    io_3_5d REAL,
    weight_6h REAL,
    weight_3_5d REAL,
    CHECK (
        (cpu_6h IS NULL AND cpu_3_5d IS NULL AND io_6h IS NULL AND io_3_5d IS NULL
            AND weight_6h IS NULL AND weight_3_5d IS NULL)
        OR (cpu_6h IS NOT NULL AND cpu_3_5d IS NOT NULL
            AND io_6h IS NOT NULL AND io_3_5d IS NOT NULL
            AND weight_6h > 0 AND weight_3_5d > 0)
    )
);
