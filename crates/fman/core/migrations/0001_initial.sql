-- Rewritten in place because no FMan has been deployed with real payments
-- (confirmed 2026-07-27); there is no paid database requiring migration.
-- The environment binding and stateless-DKG tables intentionally
-- invalidate preceding experimental databases. Roots predating environment
-- binding have unknown origin; roots containing environment binding but predating
-- callback durability have known origin but an obsolete schema. Both must fail
-- migration rather than be silently claimed or partly upgraded.
CREATE TABLE identity (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    mnemonic TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

-- A data root belongs to exactly one Manifold deployment environment. The row
-- is installed before onboarding and never changed, so state authenticated in
-- Development cannot later be served by a Staging or Production invocation.
CREATE TABLE manifold_environment (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    environment TEXT NOT NULL CHECK (
        environment IN ('development', 'staging', 'production')
    )
);

-- Guard INSERT itself, not only DELETE: SQLite implements INSERT OR REPLACE
-- with an implicit delete whose trigger depends on recursive_triggers, while
-- this trigger fires under the default connection settings too.
CREATE TRIGGER manifold_environment_immutable_reinsert
BEFORE INSERT ON manifold_environment
WHEN EXISTS (SELECT 1 FROM manifold_environment WHERE id = 1)
BEGIN
    SELECT RAISE(ABORT, 'Manifold environment binding is already installed');
END;

CREATE TRIGGER manifold_environment_immutable_update
BEFORE UPDATE ON manifold_environment
BEGIN
    SELECT RAISE(ABORT, 'Manifold environment binding is immutable');
END;

CREATE TRIGGER manifold_environment_immutable_delete
BEFORE DELETE ON manifold_environment
BEGIN
    SELECT RAISE(ABORT, 'Manifold environment binding is immutable');
END;

-- Private durable high-water mark owned by fman-nostr. The complete
-- signed event is retained so its authentication and NIP-01 replacement order
-- can be re-established after restart.
CREATE TABLE nostr_setup_payment_federations (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    event_json TEXT NOT NULL
);

CREATE TABLE seats (
    quote_id BLOB PRIMARY KEY NOT NULL CHECK (
        typeof(quote_id) = 'blob' AND length(quote_id) = 32
    ),
    -- Never-reused allocation ordinal. It names the seat directory and its
    -- four-port block, including after decommission.
    seat_no INTEGER UNIQUE NOT NULL CHECK (seat_no >= 0),
    fi_id TEXT NOT NULL,
    -- The only accepted-quote projections the seat lifecycle still uses.
    -- Paid claim recovery owns the complete generation-specific evidence
    -- in ecash_claims instead of making every seat carry a protocol transcript.
    plan TEXT NOT NULL,
    federation_size INTEGER NOT NULL CHECK (federation_size > 0),
    created_at_ms INTEGER NOT NULL
);

-- Durable accepted-claim work ledger. A free seat has no row: absence is the
-- durable fact that nothing was paid. Everything stored here is public;
-- fman-fedimint validates that evidence contains no bearer note or spend key.
CREATE TABLE ecash_claims (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    -- CBOR encoding of core's closed EcashClaimEvidence enum. The same enum is
    -- embedded directly in the CBOR backup document.
    evidence BLOB NOT NULL CHECK (typeof(evidence) = 'blob'),
    claim_outcome TEXT CHECK (claim_outcome IN ('success', 'already_spent')),
    claim_outcome_at_ms INTEGER,
    CHECK ((claim_outcome IS NULL) = (claim_outcome_at_ms IS NULL))
);

CREATE TABLE decommissioned_seats (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    decommissioned_at_ms INTEGER NOT NULL
);

-- The single durable formation fact. Ceremony inputs and progress are never
-- persisted: the final data directory is installed atomically, and this row
-- records the first observation of that completed configuration. Restore is
-- the other writer of the same fact.
CREATE TABLE formed_seats (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    federation_invite TEXT NOT NULL,
    formed_at_ms INTEGER NOT NULL
);

CREATE TRIGGER formed_seats_immutable_update
BEFORE UPDATE ON formed_seats BEGIN
    SELECT RAISE(ABORT, 'formed seat records are immutable');
END;

CREATE TRIGGER formed_seats_immutable_delete
BEFORE DELETE ON formed_seats BEGIN
    SELECT RAISE(ABORT, 'formed seat records are immutable');
END;

-- The first formation metadata target this guardian admitted. Consensus may
-- later be controlled by a hostile threshold, so live metadata cannot itself
-- authenticate formation-owned directory and recipient policy.
CREATE TABLE formation_fee_policies (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    directory TEXT NOT NULL,
    recipients TEXT NOT NULL
);

CREATE TRIGGER formation_fee_policies_immutable_update
BEFORE UPDATE ON formation_fee_policies BEGIN
    SELECT RAISE(ABORT, 'formation fee policy is immutable');
END;

CREATE TRIGGER formation_fee_policies_immutable_delete
BEFORE DELETE ON formation_fee_policies BEGIN
    SELECT RAISE(ABORT, 'formation fee policy is immutable');
END;

-- The last backup publication of each seat this install confirmed the relay
-- serves (SPEC-nostr-backup-restore). The relay is semi-trusted to keep
-- serving the latest event per coordinate, so a confirmed publication is a
-- durable fact: the backup worker republishes a seat only when its current
-- document no longer matches this row, instead of republishing everything on
-- every start. Written by the worker alone, and only after the relay's
-- read-back confirmation — a crash between confirm and record errs toward one
-- redundant republication of an addressable event, never toward a missing one.
CREATE TABLE seat_backup_publications (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    -- SHA-256 (hex) of the plaintext document, not the sealed event:
    -- sealing is nonce-randomized, so the sealed bytes are not a stable
    -- identity; the plaintext is the publication's only stable one.
    doc_sha256 TEXT NOT NULL,
    published_at_ms INTEGER NOT NULL,
    -- Digest of the guardian archive whose events were confirmed together
    -- with the document, once one exists. The archive is immutable, so a
    -- recorded digest means its bytes never need publishing again — and the
    -- assembler stops rereading the config files to prove it.
    archive_digest TEXT,
    -- The envelope schema version the publication was sealed under. The
    -- version is outside the plaintext hash, so without this column a build
    -- carrying a new version would see an unchanged doc_sha256 and leave the
    -- relay serving events it would itself refuse to restore. A record only
    -- counts as confirmed for the version that wrote it.
    schema_version INTEGER NOT NULL
);

-- One durable generation for the FMan-wide telemetry bearer. The bearer itself
-- remains root-derived and is never persisted. Advancing this singleton
-- immediately revokes the previously registered capability for every surface.
CREATE TABLE telemetry_capability (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    updated_at_ms INTEGER NOT NULL
);

INSERT INTO telemetry_capability (id, generation, updated_at_ms)
VALUES (1, 0, 0);

-- Optional FI-owned completion capability for the formation. The first
-- StartDkg choice is retained across later attempts. A fleet-wide worker joins
-- it to formed/decommissioned seat facts.
CREATE TABLE completion_callbacks (
    quote_id BLOB PRIMARY KEY NOT NULL REFERENCES seats (quote_id),
    completion_callback TEXT,
    completion_callback_status TEXT NOT NULL DEFAULT 'not_configured' CHECK (
        completion_callback_status IN (
            'not_configured', 'pending', 'operator_blocked', 'delivered', 'terminal'
        )
    ),
    completion_callback_attempts INTEGER NOT NULL DEFAULT 0 CHECK (
        completion_callback_attempts >= 0
    ),
    completion_callback_next_attempt_at_ms INTEGER,
    completion_callback_reason TEXT,
    completion_callback_completed_at_ms INTEGER
);

-- Accepted OOB ecash payment federations: the membership of the admitted
-- common setup-payment set (SPEC-setup-payment-federations), derived from
-- the event retained in nostr_setup_payment_federations. Replaced in the
-- same transaction that stores that event and — when any member was
-- removed — bumps the offer epoch, so quote admission can never observe an
-- accepted set inconsistent with the epoch. The wallet's per-federation
-- client state lives in the wallet data dir and is never removed by a set
-- change (received balances stay withdrawable).
CREATE TABLE setup_payment_federation_members (
    federation_id TEXT PRIMARY KEY NOT NULL
);

-- The whole offer, as one row: what the fleet sells seats for, plus the
-- durable random epoch regenerated whenever anything a quote was priced under
-- changes. A quote from any other epoch is permanently refused.
--
-- The wire carries a list of `Plan` values so the vocabulary can grow; this
-- daemon serves exactly one of them, so what it stores is the price. A NULL
-- price is a fleet that is not selling, which is where every fresh or
-- restored FMan starts.
CREATE TABLE offer_state (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    max_seats INTEGER NOT NULL CHECK (0 <= max_seats AND max_seats <= 4294967295),
    price_msats INTEGER CHECK (price_msats IS NULL OR price_msats >= 0),
    updated_at_ms INTEGER,
    offer_epoch BLOB NOT NULL CHECK (
        typeof(offer_epoch) = 'blob'
        AND length(offer_epoch) = 32
    )
);

INSERT INTO offer_state (id, max_seats, offer_epoch) VALUES (1, 0, randomblob(32));

-- The operator-facing setup workflow. Identity and restored-seat facts live in
-- their owning tables; this row records which operation the pre-fleet admin
-- surface accepts next. A fleet opens only after the row reaches `complete`.
CREATE TABLE onboarding_state (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    stage TEXT NOT NULL CHECK (
        stage IN ('identity', 'holder_authorization', 'initial_offer', 'complete')
    ),
    updated_at_ms INTEGER NOT NULL CHECK (0 <= updated_at_ms)
);

INSERT INTO onboarding_state (id, stage, updated_at_ms) VALUES (1, 'identity', 0);

-- Global operator-owned destination for revenue settlement. This is
-- independent of what the fleet currently offers and never participates in
-- quote invalidation.
CREATE TABLE payout_settings (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    destination TEXT CHECK (
        destination IS NULL OR length(destination) BETWEEN 1 AND 1024
    )
);

INSERT INTO payout_settings (id) VALUES (1);

-- Caller-owned payout identities and the exact wallet inputs they committed to.
-- A row exists before native wallet submission.  If the daemon dies after that
-- submission but before recording its operation id, native request metadata
-- repairs the nullable operation fields without starting another payment.
CREATE TABLE payout_jobs (
    request_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(request_id) BETWEEN 1 AND 128
    ),
    scope_kind TEXT NOT NULL CHECK (
        scope_kind IN ('payment_federation', 'guardian_fee')
    ),
    federation_id TEXT NOT NULL,
    seat_id TEXT,
    invite_code TEXT,
    destination TEXT NOT NULL CHECK (
        length(destination) BETWEEN 1 AND 1024
    ),
    operation_id TEXT CHECK (
        operation_id IS NULL OR (
            length(operation_id) = 64
            AND operation_id NOT GLOB '*[^0-9a-f]*'
        )
    ),
    amount_msat INTEGER CHECK (amount_msat IS NULL OR 0 <= amount_msat),
    created_at_ms INTEGER NOT NULL CHECK (0 <= created_at_ms),
    committed_at_ms INTEGER CHECK (
        committed_at_ms IS NULL OR 0 <= committed_at_ms
    ),
    CHECK (
        (scope_kind = 'payment_federation' AND seat_id IS NULL AND invite_code IS NULL)
        OR (
            scope_kind = 'guardian_fee'
            AND seat_id IS NOT NULL
            AND invite_code IS NOT NULL
        )
    ),
    CHECK (
        (operation_id IS NULL AND amount_msat IS NULL AND committed_at_ms IS NULL)
        OR (operation_id IS NOT NULL AND amount_msat IS NOT NULL AND committed_at_ms IS NOT NULL)
    )
);

