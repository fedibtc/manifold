-- SeatGrants (SPEC-seat-grant). Authority comes from the MG's signature over
-- a grant, refusal from the used and revoked nonces; these tables hold those
-- two refusal sets and the operator's list.

-- The grants this install issued: the operator's list. It grants nothing, so
-- it is display state: it shows each grant's state and shows an unused
-- grant's code again. The exact signed payload and its signature are kept so
-- the code shown again is the code that was issued; the payload carries the
-- issue time and label, and `grant_nonce` repeats its nonce as the key the
-- refusal sets join on. A restore starts the list empty: an unused grant needs
-- no backup, because its signature stays valid.
CREATE TABLE seat_grants (
    grant_nonce BLOB PRIMARY KEY NOT NULL CHECK (
        typeof(grant_nonce) = 'blob' AND length(grant_nonce) = 32
    ),
    payload BLOB NOT NULL,
    signature BLOB NOT NULL CHECK (
        typeof(signature) = 'blob' AND length(signature) = 64
    )
);

-- Revoked nonces, refused whether or not this install issued them: a restore
-- rebuilds the set from the fleet-level revocation backup document. Only an
-- unused grant is revoked, and only an unrevoked one is redeemed; the writer
-- transactions of both check the other table.
CREATE TABLE seat_grant_revocations (
    grant_nonce BLOB PRIMARY KEY NOT NULL CHECK (
        typeof(grant_nonce) = 'blob' AND length(grant_nonce) = 32
    )
);

-- The grant a seat was redeemed from, written in the transaction that admits
-- the seat. The primary key is what lets one nonce create one seat. The label
-- is the grant's, carried in the seat's backup document with the nonce, so a
-- restore rebuilds the used set with the seats.
CREATE TABLE seat_grant_redemptions (
    grant_nonce BLOB PRIMARY KEY NOT NULL CHECK (
        typeof(grant_nonce) = 'blob' AND length(grant_nonce) = 32
    ),
    quote_id BLOB UNIQUE NOT NULL REFERENCES seats (quote_id),
    label TEXT CHECK (label IS NULL OR length(label) > 0)
);

-- The revocation document's last relay-confirmed publication, the fleet-level
-- counterpart of seat_backup_publications: what the relay demonstrably
-- serves, scoped to the storage format version that wrote it.
CREATE TABLE seat_grant_revocations_publication (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    doc_sha256 TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    published_at_ms INTEGER NOT NULL
);
