# ARCH-fleet-manager-storage: Durable state ownership

The daemon's SQLite database is the linearization owner for admission and
offer settings (see [ARCH-fleet-manager](./ARCH-fleet-manager.md)
*Concurrency model*). Per-seat runtime state is rebuilt from its durable rows
when needed. Operator settings and their epoch are read in one transaction
with capacity for quoting. Capacity is not separate state: database snapshots
count non-decommissioned seats and derive the remaining monotonic lifetime port
grid. Admission first resolves immutable replay or an already-stale epoch from
one read snapshot; a potentially accepting request checks the same facts again
after acquiring SQLite's writer reservation. Write methods return the
committed fact directly (or no value); they never reread a just-written row to
reconstruct or let SQLite choose the live result.

Refusals have no database row. They are permanent because they compare only a
durable random offer epoch. Every quote-invalidating transaction replaces that
32-byte epoch with fresh randomness before it commits: the acceptance that uses
the last slot replaces it with its seat insert, and a changed `QuoteSettings`
value replaces it with its write. Thus a refused quote's signed epoch cannot
equal a later epoch except with the negligible collision probability of a
256-bit random value; a restored installation with a fresh epoch likewise
refuses every quote from its former incarnation. `CreateSeat` writes only on
acceptance.

The epoch is a fixed 32-byte SQLite BLOB: it is an opaque equality token, not
a number or a text identifier.

Consequences the database transaction and schema enforce:

- **Set-once facts fail loudly.** A seat's creation columns are one fact
  set together or not at all; a schema trigger rejects every later update to
  any creation column and every
  seat-row deletion. The
  connection enables recursive triggers so implicit `REPLACE` deletes are
  covered too (closing rename, delete-and-reinsert, and replacement), and
  rewriting the immutable formed invite with a different value is an error,
  never an upsert. The first optional completion callback is retained for the
  whole formation, so later ceremony sessions cannot add, remove, or redirect its
  bearer destination. Callback attempt scheduling, sanitized outcome, and bearer clearing are committed
  before/after network I/O at their respective ownership boundaries. Decommission
  idempotency is decided from the ceremony-protected runtime mirror; its
  guarded insert into the decommission table must affect exactly one row.
- **Uniqueness is structural.** The seat primary key is the quote id's 32
  bytes, so accepted quote idempotency is enforced by the schema;
  refused quote idempotency follows from epoch equality and fresh randomness;
  a conflicting insert re-signs the same acceptance. `seat_no UNIQUE` and
  SQL `MAX + 1` allocation make local ordinals monotone and never reused;
  deriving four-port blocks from them prevents overlap.
- **Commitments are not stored at all.** An accepted `CreateSeatResponse` is
  `{quote_id, Accepted{seat_id}}` and `seat_id` *is* the quote id, so the
  envelope carries no fact the seat row does not already hold: it is a
  signature over "this quote was accepted", recomputed from the row and the
  manager key whenever a retry asks. What is durable is the acceptance, not the
  bytes announcing it. Idempotency is therefore semantic — a replay returns the
  same payload under a fresh signature, since Schnorr signing takes new aux
  randomness each time — and an FI must verify a commitment rather than compare
  it byte for byte.
- **Terminal facts are set-once.** The offer epoch is replaced only with a
  fresh random value in a quote-invalidating transaction. Formation and
  decommission are immutable once recorded; the seat row survives as dispute
  material.

## Representation: columns are for constraints, not for queries

A fact's stored shape follows what the schema must enforce and how its owner
reads it, never relational habit:

- A field is a **column** only for a constraint — identity (PRIMARY KEY),
  uniqueness (`seat_no`), presence coupling (CHECK), reference (FOREIGN KEY)
  — for a startup key into an in-memory owner, or for the disk-owned payment
  hand-off ledger's point lookup.
- Everything else is an **opaque payload** stored whole: a fact that is
  one value in memory is one value in the database (a DKG code set or semantic
  completion callback as one serialized field), so a torn or partial update is
  unrepresentable. The callback's lifecycle columns are the deliberate
  exception: they enforce resumable/terminal presence coupling, expose only a
  sanitized operator projection, retain the first callback choice across DKG
  attempts, and clear the live plaintext bearer on a terminal outcome. A value
  that is a pure function of columns already present is not stored at all; see
  signed seat commitments above.
- Storage may be **more opinionated than the wire**. Where a wire type is
  general so its vocabulary can grow, the database stores what this daemon
  can actually serve, and the two directions of the correspondence live
  together as inverses with a round-trip test. The offer is the case in
  point: the wire carries a list of `Plan`s, the `offer_state` row carries
  the one price, and an offer the daemon cannot serve is refused where it
  enters rather than stored and rediscovered as a `CorruptRow` later.
- Serialized payloads deliberately reuse the owning wire/domain type's
  serde format rather than parallel "storage types" that can drift; the
  cost — a serde rename is a data migration — is accepted knowingly. A
  payload that fails to parse when loaded is a loud `CorruptRow` error, never
  a silent default.

Migrations are embedded in the binary and run at open. This pre-launch callback
change intentionally retains the repository's earlier in-place rewrite of
`0001_initial.sql`; SQLx checksum validation therefore refuses any data root
that applied the old checksum before running later migrations. There is no
supported in-place upgrade for such a pre-launch database. An operator must
stop the old binary and preserve the complete data root, withdraw/export any
value using that old binary, then start with a new data root and re-onboard or
restore through the supported backup flow. Never delete only the SQLite file
beside live seat directories, and never reset a paid database before funds are
withdrawn. This reset contract is acceptable only because no production FMan
profile has launched; a post-launch schema change must be append-only.

## Durability policy

`synchronous = FULL`: every commit is
fsynced before it returns, so a committed row survives power loss.
`NORMAL` was rejected because two kinds of rows are promises to external
parties — a signed commitment sent to an FI and the accepted-payment recovery
evidence from which the wallet deterministically resumes Fedimint work
([SPEC-locked-payment](./SPEC-locked-payment.md)) — and forgetting an
acted-on promise is worse than the small per-commit fsync cost at this
write rate.
