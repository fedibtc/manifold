# fi-client security and reliability boundaries

This crate orchestrates authenticated remote services and consumer-owned value.
Changes to identity, intent validation, quote handling, payment calls,
persistence, cancellation, or progress output are security-sensitive.

## Trust boundary

Pinned Fleet Manager locators are untrusted input except for the concrete facts
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
Pay-and-create must require the explicit payer to remain both admitted and
wallet-Ready before persisting selected formation state or requesting a quote.
The selected deployment-bootstrap entry may persist without a payer only when
it is expressly invoked without one; it requests only zero-price quotes and a
priced live offer returns typed payer-required reauthorization before any quote,
wallet, presentation, or output effect. After every exact
verified quote exists, the wallet must separately prove that payer can cover
the locked issuance sets plus transaction fees and required reserve without
generating outputs or creating recovery state. The FMan's availability response
never establishes federation-specific payment policy for FI selection.

Validate the complete formation intent before durable or remote side effects.
A pinned broad range may then use value-free availability reads to resolve one
shared release. Resolve an absent display name exactly once, validate it, and
persist it with that release before quotes, payments, seat creation, or DKG.
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
`EligibleFmanCandidate` therefore carries publisher *claims* — including
the claimed bucketing issuer — and must never be treated as a trust
conclusion; the walk binds each verified badge subject to the
authenticated event author and requires the verified issuer to equal the
untrusted bucketing claim before seating a candidate. The sealed
`EligibleFmanCandidate`/`VerifiedCandidate`/`VerifiedBadgeFacts` types mean
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
writes, take no driver lease, reserve no seats, and publish no status, so
they sit outside the durable-ordering and lease invariants below. Consumers
can construct registry-only `FmanRegistryQuery` for static discovery without
any trust capability, then add a concrete verifier with `with_verifier` to
obtain `FmanSelectionQuery` for verified preview. Neither query surface has an
FI identity, database, payment port, or consensus reader; `FmanSelectionQuery`
may optionally hold a read-only FMan connector added with
`with_fman_connector`, used solely for the selection walk's value-free live
availability probe. The
relays they query are deployment-pinned trusted infrastructure — the same
trust tier as the rest of the environment profile
— but they cannot gate writes, so the untrusted input is publisher-authored
content on open-write relays, and the enumeration bounds are resource
hygiene against publisher volume, not defenses against the relay itself.

A probing selection walk (`FiClient` previews always; a `FmanSelectionQuery`
holding a connector) dials only reached, badge-verified, non-duplicate
candidates' self-attested locators, so advertisement spam cannot multiply
dials beyond the verifier round trips it already costs. The live
`GetAvailabilityResponse` is untrusted FMan-authored input consumed only by
the shared exactly-one-version, range, selected-release, size, accepting-seats, and plan predicate
into typed rejections; its
offered plans and prices confer nothing — the signed quote remains the only
commercial term. Probe failure text carries only the sanitized-by-contract
local connector error descriptions or a fixed marker for a Fleet
Manager-returned error; remote error text is never embedded. Each probe runs
under the per-candidate `FMAN_SELECTION_PROBE_TIMEOUT` (10 s) capped by the
absolute preview deadline, and expiry drops the in-flight dial.
Cancelling a preview drops local discovery and selection values, cancels
their relay reads, and drops any in-flight availability-probe dial; the
probe spawns no detached tasks of its own. Registry enumeration can leave a best-effort unsubscribe on
the caller-owned relay pool. Its unique subscription id prevents it from
unsubscribing a later query, though the cleanup can briefly contend with later
shared-pool use. Badge verification can leave the same cleanup on its private
ephemeral client. These detached cleanup tasks are not joined by the preview
deadline, but neither owns durable state, a lease, a reservation, or selection
authority.
The bounds are 2048 observed candidates, 256 KiB per normalized event, 16
MiB aggregate, and 4 examined trust envelopes per walked advertisement
(derivations and the walk in
`specs/ARCH-fi-client-discovery-selection.md`). Reaching the
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
pooled profile-relay client (as `fi-cli` does), it runs over every
canonical profile relay, and is accepted only when at least one relay
delivers its EOSE-complete answer or a local resource bound is reached —
slower relays merge best-effort, an unreachable relay never blocks, and
zero complete answers is a typed `FiError::Registry` error rather than a
silently small listing. A walk that cannot verified-fill the requested seat count is the
typed `InsufficientFmanSeats` partial failure, never a silently short seat
set.

