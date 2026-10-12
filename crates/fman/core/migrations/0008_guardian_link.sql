-- The operator's linked Fedi app (SPEC-guardian-link): one device per FMan.
-- The push-gateway hook is a bearer and is cleared once the gateway has
-- rejected it for good; the row then shows the operator why a relink is due.
CREATE TABLE guardian_link (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    -- Hex x-only key the device signs its verbs with.
    device_id TEXT NOT NULL CHECK (length(device_id) = 64),
    device_label TEXT NOT NULL,
    -- JSON DkgCompletionCallback (hook URL + idempotency key); NULL once terminal.
    callback TEXT,
    -- Unix seconds the hook expires at the gateway.
    callback_expires_at INTEGER NOT NULL CHECK (callback_expires_at >= 0),
    linked_at_ms INTEGER NOT NULL CHECK (linked_at_ms >= 0),
    -- Count of delivered notifications; part of the next idempotency key.
    notification_seq INTEGER NOT NULL DEFAULT 0 CHECK (notification_seq >= 0),
    -- JSON array of reason codes the device was last told about and that
    -- still hold. A reason leaving and returning notifies again.
    notified_reasons TEXT NOT NULL DEFAULT '[]',
    last_notified_at_ms INTEGER,
    delivery TEXT NOT NULL DEFAULT 'active' CHECK (delivery IN ('active', 'terminal')),
    delivery_reason TEXT
);
