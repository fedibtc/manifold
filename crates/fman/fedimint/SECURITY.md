# Fleet Manager wallet security and reliability boundaries

This crate holds FMan Fedimint wallet roots and implements the payee side of
key-locked FI-to-FMan seat payments. Payment verification, claim, refund,
formatting, and database changes require monetary-safety review.

## Role separation

The FMan payee lives in `payee`, implementing `fman-core`'s `EcashWallet`;
`guardian_fee` implements that crate's `GuardianFeeVault`. Shared protocol and
cryptographic mechanics live in the separate `locked-payments` crate. The
development/reference FI payer lives in `fi-cli`; production consumers provide
their own `fi-client` payment adapter. Keep these roles separate: this crate
does not expose payer operations or own FI payment holds, and FI consumers must
not obtain payee spend authority.

This crate holds no policy about who gets paid. Guardian-fee accounts and the
metadata value a seat votes for are core's; `guardian_fee` here signs and moves
with the key core hands it, and refuses when the stability-pool module carries
any other account.

Guardian-fee collection can leave a durable operation behind before a later
phase fails. Its incomplete result counts only amounts reported by terminal
success states and performs a best-effort balance read without replacing the
original failure. The result carries only a closed phase and submission state,
not dependency error text; core selects the static operator-facing message when
it projects the successful Admin response.

The pinned Fedimint client creates the operation log entry and state machines in
the same local database autocommit as transaction submission, and returns the
operation ID only after that commit. Collection therefore treats a returned ID
as the durable boundary. A returned submission error before an ID does not report
a durable local operation; process death after commit but before the dependency
returns remains a lost-response case. Re-audit this boundary when the pinned
client changes operation creation, transaction submission, or database commit
ordering.

Native payout start uses the same returned-ID durability boundary for both
Lightning generations. FMan serializes request lookup and possible start within
one wallet scope and
rejects a v1 LNURL invoice whose payment hash already has a completed operation,
because the pinned v1 client would otherwise return that old operation without
committing a new one. After start, FMan reads only the exact returned v1
operation and requires its request ID, destination, rail, and amount to match;
missing, legacy, or mismatched metadata fails closed without snapshotting the
wallet's growing operation history. Native status and await apply the same exact
operation metadata binding in the selected wallet scope; they subscribe to or
read that operation and never fetch another invoice or call a payment-start API.
Both Lightning generations persist the caller request id in native payout
metadata. A start under the scope fence returns the existing matching operation
instead of requesting another invoice; this reconciles process death after the
native commit but before the FMan SQLite job records the operation id.
The normalized rail outcome is independent of the operation's active-state-machine
fact: v2 success may retain mint change work, while a rejected or refunded rail
may retain mint input/refund work. The payout worker in this crate durably binds
that request id to one scope and destination before invoking the native wallet
boundary.

Payout diagnostics may record the public federation id, rail, selected public
gateway key, operation id, recipient amount, gateway fee quote, and the known
outgoing-contract fee effect. They must label a gateway fee as a quote and must
not imply it includes federation or mint fees not exposed by the pinned start
API. They do not log the LNURL destination, invoice, preimage, request metadata,
raw dependency errors, or invented individual peer-connectivity details.
`SafeUrl::as_str()` is not a log sanitizer: it exposes userinfo, query, and
fragment data, so payout diagnostics never format a gateway API URL. The upstream
v2 selector reports only aggregate gateway outcomes; a failure to obtain routing
information is not evidence about any particular guardian or peer.

The pinned Lightning v1 client performs unchecked arithmetic over the selected
gateway's advertised fee schedule. Before calling its affordability or payment
paths, FMan rejects a schedule whose proportional divisor is zero or whose
maximum invoice fee or contract amount would overflow at the current wallet
balance. Do not move a selected v1 gateway into dependency fee arithmetic before
this check.

Each LNURL-pay discovery or invoice response is streamed into at most 64 KiB
before JSON decoding. These responses need only carry the callback URL,
human-readable metadata, amount bounds, and one BOLT 11 invoice; endpoints which
need more than the deliberately generous compatibility cap are unsupported.
The streaming cap applies when `Content-Length` is absent and to decoded bytes if
the HTTP client gains transparent content decoding in a future dependency build.

Await treats the terminal emitted by the native subscription as authoritative
even when the dependency's best-effort terminal-outcome cache write fails. It
rereads active state machines independently and uses cached v1 state only to
distinguish a completed refund from the v1 API's aggregate failure result.

The accepted-federation set is policy owned outside this wallet. Joining a
federation or receiving its invite does not establish that it is trusted or
approved for a particular role.

## Secret handling

`WalletSecret`, derived note secrets, finalized bearer notes, private issuance
requests, and refund contexts are secret material. They must not implement
`Debug`, `Display`, or serialization; enter logs, metrics, errors, RPC values,
or Nostr events; or be persisted outside the Fedimint wallet database.

