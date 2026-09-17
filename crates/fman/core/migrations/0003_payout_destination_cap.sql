ALTER TABLE payout_jobs ADD COLUMN cap_maximum_msat INTEGER CHECK (
    cap_maximum_msat IS NULL OR 0 <= cap_maximum_msat
);
ALTER TABLE payout_jobs ADD COLUMN cap_remaining_msat INTEGER CHECK (
    cap_remaining_msat IS NULL OR 0 <= cap_remaining_msat
);

DROP TRIGGER payout_jobs_commit_monotone;

CREATE TRIGGER payout_jobs_commit_monotone
BEFORE UPDATE OF
    operation_id, amount_msat, committed_at_ms, cap_maximum_msat, cap_remaining_msat
ON payout_jobs
WHEN OLD.operation_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'payout job operation is immutable');
END;
