-- Snapshot eligible local upgrade evidence once. This table never reads a
-- relay and is not populated by a mnemonic restore or subsequent refresh.
CREATE TABLE holder_binding_legacy_evidence (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    event_json TEXT NOT NULL
);
-- Db::open marks upgrades from the old multi-event schema before 0005 runs.
CREATE TABLE IF NOT EXISTS holder_binding_ambiguous_upgrade (
    id INTEGER PRIMARY KEY CHECK (id = 1)
);
INSERT INTO holder_binding_legacy_evidence
SELECT 1, h.event_json FROM holder_authorization h, onboarding_state o
WHERE h.id = 1 AND o.id = 1 AND o.stage = 'complete'
  AND NOT EXISTS (SELECT 1 FROM holder_binding_ambiguous_upgrade);

-- Provenance records how ownership was established, not how it was renewed.
CREATE TABLE holder_binding (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    holder BLOB NOT NULL CHECK (length(holder) = 32),
    provenance TEXT NOT NULL CHECK (provenance IN ('legacy_active', 'paired'))
);
CREATE TABLE holder_pairing_grant (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    digest BLOB NOT NULL CHECK (length(digest) = 32),
    expires_at_ms INTEGER NOT NULL
);
