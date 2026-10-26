-- Additive facts; seats, payment evidence, formed facts and fee-wallet identity
-- remain untouched. Incoming provenance survives a later outgoing transfer.
-- A waiting row holds the live FA grant; an accepted row holds only the
-- guardian's Iroh secrets. The archive itself lives in the seat data directory
-- and in the backup.
CREATE TABLE seat_transfer_in (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    state TEXT NOT NULL CHECK (state IN ('waiting', 'imported')),
    binding TEXT NOT NULL,
    callback_commitment BLOB CHECK (callback_commitment IS NULL OR length(callback_commitment) = 32),
    grant TEXT,
    iroh_api_sk BLOB CHECK (iroh_api_sk IS NULL OR length(iroh_api_sk) = 32),
    iroh_p2p_sk BLOB CHECK (iroh_p2p_sk IS NULL OR length(iroh_p2p_sk) = 32),
    CHECK ((state = 'waiting') = (grant IS NOT NULL)),
    CHECK ((state != 'waiting') = (iroh_api_sk IS NOT NULL AND iroh_p2p_sk IS NOT NULL))
);

CREATE TABLE seat_transfer_out (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    phase TEXT NOT NULL CHECK (phase IN ('stopped', 'removed'))
);

CREATE TRIGGER seat_transfer_out_no_delete
BEFORE DELETE ON seat_transfer_out
BEGIN SELECT RAISE(ABORT, 'transfer retirement is permanent'); END;

CREATE TRIGGER seat_transfer_out_monotone
BEFORE UPDATE ON seat_transfer_out
WHEN OLD.quote_id != NEW.quote_id OR NOT (
    OLD.phase = NEW.phase
    OR (OLD.phase = 'stopped' AND NEW.phase = 'removed')
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

-- The transfer fence carried by the seat's last relay-confirmed document
-- (its `transfer.outgoing`). A source releases the guardian's material only
-- once this reads 'stopped', so a restore from the relays never starts the
-- moved guardian again.
ALTER TABLE seat_backup_publications
ADD COLUMN confirmed_outgoing TEXT CHECK (confirmed_outgoing IN ('stopped', 'removed'));
