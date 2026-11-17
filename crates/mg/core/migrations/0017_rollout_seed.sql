-- Staged rollouts (specs/SPEC-setup-payment-federations.md#staged-rollouts).
--
-- rollout_seed holds the one private random seed this MG draws its rollout
-- positions from: 32 bytes from the OS RNG, created at the first start that
-- needs it and never changed, so a seat keeps its draw for a feature across
-- restarts. It is not derived from the root mnemonic, so rollout state never
-- depends on identity secrets. No row means no seed yet.
CREATE TABLE rollout_seed (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    seed BLOB NOT NULL CHECK (length(seed) = 32)
);
