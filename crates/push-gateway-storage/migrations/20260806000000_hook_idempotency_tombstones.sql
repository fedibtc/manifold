CREATE TABLE hook_idempotency_tombstones (
    hook_id TEXT NOT NULL,
    caller_idempotency_key TEXT NOT NULL,
    target_count BIGINT NOT NULL CHECK (0 <= target_count),
    accepted_at BIGINT NOT NULL CHECK (0 <= accepted_at),
    retain_until BIGINT NOT NULL CHECK (0 <= retain_until),
    PRIMARY KEY (hook_id, caller_idempotency_key),
    FOREIGN KEY (hook_id) REFERENCES notification_hooks(hook_id) ON DELETE CASCADE
);

CREATE INDEX hook_idempotency_tombstones_retention_idx
ON hook_idempotency_tombstones (retain_until);

-- Preserve accepted idempotency keys across this additive upgrade. Existing
-- hooks can be at most one year long; the extra seven days is the bounded
-- cleanup margin used for newly accepted invocations too.
INSERT INTO hook_idempotency_tombstones (
    hook_id, caller_idempotency_key, target_count, accepted_at, retain_until
)
SELECT events.hook_id,
       events.caller_idempotency_key,
       events.target_count,
       events.created_at,
       COALESCE(hooks.expires_at, events.created_at + 31536000) + 604800
FROM notification_events AS events
JOIN notification_hooks AS hooks ON hooks.hook_id = events.hook_id
WHERE events.caller_idempotency_key IS NOT NULL;
