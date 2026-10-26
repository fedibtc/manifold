-- The newest seat capacity and seat readiness report each MG journaled, as
-- validated JSON, with its observation time.
CREATE TABLE mg_reports (
    target_id TEXT NOT NULL REFERENCES targets(target_id),
    kind TEXT NOT NULL CHECK (kind IN ('seat_capacity', 'seat_readiness')),
    observed_at_ms INTEGER NOT NULL CHECK (observed_at_ms >= 0),
    report_json TEXT NOT NULL CHECK (length(report_json) <= 4096),
    PRIMARY KEY (target_id, kind)
);