Accepted risks, revisited with the design records
(`specs/ARCH-fi-client-discovery-selection.md`): a
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
within and across compatible release cohorts is likewise a publisher claim;
the exact signed quote at formation time is the only commercial term.
Selection also treats the locator's self-attested commitment-signing
`service_pubkey` as an operator/failure-domain identity: after both authors
verify, a key already held by a selected seat rejects the later author with a
typed diagnostic and the same bucket continues. Endpoint differences cannot
turn one signing authority into independent guardians, and an unverified key
claim cannot exclude a candidate because deduplication follows badge binding.
Replacement preview seeds this verified walk with every retained sibling's
service key, and replacement apply revalidates the complete final key set in
the same database transaction that changes the rows. A distinct advertised
author therefore cannot reintroduce a retained signing authority.
Because the claimed price and claimed issuer are both publisher-
controlled and rank the deterministic walk, a spam campaign can
deliberately claim the cheapest prices in every bucket and thereby sit
at the front of the walk, consuming its bounded verifier round trips
before any honest candidate is reached; the safeguard is that the walk
is deadline-bounded and a starved walk surfaces as the typed
`SelectionPreviewTimeout` rather than a silently degraded seat set. Pool
exhaustion before the deadline remains the typed `InsufficientFmanSeats`
shortfall. Two probe-specific accepted risks: probes run sequentially, so
several unresponsive badge-verified candidates (up to 10 s each against the
60 s default preview deadline) can consume the deadline and surface as the
typed `SelectionPreviewTimeout` or `InsufficientFmanSeats` even when live
backups remain in the pool — recovery is a retry or a larger `with_timeout`;
revisit if probe concurrency or the per-candidate budget changes. And
probing reveals the FI's transport address and formation interest to every
probed candidate before any approval — the same exposure quote time already
creates for selected candidates, widened to reached candidates; consumers
for whom pre-approval linkability matters should supply an ephemeral probe
transport, as `fi-cli` does. Every product-path seat must pass the complete verification; there
is no pinned/BYO exception. The sealed two-minute preview approval is the only input
to selected creation. Its usable window begins after the verified walk and it
binds the complete request plus canonical verifier/environment provenance. The
stable provenance is persisted separately on every selected or replacement
seat. Reopening under a different profile requires fresh approval for every
unconsumed seat before an irreversible effect. Each admission has a private
monotone state: `Fresh`, or `EffectAuthorized` bound to the exact quote and to
either paid output generation or free `CreateSeat` presentation. Initial paid
rows transition in the aggregate output-start transaction; every post-output
paid replacement sharing a reservation transitions in one atomic authorization
update before the first new payment begins; the newly started paid members are
then driven one at a time. An independent free row transitions before its first
presentation. Exact recovery therefore retains historical authority after a
crash without treating
the formation-wide output tombstone as replacement authorization. Replacement
approval always creates new per-seat provenance; it never inherits authority
from the displaced guardian. Until the exact effect-authorization transition,
the displaced guardian's terminal quote and locator remain as a durable
provisional-replacement proof. Expiry, verifier drift, or definite
unavailability before that boundary restores the proof for a fresh preview;
if an exact replacement reservation exists, its successful wallet release is
authoritative and the durable restore clears only the matching reservation id.
That release is preceded by a durable release commitment, so the exact id's
authoritative absence under the persisted commitment completes an interrupted
restore on a later run; absence without the commitment retains the formation.
Resume recover-probes an authorized replacement aggregate before freshness or
verifier policy even when the wallet reserve result was lost before FI could
persist that id. Exact presence is checkpointed and released once before
restore; authoritative absence permits restore without a wallet call, while a
binding mismatch, storage error, or ambiguous probe retains the full state.
A failed, timed-out, cancelled, or dropped release retains the entire exact
state. The preview approval itself is intentionally not serializable or cached.
Discovery is now multi-relay (this section's revisit): with the pooled
profile-relay client a consumer such as `fi-cli` supplies, it reads every
canonical profile relay, availability only — the
trust model above is unchanged, the verification order is unchanged, and
the per-run caps count deduplicated events so adding a relay neither
multiplies badge-verification work nor shrinks the effective window.
Re-check this section when the verification order or any cap changes, or
when the sealed selected-creation contract changes.

The purpose-specific `insecure_discover_untrusted_pinned_fmans` surface is not a
product-path exception. It returns only authenticated, fresh, compatible and
dialable identities/locators for the existing pinned protocol-test driver; it
does not return `EligibleFmanCandidate`, a verified candidate, badge facts, or
a selection approval. Production consumers must not expose it.

## Durable ordering

The following ordering is part of the monetary safety boundary:

1. verify every selected seat and seal its advertisement-only preview for two
   minutes from completion of the verified walk; request no quote before
   Pay-and-create;
2. for paid creation, authenticate the explicit setup payer and require wallet
   Ready state before persisting the resolved intent, formation id, FI identity,
   and exact selected locators. The explicit no-payer bootstrap persists that
   absence and fails with typed payer-required reauthorization on any priced
   live offer before requesting its quote;
3. authenticate and persist the highest setup-payment policy, then request,
   verify, and persist each exact formation quote without automatically
   replacing a selected guardian;
4. check the complete exact total against the original cap and persist one
   aggregate commercial authorization bound to that complete quote set. Cap
   self-authorization is one-shot; every later replacement set needs explicit
   renewed authorization;
5. ask the wallet to recover every exact quote operation, refresh every proven
   `NotStarted` quote as one barrier, preserve authorization only for unchanged
   commercial terms, resolve every final FMan connection, and prepare each
   wallet-output timeout budget without polling the wallet;
6. derive a deterministic id from the formation and fresh exact aggregate;
   first recover-probe that exact id without creating wallet state, then, only
   after authoritative absence and live selected-admission validation,
   idempotently reserve the exact signed output plans, virtual value,
   independently dry-run fee-aware logical debit allocations, and reserve in
   the wallet without requiring disjoint physical notes, and persist that id;
7. atomically set the monotone `payment_outputs_started` tombstone and bind
   every initial paid admission to its exact paid-output effect;
8. at the next await, poll the first `FiPayments::create_seat_payment` with the
   opaque aggregate capability and establish exact recoverability before value
   commits; a successful adapter return means the exact transaction is accepted
   and its payer-owned change is final and spendable;
9. present and durably checkpoint that completed payment, using its exact quote
   and evidence in `CreateSeat`, and atomically persist the FMan-signed seat id
   and complete guardian-fee account before polling the next member's wallet
   future; initial formation, resume of `NotStarted` rows, and paid replacement
   all use this same ordered path; and
10. persist later DKG facts with narrow row updates.

The reserve adapter may request selected-flow payer retry only through the
typed `insufficient_funds_without_reservation` result, which asserts that its
balance check failed before the wallet created or observed a same-id journal.
A binding mismatch, wallet-storage failure, FI checkpoint failure, or lost or
ambiguous post-journal response has no such cleanup proof: it remains a payment
error, retains the formation, and recover-probes the deterministic reservation
id before freshness or policy on resume. The probe may report exact existing
or authoritative absence without creating a journal; mismatch, storage error,
or ambiguity fails closed. Never infer journal absence from an error message.
This pre-policy recovery ordering applies equally to a post-output replacement
aggregate: the formation-wide output tombstone does not prove that its newer
subset reservation id was checkpointed.

After step 7, newly approved paid replacements sharing an exact reservation use
one aggregate authorization transition before the first new payment, then start
value movement one member at a time because the formation tombstone is already
true. A free seat uses the corresponding exact per-seat transition before
`CreateSeat`. Persisted
combinations that bind an admission to another
quote/effect, authorize paid output before the aggregate tombstone, or record an
accepted selected seat without pre-effect authorization fail closed.

Commercial authorization is not the money boundary. `abandon_formation` may
wipe state while `payment_outputs_started` is false and formation is not
`Formed`; selected-flow reauthorization errors must do so before returning.
If a crash interrupts that cleanup, the durable selected discriminator and
preview deadline prevent legacy authorization or stale cap reuse on resume.
Once the output tombstone is true,
it survives authorization and quote replacement and abandon is forbidden until
a refund-safe teardown exists
(`specs/ARCH-fi-client.md`, *Payment execution and recovery*).

Do not start an independent spend before aggregate authorization. Do not reuse
authorization after any commercial term changes. A refreshed quote ID can retain
its authorization only when every authorized commercial term remains unchanged.
Refresh every `NotStarted` sibling as one barrier and validate the complete
refreshed set before funding any sibling. Do not arm the output tombstone before
the exact wallet and transport barriers complete, and do not poll a wallet output
call before the tombstone commit. Do not replace a quote after an ambiguous
request merely because it expired. Do not substitute a selected FMan without a
fresh preview and renewed user authorization. After outputs, only a terminally
rejected or settled-refund row may enter the typed replacement state; release
that exact aggregate member with the wallet-issued opaque terminal proof before
invalidating the old id. A generic quote refresh cannot manufacture replacement
authority. A required-but-unapproved row may not carry a new quote; only a
freshly approved provisional row may combine its new quote with the retained
old terminal proof, and every other mixed state fails closed. Accepted,
prepared, paid, and ambiguous siblings remain pinned. The fresh verified subset
approval is the one renewed cap authorization and a new reservation covers only
its replacement quotes. Applying that approval is provisional: the old terminal
quote/locator proof is removed only when the exact new effect is authorized.

Transport retry authority comes only from the consumer connector's local,
non-serializable outer call error. A serialized `FleetManagerError` is an
FMan-authored domain result and must never be interpreted as proof that an RPC
stream failed locally, even if its text claims otherwise.

## Secrets and sensitive outputs

FI storage may contain the complete authenticated setup-payment event, resolved
intent, public locators, exact signed quote terms, authorization binding, seat
ids, public guardian-fee accounts, DKG codes, invite code, and an optional
push-gateway callback URL. The callback URL is a bearer capability and its
stable idempotency key can identify one formation operation. Neither is
projected in public status or `Debug`, but database files and backups containing
them remain sensitive. Schema 11 preserves the callback through every
pre-`Formed` recovery and atomically clears it with the `Formed` checkpoint,
after every FMan has durably assumed retry ownership. Older pre-production
schemas fail closed and require reset.
Logical clearing does not erase old pages or backups. FI storage must never
contain raw bearer ecash, payment signatures, identity secret material, or
wallet-private refund secrets.

Although the durable FI record contains no wallet or identity secret, it still
contains operationally sensitive formation history and a federation invite.
Consumers must protect, namespace, back up, and erase its backing database
according to their wallet policy. Avoid including snapshots, locators, invite
codes, or detailed remote errors in routine logs and telemetry.
The same rule covers push hook URLs and their idempotency keys.

Identity and payment adapter errors crossing the public boundary must be
sanitized. Never add `Debug`, serialization, metrics, or error formatting that
can expose secret keys, refund context, bearer notes, or raw payment evidence.

Guardian-fee arrangement accepts no recipient account in its operation input.
Only after loading a durable `Formed` record does `fi-client` parse the exact
Fedimint federation id from the persisted invite and pass it to the
consumer-provided `FiFeeAccountProvider`. Production implementations must
resolve the already joined client for that id and return its own SPv2
`BtcDepositor` account without network or value-moving work; user, RPC, and UI
input are not valid account sources. Provider failure becomes typed unavailable
fee-arrangement capability before any guardian vote. The returned public
descriptor is still untrusted data: `fi-client` validates its account shape,
destination-account uniqueness across the FI account, the Guardian Verification
Fee account, and every guardian account, the canonical complete recipient
vector, and exact threshold readback. The library
cannot prove that a consumer implemented local derivation honestly, so consumers
remain within the trusted integration boundary and must sanitize provider errors.

Post-formation metadata maintenance caps the complete raw consensus object at
1,048,576 bytes immediately after every read, before hashing, parsing, cloning,
connection, signing, or fan-out. The same inclusive cap is enforced by each
FMan on both the current object and canonical target. Per-field limits do not
substitute for this whole-object bound.

Iroh handlers may reorder before entering a seat queue. A live FMan seat loop
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
Combined with FI lease serialization, fresh readback, and exact-base rebasing,
this prevents a late old handler from restoring an object after a newer target
was submitted. Pinning a live occurrence to its first target until consensus
moves or the process restarts is the explicit availability cost of avoiding
new durable sequence state in MVP.

## Cancellation and replay

Dropping a driving future cannot undo a wallet transaction or remote request
already accepted. Every await between a durable write and its response is a
possible interruption point. Reopen and `resume` must be sufficient after any
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
output spendable. Resume recovers authorized quotes before current policy
selection: finish all recover-only wallet probes, replay every already-submitted
`Prepared` operation as one concurrent read/presentation wave, and only after
that wave clear `Rejected` or verified-refused quotes, and then only if every
sibling reached a durable recovery checkpoint. Explicitly release each exact
safe aggregate member before clearing its reservation identity; dropping a
capability is never release.
Prepared replay creates no new value movement. Every recovered `NotStarted`
member starts only after the prior newly started payment has returned with
spendable change and reached its durable FI seat checkpoint.
Automatic selected-flow expiry/provenance cleanup uses the same
recover-existing-release-before-wipe transition as explicit abandon. This
probe precedes expiring admission checks and never creates a reservation, so a
lost successful reserve result remains releasable after preview expiry or
verifier drift. The release itself is preceded by a durable release
commitment naming the exact reservation, so a crash between the wallet
release and the local wipe or restore leaves an expected absence that
completes the interrupted cleanup on the next run; adopting a live
reservation clears the commitment. Any mismatch, ambiguous lookup, or
ambiguous wallet release — and any authoritative absence without that
commitment — retains the FI record and reservation id for retry; local
deletion is never treated as release.
Successful prepared siblings must be checkpointed even when
another probe, replay, or checkpoint fails. Consult current policy only for
`NotStarted`. A recovered operation must reproduce the exact presentation and
matching refund context. Settlement of the same FMan-signed refund must be safe
to repeat after a lost result.

Free `CreateSeat` is also ambiguous on interruption. Replay the exact stored
free quote until a verified signed refusal establishes a safe replacement
boundary. If that refusal occurs in a selected all-free formation before any
wallet output boundary exists, the consumed quote-bound admission cannot
authorize a generic re-quote or second presentation. Retain the exact quote and
admission while returning through selected reauthorization cleanup; only the
durable transition to `Idle` removes them. If cleanup fails or is
interrupted, reopen replays the same signed refusal before retrying cleanup. The
next attempt requires a fresh selection preview. Pinned and post-output
formations retain their exact refusal/replacement behavior.

## Concurrency and hosting

The primary Fedi consumer runs one active formation driver in one app process,
and a process-local guard serializes clones. A process restart ends the old
driver before persisted state is reopened. A renewable database lease coarsely
rejects a second mutating driver, but payment safety does not depend on renewing
it immediately before each member or on cross-process takeover. Restart safety
comes from exact durable payment recovery. Cancellation retains the same
durable recovery facts, and parallel non-value seat work must not rewrite
sibling facts from stale snapshots.

Ordinary formation-state read/modify/write transactions use bounded optimistic
retries. Every retry reloads the active formation and affected seat rows,
rechecks formation identity and exact expected quote or recovery facts, and
recomputes aggregate authorization and completion state before writing. These
transaction closures contain no wallet, signing, or network effects. Exhausted
or non-conflict commit failures return a sanitized storage error rather than
unwinding. Initialization remains single-attempt because the driver lease
serializes it before parallel seat work. Setup-payment policy retention is a
separate bounded read/modify/write transaction: every retry compares the
candidate against the durable NIP-01 high-water so a residual replaced driver
cannot roll policy back.

Lease-key write conflicts during acquire, renewal, or release return `Busy`;
other commit failures return a sanitized storage error and never panic. Tests
pin deferred effect construction and polling, the registry-to-wallet composite
boundary, and coarse run-lease conflict handling.

The durable root is bound to the FI public identity before any status is
published. Storage schema changes fail closed. Schemas 4 through 8 predate one
or more of durable identity binding, the post-DKG seat-binding directory,
commercial authorization history, the distinct wallet-output tombstone, and
the explicit selected-vs-pinned recovery discriminator/deadline, and signed
guardian-fee acceptance facts.
They belong only to the unreleased development implementation; consumers must
reset those namespaces rather than infer an irreversible money boundary that
older records could not represent.

The consensus-read capability carries an obligation the library cannot enforce.
After DKG the driver writes the FMan seat-binding directory on every seat and
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
offers no atomic CAS. FMan's per-key raw caps and the shared
1,048,576-byte complete-object cap bound each retry wave before child work.
Metadata maintenance and guardian-fee arrangement share the formation driver's
process guard and renewable database lease, accept no arbitrary key, and expose only shared semantic
values whose fallible constructors enforce Guardianito validation plus the
65,536-byte absolute raw cap before lease, signing, connection, or network
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
FMan; a new value needs threshold liveness within the caller's deadline, not
unanimity. Terminal policy/lifecycle refusals and bounded convergence failures
remain distinct typed maintenance outcomes.
Guardian-fee arrangement accepts no consumer-supplied guardian or Guardian
Verification Fee accounts: guardian accounts come from signed acceptances, the
Guardian Verification Fee account comes from the deployment profile, and only
the FI role account and bounded rate cross the consumer boundary. Before
directory publication, every post-DKG FMan attestation must repeat the account
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
that preceded it. An adapter that fabricates or caches a snapshot makes the FI
believe the directory reached consensus when it did not. Implement the port
against a real invite-code preview.

The driver uses Fedimint's native/WASM runtime abstraction for deadlines and
retry wakeups. The WASM adapter represents timer milliseconds as `i32`, so every
public formation timing is limited to the inclusive range from one through
`i32::MAX` integral milliseconds on all targets. Private fields and fallible construction
prevent fractional-millisecond values and derived lease-duration overflow, returning
`InvalidFormationRunOptions` before lease, formation-state, wallet, signing, or network
effects. Runtime request and retry timers pass the limiting configured duration
directly, or otherwise pass a checked one-millisecond-or-greater driver-invocation
remainder. A smaller remainder returns `Timeout` before another effect rather
than reaching the WASM adapter as zero. Tests pin the
effective one-millisecond quantum, maximum accepted, and first cross-platform-unsafe values,
observe lease acquisition directly, and exercise both option-consuming public
operations. Re-check this bound and its regressions whenever the runtime timer
implementation or supported WASM target changes.
The persisted maximum lease expires no later than the invocation timeout plus
60 seconds; each renewal uses the smaller of request and invocation timeout plus
60 seconds. Abandoned drivers therefore cannot inherit a request timeout longer
than their run horizon.

Apart from best-effort cleanup of a cancelled database lease,
the crate does not spawn unmanaged tasks. Consumers own execution and
cancellation. Keep dependencies compatible with native mobile targets and
`wasm32-unknown-unknown`; platform-specific adapters belong in the consumer.

Report security-sensitive issues through the repository process in the root
[SECURITY.md](../../SECURITY.md).
