-- The bundled fedimintd line each seat runs
-- (ARCH-manifold-guardian-seat-processes *Subprocess model*).
--
-- Every seat created before this migration was formed on, and still runs, the
-- 0.12 line: it was the only line those releases hosted. The DEFAULT exists
-- only because SQLite requires one to add a NOT NULL column; every insert
-- binds the line explicitly.
ALTER TABLE seats ADD COLUMN fedimintd_line TEXT NOT NULL DEFAULT '0.12'
    CHECK (fedimintd_line IN ('0.12', '0.13'));
