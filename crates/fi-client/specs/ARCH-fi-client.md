# ARCH-fi-client: Federation Initiator client library

`fi-client` is the consumer-neutral state engine for the FI role: it discovers,
verifies, selects, pays, and coordinates Fleet Managers to form a Fedimint
federation without operating a guardian. The Fedi app bridge is the production consumer;
`crates/fi-cli` ([ARCH-fi-cli](../../fi-cli/specs/ARCH-fi-cli.md)) is the thin
testing and E2E consumer.

Read [SECURITY.md](../SECURITY.md) before changing persistence, identity,
payments, recovery, transport, or status output; it owns the durable-ordering,
secret-exclusion, and replay invariants that make the money path safe.

## Scope

The current product path implements advertisement preview through selected-set
payment, DKG, and invite-code recovery. The lower-level pinned-locator driver
is excluded from the default public API and exists only behind the
`dev-pinned-formation` feature for CLI diagnostics and protocol tests. A
locator is dialing data plus the FMan
commitment-verification key — an out-of-band fact, never an issuer attestation
or a consumer trust verdict. Registry discovery is implemented as a
read-only statically admitted enumeration, and trust-based selection with
its read-only preview is implemented over it (see *Registry discovery and
selection* below). Typed post-formation maintenance, liquidity orchestration,
and guardian-fee arrangement are implemented as separate operations.
Post-formation liquidity is governed by
[`SPEC-fi-post-formation-liquidity`](./SPEC-fi-post-formation-liquidity.md).
Each increment extends this consumer boundary and state engine rather than
adding a second driver to any consumer. The public `FormationIntent`
therefore carries only an optional display name (resolved to one generated
two-word name and persisted before stateful remote work), federation size (accepted
product range 7 through 20 guardians), current plan
family, a half-open `FedimintdVersionRange`, and an optional aggregate spending
cap `max_total_msats` (must be greater than zero; persisted with the
resolved intent so it survives resume). The formation intent carries no guardian-fee rate: formation installs the
fixed recipient mapping and an initial rate, while post-formation
`propose_guardian_fees` changes only that rate under the payer-compatible
210,000-ppm ceiling and at or above the published minimum
carried by the admitted setup-payment event (1,500 ppm when no event has
been admitted, never zero). That floor is the same one every FMan enforces
([`SPEC-setup-payment-federations`](../../../specs/SPEC-setup-payment-federations.md));
refusing it here means a rate the whole fleet would vote down names its own
reason instead of retrying to the run deadline. Serialized external intent is a
strict schema: unknown fields and values outside the product name, size,
and cap invariants are rejected rather than producing an invalid intent.
The optional cap has a default: a serialized intent without it decodes to no
cap and a capless intent serializes without the field, while any *unknown*
field stays rejected. Durable FI storage is a separate fail-closed schema:
schema 11 also persists the selected major/minor/vendor DKG identity beside
the selected-vs-pinned mode, durable verifier provenance, selected preview
deadline, exact aggregate reservation identity, commercial-history tombstone,
and wallet-output tombstone. Every older record must be
reset rather than migrated in this pre-launch namespace.

The cap changes only payment-readiness behavior, and it is **one-shot**: it
is the consumer's approval of the initial aggregate only. In the product path
it is sealed into the two-minute advertisement approval returned by
`FmanSelectionPreview::approve`; no FMan quote exists before the consumer
invokes `pay_and_create` with that approval and one explicit payer, or invokes
`create_without_payer` for the all-zero deployment bootstrap. The latter
persists an explicitly absent payer, requests free quotes without payment
policy, and returns typed payer-required reauthorization before requesting a
quote if any selected FMan's live offer is priced. An all-zero advertisement
estimate may be sealed with a zero limit; because this mode cannot pay, that
limit is not projected into the resolved intent's paid spending cap. When the
complete exact quote total is within the cap and no aggregate authorization
was ever recorded, the engine records the same durable
quote-bound aggregate authorization that an explicit `authorize_payments`
call would write. It refreshes every unpaid quote, connects every selected
FMan, and derives a deterministic reservation id from the formation and fresh
exact quote set. The wallet first recover-probes that exact id without creating
state; only authoritative absence plus live admission permits a new
reservation. The wallet idempotently reserves the exact signed output plans,
fee-aware logical debit allocations, required reserve, and virtual value
without assigning disjoint physical notes to the members. It obtains the
allocations by independently dry-running each exact transaction and re-quotes
the current net debit when each member is submitted; any increase may consume
only unreserved headroom. The engine persists that id before the independent
`payment_outputs_started` tombstone and carries the reconstructed opaque
capability into every new seat payment. Only the wallet's typed proof
that an insufficient-balance check failed before creating or observing a
same-id journal permits selected formation to discard that state and return to
payer selection. Binding mismatches, storage errors, and lost or ambiguous
results after journal persistence keep the exact formation durable and
recover-probe the deterministic id before freshness or policy on resume.

