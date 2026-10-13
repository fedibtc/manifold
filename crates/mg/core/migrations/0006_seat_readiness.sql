-- The daemon's last readiness verdict for new seats. It survives restarts so a
-- healthy FMan keeps selling across one and a failing FMan stays closed. A
-- fresh, restored, or upgraded FMan starts closed until its first run passes.
ALTER TABLE offer_state ADD COLUMN ready_for_new_seats INTEGER NOT NULL DEFAULT 0
    CHECK (ready_for_new_seats IN (0, 1));
