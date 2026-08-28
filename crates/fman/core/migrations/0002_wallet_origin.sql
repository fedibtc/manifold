-- Existing identities predate the proof that their wallet database and
-- mnemonic have uninterrupted fresh-install history. Classify them
-- conservatively: their first uninitialized scope continues to recover.
-- New onboarding explicitly writes `fresh`; restore explicitly writes
-- `restored`.
ALTER TABLE identity ADD COLUMN wallet_origin TEXT NOT NULL DEFAULT 'restored'
    CHECK (wallet_origin IN ('fresh', 'restored'));