The diagnostic pinned path parks when the cap is absent or exceeded and for
every replacement aggregate after its one-shot authorization. Before any
output generation, the selected product path never publishes or accepts that
second exact-quote authorization action. It instead returns typed
`SelectionReauthorizationRequired` and value-safely restores durable `Idle`.
If a process dies before cleanup, schema 11 preserves selected mode and approval
expiry: reopen exposes no superseded authorization action, and resume either continues the still-live
exact set or performs the same typed cleanup. A cloned live approval can then
retry a different ready payer. An unavailable or replacement FMan always
requires a fresh preview; selected guardians are never silently swapped.
After outputs start, only a row whose wallet operation terminally rejected or
whose FMan-signed refund was settled becomes `replacement_required`. Its prior
quote and locator remain durable; accepted, prepared, paid, and ambiguous
siblings remain pinned. A replacement preview excludes every current FMan and
is verified under the current profile. Sealing it with a renewed cap is the
replacement's required fresh user authorization. Applying it
advances the candidate locator and admission provisionally while retaining the
terminal quote/locator proof. Before exact new effect authorization, expiry,
verifier drift, or definite unavailability restores that proof for another
fresh preview. An unstarted exact replacement reservation is reconstructed and
authoritatively released first; a durable release commitment is written
immediately before the wallet call, and only a successful release — or the
exact id's authoritative absence under that prior commitment, which completes
an interrupted restore on a later run — permits the durable store transition
to clear the matching id. Exact replacement quotes
recover their deterministic subset id before freshness and policy even when a
lost reserve response preceded the FI-side id checkpoint. Exact presence is
durably adopted and released before restore; authoritative absence may restore
directly, while mismatch, storage failure, or ambiguity retains the formation.
Within the renewed cap, those replacement quotes self-authorize and receive the
new subset-only reservation id.
If their checked total exceeds the renewed cap, the already non-abandonable
post-output formation publishes `AuthorizePayments` for that exact replacement
subset; the consumer must render and explicitly authorize those terms before
another output can start. This differs from a pre-output selected cap failure,
which returns to `Idle` and needs a new preview/Pay-and-create attempt.
Definite connection failures are retried with bounded exponential backoff for
up to two minutes, clipped by the current driver deadline, before that
pre-output replacement result is returned. After outputs start, transport
failure is an exact-replay error instead and never replacement advice.
The product range does not imply that every FMan release supports every admitted
size. Before requesting a quote from an FMan, the FI requires that FMan's live
availability to advertise the intent's exact size. Concurrently processed
siblings may already have supplied quotes, but formation can proceed only where
product policy and every selected operator's release capabilities overlap.
Selected formation persists the original FI range and the chosen
major/minor/vendor DKG identity. Quote-time checks accept patch or prerelease
differences inside both; replacements and restart recovery remain fixed to that
identity. Stored quotes must name an exact build inside both persisted constraints.
The diagnostic pinned path accepts only a range containing one patch release.
That range determines its `major.minor+fedi` DKG identity before any durable or
remote work; the ordinary quote-time live gate still checks every pinned FMan.

The callback-aware pinned-formation entry point accepts one
`DkgCompletionCallback` created by the consumer for the initiating app
installation. FI derives the pinned set's common DKG identity from the single
allowed patch, then persists the bearer before quotes, payments, seat creation,
or DKG and sends the same callback and idempotency key to every FMan in the
signed `StartDkg` wave. The ordinary entry point leaves the optional
callback absent. An ordinary resume repeats the idempotent `StartDkg` wave
with the same durable guardian codes; each FMan retains the first start
choice.
Callback state is deliberately absent from `FormationSnapshot`: a push is
non-authoritative transport, while the durable formation driver remains the
only source of lifecycle progress after the app resumes.
Cross-component durability verification is recorded in
[`crates/fman/testing.md`](../../fman/testing.md).

Formation storage schema 11 owns this callback lifecycle and the selected
Fedimint DKG identity. Older pre-production records fail closed and require reset.
FI retains the bearer across every pre-`Formed` crash, then clears it in
the same transaction that records the terminal invite because every FMan has
already accepted durable retry ownership.

