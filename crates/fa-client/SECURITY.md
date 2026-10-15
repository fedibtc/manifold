# fa-client security and reliability boundaries

This crate orchestrates authenticated remote services and consumer-owned value.
Changes to identity, intent validation, quote handling, payment calls,
persistence, cancellation, or progress output are security-sensitive.

## Trust boundary

Locators enter the FA only through verified selection. They are
untrusted input except for the concrete facts
they encode: dialing information and a manager commitment key. A locator is not
an issuer attestation or a consumer-provided trust decision. Verify every
manager commitment against its locator key and validate all echoed request
terms before acting on it.

The consumer may supply capabilities but cannot supply authenticated protocol
objects, lifecycle transitions, or a claim that untrusted registry material was
validated. Reuse the private-constructor verification proofs and protocol types
from sibling crates.

Paid federation selection starts only from a complete kind-37707 event signed
by the deployment-pinned Fedi publisher. Verify the event ID, signature,
publisher, kind, exact `d` tag, bounded strict content, public invites, and
replacement order before persisting or using it. Retain the complete highest
event atomically, statically revalidate it after restart, and reject rollback.
An empty admitted set stops new paid formation and funding that the wallet
proves never started. It does not strand an already-prepared quote-bound
payment, which recovery replays before consulting current policy. Missing or
invalid candidates fall back only to a previously authenticated last-known-good
event.

Wallet holdings are capability, not policy. The admitted-payer query may expose
the complete authenticated set, including zero-balance members. Paid
`buy_seats` must require the explicit payer to remain both admitted and
wallet-Ready before persisting selected purchase state or requesting a quote.
The selected deployment-bootstrap entry may persist without a payer only when
it is expressly invoked without one; it requests only zero-price quotes and a
priced live offer returns typed payer-required reauthorization before any quote,
wallet, presentation, or output effect. After every exact
verified quote exists, the wallet must separately prove that payer can cover
the locked issuance sets plus transaction fees and required reserve without
generating outputs or creating recovery state. The MG's availability response
never establishes federation-specific payment policy for FA selection.

Validate the complete formation intent before durable or remote side effects.
`create_federation` resolves an absent display name exactly once, validates
it, and persists it before any purchase or DKG.
Unsupported future fields must be rejected or absent from the public intent,
never silently ignored.

## Registry discovery and selection

Fetched kind-37701 advertisement events are untrusted relay data. Static
admission happens in `src/discovery.rs`, in the recorded order: event role
and signature, document proof and the payload-author identity rule,
per-author NIP-01 replacement, freshness, and eligibility — cheap local
checks only. Relay-backed PeerBadge verification deliberately does not run
over that pool: it runs lazily in `src/selection.rs`, in selection order,
inside the ranked round-robin walk, so attacker-authored advertisements
cannot multiply verifier round trips beyond the walk they pollute. An
`EligibleMgCandidate` therefore carries publisher *claims* — including
the claimed bucketing issuer — and must never be treated as a trust
conclusion; the walk binds each verified badge subject to the
authenticated event author and requires the verified issuer to equal the
untrusted bucketing claim before seating a candidate. The sealed
`EligibleMgCandidate`/`VerifiedCandidate`/`VerifiedBadgeFacts` types mean
a consumer can neither construct a trust conclusion nor smuggle an
unadmitted advertisement into selection.
`VerifiedCandidate` also proves that the authentic badge met the selected
environment's minimum trust level (currently `9`, admitting Trusted-or-higher),
not merely that its issuer, holder, subject, and schema verified.

Eligibility ends with a dial-eligibility step: a candidate must carry a
parseable x-only commitment-signing `service_pubkey` and at least one
`iroh://<endpoint-id>` endpoint, from which discovery builds the candidate's
dialing `Locator`. Each failure is its own typed rejection —
`MalformedServicePubkey` and `NoDialableEndpoint` — so undialable
advertisements surface in the rejection summary instead of yielding a
candidate that cannot be dialed with verifiable responses.

