-- Whether this MG installs newer signed releases on its own. The owner's
-- opt-out, so every existing and new install starts with updates allowed.
-- Not part of the offer: changing it never touches the offer epoch.
CREATE TABLE auto_update_settings (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1))
);

INSERT INTO auto_update_settings (id) VALUES (1);
