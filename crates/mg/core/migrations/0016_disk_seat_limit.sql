-- The disk seat limit (REQ-seat-capacity-default).
--
-- disk_max_seats is the size of the filesystem holding the MG data directory
-- divided by the setup-payment publication's per-seat disk figure, rounded
-- down. Disk size stands in for the drive's write endurance. NULL is no disk
-- limit, which applies until the first measurement and wherever the MG does
-- not measure (local E2E, or a failed measurement). Admission sells up to the
-- lowest of it and the other limits.
ALTER TABLE offer_state ADD COLUMN disk_max_seats INTEGER
    CHECK (disk_max_seats BETWEEN 0 AND 4294967295);