Whether a formation pays at all is decided by configuration, not by the
formation intent: an FI opened without a deployment-pinned setup-payment
publisher has no authenticated set of federations to fund from, so it arranges
no payment and can only take seats an FMan offers at a price of zero. This is
what makes the first federation in a deployment formable, before any ecash to
pay with exists. The verified product path represents that case through
`create_without_payer`; the diagnostic pinned path is not required for
bootstrap.

Paid formation additionally fetches
the common kind-37707 setup-payment set, authenticates it against the
deployment-pinned Fedi publisher, and atomically retains the complete highest
admitted event as a durable rollback-protected high-water mark
([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md),
[SPEC-locked-payment](../../fman/specs/SPEC-locked-payment.md)).
The public `admitted_setup_payment_federations` query returns that authenticated
set under the same authentication, retention, and empty-set semantics, each
member paired with the signed invite it was admitted from. This includes a
valid empty stop set and zero-balance members, so a consumer can combine it
with joined wallet state for selection/refill UI. Ids alone describe only the
members a consumer has already joined, because an id cannot be turned back
into an invite; a consumer that offers joining an unjoined member needs the
join material the publication already carries. Operations that need a payer
reject the empty set before consulting the wallet.
`pay_and_create` separately requires the explicit payer to be admitted and
Ready before persisting the formation or requesting the first quote.
`create_without_payer` bypasses that paid capability only by failing closed on
a priced live offer before requesting its quote. The consumer wallet can only filter the
authenticated set; canonical selection uses canonical set order.

## Registry discovery and selection

`discover_fman_candidates` turns one bounded, authorless kind-37701
enumeration ([ARCH-nostr-clients](../../nostr-clients/specs/ARCH-nostr-clients.md))
into a statically admitted candidate set without writing durable state,
taking the driver lease, or touching formation status. Per advertisement,
admission runs in a fixed order — event role and signature, the document's
own payload proof and the `fman_id_pubkey == event author` identity rule,
per-author newest-`created_at` replacement (NIP-01 lowest-id tie-break),
the freshness policy, and the caller's eligibility requirements — all
cheap local checks, and every non-admitted advertisement is reported with
a typed reason so consumers can render "N seen, M eligible" honestly. The
candidate set comes back in a fresh random order on every run, because the
advertisement publishes no capacity to spread picks by and any stable order
would send every FI to the same FMans.
The expensive relay-backed PeerBadge verification deliberately does not
run over that pool: it runs lazily, in selection order, inside the
selection walk, so only candidates the ranked walk reaches cost verifier
round trips. Verification includes the canonical environment's minimum trust
level, so a cryptographically authentic badge below that policy is rejected.
Selection evaluates each Fedimint major/minor/vendor identity intersecting the
FI range as a separate DKG cohort, chooses the cheapest complete cohort, and
prefers the newer compatible line when complete cohorts have equal advertised
totals. Patch and prerelease differences inside one identity may mix.
The walk buckets eligible candidates by the advertisement's
claimed issuance key (untrusted, read locally), ranks each bucket by
advertised fee with the freshly randomized discovery order breaking ties, and
fills seats round-robin across
buckets, and verifies each reached candidate's badge with the returned
subject required to equal the authenticated event author and the verified
issuer required to equal the untrusted bucketing claim
([ARCH-fi-client-discovery-selection](./ARCH-fi-client-discovery-selection.md)). The
subject-to-author binding is the security point deferred by the verifier
contract
([SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)):
a valid badge presented on another identity's advertisement must fail. One
commitment-signing `service_pubkey` may likewise occupy at most one selected
seat: after badge verification, a later verified author sharing a selected
key receives a typed rejection and its bucket continues to its next candidate.
Replacement walks begin with retained siblings' keys already occupied, and the
same final-set uniqueness is checked atomically when the approved rows are
applied. Each reached, verified, non-duplicate candidate is then probed live
over the consumer's FMan connector with the same availability predicate
quoting applies; a probe failure or incompatible live response is a typed
rejection, the bucket continues, and a stale advertisement is rejected before
approval instead of invalidating a sealed selection at quote time
([ARCH-fi-client-discovery-selection](./ARCH-fi-client-discovery-selection.md)). Only the
transport-less `FmanSelectionQuery` (no `with_fman_connector`) walks on
advertised claims alone.
One absolute deadline covers enumeration, admission, and the walk, including
an in-flight badge verification or live availability probe bounded further by
a per-candidate probe budget. The preview must complete strictly before the
deadline; expiry wins simultaneous readiness, drops the preview, and returns
`SelectionPreviewTimeout`. The cooperative async bound does not preempt
synchronous work between yield points or executor starvation. The enumeration
itself failing is a typed registry error, and pool exhaustion before the deadline is the typed
`InsufficientFmanSeats` partial failure. The approved freshness and
eligibility policy, the bounded envelope prefix, and locator construction
are recorded in
[ARCH-fi-client-discovery-selection](./ARCH-fi-client-discovery-selection.md). Eligible candidates
and selected seats expose the two-word FMan name derived locally from their
authenticated author identity
([ARCH-service-fleet-manager](../../service-fleet-manager/specs/ARCH-service-fleet-manager.md)) and
carry the ad's declared endpoints, price, and a dialing protocol
`Locator` built during static admission
from the advertised iroh endpoint and the self-attested commitment-signing
`service_pubkey`; after the walk they additionally carry verified badge
facts.