The advertisement-derived locator's service key is self-attested: it sits
inside the self-signed payload, whose signer the pipeline binds to the
authenticated event author and to the badge-vouched subject, so a consumer
that dials with it trusts the key exactly as far as it trusts the
badge-vouched author that asserted it — no independent vouching exists or
is claimed (mirrored in `crates/nostr/SECURITY.md`).

Discovery and the selection preview are read-only: they perform no durable
writes, take no run guard, reserve no seats, and publish no status, so
they sit outside the durable-ordering and run-guard invariants below. Consumers
can construct registry-only `MgRegistryQuery` for static discovery without
any trust capability, then add a concrete verifier with `with_verifier` to
obtain `MgSelectionQuery` for verified preview. Neither query surface has an
FA identity, database, payment port, or consensus reader; `MgSelectionQuery`
may optionally hold a read-only MG connector added with
`with_mg_connector`, used solely for the selection walk's value-free live
availability probe. The
relays they query are deployment-pinned trusted infrastructure — the same
trust tier as the rest of the environment profile
— but they cannot gate writes, so the untrusted input is publisher-authored
content on open-write relays, and the enumeration bounds are resource
hygiene against publisher volume, not defenses against the relay itself.

A probing selection walk (`FaClient` previews always; a `MgSelectionQuery`
holding a connector) dials only reached, badge-verified, non-duplicate
candidates' self-attested locators, so advertisement spam cannot multiply
dials beyond the verifier round trips it already costs. The live
`GetAvailabilityResponse` is untrusted MG-authored input consumed only by
the shared scalar-version, FA-range, selected major/minor/vendor, size,
accepting-seats, and exact-plan predicate into typed rejections. A live
offer without the advertised plan and price rejects the candidate. Matching
it confers nothing, because the signed quote remains the only commercial term. Probe failure text carries only the sanitized-by-contract
local connector error descriptions or a fixed marker for a Fleet
Manager-returned error; remote error text is never embedded. Each probe runs
under the per-candidate `MG_SELECTION_PROBE_TIMEOUT` (10 s) capped by the
absolute preview deadline, and expiry drops the in-flight dial.
Cancelling a preview drops local discovery and selection values, cancels
their relay reads, and drops any in-flight availability-probe dial; the
probe spawns no detached tasks of its own. Registry enumeration can leave a best-effort unsubscribe on
the caller-owned relay pool. Its unique subscription id prevents it from
unsubscribing a later query, though the cleanup can briefly contend with later
shared-pool use. Badge verification can leave the same cleanup on its private
ephemeral client. These detached cleanup tasks are not joined by the preview
deadline, but neither owns durable state, a reservation, or selection
authority.
The bounds are 2048 observed candidates, 256 KiB per normalized event, 16
MiB aggregate, and 4 examined trust envelopes per walked advertisement
(derivations and the walk in
`specs/ARCH-fa-client-discovery-selection.md`). Reaching the
candidate ceiling or the aggregate byte bound completes the enumeration
with the retained prefix — the caps degrade the listing, never error it,
because with open-write relays a spammer can always exceed any ceiling. One
absolute deadline covers enumeration, admission, and the walk. The preview
must complete strictly before it; expiry wins simultaneous readiness, drops
the preview, cancels an in-flight badge verification and its relay reads, and
returns `SelectionPreviewTimeout`. This is the runtime's cooperative async bound:
synchronous parsing or cryptography between yield points and executor
starvation are not preempted. The enumeration itself is fail-closed
against a stalled or truncated answer: when the consumer supplies the
pooled profile-relay client (as `fa-cli` does), it runs over every
canonical profile relay, and is accepted only when at least one relay
delivers its EOSE-complete answer or a local resource bound is reached —
slower relays merge best-effort, an unreachable relay never blocks, and
zero complete answers is a typed `FaError::Registry` error rather than a
silently small listing. A walk that cannot verified-fill the requested seat count is the
typed `InsufficientMgSeats` partial failure, never a silently short seat
set.

