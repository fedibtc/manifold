-- An accepted, validated StartDkg/RestartDkg envelope survives child and daemon death.
-- The set is public setup material, not the guardian's private keys or config.
CREATE TABLE dkg_inputs (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    guardian_codes TEXT NOT NULL CHECK (json_valid(guardian_codes))
);