Every MVP seat is subject to that complete PeerBadge verification. There is no
unverified FI-owned, pinned, or bring-your-own exception in the product path.
The separate `insecure_discover_untrusted_pinned_fmans` query exists only for explicit
development/test consumers that enable `dev-pinned-formation` alongside the
already-diagnostic pinned formation driver.
It still authenticates the event and advertisement, freshness, compatibility,
capacity, and dialing material, but deliberately projects no credential or
trust conclusion and cannot construct a selection approval.

`preview_fman_selection` is the read-only public surface over
enumerate → admit → select: it returns the selected seat set, the checked
aggregate advertised estimate, and the seen/eligible/selected summary plus
static non-admissions and chosen-cohort failures for the consumer's estimate
screen. It takes the same
`FmanDiscoveryOptions` clamped-timeout bound as `discover_fman_candidates`
— not formation run options, since no lease or driver timing applies to a
read-only query. It has no durable state and no lease. The result carries a
two-minute validity bound anchored after enumeration and verification and can
be consumed into a sealed, non-serializable `FmanSelectionApproval`; leaving
and re-entering the flow refetches instead of caching it. The approval binds
the complete selection request and immutable verifier/environment provenance.
`pay_and_create` accepts only that sealed approval, an explicit authenticated
Ready payer, and the original spending cap. The sibling
`create_without_payer` entry accepts the same approval for all-zero bootstrap
and never consults payment policy or the wallet. Exact quotes are obtained only
inside those actions. Expiry, quote drift/cap excess, required/missing payer,
failure, or selected-seat unavailability before outputs restores `Idle` and
requires the appropriate payer retry or a fresh preview.

## Consumer boundary

Consumers supply capabilities; they can never supply authenticated protocol
objects, trust conclusions, or lifecycle transitions:

- **Identity** — one stable consumer-scoped FI root. The library derives the
  protocol signer and FI-specific backup author/encryption families.
- **Storage** — an already-namespaced Fedimint `Database`; backend,
  namespacing and local encryption remain consumer concerns. The library owns
  the purpose-built encrypted Nostr recovery document and imports only its
  lean authenticated facts as `Unsynced` state
  ([`SPEC-fi-backup-payload`](./SPEC-fi-backup-payload.md)).
- **Payments** — a wallet adapter that reports Ready federations, performs a
  value-free exact aggregate sufficiency check over a non-serializable typed
  aggregate binding each requirement to its verified quote (foreign output
  values/module fees, independently dry-run primary fees, and required
  reserve), makes each quote-bound funding operation recoverable before
  committing value, re-checks its current net cost against the aggregate's
  remaining headroom, and returns `Prepared` only after the exact transaction
  is accepted and payer change is spendable. It settles signed refunds
  retry-safely. Wallet-private refund material never enters FI storage.
- **Transport** — an iroh connector dialing a protocol `Locator`. Its
  availability and quote calls retain local RPC failures in a non-serializable
  outer call error; the inner result is the Fleet Manager's wire-domain
  response. This prevents a remote service error from impersonating a definite
  local transport failure and gaining retry or reselection treatment. The
  selection walk's live availability probe reads through the same connector;
  read-only preview consumers add one with `with_fman_connector`.
- **PeerBadge trust** — a concrete shared `PeerBadgeVerifier`, constructed
  from the deployment's canonical issuer-identity roots, authority relays, and
  validated minimum trust level. Stateful consumers inject it at
  `FiClient::open`. Read-only consumers construct registry-only
  `FmanRegistryQuery` for static discovery, then add a verifier with
  `with_verifier` to obtain `FmanSelectionQuery` only when they need a verified
  preview. Discovery emits statically admitted publisher claims, while the
  selection walk consumes the verifier lazily and binds every verified badge
  subject to the advertisement's authenticated author and issuer claim; an
  authentic badge below the selected environment's minimum is not a verified
  candidate
  ([SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)).