CREATE TRIGGER payout_jobs_identity_immutable
BEFORE UPDATE OF request_id, scope_kind, federation_id, seat_id, invite_code, destination, created_at_ms
ON payout_jobs
BEGIN
    SELECT RAISE(ABORT, 'payout job identity is immutable');
END;

CREATE TRIGGER payout_jobs_commit_monotone
BEFORE UPDATE OF operation_id, amount_msat, committed_at_ms
ON payout_jobs
WHEN OLD.operation_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'payout job operation is immutable');
END;

CREATE TRIGGER payout_jobs_no_delete
BEFORE DELETE ON payout_jobs
BEGIN
    SELECT RAISE(ABORT, 'payout jobs are durable');
END;


-- Every column is one creation-time fact. Reject
-- even a same-value UPDATE so no later writer can rename a seat, change its
-- owner, or release a lifetime-unique quote or port identity for reuse.
CREATE TRIGGER seats_creation_immutable
BEFORE UPDATE OF
    quote_id,
    seat_no,
    fi_id,
    plan,
    federation_size,
    created_at_ms
ON seats
BEGIN
    SELECT RAISE(ABORT, 'seat creation facts are immutable');
END;

-- Claim evidence and addressing are immutable recovery facts. Only the
-- terminal observation advances.
CREATE TRIGGER ecash_claims_facts_immutable
BEFORE UPDATE OF quote_id, evidence
ON ecash_claims
BEGIN
    SELECT RAISE(ABORT, 'ecash claim evidence is immutable');
END;

CREATE TRIGGER ecash_claims_no_delete
BEFORE DELETE ON ecash_claims
BEGIN
    SELECT RAISE(ABORT, 'ecash claim rows are retained recovery material');
END;

-- Seat rows are lifetime identities and retained dispute material. Prevent a
-- future delete-and-reinsert path from walking around owner immutability.
-- Db::open enables recursive_triggers so this also rejects the implicit delete
-- used by INSERT/UPDATE OR REPLACE.
CREATE TRIGGER seats_no_delete
BEFORE DELETE ON seats
BEGIN
    SELECT RAISE(ABORT, 'seat rows are immutable lifetime identities');
END;

-- Holder-authorization events retained verbatim: complete public kind-37705
-- Nostr events already classified as public credential material, keyed by
-- credential digest.
CREATE TABLE holder_authorization_events (
    credential_digest BLOB PRIMARY KEY NOT NULL
        CHECK (typeof(credential_digest) = 'blob' AND length(credential_digest) = 32),
    authorization_issued_at BLOB NOT NULL
        CHECK (typeof(authorization_issued_at) = 'blob' AND length(authorization_issued_at) = 8),
    event_json TEXT NOT NULL
);
