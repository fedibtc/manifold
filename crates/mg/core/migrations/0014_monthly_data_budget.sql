-- A monthly data limit on seats (REQ-seat-capacity-default). Admission sells
-- no more seats than the operator's monthly data budget divided by the data
-- one seat uses, as the setup-payment publication states it:
--
-- - monthly_data_budget, the operator's answer, as one token: 'not_set'
--   (not answered, as on every MG set up before the question), 'unlimited'
--   (no monthly data cap), or the GB as a decimal number from 1. Only a
--   number limits seats.
-- - policy_seat_monthly_data_gb, the data one seat uses, from the
--   publication. Until the first write it is 65, the default when the
--   publication does not set it.
--
-- GB is 10^9 bytes, per 30 days. Existing rows become 'not_set', which
-- limits nothing, so no limit moves and the offer epoch stays.
ALTER TABLE offer_state ADD COLUMN monthly_data_budget TEXT NOT NULL DEFAULT 'not_set'
    CHECK (
        monthly_data_budget IN ('not_set', 'unlimited')
        OR (
            CAST(monthly_data_budget AS INTEGER) BETWEEN 1 AND 4294967295
            AND CAST(CAST(monthly_data_budget AS INTEGER) AS TEXT) = monthly_data_budget
        )
    );
ALTER TABLE offer_state ADD COLUMN policy_seat_monthly_data_gb INTEGER NOT NULL DEFAULT 65
    CHECK (policy_seat_monthly_data_gb BETWEEN 1 AND 4294967295);