- **Consensus read** — invite-code reads over real, uncached upstream queries.
  The metadata operation returns the downloaded client config and raw `meta`
  consensus value, never a verdict: the library derives the peer set, verifies
  every FMan attestation, and performs equality checks itself. The separate
  LNv2 operation uses Fedimint's threshold response strategy and returns the
  individually valid public gateway endpoints in that aggregate; it omits an
  unrelated entry outside `GatewayApiUrl` policy rather than making that entry
  hide an exact valid target, while FI still decides target membership. This is
  the capability the library cannot check the honesty of — the upstream reads
  earn their guarantees from threshold peer queries and return no signatures,
  so an adapter that fabricates or caches a result defeats readback silently.
  The obligations are stated on the port and in [`SECURITY.md`](../SECURITY.md).
- **Presentation** — collects intent, renders an uncached advertisement-only
  preview and authenticated payer choices, retains the sealed preview approval
  only for that screen visit, and invokes Pay-and-create with its explicit
  payer and original cap, or the explicit no-payer bootstrap entry when the
  deployment has no setup-payment policy.

`FormationIntent` itself rejects invalid construction and decoding. The creation
APIs additionally expose side-effect-free, operation-specific preflights for
consumers: selected creation validates the sealed seat count, preview age,
explicit payer, and cap; the development-only pinned creation preflight
validates a single-patch range, locator count, and unique FMan keys.
Timing options likewise have private fields and checked construction; invalid
runtime or lease bounds cannot reach identity, durable state, lease, wallet, or
network access.

Formation installs guardian-fee arrangement on the same state engine. Opening
with a `ManifoldEnvironmentProfile` supplies the setup-payment publisher and
deployment-pinned Guardian Verification Fee account; every constructor also
accepts a `FiFeeAccountProvider`. After DKG, the engine parses the federation id from the
persisted invite, resolves the FI's single-signature SPv2 `BtcDepositor`
account, loads guardian accounts from signed seat acceptances, and collects
FMan attestations paired with their seat-endpoint proofs. It derives
account-keyed recipients from those attestations, refusing any shared
destination account. Before the first remote proposal it
persists the complete formation target: canonical-directory readback prediction,
paired attestation/proof request entries, FI account,
fixed recipients with FI at weight four, every guardian at weight one, and the
Guardian Verification Fee at weight one, plus the initial rate. It then
proposes that target as one base-bound metadata update. Every FMan
constructs the same bounded canonical directory, verifies every paired proof
against final configured endpoint keys, and derives
the same recipients; FI waits for all three exact consensus fields and durably
checkpoints their exact readback. An interrupted proposal replays the persisted
target without re-resolving accounts or policy. Once confirmed, formed-state
reconciliation requires the immutable directory and recipients to remain exact,
accepts later rate changes, and needs no formation account provider.

After `Formed`, `propose_guardian_fees` changes only the rate through the generic
metadata verb. It does not resolve or resend recipient accounts. Directory and
recipient keys are formation-owned and generic writes carrying them revalidate
the stored signed directory and compiled split before voting.

## Public state and concurrency

