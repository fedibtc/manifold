-- The ledger names each payout's native operation before it starts, so
-- recovery reads that one operation instead of scanning the wallet's
-- operation log for request metadata.
--
-- An ecash payout's operation id derives from its request id alone. A
-- Lightning payout's derives from its rail and invoice, which the ledger
-- writes before paying: `lightning_rail` and `invoice` hold the attempt being
-- paid, and with them the destination cap that sized it. While no operation
-- is committed, a retry that finds no native operation for the attempt may
-- replace it; a committed attempt is immutable. Rows from before this change
-- have no attempt.

ALTER TABLE payout_jobs ADD COLUMN lightning_rail TEXT CHECK (
    lightning_rail IS NULL
    OR (method = 'lightning' AND lightning_rail IN ('lnv1', 'lnv2'))
);
ALTER TABLE payout_jobs ADD COLUMN invoice TEXT CHECK (
    (invoice IS NULL) = (lightning_rail IS NULL)
    AND (invoice IS NULL OR length(invoice) BETWEEN 1 AND 4096)
);

DROP TRIGGER payout_jobs_commit_monotone;

CREATE TRIGGER payout_jobs_commit_monotone
BEFORE UPDATE OF
    operation_id, amount_msat, committed_at_ms, cap_maximum_msat,
    cap_remaining_msat, lightning_rail, invoice
ON payout_jobs
WHEN OLD.operation_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'payout job operation is immutable');
END;
