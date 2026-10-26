-- A destination seat stays bound to one move, and a grant has no expiry, so
-- no admission replaces another. The generation that fenced replaced
-- admissions is gone.
ALTER TABLE seat_transfer_in DROP COLUMN generation;