The engine exposes `FiStatus::Idle` or one active formation that always
carries its formation id and fully resolved persisted intent. Aggregate phases
are `Preparing`, `AwaitingPaymentReadiness`, `AcquiringSeats`, `PreparingDkg`,
`DkgUnderway`, `PublishingSeatBindings`, `Formed`; status is published through a
watch channel
independently of the future driving the run. Durable phases advance atomically
with their required recovery facts: `Formed` requires every accepted seat's
quote, signed guardian-fee account, and bare upstream guardian setup code plus
the common invite,
and inconsistent storage fails
closed before the library publishes status. The seat/fee-account pairing is
validated on load, so formation records persisted before signed fee-account
acceptance existed fail closed and must be reset rather than migrated, per
this pre-launch namespace's schema policy. Payment readiness and
authorization are aggregate formation state — one authorization covers the
complete verified quote set. Immediately before funding, the FI refreshes every
unpaid paid quote as one barrier and carries the authorization forward only
after every sibling retains unchanged commercial terms. It then repeats the
value-free exact funding preflight and resolves every FMan transport before
durably setting `payment_outputs_started`. The next fallible effect is the
wallet output-generation poll. Changed terms return the pinned path to
aggregate readiness; the selected path restores `Idle` for a fresh approval.
The FI replays a stored free quote exactly until the FMan signs a refusal, so a
lost acceptance response cannot allocate a second quote-derived seat. In a
selected all-free formation, that signed refusal consumes the exact admission
without crossing the wallet-output boundary; the engine retains the terminal
quote until abandon returns durable `Idle`, then requires a fresh
selection preview rather than re-quoting the same admission. If abandon fails,
reopen replays the exact refusal before retrying cleanup. A paid
quote whose wallet payment already started is likewise replayed exactly rather
than refreshed, because its funds are locked to that quote's blind nonces.
Every selected row persists a typed admission state. Initial paid admissions
are bound to their exact quote and paid-output effect in the aggregate
output-start transaction. Paid replacements whose formation already crossed
that tombstone and share one exact reservation are bound by one atomic
authorization update before the first new payment begins; an
independent free admission is bound to its exact presentation before
`CreateSeat`. Crash recovery therefore consults per-seat
effect authority instead of inferring it from the formation-wide tombstone.
Before the first payment, the driver completes quote refresh, resolves every
final FMan connection, prepares effect-free payment work, and durably sets
`payment_outputs_started`. Newly started paid members then run in stable seat
order. Each adapter return means the exact Fedimint transaction is accepted and
its payer-owned change is spendable; the FI presents and durably checkpoints
that member before starting the next payment. A later failure therefore
preserves every prior seat. Initial formation, resume of `NotStarted` members,
and paid replacements use this same ordered path.
Resume asks the wallet about every authorized stored quote before consulting
current setup-payment policy. It completes the wallet probes as one concurrent
read-only barrier, concurrently replays every already-submitted prepared
payment, checkpoints successful
siblings despite other failures, and clears terminally rejected or
verified-refused quotes only when every sibling reached a durable recovery
checkpoint. Those replays create no new wallet value movement; after the
barrier, every newly started `NotStarted` member follows the same sequential
checkpoint-before-next rule. Before invalidating the old aggregate, it
explicitly releases each exact rejected/refunded member in the wallet;
dropping a capability or reservation id is never release. Current policy
replaces only operations the wallet proves safe. Per-seat progress is
diagnostic and carries independent recovery facts. Public seat progress,
payment requirement, and replacement rows expose the badge-vouched identity
of their assigned — for a replacement row, outgoing — FMan when one exists,
and derive the two-word FMan name from it locally
([ARCH-service-fleet-manager](../../service-fleet-manager/specs/ARCH-service-fleet-manager.md));
pinned rows expose none. The identity is presentation material: the locator
remains the protocol-owned dialing and verification binding, and a
replacement approval swaps a row's exposed identity together with its
admission.

While the formation is value-safe — before wallet output generation was
durably armed and before `Formed` — `abandon_formation`
wipes the durable formation state back to `Idle` under the run guard. Any exact
pre-output wallet reservation is first reconstructed
by a recover-existing-only id probe and explicitly released. Authoritative
absence permits the wipe; mismatch, storage failure, or ambiguity retains the
formation. The probe never creates a reservation and precedes admission
freshness/provenance checks. It contacts no FMan, so free seats already accepted
server-side are forfeited, and the setup-payment policy retention is
preserved. Outside that window it returns the typed
`AbandonUnavailable` error. Commercial quote authorization alone does not
close the window; `payment_outputs_started` is an independent monotone fact
that survives authorization or quote replacement. The next await after that
durable boundary starts the first payment.
Post-output abandon remains deferred. Once output generation is armed, funds
may be locked to quote-bound nonces; safe teardown needs recovery and refund
handling before any state can be destroyed.

The library returns run futures instead of spawning tasks; dropping one
cancels local work only, and reopening the same database, identity, and wallet
then calling the continuation API is the supported resume. A process-local
guard serializes clones. The primary Fedi host runs one active formation driver
in one app process; a process restart ends that driver before persisted state is
reopened. A renewable database lease is a coarse guard against accidentally
opening a second mutating driver, but sequential payment safety does not depend
on renewing it immediately before each payment or on cross-process takeover.
Exact durable operation recovery prevents a restarted consumer from submitting
a payment twice. Each invocation has one monotonic deadline, and prepared
payment work remains effect-free until it is polled. Parallel non-value seat
work uses narrow row updates so one completion cannot overwrite another seat's
recovery facts. The crate stays compatible with native mobile targets and
`wasm32-unknown-unknown`.

## Protocol ownership

The driver owns availability checks, signed-quote verification, aggregate
readiness, payment presentation, signed seat creation, DKG coordination,
status polling, and invite recovery (rejecting bearer API secrets and
requiring every FMan's invite to identify the same federation before
persisting the deliverable). Every ordinary resume repeats the idempotent
`StartDkg` with the same durable guardian codes, so a crashed child starts a
fresh ceremony while an existing `DkgInProcess` ceremony keeps being polled.
Resume never selects the destructive `RestartDkg`: replacing a stuck ceremony
is an explicit, user-authorized action, and ordinary resume must not infer
that intent. Wire types,
envelopes, and the service trait come
from `crates/service-fleet-manager` and must not be duplicated here; the
FI-facing verb contract is
[SPEC-fi-rpc](../../fman/specs/SPEC-fi-rpc.md) and the byte-level
authentication is
[SPEC-signed-envelopes](../../service-fleet-manager/specs/SPEC-signed-envelopes.md).

