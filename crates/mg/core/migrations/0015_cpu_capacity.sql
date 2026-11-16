-- The CPU seat limit (REQ-seat-capacity-default).
--
-- cpu_capacity holds the one benchmark measurement of this host: its score
-- in benchmark iterations per second summed over one worker per CPU, the
-- score of one worker alone, and the CPU count (Rust's
-- `available_parallelism`, which follows the cgroup CPU quota) and CPU model
-- (the first `model name` of /proc/cpuinfo, empty where there is none) it
-- was taken at, and the version of the benchmark that took it. No row means
-- no measurement yet. The MG measures again only when the benchmark version,
-- CPU count or CPU model differs at a later start.
CREATE TABLE cpu_capacity (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    benchmark_version INTEGER NOT NULL
        CHECK (benchmark_version BETWEEN 1 AND 4294967295),
    cpu_count INTEGER NOT NULL CHECK (cpu_count BETWEEN 1 AND 4294967295),
    cpu_model TEXT NOT NULL,
    iterations_per_sec INTEGER NOT NULL
        CHECK (iterations_per_sec BETWEEN 0 AND 4294967295),
    single_core_iterations_per_sec INTEGER NOT NULL
        CHECK (single_core_iterations_per_sec BETWEEN 0 AND 4294967295),
    measured_at_ms INTEGER NOT NULL CHECK (measured_at_ms >= 0)
);

-- cpu_max_seats is the measured iterations per second divided by the
-- setup-payment publication's per-seat CPU budget in iterations per second,
-- rounded down. NULL is no CPU limit, which applies while the publication
-- sets no budget. Admission sells up to the lowest of it and the other
-- limits.
ALTER TABLE offer_state ADD COLUMN cpu_max_seats INTEGER
    CHECK (cpu_max_seats BETWEEN 0 AND 4294967295);
