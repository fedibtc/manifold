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
