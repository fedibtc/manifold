ALTER TABLE journal_streams ADD COLUMN archive_day TEXT;
ALTER TABLE journal_streams ADD COLUMN archive_offset INTEGER NOT NULL DEFAULT 0;
ALTER TABLE journal_streams ADD COLUMN archive_hash BLOB;
ALTER TABLE journal_streams ADD COLUMN gap_count INTEGER NOT NULL DEFAULT 0;

CREATE UNIQUE INDEX journal_stream_target_selector
    ON journal_streams(target_id, journal_selector);

CREATE TABLE archive_frames (
    stream_id TEXT NOT NULL REFERENCES journal_streams(stream_id),
    reception_day TEXT NOT NULL,
    start_offset INTEGER NOT NULL CHECK (start_offset >= 0),
    end_offset INTEGER NOT NULL CHECK (end_offset > start_offset),
    frame_hash BLOB NOT NULL CHECK (length(frame_hash) = 32),
    observed_generation INTEGER NOT NULL CHECK (observed_generation >= 0),
    continuity_gap INTEGER NOT NULL CHECK (continuity_gap IN (0, 1)),
    PRIMARY KEY(stream_id, reception_day, end_offset)
);
