-- FCM tokens are provider-issued device capabilities. Keep their stable
-- installation binding separate from the refreshable recipient route so stale
-- registration GC cannot authorize transfer to a different installation. The
-- exact token/installation pair may follow an authenticated account switch.
CREATE TABLE push_registration_token_owners (
    fcm_token TEXT PRIMARY KEY,
    recipient_id TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    updated_at BIGINT NOT NULL CHECK (0 <= updated_at)
);

INSERT INTO push_registration_token_owners (
    fcm_token, recipient_id, installation_id, updated_at
)
SELECT fcm_token, recipient_id, installation_id, updated_at
FROM push_registrations
WHERE 1 = 1
ON CONFLICT(fcm_token) DO NOTHING;

CREATE INDEX push_registration_token_owners_installation_idx
ON push_registration_token_owners (recipient_id, installation_id);

-- Portable transaction mutexes make the count-and-insert admission decisions
-- serializable on both SQLite and PostgreSQL.
CREATE TABLE push_gateway_admission_locks (
    resource TEXT PRIMARY KEY,
    updated_at BIGINT NOT NULL CHECK (0 <= updated_at)
);

INSERT INTO push_gateway_admission_locks (resource, updated_at)
VALUES ('registration', 0), ('hook', 0)
ON CONFLICT(resource) DO NOTHING;
