-- One row per target an unbadged collector created, holding only the client
-- network prefix and the creation time, so the per-source creation limit is
-- counted from committed creations inside the admission transaction. Nothing
-- links a row to its target, and rows are deleted once they leave the
-- limit's one-hour window.
CREATE TABLE source_creations (
    source_prefix TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX source_creations_by_prefix ON source_creations(source_prefix, created_at);
