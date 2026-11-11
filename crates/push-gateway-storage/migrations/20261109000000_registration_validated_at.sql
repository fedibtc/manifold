-- When the provider last accepted the token: at registration, or on the
-- periodic validate-only re-check. Registrations no longer go stale by age;
-- the re-check disables tokens the provider rejects.
ALTER TABLE push_registrations ADD COLUMN validated_at BIGINT NOT NULL DEFAULT 0;

UPDATE push_registrations SET validated_at = last_seen_at;

CREATE INDEX idx_push_registrations_validated_at
ON push_registrations(validated_at);