## Post-formation metadata maintenance

`update_federation_metadata` exposes only the FMan's compiled MVP policy as
fallibly constructed semantic values: display name, HTTP(S) icon URL, welcome
message/description, and Guardianito's fixed terms document. The shared types
apply Guardianito's trim-for-validation/raw-for-submission rules and the
65,536-byte absolute raw resource cap before lease, signing, connection, or
network effects; every FMan repeats the same authoritative validation. The API
does not accept raw metadata keys, clear fields, upload image bytes, or widen
the FMan allowlist. The operation requires
the durable FI formation to be `Formed`, reconstructs every FI-owned seat from
that record, and shares the formation driver's process guard, database lease,
deadline, connector, identity, and real consensus-reader capabilities.

The complete consensus metadata object has a separate inclusive 1,048,576-byte
ceiling. The driver checks it immediately after every live read and returns a
typed terminal maintenance error before hashing, parsing, cloning, connecting,
signing, or fan-out. FMan applies the same ceiling before parsing the current
object and before submitting the canonical target. Initial seat-binding
publication applies the same FI ceiling before hashing or fan-out and fails the
formation rather than amplifying an oversized pre-existing object.

The public operation accepts `MaintenanceRunOptions`, whose checked native/WASM
timers and errors use maintenance vocabulary. Its private representation reuses
the formation driver's generic deadline, lease, and request-timeout machinery;
that implementation sharing is not exposed as formation policy in the
maintenance API.

Because the meta module votes on one opaque whole object, each retry reads the
live raw value, commits every FI-signed seat request to its exact
`MetaConsensusBase`, attempts one non-short-circuiting best-effort wave across
the reachable guardians, and considers stale-base, connection, temporary-seat,
request-timeout, and consensus-read responses retryable. Within one exact base,
the driver retains each acknowledged seat for that live invocation and
reconnects/resubmits only unresolved rows with bounded exponential backoff; a
different fresh consensus base resets the row set because the mutation must be
signed against that new whole object. Cancellation or reopen uses safe exact
replay and may resubmit rows because acknowledgements are not durable. This
avoids multiplying already accepted 65,536-byte values while one live
invocation waits on an unavailable minority. It reads consensus before
connecting and after every partial wave, so an already-adopted value needs no
live FMan and a threshold-live subset may finish while minority seats remain
offline.

The FMan's per-seat single-owner loop adds a live-occurrence pin: before that
process enters the fallible child submit RPC for one
whole-object target at the current base, it pins the target. A differently
targeted handler delayed before queue entry is then refused even if threshold
consensus has not advanced or the first submit response was lost; exact
replay remains allowed. Bases are bound to the meta module's monotone
consensus revision, so an exact `O -> B -> O` recurrence is a fresh
occurrence that stales old handlers, and a superseded occurrence's pin is
simply replaced — one pin is the complete state, with no history or cap.
Process restart clears the pin and destroys all delayed handlers, so the pin
need not outlive the process. Pinning a live occurrence to its first target
is the deliberate liveness cost of avoiding a signed durable sequence in
MVP.

It reports success only after a fresh consensus read contains the exact
requested string. A non-formed local record returns the maintenance-specific
wrong-state error before connector work; an intrinsic FMan policy/lifecycle
refusal remains a typed terminal maintenance rejection; and retryable failures
that outlive the caller's bound return one maintenance-convergence error with
the unresolved seats and last sanitized consensus/guardian failures. A
concurrent winning change is therefore preserved when this mutation is rebased;
a single FMan acknowledgement never masquerades as threshold adoption. The
operation itself stores no second mutable maintenance state: cancelling or
crashing is safe because a later identical request rereads consensus and either
observes success or replays the idempotent typed mutation.

## Modules

- `state` — public intent, status, and phase types.
- `ports` — consumer and transport capability traits.
- `db` — Fedimint-database recovery facts, checkpoints, and the driver lease.
- `discovery` — read-only FMan advertisement discovery and static
  admission.
- `selection` — ranked round-robin seat selection with lazy badge
  verification, and the read-only selection preview.
- `setup_payment_federations` — authenticated common-set refresh, durable
  high-water retention, and canonical selection.
- `formation` — formation, durable aggregate reservation, exact recovery,
  release-before-wipe cleanup, proven-safe subset replacement transitions,
  and guardian-fee policy orchestration.
