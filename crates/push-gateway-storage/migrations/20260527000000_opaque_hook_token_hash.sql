ALTER TABLE notification_hooks RENAME COLUMN secret_hash TO hook_token_hash;

CREATE UNIQUE INDEX notification_hooks_hook_token_hash_idx
ON notification_hooks (hook_token_hash);
