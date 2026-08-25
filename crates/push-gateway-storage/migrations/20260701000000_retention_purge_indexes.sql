CREATE INDEX idx_delivery_outbox_retention_terminal
ON delivery_outbox(status, updated_at);

CREATE INDEX idx_push_registrations_disabled_at
ON push_registrations(disabled_at);

CREATE INDEX idx_notification_events_retention
ON notification_events(created_at);