Raw OOB tokens must not leave the wallet boundary or enter logs. The pinned
mint-v2 client has one narrow retention exception: its receive operation metadata
stores the full encoded bearer ecash in the protected Fedimint wallet database.
Treat that database and its backups as bearer-secret material. FMan reads the
metadata only to recover an exact receive replay, including accepted-payment
claims and explicit token imports: the operation must have the mint-v2 module
kind, decode as receive metadata, and contain byte-for-byte identical encoded
ecash. Missing, malformed, wrong-kind, and non-identical entries remain errors.
Re-audit this recovery boundary whenever Fedimint changes receive operation-ID
derivation, metadata shape or persistence, ecash encoding, or `AlreadyReceived`
semantics.

Outside that dependency-owned metadata, raw OOB tokens exist only while being
constructed and received or returned directly to the authorized caller. Do not
log or retain them. Accepted-claim evidence may retain only the public inputs
needed to reconstruct them: the federation's public invite code (which also
carries the federation identity), module identity, issuance requests,
aggregate blinded signatures, and the mint-v1 quote nonce. It must not retain a
bearer token, derived note secret, refund context, or unrelated quote/commercial
fields. Core owns this closed enum; SQLite and backup transport serialize it.
Public quote-binding hashes, operation ids, transaction ids, module ids, and
outpoint ranges are non-secret, but logging them should still be deliberate and
low-volume.

The operator wallet-drain projection reads payout operation metadata only from
the protected client database and exposes public operation ids plus invoice,
contract, and available-note amounts. Its Lightning v1 affordability query
refreshes the dependency's local gateway cache, but it starts no payment,
submits no transaction, consumes no payout subscription, and writes no outcome
cache. It never exposes invoices, gateway URLs, preimages, contract keys, raw
dependency errors, or arbitrary operation metadata. Metadata or outcome
decoding failure becomes a closed query-error category and an unknown drain
state. Active state machines override any cached rail-terminal result for
destruction decisions.

The explicit `fi-cli --funding-token-file` import is a narrow restart-safety
exception: the CLI atomically renames the user-supplied file to a deterministic
in-progress journal before awaiting the idempotent wallet receive operation,
reuses that same journal after a crash, and deletes it only after confirmed
receipt. It must not copy the token into FI state or logs.

`Wallet::join` rejects a parsed invite containing `api_secret()` before taking
the per-federation lock or invoking connector, client, preview, join, or
database operations. (`Wallet::open`
already holds the root lock.)
The pinned Fedimint client debug-logs complete invites during preview, so this
ordering is a credential-confinement boundary covered by
`private_invite_is_rejected_before_fedimint_client_use`. Databases written
before the single partitioned database are no longer opened at all, but they
may contain `ApiSecretKey`; treat them and their backups as credential
material. Re-audit reload, log, formatting, and error paths whenever
the Fedimint dependency changes. Root `SECURITY.md` records historical
exposure and rotation guidance.

Protect the wallet root and its database as monetary secrets. A copied root
plus database controls every federation client in it. Backups and test artifacts
need the same access controls as live state.


## Join persistence

Every client shares one RocksDB under the wallet root, partitioned by a
monotonically allocated prefix per client scope: one payment scope per
federation and one guardian-fee scope per seat/federation pair. Prefix zero
holds the allocator and the prefix-to-scope map; a prefix is reserved in one
committed transaction *before* Fedimint is handed the database and is never
reused, so a join that fails partway cannot leave state a later scope inherits.
On open, a client whose contents name a different federation than its mapped
scope is refused rather than used.

The wallet-root lock file is opened without truncation or symlink following and
held until the `Wallet` drops. RocksDB does take its own exclusive lock on the
database directory, but a second opener *blocks* on it rather than failing, so
the root lock is what turns a concurrent open into a diagnosable error instead
of a hang. For the FMan this sits inside the daemon's own data-root `flock`;
for `fi-cli`, which takes no such lock, it is the only guard.

A failure or cancellation before fedimint-client returns a `ClientHandle`
leaves that scope's reserved prefix in place, possibly with partially
written client state under it. A retry reuses the same prefix and lets Fedimint
decide between opening and initializing it, so repeated failures consume no
additional space and there is no residue to bound, census, or remove by hand —
the disk-growth and manual-cleanup hazards of the predecessor's per-attempt
staging directories do not exist here.

