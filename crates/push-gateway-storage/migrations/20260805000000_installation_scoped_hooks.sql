-- Hooks created for FI formation target only the initiating app installation.
-- A legacy hook can be narrowed without ambiguity only when its recipient has
-- exactly one installation. Invalidate every other legacy hook rather than
-- preserving broadcast behavior.
ALTER TABLE notification_hooks
ADD COLUMN installation_id TEXT NOT NULL
DEFAULT '__invalid_legacy_installation__';

UPDATE notification_hooks
SET installation_id = (
    SELECT MIN(registration.installation_id)
    FROM push_registrations registration
    WHERE registration.recipient_id = notification_hooks.recipient_id
)
WHERE (
    SELECT COUNT(*)
    FROM push_registrations registration
    WHERE registration.recipient_id = notification_hooks.recipient_id
) = 1;

DELETE FROM delivery_outbox
WHERE event_id IN (
    SELECT event.event_id
    FROM notification_events event
    JOIN notification_hooks hook ON hook.hook_id = event.hook_id
    WHERE hook.installation_id = '__invalid_legacy_installation__'
);
DELETE FROM notification_events
WHERE hook_id IN (
    SELECT hook_id FROM notification_hooks
    WHERE installation_id = '__invalid_legacy_installation__'
);
DELETE FROM notification_hooks
WHERE installation_id = '__invalid_legacy_installation__';

CREATE INDEX notification_hooks_recipient_installation_idx
ON notification_hooks (recipient_id, installation_id);

CREATE INDEX push_registrations_active_last_seen_idx
ON push_registrations (disabled_at, last_seen_at);

CREATE INDEX notification_hooks_lifecycle_idx
ON notification_hooks (revoked_at, expires_at);
