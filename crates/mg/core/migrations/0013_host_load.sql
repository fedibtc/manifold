-- The host load averages the readiness load check compares, and whether the
-- check holds new seats. No row means no sample yet.
CREATE TABLE host_load (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    six_hours REAL NOT NULL CHECK (six_hours >= 0),
    day REAL NOT NULL CHECK (day >= 0),
    sampled_at_ms INTEGER NOT NULL CHECK (sampled_at_ms >= 0),
    held INTEGER NOT NULL CHECK (held IN (0, 1))
);
