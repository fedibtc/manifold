-- Additive facts; seats, payment evidence, formed facts and fee-wallet identity
-- remain untouched. Incoming provenance survives a later outgoing transfer.
CREATE TABLE seat_transfer_in (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    generation INTEGER NOT NULL CHECK (generation > 0),
    import_complete INTEGER NOT NULL CHECK (import_complete IN (0, 1)),
    record BLOB NOT NULL
);

CREATE TABLE seat_transfer_out (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    phase TEXT NOT NULL CHECK (phase IN ('stopping', 'stopped', 'removing', 'removed'))
);

CREATE TRIGGER seat_transfer_out_no_delete
BEFORE DELETE ON seat_transfer_out
BEGIN SELECT RAISE(ABORT, 'transfer retirement is permanent'); END;

CREATE TRIGGER seat_transfer_out_monotone
BEFORE UPDATE ON seat_transfer_out
WHEN OLD.quote_id != NEW.quote_id OR NOT (
    OLD.phase = NEW.phase
    OR (OLD.phase = 'stopping' AND NEW.phase = 'stopped')
    OR (OLD.phase = 'stopped' AND NEW.phase = 'removing')
    OR (OLD.phase = 'removing' AND NEW.phase = 'removed')
)
BEGIN SELECT RAISE(ABORT, 'transfer retirement cannot regress'); END;

-- Defense in depth below the seat loop: a transfer fence can never be erased
-- through either ceremony entry point, including after daemon restart.
CREATE TRIGGER dkg_inputs_no_transfer_insert
BEFORE INSERT ON dkg_inputs
WHEN EXISTS (SELECT 1 FROM seat_transfer_in WHERE quote_id = NEW.quote_id)
  OR EXISTS (SELECT 1 FROM seat_transfer_out WHERE quote_id = NEW.quote_id)
BEGIN SELECT RAISE(ABORT, 'transfer seat cannot enter DKG'); END;

CREATE TRIGGER dkg_inputs_no_transfer_update
BEFORE UPDATE ON dkg_inputs
WHEN EXISTS (SELECT 1 FROM seat_transfer_in WHERE quote_id = NEW.quote_id)
  OR EXISTS (SELECT 1 FROM seat_transfer_out WHERE quote_id = NEW.quote_id)
BEGIN SELECT RAISE(ABORT, 'transfer seat cannot enter DKG'); END;