The residual hazard is narrower and named: the pinned fedimint-client can spawn
database-owning tasks before returning the handle that normally drains them, so
a cancelled attempt's task can outlive the attempt. Its writes land in that
scope's own partition — never another scope's prefix, which is what the
per-scope prefix buys — but an in-process retry for the same scope
can therefore run concurrently with a stale writer over one partition. Restart
clears it. The wallet therefore fences every client-scope join and lazy payment
reopen through one process-owned registry of attempted scopes.
Cancelling or replacing a reconciler task does not reset it, and a lazy reopen
cannot bypass a join attempt (or vice versa). Newly admitted federations are
still attempted, but a transient failure leaves that federation unavailable
until restart. A failed guardian-fee open likewise leaves that seat's fee wallet
unavailable until restart; the scope-keyed fence does not mark another seat or
the payment wallet in the same federation as attempted. All scopes still share
one RocksDB and runtime and may contend for those common resources. Do not add
an automatic in-process retry loop or reset the wallet registry before process
teardown without closing the stale-writer window first.

Payment and guardian-fee joins apply a 30-second timeout only while previewing
the federation configuration, before the permanent open fence and any client
database initialization. Every fresh client-scope database is recovered,
because both mnemonic-derived roots may have been used with a database lost to
a restore. Before any scan, a stored config must name its mapped federation;
new and stored configs must contain the supported mint modules. The wallet then
requires a durable Fedimint recovery record for each configured mint and checks
that every such record is done. This is deliberately stronger than
`wait_for_all_recoveries()`: the pinned client treats an unregistered or
API-incompatible module as skipped, yet can report the recoveries it did spawn
as complete.

Recovery has no artificial timeout. It waits for those required mint scans,
then shuts down the recovery handle, reopens the same database, and waits for
the recovered output state machines before recording an FMan readiness marker.
The marker remains absent when a crash happens after Fedimint changes its own
state to `Complete(Recover)` but before FMan has checked the required mint
records or the output state machines. Both the normal join and the lazy
retained-payment open then finish or fail that readiness work; no recovery-only
handle is published. This makes an initial join slower even for a never-used
root, but avoids the pinned client's fund-loss behavior when a root is reused
with a fresh join.

This recovery applies to unspent mint ecash only. The pinned Lightning v1 and
v2 modules use no-op recovery, so a completed wallet recovery neither recovers
nor makes a guarantee about an in-flight Lightning payout, contract, or refund.
The pinned `shutdown()` waits at most 30 seconds internally and returns no
quiescence result; it can therefore return after logging that tasks remain.
Manifold fences every failed or cancelled in-process open and never publishes a
handle on a readiness failure, but its immediate post-recovery reopen has no
formal proof that a stale dependency writer exited. Restart remains the
recovery boundary for that residual upstream limitation.
Per-scope locks prevent duplicate joins for one exact scope while allowing
unrelated scopes to enter their attempts concurrently.

There is no join-quiescence API. The predecessor's existed to drain detached
pointer-publication continuations; a join is now one awaited call that detaches
nothing, and dropping the `Wallet` drops its clients. It never covered the
dependency-owned pre-handle tasks described above, which remain the only
join work that can outlive its caller.

## FMan payee ordering

Payment verification is offline and bound to the exact signed quote. Acceptance
must persist the seat and payment evidence atomically before returning the
recomputable signed commitment or starting background claim work. Refusals have
no durable row: the seat row is the sole admission outcome, an existing row
dominates a later epoch mismatch under SQLite's writer reservation, and every
epoch change commits before a refund-bearing refusal can depend on it. Preserve
that ordering so accepted and refunded outcomes for one quote remain mutually
exclusive.

Quote and refund denomination expansion is capped at 64 notes before allocating
the output vectors. Quote prices or fee-balanced refunds that require more than
64 outputs are now unrepresentable, including with a standard but truncated
power-of-two mint-v1 tier set. This keeps an unauthenticated quote request from
consuming work proportional to the price.

Claim and refund work is replayable by exact quote identity. Replays must not
create unbounded concurrent wallet work. Never move a refused quote's locked
notes through an ordinary spend path.

The accepted-claim worker must inspect the deterministic Fedimint operation id
before attempting handoff, bound attempt duration and concurrency, retry
nonterminal failures periodically, and join on shutdown. Detailed dependency
errors are not safe-to-share tracing values.

The current accepted-seat claim path may consolidate pre-existing ordinary
wallet notes and charge their fees to the combined operation. It is not a
principal-isolation or minimum-net-revenue guarantee; do not document or price
it as one until the governing economic work is implemented.

## Tests

Changes to payee verification or evidence require tests for exact quote and
issuance binding, mutually exclusive acceptance/refund, durable replay, and
claim/refund convergence. Secret-bearing types and error paths require
formatting and serialization audits.

Changes to the prefix allocator require tests for reservation durability
across reopen, refusal of the reserved zero prefix and duplicate mappings, and
refusal of a client whose federation disagrees with its prefix mapping.

The root repository [SECURITY.md](../../../SECURITY.md) contains the wider FMan
payment and deployment boundaries.