- `liquidity` — post-formation FLIP selection and signed allocation recovery,
  including exact-URL gateway attachment and its durable readback proof.
- `maintenance` — typed post-formation consensus maintenance orchestration.
- `unavailable` — deliberately unavailable adapters for consumer scaffolding.

## Testing

State-engine tests use fake ports to cover validation, default-name
persistence, quote-set readiness, aggregate authorization, parallel narrow
writes with bounded conflict retries, cancellation, exact replay, refund retry,
and reopen/resume; policy tests prove canonical kind-37707 selection and rollback
rejection. Discovery tests cover one typed rejection per cheap admission
stage plus per-author replacement, the local candidate cap,
deadline-expiry reporting, and the multi-candidate happy path. Selection
tests substitute a deterministic badge-verification port and cover the
subject-binding and claimed-issuer failures, the bounded envelope prefix,
bucket ordering and the round-robin fill, lazy-verification call counts,
deadline expiry, and the preview summary and typed shortfall; the
selected-flow tests additionally cover preview expiry, explicit payer
readiness, verified all-zero selected bootstrap without a publisher or wallet,
typed priced-offer rejection, zero-balance admitted-payer listing, exact aggregate preflight,
same-id reconstruction and mismatch refusal, capability carry and one-shot
member consumption, drop-not-release, whole/member proven-safe release and
crash ordering, over-cap and unavailable-seat release-before-`Idle`, and the
independent output tombstone. A focused payer fixture gives the wallet one note
covering several setup prices and fees, proves concurrent independent spends
exhaust it, and proves ordered settlement forms successfully with exact final
balance/change accounting. Replacement tests use the verified excluding-set
and retained-signing-key preview and cover apply-time key-collision refusal,
reopen, accepted/ambiguous sibling preservation, renewed
authorization, completion, and non-repetition of prior outputs/payments. The
fi-cli's concrete reference FI payer adapter separately covers logical
aggregate debit reservation, current net-fee headroom checks, exact
foreign-output fee aggregation, fee-bearing reservation reconstruction and
rollback, same-id reconstruction after partial consumption, conflict-retried
concurrent member mutations, unordered/duplicate rejected-input refund
recovery through spendable outputs, mint-v1 per-note refund fallback, and
terminal-proof enforcement. Its full funded-wallet path remains covered by the
paid `defe` E2E described below, which exports one large ecash note, reopens the
wallet to recover exact current operations without resubmission, and
independently reconciles setup prices, Fedimint transaction fees, and returned
spendable change. The
shared verifier's own behavior stays with
its focused crate tests. The `defe`
E2E split is intentional: a seven-FMan free formation proves parallel ceremony
scale, and a separate seven-FMan paid formation proves the real quote-bound
mint funding and claim path, including reuse of the same durable FI state and
wallet when a recoverable pre-output reservation failure requires one resume.
A third Linux `defe` E2E kills three real
`fedimintd` children after DKG starts, kills the FI process, reopens its durable
guardian-code preparation, waits out the abandoned driver lease, and proves
that a fresh process repeats the idempotent `StartDkg` wave and resumes the
same seven-guardian formation; each killed guardian records its replacement
start while intact guardians may retain the first ceremony. The crash-recovery
process also retains one callback bearer and idempotency key across that FI
crash, then proves callback retry survives an FMan restart and reaches
delivery on every guardian after an HTTP 500 endpoint recovers. The
paid E2E does not claim post-output or refund fault injection — exact recovery
and refund replay stay with the focused tests.

Maintenance tests split at the authorization boundary. The shared service and
FMan suites own every Guardianito semantic boundary, compiled-key dispatch,
raw-resource rejection, whole-map preservation, and stale-base refusal before
child work. `fi-client` owns exact public-variant projection, pre-Formed and
oversized no-effect rejection, already-adopted/offline success, threshold-live
partial convergence, below-threshold bounded failure, transient consensus-read
retry, stale-wave rebase, cancellation-safe replay, and preservation of
unrelated consensus fields. Composition intentionally relies on the service and
FMan suites for authoritative semantic rejection rather than duplicating those
validators in the client fake.

Liquidity tests use fake discovery, provider, FMan, and consensus-reader ports
to cover durable completion replay before provider discovery, exact gateway-URL
proof invalidation, and signed completion-evidence validation. The
federation-preview unit test separately covers per-entry admission and
deduplication after upstream aggregation; module discovery, the live threshold
query, and network freshness remain that adapter's integration boundary. The
FMan suite owns idempotent endpoint insertion, and the FI fake deliberately does
not stand in for either concrete boundary.