Accepted risks, revisited with the design records
(`specs/ARCH-fa-client-discovery-selection.md`): a
publisher spam campaign can fill
the bounded window with valid spam identities while candidates order by
author pubkey, so the window is a resource backstop rather than a fairness
guarantee; the transport counts one signed event once however many relays
serve it, so caps are not eroded by relay count, though distinct spam
events still fill the window;
freshness tolerates zero clock skew, so a consumer with a fast clock
rejects fresh advertisements; and the claimed-issuer bucketing key is
publisher-controlled, so a publisher can choose its bucket — the issuer
equality check makes a false claim cost the candidate its seat, but
region spread remains a heuristic, not a guarantee. The advertised price used
within and across compatible DKG cohorts is likewise a publisher claim;
the exact signed quote at formation time is the only commercial term.
Selection also treats the locator's self-attested commitment-signing
`service_pubkey` as an operator/failure-domain identity: after both authors
verify, a key already held by a selected seat rejects the later author with a
typed diagnostic and the same bucket continues. Endpoint differences cannot
turn one signing authority into independent guardians, and an unverified key
claim cannot exclude a candidate because deduplication follows badge binding.
Verified badge holders also occupy at most one seat, even across issuers and
service keys. FA saves each selected holder for recovery.
Only successfully seated candidates reserve a holder. Older saved seats lacking
holder identities retain their existing behavior without this guarantee.
Because the claimed price and claimed issuer are both publisher-
controlled and rank the deterministic walk, a spam campaign can
deliberately claim the cheapest prices in every bucket and thereby sit
at the front of the walk, consuming its bounded verifier round trips
before any honest candidate is reached; the safeguard is that the walk
is deadline-bounded and a starved walk surfaces as the typed
`SelectionPreviewTimeout` rather than a silently degraded seat set. Pool
exhaustion before the deadline remains the typed `InsufficientMgSeats`
shortfall. Two probe-specific accepted risks: probes run sequentially, so
several unresponsive badge-verified candidates (up to 10 s each against the
60 s default preview deadline) can consume the deadline and surface as the
typed `SelectionPreviewTimeout` or `InsufficientMgSeats` even when live
backups remain in the pool — recovery is a retry or a larger `with_timeout`;
revisit if probe concurrency or the per-candidate budget changes. And
probing reveals the FA's transport address and formation interest to every
probed candidate before any approval — the same exposure quote time already
creates for selected candidates, widened to reached candidates; consumers
for whom pre-approval linkability matters should supply an ephemeral probe
transport, as `fa-cli` does. Every product-path seat must pass the complete verification; there
is no pinned/BYO exception. The five-minute preview is the only input
to selected creation and is the consumer's payment approval. Its usable window
begins after the verified walk and it binds the complete request, each seat's
advertised plan and price, and canonical verifier/environment provenance. The
stable provenance and the deadline are persisted on the purchase. Reopening
under a different profile requires fresh approval before the first seat binds
its quote. A stored seat quote is the seat's authority for that quote's
effect: paid output generation or free `CreateSeat` presentation. One
autocommit binds the quote before the seat's payment or first presentation. Only a paid binding whose wallet operation is proven
`NotStarted` may move to a new paid quote. Exact recovery therefore retains
historical authority after a crash. The preview itself is intentionally not serializable or cached.
Discovery is now multi-relay (this section's revisit): with the pooled
profile-relay client a consumer such as `fa-cli` supplies, it reads every
canonical profile relay, availability only — the
trust model above is unchanged, the verification order is unchanged, and
the per-run caps count deduplicated events so adding a relay neither
multiplies badge-verification work nor shrinks the effective window.
Re-check this section when the verification order or any cap changes, or
when the preview-as-approval contract changes.

## Durable ordering

The following ordering is part of the monetary safety boundary:

1. verify every selected seat and seal its advertisement-only preview for five
   minutes from completion of the verified walk; request no quote before
   `buy_seats`;
2. for paid creation, authenticate the explicit setup payer and require wallet
   Ready state before persisting the resolved intent, formation id, FA identity,
   and exact selected locators. The explicit no-payer bootstrap persists that
   absence and fails with typed payer-required reauthorization on any priced
   live offer before requesting its quote;
3. authenticate and persist the highest setup-payment policy. Before the
   first seat is bound, confirm the live availability of every seat at its
   previewed plan and ask the wallet one value-free funds check for the
   remaining priced seats. A failed seat leaves, and short funds end every
   seat, before any quote is bound. Then buy the seats one at a time in seat
   order;
4. for a seat without a bound quote, request and verify an exact quote for
   exactly the seat's previewed plan, without automatically replacing a
   selected guardian, and bind it in one autocommit. A live offer without that
   plan returns typed `PriceChanged` reauthorization and the seat leaves. Until
   the purchase binds its first seat, the bind requires a fresh preview under
   the same verifier provenance;
5. for a bound paid quote, ask the wallet to recover that exact quote before
   any new payment. `Prepared` replays its presentation without a new spend.
   `Rejected` makes the seat leave. `NotStarted` permits a new quote at the
   same previewed plan, bound in place of the old one;
6. poll `FaPayments::create_seat_payment` for the bound quote and establish
   exact recoverability before value commits. A successful return means the
   exact transaction is accepted and its payer-owned change is final and
   spendable;
7. present the payment with its exact quote and evidence in `CreateSeat`, and
   atomically persist the MG-signed seat id and complete guardian-fee account
   before the next seat's quote or payment starts. A first run and a
   restarted run use this same ordered path; and
8. persist later DKG facts with narrow row updates.

The wallet may report insufficient funds only through the typed
`InsufficientFunds` error kind, which asserts that its balance check failed and
that no operation for the quote exists. The seat and every seat without a bound
quote then leave the purchase. A binding mismatch, wallet-storage failure, FA
checkpoint failure, or lost or ambiguous response has no such proof: it remains
a payment error, keeps the seat, and the next run recovers the exact quote
first. Never infer operation absence from an error message.

A seat leaves the purchase only when no payment for it is outstanding: before
its quote is paid, after a terminal wallet rejection, or after its MG-signed
refund settles. One transaction deletes its row. Accepted, prepared, paid, and
ambiguous seats remain. When every remaining seat is accepted, the purchase
completes into the pool, and a new purchase with a fresh preview buys any seat
still missing. Persisted combinations that bind an admission to another
quote or effect, or that record a seat quote without its bound admission, fail
closed.

`cancel_creation` may remove the purchase only while no seat is accepted, the
pool is empty, and the wallet recovers no bound paid quote as `Prepared`. A
recovery error retains the purchase.

Do not request any plan other than the seat's previewed plan. Do not replace a
bound quote unless the wallet proves its payment `NotStarted`. Do not replace a
quote after an ambiguous request merely because it expired. Do not substitute
a selected MG. Do not start the next seat's payment before the prior seat
reaches its durable checkpoint or leaves.

Transport retry authority comes only from the consumer connector's local,
non-serializable outer call error. A serialized `ManifoldGuardianError` is an
MG-authored domain result and must never be interpreted as proof that an RPC
stream failed locally, even if its text claims otherwise.

## Secrets and sensitive outputs

FA storage may contain the complete authenticated setup-payment event, resolved
intent, public locators, exact signed quote terms, authorization binding, seat
ids, public guardian-fee accounts, DKG codes, invite code, and an optional
push-gateway callback URL. The callback URL is a bearer capability and its
stable idempotency key can identify one formation operation. Neither is
projected in public status or `Debug`, but database files and backups containing
them remain sensitive. Schema 11 preserves the callback through every
pre-`DkgComplete` recovery and atomically clears it with that checkpoint,
after configured MGs have durably assumed retry ownership; an MG without
callback delivery configured may have proceeded callback-free. Older schemas
from before the production compatibility baseline fail closed and require reset.
Logical clearing does not erase old pages or backups. FA storage must never
contain raw bearer ecash, payment signatures, identity secret material, or
wallet-private refund secrets.

The encrypted Nostr backup is not a database copy. Its purpose-built payload
contains only the formed invite, stable seat identities/locators, and an
optional exact live-liquidity commitment. It excludes quotes, guardian codes,
fee accounts, payment and trust state, signed responses, history, secrets, and
value. One CBOR item is fixed-frame padded and authenticated under FA-specific
key/AAD domains; restore ignores invalid candidates and chooses the highest
authenticated generation. Publication workers never hold the operation guard
or block ordinary FA work. Avoid including snapshots, locators, invite
codes, or detailed remote errors in routine logs and telemetry.
The same rule covers push hook URLs and their idempotency keys.

Identity and payment adapter errors crossing the public boundary must be
sanitized. Never add `Debug`, serialization, metrics, or error formatting that
can expose secret keys, refund context, bearer notes, or raw payment evidence.

Guardian-fee arrangement accepts no recipient account in its operation input.
Only after saving `DkgComplete` does `fa-client` parse the exact
Fedimint federation id from the persisted invite and pass it to the
consumer-provided `FaFeeAccountProvider`. Production implementations must
resolve the already joined client for that id and return its own SPv2
`BtcDepositor` account without network or value-moving work; user, RPC, and UI
input are not valid account sources. Provider failure becomes typed unavailable
fee-arrangement capability before any guardian vote. The returned public
descriptor is still untrusted data: `fa-client` validates its account shape,
destination-account uniqueness across the FA account, the Guardian Verification
Fee account, and every guardian account, the canonical complete recipient
vector, and exact threshold readback. The library
cannot prove that a consumer implemented local derivation honestly, so consumers
remain within the trusted integration boundary and must sanitize provider errors.

Post-formation metadata maintenance caps the complete raw consensus object at
1,048,576 bytes immediately after every read, before hashing, parsing, cloning,
connection, signing, or fan-out. The same inclusive cap is enforced by each
MG on both the current object and canonical target. Per-field limits do not
substitute for this whole-object bound.

Iroh handlers may reorder before entering a seat queue. A live MG seat loop
therefore keeps one pin — the single whole-object target admitted for the
live consensus occurrence — while allowing identical replay. Bases are bound
to the meta module's monotone revision, so an exact `O -> B -> O` content
recurrence is a fresh occurrence that stales old handlers outright, and a
superseded occurrence's pin fences nothing and is simply replaced; no
history, cap, or eviction policy exists. Restart clears the pin and destroys
every delayed and in-flight handler it fences. The target is pinned before
the fallible child submit call, so a
lost or error response cannot reopen the occurrence to a conflicting target;
exact replay is still allowed.
Combined with FA run-guard serialization, fresh readback, and exact-base rebasing,
this prevents a late old handler from restoring an object after a newer target
was submitted. Pinning a live occurrence to its first target until consensus
moves or the process restarts is the explicit availability cost of avoiding
new durable sequence state in MVP.

## Cancellation and replay

Rechecking an already formed federation uses fresh threshold consensus, not
Manifold Guardian liveness. Verify the saved invite's federation identity and the
complete signed directory; local formations also require their saved directory
and fee recipients to remain exact. Restored backups must match every saved seat
and any liquidity commitment before exposing fresh authority. Initial formation
still requires every manager's health and agreed invite.
If the saved invite cannot reach consensus, recovery may ask the saved managers
for alternative invites. They are dialing hints only: reject a different
federation id and apply the same consensus and directory checks. Retain a
multi-guardian read invite in memory after verification, never a cached
consensus result. The original invite, backup, and signed liquidity commitment
remain unchanged; liquidity and gateway verification use the read invite.

Dropping a driving future cannot undo a wallet transaction or remote request
already accepted. Every await between a durable write and its response is a
possible interruption point. Reopen and `run` must be sufficient after any
such interruption.

Liquidity recovery revalidates the formed federation and exact request against
fresh federation consensus, then consumes any durable signed completed-gateway
evidence before rediscovering or connecting to FLIP. Threshold guardian replay
and LNv2 readback are idempotent. Their durable proof stores the exact canonical
URL and is valid only while current completion evidence carries that same URL;
the marking transaction rechecks the match, and a later differing provider
status clears the proof. A completed gateway-only request therefore remains
attachable after the provider disappears without allowing one URL's readback to
authorize another.

The wallet's recover-only result is authoritative: `NotStarted` is permitted
only when it can prove no funding began, while `Rejected` requires both a
terminal consensus rejection and proof that accepted automatic refund
transactions exactly restore the original inputs with every restored payer
output spendable. Recovery is keyed by the exact quote and never creates wallet
state. Prepared replay creates no new value movement. A recovered operation
must reproduce the exact presentation and matching refund context. Settlement
of the same MG-signed refund must be safe to repeat after a lost result.

Free `CreateSeat` is also ambiguous on interruption. Replay the exact stored
free quote until a verified signed refusal proves that the seat ended. The
consumed quote-bound admission cannot authorize a re-quote or a second
presentation, so the refused seat leaves the purchase.

## Concurrency and hosting

The consumer opens at most one client for a database, and the client's
in-memory run guard serializes the run loop and every user action. A
RocksDB-backed database refuses a second open of the same path, so two
processes cannot share FA state. Nothing guards against a second client on
the same database handle; opening one breaks the payment-safety argument
below. A process restart ends the old driver before persisted state is
reopened. Restart safety comes from exact durable payment recovery. Cancellation retains the same
durable recovery facts, and parallel non-value seat work must not rewrite
sibling facts from stale snapshots.

Ordinary formation-state read/modify/write transactions use bounded optimistic
retries. Every retry reloads the creation and its slots, rechecks the
purchase or formation identity and exact expected quote or recovery facts,
and revalidates every slot before writing. Accepting a seat writes only its
own slot, so concurrent acceptances cannot overwrite each other. These
transaction closures contain no wallet, signing, or network effects. Exhausted
or non-conflict commit failures return a sanitized storage error rather than
unwinding. Initialization remains single-attempt because the run guard
serializes it before parallel seat work. Setup-payment policy retention is a
separate bounded read/modify/write transaction: every retry compares the
candidate against the durable NIP-01 high-water so a residual replaced driver
cannot roll policy back.

Commit failures return a sanitized storage error and never panic. Tests pin
deferred effect construction and polling, and the registry-to-wallet
composite boundary.

The durable root is bound to the FA public identity before any status is
published. Storage schema changes fail closed. Schemas 4 through 8 predate one
or more of durable identity binding, the post-DKG seat-binding directory,
commercial authorization history, the wallet-output boundary, and
the explicit selected-vs-pinned recovery discriminator/deadline, and signed
guardian-fee acceptance facts.
They belong only to the unreleased development implementation; consumers must
reset those namespaces rather than infer an irreversible money boundary that
older records could not represent.

The consensus-read capability carries an obligation the library cannot enforce.
After DKG the driver writes the MG seat-binding directory on every seat and
reads consensus back until it matches. It first binds the exact raw consensus
object *and its monotone consensus revision* into one `expected_base` shared
by the whole submission wave, so the base names one occurrence of the board
state and a byte-identical recurrence can never re-match earlier requests,
handlers, or guardian admission pins. An
all-stale wave triggers a fresh read and a byte-identical retry rebased on the
new object; a late stale response after threshold adoption is harmless only
when fresh readback proves the exact target won. The same serialization and
readback invariant applies to typed post-formation metadata and guardian-fee
operations because both mutate one opaque meta value and the upstream module
offers no atomic CAS. MG's per-key raw caps and the shared
1,048,576-byte complete-object cap bound each retry wave before child work.
Metadata maintenance and guardian-fee arrangement share the formation driver's
process guard, accept no arbitrary key, and expose only shared semantic
values whose fallible constructors enforce Guardianito validation plus the
65,536-byte absolute raw cap before signing, connection, or network
effects. It reads consensus before connecting, binds every non-short-circuiting
best-effort submission wave to the exact raw base, treats stale-base and
per-seat availability failures as reread/retry signals, and returns only after
a real consensus read carries the requested value. For that exact base and one
live invocation it retains acknowledged rows and retries only unresolved seats
with capped exponential backoff; a changed base resets the set so every new
request is signed against the current whole object. A guardian answering the
distinct `MetaTargetConflict` has pinned the base to a different admitted
target: the driver stops same-base submissions to that seat and carries the
diagnosis into its convergence result — retrying there cannot help until the
conflicting write converges or the guardian restarts — while a fresh base
clears the exclusion. Cancellation or reopen
safely replays the exact mutation and may resubmit rows; acknowledgements are
not durable. An already-adopted value needs no live
MG; a new value needs threshold liveness within the caller's deadline, not
unanimity. Terminal policy/lifecycle refusals and bounded convergence failures
remain distinct typed maintenance outcomes.
Guardian-fee arrangement accepts no consumer-supplied guardian or Guardian
Verification Fee accounts: guardian accounts come from signed acceptances, the
Guardian Verification Fee account comes from the deployment profile, and only
the FA role account and bounded rate cross the consumer boundary. Before
directory publication, every post-DKG MG attestation must repeat the account
from its persisted signed acceptance. Recovery exactly replays the persisted
ordered attestation/proof
entries; the separately persisted canonical-directory prediction is used only
for consensus comparison. Fee voting later requires every account in that fully verified consensus directory, so a
minority substitution cannot be hidden behind threshold votes. Cancellation
needs no separate recovery record: replaying the same typed mutation is idempotent and
always rebases from current consensus, so it cannot overwrite an unrelated
change merely because the previous caller disappeared. The fee producer accepts
only the typed proposal described above.
The driver performs the peer-set derivation, the
attestation signature checks, and the equality check itself, so no trust
conclusion crosses the boundary — but the query is the consumer's. Fedimint's
`get_consensus` derives its guarantee from the caller performing threshold
agreement across peers and returns no signatures, so nothing in a returned
snapshot distinguishes a genuine read from bytes echoed back from the write
that preceded it. An adapter that fabricates or caches a snapshot makes the FA
believe the directory reached consensus when it did not. Implement the port
against a real invite-code preview.

The driver uses Fedimint's native/WASM runtime abstraction for deadlines and
retry wakeups. The WASM adapter represents timer milliseconds as `i32`, so every
public formation timing is limited to the inclusive range from one through
`i32::MAX` integral milliseconds on all targets. Private fields and fallible construction
prevent fractional-millisecond values and deadline overflow, returning
`InvalidFormationRunOptions` before formation-state, wallet, signing, or network
effects. Runtime request and retry timers pass the limiting configured duration
directly, or otherwise pass a checked one-millisecond-or-greater driver-invocation
remainder. A smaller remainder returns `Timeout` before another effect rather
than reaching the WASM adapter as zero. Tests pin the
effective one-millisecond quantum, maximum accepted, and first cross-platform-unsafe values,
and exercise both option-consuming public
operations. Re-check this bound and its regressions whenever the runtime timer
implementation or supported WASM target changes.

Payment creation, recovery, and refund settlement do not start after the
invocation deadline. A started wallet call is bounded by the run timeout, not
by the deadline, because it includes multiple bounded stages. Each seat of a
purchase has its own run timeout, so earlier seats cannot use up the time of
a later one. A timeout cancels the call and leaves exact payment recovery
intact.

The crate does not spawn unmanaged tasks. Consumers own execution and
cancellation. Keep dependencies compatible with native mobile targets and
`wasm32-unknown-unknown`; platform-specific adapters belong in the consumer.

Report security-sensitive issues through the repository process in the root
[SECURITY.md](../../SECURITY.md).
