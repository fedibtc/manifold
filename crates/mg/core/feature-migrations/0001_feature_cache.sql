-- Independent replacement floor. Never rebuild or touch the payment membership here.
CREATE TABLE IF NOT EXISTS nostr_feature_config_v1 (
    publisher TEXT PRIMARY KEY NOT NULL,
    event_json TEXT NOT NULL
);
