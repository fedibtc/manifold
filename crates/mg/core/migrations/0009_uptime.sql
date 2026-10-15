-- The daemon's uptime scores and the last slot it recorded. No row means no
-- up slot yet: history starts at the first one.
CREATE TABLE uptime (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    day REAL NOT NULL CHECK (day >= 0 AND day <= 1),
    week REAL NOT NULL CHECK (week >= 0 AND week <= 1),
    month REAL NOT NULL CHECK (month >= 0 AND month <= 1),
    slot INTEGER NOT NULL CHECK (slot >= 0)
);
