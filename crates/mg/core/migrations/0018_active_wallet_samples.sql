-- One local backup observation per formed seat and UTC date. Missing is not zero.
CREATE TABLE active_wallet_samples (
    quote_id BLOB NOT NULL REFERENCES formed_seats (quote_id),
    utc_day INTEGER NOT NULL CHECK (utc_day >= 0),
    wallet_count INTEGER NOT NULL CHECK (wallet_count >= 0),
    complete INTEGER NOT NULL CHECK (complete IN (0, 1)),
    PRIMARY KEY (quote_id, utc_day)
) WITHOUT ROWID;
