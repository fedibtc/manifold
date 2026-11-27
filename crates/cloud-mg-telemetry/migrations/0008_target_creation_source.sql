-- The client network prefix an unbadged collector created each target from,
-- so its per-source creation limit is counted from committed targets inside
-- the admission transaction. NULL on a badged collector and on targets
-- created before this migration.
ALTER TABLE targets ADD COLUMN created_source_prefix TEXT;
CREATE INDEX targets_by_creation_source ON targets(created_source_prefix, created_at)
    WHERE created_source_prefix IS NOT NULL;
