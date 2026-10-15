-- Runtime limits on total seats beside the operator's max_seats
-- (REQ-seat-capacity-default). A background task rewrites both: the memory
-- limit from sampled host memory, the policy limit from the setup-payment
-- publication. Admission uses the lowest limit. They start at the u32
-- maximum, which never binds, so this migration removes no slot under a live
-- offer epoch. The first write by the task lowers them in a transaction that
-- keeps the epoch invariants.
ALTER TABLE offer_state ADD COLUMN memory_max_seats INTEGER NOT NULL
    DEFAULT 4294967295 CHECK (memory_max_seats BETWEEN 0 AND 4294967295);
ALTER TABLE offer_state ADD COLUMN policy_max_seats INTEGER NOT NULL
    DEFAULT 4294967295 CHECK (policy_max_seats BETWEEN 0 AND 4294967295);

-- An operator max_seats at the u32 maximum is automatic: the runtime limits
-- decide. The old onboarding form pre-filled a RAM recommendation capped at
-- 8, so a value from 1 to 8 is most likely that recommendation; it becomes
-- automatic. A value above 8 was typed by the operator, and 0 means not
-- selling; both stay. The replaced value is kept so it can be restored.
-- Raising a limit only adds slots, so no outstanding quote loses its slot and
-- the offer epoch stays.
ALTER TABLE offer_state ADD COLUMN pre_automatic_max_seats INTEGER
    CHECK (pre_automatic_max_seats BETWEEN 0 AND 4294967295);
UPDATE offer_state
SET pre_automatic_max_seats = max_seats, max_seats = 4294967295
WHERE max_seats BETWEEN 1 AND 8;
