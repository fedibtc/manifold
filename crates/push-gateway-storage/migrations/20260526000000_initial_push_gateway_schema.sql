CREATE TABLE push_registrations (
    recipient_id TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    fcm_token TEXT NOT NULL,
    platform TEXT,
    created_at BIGINT NOT NULL CHECK (created_at >= 0),
    updated_at BIGINT NOT NULL CHECK (updated_at >= 0),
    last_seen_at BIGINT NOT NULL CHECK (last_seen_at >= 0),
    disabled_at BIGINT,
    disabled_reason TEXT,
    CHECK (disabled_at IS NULL OR disabled_at >= 0),
    PRIMARY KEY (recipient_id, installation_id)
);

CREATE UNIQUE INDEX push_registrations_fcm_token_idx
ON push_registrations (fcm_token);

CREATE UNIQUE INDEX push_registrations_installation_id_idx
ON push_registrations (installation_id);

CREATE INDEX push_registrations_recipient_active_idx
ON push_registrations (recipient_id, disabled_at, installation_id);

CREATE TABLE notification_hooks (
    hook_id TEXT PRIMARY KEY,
    secret_hash TEXT NOT NULL,
    recipient_id TEXT NOT NULL,
    label TEXT,
    kind TEXT,
    workflow TEXT,
    action TEXT,
    deep_link TEXT,
    open_behavior TEXT NOT NULL DEFAULT 'open_app',
    privacy TEXT NOT NULL DEFAULT 'display_text',
    title TEXT,
    body TEXT,
    data_json TEXT NOT NULL DEFAULT '{}',
    created_at BIGINT NOT NULL CHECK (created_at >= 0),
    expires_at BIGINT,
    revoked_at BIGINT,
    max_uses BIGINT,
    use_count BIGINT NOT NULL DEFAULT 0 CHECK (use_count >= 0),
    last_used_at BIGINT,
    rate_limit_window_seconds BIGINT NOT NULL DEFAULT 3600 CHECK (rate_limit_window_seconds > 0),
    rate_limit_max_requests BIGINT NOT NULL DEFAULT 2 CHECK (rate_limit_max_requests > 0),
    rate_limit_window_started_at BIGINT,
    rate_limit_count BIGINT NOT NULL DEFAULT 0 CHECK (rate_limit_count >= 0),
    CHECK (expires_at IS NULL OR expires_at >= 0),
    CHECK (revoked_at IS NULL OR revoked_at >= 0),
    CHECK (max_uses IS NULL OR max_uses > 0),
    CHECK (last_used_at IS NULL OR last_used_at >= 0),
    CHECK (rate_limit_window_started_at IS NULL OR rate_limit_window_started_at >= 0)
);

CREATE INDEX notification_hooks_recipient_id_idx
ON notification_hooks (recipient_id);

CREATE TABLE notification_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    hook_id TEXT NOT NULL,
    caller_event_id TEXT,
    recipient_id TEXT NOT NULL,
    notification_json TEXT NOT NULL,
    target_count BIGINT NOT NULL DEFAULT 0 CHECK (target_count >= 0),
    created_at BIGINT NOT NULL CHECK (created_at >= 0),
    FOREIGN KEY (hook_id) REFERENCES notification_hooks(hook_id) ON DELETE CASCADE,
    UNIQUE (hook_id, caller_event_id)
);

CREATE TABLE delivery_outbox (
    outbox_id TEXT PRIMARY KEY NOT NULL,
    event_id TEXT NOT NULL,
    recipient_id TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    fcm_token TEXT NOT NULL,
    platform TEXT,
    notification_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'in_progress', 'retrying', 'succeeded', 'invalid_token', 'dead_letter')),
    attempts BIGINT NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at BIGINT NOT NULL CHECK (next_attempt_at >= 0),
    last_attempt_at BIGINT,
    last_error TEXT,
    claim_id TEXT,
    created_at BIGINT NOT NULL CHECK (created_at >= 0),
    updated_at BIGINT NOT NULL CHECK (updated_at >= 0),
    FOREIGN KEY (event_id) REFERENCES notification_events(event_id) ON DELETE CASCADE,
    UNIQUE (event_id, recipient_id, installation_id),
    CHECK (last_attempt_at IS NULL OR last_attempt_at >= 0)
);

CREATE INDEX idx_delivery_outbox_due
ON delivery_outbox(status, next_attempt_at, created_at);

CREATE INDEX idx_delivery_outbox_event
ON delivery_outbox(event_id);

CREATE INDEX idx_delivery_outbox_installation
ON delivery_outbox(recipient_id, installation_id);
