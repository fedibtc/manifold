ALTER TABLE notification_hooks RENAME COLUMN hook_token_hash TO hook_secret_hash;
ALTER TABLE notification_events RENAME COLUMN caller_event_id TO caller_idempotency_key;

DROP INDEX IF EXISTS notification_hooks_hook_token_hash_idx;
CREATE UNIQUE INDEX notification_hooks_hook_secret_hash_idx
ON notification_hooks (hook_secret_hash);
