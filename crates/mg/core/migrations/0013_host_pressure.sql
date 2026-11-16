-- Pressure samples of the MG's cgroup that the readiness pressure check
-- averages: cumulative PSI `some` stall microseconds for CPU and IO, at most
-- one per 9 minutes, kept for a day.
CREATE TABLE host_pressure_samples (
    sampled_at_ms INTEGER PRIMARY KEY CHECK (sampled_at_ms >= 0),
    cpu_some_us INTEGER NOT NULL CHECK (cpu_some_us >= 0),
    io_some_us INTEGER NOT NULL CHECK (io_some_us >= 0)
);

-- Whether the pressure check holds new seats, so a restart does not reopen
-- them. No row means not held.
CREATE TABLE host_pressure_hold (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    held INTEGER NOT NULL CHECK (held IN (0, 1))
);
