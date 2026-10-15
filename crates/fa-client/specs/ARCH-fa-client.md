# ARCH-fa-client: Federation Ambassador client library

`fa-client` is the consumer-neutral state engine for the FA role: it discovers,
verifies, selects, pays, and coordinates Manifold Guardians to form a Fedimint
federation without operating a guardian. The Fedi app bridge is the production consumer;
`crates/fa-cli` ([ARCH-fa-cli](../../fa-cli/specs/ARCH-fa-cli.md)) is the thin
testing and E2E consumer.

Read [SECURITY.md](../SECURITY.md) before changing persistence, identity,
payments, recovery, transport, or status output; it owns the durable-ordering,
secret-exclusion, and replay invariants that make the money path safe.

## Scope

The current product path implements advertisement preview through selected-set
payment, DKG, and invite-code recovery. Every seat comes from a verified
selection; there is no pinned-locator path. A
locator is dialing data plus the MG
commitment-verification key — an out-of-band fact, never an issuer attestation
or a consumer trust verdict. Registry discovery is implemented as a
read-only statically admitted enumeration, and trust-based selection with
its read-only preview is implemented over it (see *Registry discovery and
selection* below). Typed post-formation maintenance, liquidity orchestration,
and guardian-fee arrangement are implemented as separate operations.
Post-formation liquidity is governed by
[`SPEC-fa-post-formation-liquidity`](./SPEC-fa-post-formation-liquidity.md).
Each increment extends this consumer boundary and state engine rather than
adding a second driver to any consumer. The consumer records a federation
creation, and one run loop buys its seats and forms it (see *Seat purchase,
pool, and formation* below). The
selection request carries federation size (accepted product range 7 through 20
guardians), current plan family, and a half-open `FedimintdVersionRange`.
`create_federation` takes an optional
display name (resolved to one generated two-word name and persisted before
remote work), an optional DKG completion callback, the preview, and an
optional payer. Creation carries no guardian-fee rate: formation installs the
fixed recipient mapping and an initial rate, while post-formation
`propose_guardian_fees` changes only that rate under the payer-compatible
210,000-ppm ceiling and at or above the published minimum
carried by the admitted setup-payment event (1,500 ppm when no event has
been admitted, never zero). That floor is the same one every MG enforces
([`SPEC-setup-payment-federations`](../../../specs/SPEC-setup-payment-federations.md));
refusing it here means a rate the whole fleet would vote down names its own
reason instead of retrying until the maintenance limit. Durable FA storage is a
separate fail-closed schema. Schema 12 keeps one creation row, one slot row
per selected guardian, and one cached guardian code per accepted seat. A slot
is keyed by a stable id, and the ids of the live slots order the guardians.
It holds its verified MG and badge holder, and one state: `picked` with its
previewed plan, `bound` to its exact signed quote, `accepted` with its seat
id, guardian-fee account, and exact Fedimint build, or `ended` with the seat
id its MG ended before DKG. An ended slot is not live, and a new slot
replaces it. The creation row holds the
creation's inputs, the formation's progress (its id, invite, and metadata
target), the reason the last slot left, and the last purchase: its
selected major/minor/vendor DKG identity and optional explicit payer. The
purchase runs while some slot is
not accepted. The pool is the accepted slots, and the formation runs when
`size` live slots are accepted. Schema 11
kept the purchase and the formation in one row; opening such an FA migrates it.
A schema 11 pinned formation cannot be migrated, because its MGs have no
authenticated identity; opening it fails with reset guidance.
Each schema 11 seat becomes the slot at its index, so the seat that named the
federation stays first. Accepted seats keep their cached codes, and seats
bound to an effect keep their quote. Replacement seats are dropped. Schema 11
did not record the previewed plan, so a seat that was never bound leaves with
`SeatUnavailable`. Aggregate authorization and reservation
fields are ignored, so the wallet adapter must release any unstarted part of an
old reservation before it opens the FA.
A formed row yields only the federation record, when none exists. Schema 11
also kept an imported backup in its own row until consensus verified it. A
verified one becomes the federation record. An unverified one is dropped with
the completed-lookup markers, so the next restored open looks it up again.
Records older than
schema 11 are reset because they predate the production compatibility
baseline. Subsequent schema changes are governed by
[`GATE-production-compatibility`](../../../specs/GATE-production-compatibility.md).

The five-minute selection preview is the consumer's payment approval. Each
selected seat stores the plan its advertisement showed, price included, and
every quote for that seat requests exactly that plan. A quote therefore has
exactly the previewed price, and the purchase total is the previewed total.
No MG quote exists before the consumer invokes `create_federation` or
`buy_seats` with the preview
and one explicit payer, or without a payer for the all-zero deployment
bootstrap. The latter persists an explicitly absent payer, requests free
quotes without payment policy, and returns typed payer-required
reauthorization before requesting a quote if any selected MG's live offer is
priced. When the MG's live offer no longer contains the previewed plan, the
purchase returns typed `SeatUnavailable` reauthorization before outputs.
A reauthorization reason is `SeatUnavailable` for every problem that a new
selection fixes. The payer reasons (`SelectedPayerUnavailable`,
`SelectedPayerInsufficientFunds`, `PaymentFederationRequired`) stay separate,
because the user must fix the payer instead. `SeatEnded` also stays separate:
an MG ended a seat the FA paid for.

While no slot is bound, each run checks the whole purchase once and pays
nothing. It asks every selected MG for live availability at the
previewed plan, with the same connection retries as a quote. A seat that
fails leaves. Then the wallet checks, without value movement, that the payer
can fund the remaining priced seats one after another. If it cannot, every
seat leaves with `InsufficientFunds`.

The purchase buys seats one at a time, in slot order. For each picked slot,
the driver first binds a quote: one autocommit stores the exact
signed quote before its payment or presentation can start. Only
`create_federation` and `buy_seats` check that the preview is fresh and
under the same verifier provenance, so a slow or resumed purchase never
expires. A bound free
quote is presented again exactly. A bound paid quote is first given to the
wallet's per-quote recovery:

- `Prepared` — the wallet operation for that quote exists, so the driver
  presents its payment without a new spend.
- `Rejected` — the wallet operation terminally failed, so the seat leaves.
- `NotStarted` — no value moved, so the driver requests a new quote at the
  same previewed plan and binds it in place of the old one.

A seat with a bound unpaid quote then asks the wallet to create the payment.
The wallet returns only after the exact transaction is accepted and payer
change is spendable. The driver presents the payment with `CreateSeat` and
checkpoints the accepted seat before it starts the next seat. A signed
refusal is settled in the wallet first, then the seat leaves. The FA replays
a stored free quote exactly until the MG signs a refusal, so a lost acceptance
response cannot allocate a second quote-derived seat.

An accepted seat joins the pool at once. A seat leaves the purchase with a
typed reason when its MG or payer becomes unavailable, its price changes, or
it was refused. One transaction deletes its slot. If the wallet proves that
the payer cannot fund a seat, that seat and every seat without a bound quote
leave with `InsufficientFunds`. Other errors keep the seat and end the run,
so the next run continues it. The purchase ends when no slot is left to buy.
If a seat left, the creation returns to `NeedsSeats` with the first leave
reason, and a new purchase adds slots with new ids. Selected guardians are never silently swapped.
Definite connection failures are retried with bounded exponential backoff for
up to two minutes before the
pre-output fresh-selection result is returned. After outputs start, transport
failure is an exact-replay error instead and never requires a fresh selection.
The product range does not imply that every MG release supports every admitted
size. Before requesting a quote from an MG, the FA requires that MG's live
availability to advertise the intent's exact size. Concurrently processed
siblings may already have supplied quotes, but formation can proceed only where
product policy and every selected operator's release capabilities overlap.
Selected formation persists the original FA range and the chosen
major/minor/vendor DKG identity. Quote-time checks accept patch or prerelease
differences inside both; restart recovery remains fixed to that
identity. Stored quotes must name an exact build inside both persisted constraints.

`create_federation` accepts one optional `DkgCompletionCallback` created by the
consumer for the initiating app installation. FA persists the bearer in the
creation record before any DKG work and sends the same callback and
idempotency key to every MG in the signed `StartDkg` wave. A restarted run
repeats the idempotent `StartDkg` wave
with the same durable guardian codes; each MG retains the first start
callback it accepts for delivery, while an MG without callback delivery
configured discards it and proceeds callback-free.
Callback state is deliberately absent from `FormationSnapshot`: a push is
non-authoritative transport, while the durable formation driver remains the
only source of lifecycle progress after the app resumes.
Cross-component durability verification is recorded in
[`crates/mg/testing.md`](../../mg/testing.md).

The formation record owns this callback lifecycle.
FA retains the bearer across every pre-`DkgComplete` crash, then clears it in
the same transaction that records the DKG invite. Callback delivery remains
best effort: configured MGs have accepted durable retry ownership by then,
while MGs without callback delivery configured completed DKG without taking
ownership.

Whether a formation pays at all is decided by configuration, not by the
formation intent: an FA opened without a deployment-pinned setup-payment
publisher has no authenticated set of federations to fund from, so it arranges
no payment and can only take seats an MG offers at a price of zero. This is
what makes the first federation in a deployment formable, before any ecash to
pay with exists. The verified product path represents that case through
`buy_seats` without a payer.

Paid formation additionally fetches
the common kind-37707 setup-payment set, authenticates it against the
deployment-pinned Fedi publisher, and atomically retains the complete highest
admitted event as a durable rollback-protected high-water mark
([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md),
[SPEC-locked-payment](../../mg/specs/SPEC-locked-payment.md)).
The public `admitted_setup_payment_federations` query returns that authenticated
set under the same authentication, retention, and empty-set semantics, each
member paired with the signed invite it was admitted from. This includes a
valid empty stop set and zero-balance members, so a consumer can combine it
with joined wallet state for selection/refill UI. Ids alone describe only the
members a consumer has already joined, because an id cannot be turned back
into an invite; a consumer that offers joining an unjoined member needs the
join material the publication already carries. Operations that need a payer
reject the empty set before consulting the wallet.
`buy_seats` with a payer separately requires it to be admitted and
Ready before persisting the purchase or requesting the first quote.
`buy_seats` without a payer bypasses that paid capability only by failing closed on
a priced live offer before requesting its quote. The consumer wallet can only filter the
authenticated set; canonical selection uses canonical set order.

## Registry discovery and selection

`discover_mg_candidates` turns one bounded, authorless kind-37701
enumeration ([ARCH-nostr-clients](../../nostr-clients/specs/ARCH-nostr-clients.md))
into a statically admitted candidate set without writing durable state,
taking the run guard, or touching formation status. Per advertisement,
admission runs in a fixed order — event role and signature, the document's
own payload proof and the `fman_id_pubkey == event author` identity rule,
per-author newest-`created_at` replacement (NIP-01 lowest-id tie-break),
the freshness policy, and the caller's eligibility requirements — all
cheap local checks, and every non-admitted advertisement is reported with
a typed reason so consumers can render "N seen, M eligible" honestly. The
candidate set comes back in a fresh random order on every run, because the
advertisement publishes no capacity to spread picks by and any stable order
would send every FA to the same MGs.
The expensive relay-backed PeerBadge verification deliberately does not
run over that pool: it runs lazily, in selection order, inside the
selection walk, so only candidates the ranked walk reaches cost verifier
round trips. Verification includes the canonical environment's minimum trust
level, so a cryptographically authentic badge below that policy is rejected.
Selection evaluates each Fedimint major/minor/vendor identity intersecting the
FA range as a separate DKG cohort, chooses the cheapest complete cohort, and
prefers the newer compatible line when complete cohorts have equal advertised
totals. Patch and prerelease differences inside one identity may mix.
The walk buckets eligible candidates by the advertisement's
claimed issuance key (untrusted, read locally), ranks each bucket by
advertised fee with the freshly randomized discovery order breaking ties, and
fills seats round-robin across
buckets, and verifies each reached candidate's badge with the returned
subject required to equal the authenticated event author and the verified
issuer required to equal the untrusted bucketing claim
([ARCH-fa-client-discovery-selection](./ARCH-fa-client-discovery-selection.md)). The
subject-to-author binding is the security point deferred by the verifier
contract
([SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)):
a valid badge presented on another identity's advertisement must fail. One
commitment-signing `service_pubkey` may likewise occupy at most one selected
seat: after badge verification, a later verified author sharing a selected
key receives a typed rejection and its bucket continues to its next candidate.
The same rule applies to verified badge holders, which are saved with
new seats and checked again on recovery. Older saved seats without holder
identities keep their existing behavior without a holder-uniqueness guarantee.
Each reached, verified, non-duplicate candidate is then probed live
over the consumer's MG connector with the same availability predicate
quoting applies; a probe failure or incompatible live response is a typed
rejection, the bucket continues, and a stale advertisement is rejected before
approval instead of invalidating a sealed selection at quote time
([ARCH-fa-client-discovery-selection](./ARCH-fa-client-discovery-selection.md)). Only the
transport-less `MgSelectionQuery` (no `with_mg_connector`) walks on
advertised claims alone.
One absolute deadline covers enumeration, admission, and the walk, including
an in-flight badge verification or live availability probe bounded further by
a per-candidate probe budget. The preview must complete strictly before the
deadline; expiry wins simultaneous readiness, drops the preview, and returns
`SelectionPreviewTimeout`. The cooperative async bound does not preempt
synchronous work between yield points or executor starvation. The enumeration
itself failing is a typed registry error, and pool exhaustion before the deadline is the typed
`InsufficientMgSeats` partial failure. The approved freshness and
eligibility policy, the bounded envelope prefix, and locator construction
are recorded in
[ARCH-fa-client-discovery-selection](./ARCH-fa-client-discovery-selection.md). Eligible candidates
and selected seats expose the two-word MG name derived locally from their
authenticated author identity
([ARCH-service-manifold-guardian](../../service-manifold-guardian/specs/ARCH-service-manifold-guardian.md)) and
carry the ad's declared endpoints, price, and a dialing protocol
`Locator` built during static admission
from the advertised iroh endpoint and the self-attested commitment-signing
`service_pubkey`; after the walk they additionally carry verified badge
facts.

Every MVP seat is subject to that complete PeerBadge verification. There is no
unverified FA-owned, pinned, or bring-your-own exception.

`preview_mg_selection` is the read-only public surface over
enumerate → admit → select: it returns the selected seat set, the checked
aggregate advertised estimate, and the seen/eligible/selected summary plus
static non-admissions and chosen-cohort failures for the consumer's estimate
screen. It takes the same
`MgDiscoveryOptions` clamped-timeout bound as `discover_mg_candidates`
— not formation run options, since no driver timing applies to a
read-only query. It has no durable state. The result carries a
five-minute validity bound anchored after enumeration and verification;
leaving and re-entering the flow refetches instead of caching it. The preview
binds the complete selection request, each seat's advertised plan, and
immutable verifier/environment provenance, and it cannot be serialized.
`create_federation` and `buy_seats` accept the preview and an optional
explicit authenticated Ready payer. Without a payer it serves all-zero bootstrap and never consults payment
policy or the wallet. Exact quotes are obtained only inside that action.
Expiry, a changed price, quote drift, required/missing payer, failure, or
selected-seat unavailability before outputs removes the purchase and requires
the appropriate payer retry or a fresh preview.

## Seat purchase, pool, and formation

A seat belongs to the FA once its MG accepts it, and it is not bound to a
federation until DKG starts. The engine keeps one record per concern:

- **Creation** — `create_federation` saves the name, size, version range,
  plan, and optional completion callback, and starts the first purchase. The
  record lives until the federation forms. `cancel_creation` deletes it
  before any seat is bought.
- **Purchase** — quotes, pays for, and creates the seats of one approval,
  under every payment rule in this document. It runs while some slot is
  picked or bound, so at most one purchase exists. When a purchase ends
  before it buys every seat, the state asks for a preview of the missing
  seats; `preview_missing_seats` avoids the MGs, service keys, and badge
  holders already in the pool and keeps their DKG line, and `buy_seats` buys
  them.
- **Pool** — the accepted slots. Each keeps its MG identity, locator, seat
  id, guardian-fee account, and the exact Fedimint build it was bought for,
  until the federation forms.
- **Formation** — runs when every slot is accepted, over the slots in index
  order. The first slot's guardian code carries the federation name.
  It runs DKG and seat-binding publication, and keeps its invite and metadata
  target in the creation record. MGs refuse the guardian code request once
  DKG starts, so the codes are cached per seat for a DKG restart until MGs
  return them in every state. Confirming the formation metadata ends the
  creation (see below).

`FaClient::run` is the only driver. The consumer spawns it once. It continues
the purchase, then the formation of a full pool. A formed or restored
federation gives the loop no work. User actions take the
run guard, which makes the loop yield, save their input, and wake the loop.
Failed work is retried with a growing delay up to five minutes, and its error
appears in the state. A step has no deadline: it ends when its work is done,
when a call fails, or when a user action wakes the loop. So the DKG wait, the
guardian-code wait, and the metadata readback wait as long as they must. Each
call has a fixed timeout: 30 s for an ordinary call, and 10 minutes for a
payment, its recovery, a refund, or a backup read. A timeout cancels the call
and leaves exact payment recovery intact. The library has no timing options.

## Consumer boundary

Consumers supply capabilities; they can never supply authenticated protocol
objects, trust conclusions, or lifecycle transitions:

- **Identity** — one stable consumer-scoped FA root. The library derives the
  protocol signer and FA-specific backup author/encryption families.
- **Storage** — an already-namespaced Fedimint `Database`; backend,
  namespacing and local encryption remain consumer concerns. The library owns
  the purpose-built encrypted Nostr recovery document and imports only its
  lean authenticated facts after it verifies them against consensus
  ([`SPEC-fa-backup-payload`](./SPEC-fa-backup-payload.md)).
- **Payments** — a wallet adapter that reports Ready federations and pays one
  verified quote at a time. Its funding operation is recoverable from the quote
  alone before it commits value. It reports insufficient funds only when the
  payer cannot cover that quote's current net cost and no operation for it
  exists. It returns `Prepared` only after the exact transaction is accepted
  and payer change is spendable. Recovery of a quote reports `NotStarted`,
  `Prepared`, or `Rejected`. It settles signed refunds retry-safely. Wallet-private refund material never enters FA storage.
- **Transport** — an iroh connector dialing a protocol `Locator`. Its
  availability and quote calls retain local RPC failures in a non-serializable
  outer call error; the inner result is the Manifold Guardian's wire-domain
  response. This prevents a remote service error from impersonating a definite
  local transport failure and gaining retry or reselection treatment. The
  selection walk's live availability probe reads through the same connector;
  read-only preview consumers add one with `with_mg_connector`.
- **PeerBadge trust** — a concrete shared `PeerBadgeVerifier`, constructed
  from the deployment's canonical issuer-identity roots, authority relays, and
  validated minimum trust level. Stateful consumers inject it at
  `FaClient::open`. Read-only consumers construct registry-only
  `MgRegistryQuery` for static discovery, then add a verifier with
  `with_verifier` to obtain `MgSelectionQuery` only when they need a verified
  preview. Discovery emits statically admitted publisher claims, while the
  selection walk consumes the verifier lazily and binds every verified badge
  subject to the advertisement's authenticated author and issuer claim; an
  authentic badge below the selected environment's minimum is not a verified
  candidate
  ([SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)).
- **Consensus read** — invite-code reads over real, uncached upstream queries.
  The metadata operation returns the downloaded client config and raw `meta`
  consensus value, never a verdict: the library derives the peer set, verifies
  every MG attestation, and performs equality checks itself. The separate
  LNv2 operation uses Fedimint's threshold response strategy and returns the
  individually valid public gateway endpoints in that aggregate; it omits an
  unrelated entry outside `GatewayApiUrl` policy rather than making that entry
  hide an exact valid target, while FA still decides target membership. This is
  the capability the library cannot check the honesty of — the upstream reads
  earn their guarantees from threshold peer queries and return no signatures,
  so an adapter that fabricates or caches a result defeats readback silently.
  The obligations are stated on the port and in [`SECURITY.md`](../SECURITY.md).
- **Presentation** — collects intent, renders an uncached advertisement-only
  preview and authenticated payer choices, retains the preview only for that
  screen visit, and invokes `create_federation` with it and its explicit
  payer, or without a payer when the deployment has no setup-payment policy.

`MgSelectionRequest` rejects invalid sizes and ranges, and
`validate_federation_name` rejects names `create_federation` would refuse. The
purchase APIs additionally expose side-effect-free, operation-specific preflights for
consumers: the purchase validates the preview's seat count, preview age, and
explicit payer.

Formation installs guardian-fee arrangement on the same state engine. Opening
with a `ManifoldEnvironmentProfile` supplies the setup-payment publisher and
deployment-pinned Guardian Verification Fee account; every constructor also
accepts a `FaFeeAccountProvider`. After DKG, the engine parses the federation id from the
persisted invite, resolves the FA's single-signature SPv2 `BtcDepositor`
account, loads guardian accounts from signed seat acceptances, and collects
MG attestations paired with their seat-endpoint proofs. It derives
account-keyed recipients from those attestations, refusing any shared
destination account. Before the first remote proposal it
persists the complete formation target: canonical-directory readback prediction,
paired attestation/proof request entries, FA account,
fixed recipients with FA at weight four, every guardian at weight one, and the
Guardian Verification Fee at weight one, plus the initial rate. It then
proposes that target as one base-bound metadata update. Every MG
constructs the same bounded canonical directory, verifies every paired proof
against final configured endpoint keys, and derives
the same recipients; FA waits for all three exact consensus fields and durably
checkpoints their exact readback. An interrupted proposal replays the persisted
target without re-resolving accounts or policy.
Formed state lives in one federation record: invite, seats, and name.
Confirming formation metadata writes it and deletes the creation, its slots,
and their cached codes in the same transaction. A fetched backup stays
in memory until its seats are verified against consensus; the import then
writes the record and makes the backup the last published generation. Backups and post-formation workflows read only this
record. Opening migrates storage written before the record existed.
A restored backup is verified once, before the import. Fresh threshold
consensus must match the saved federation identity, the verified directory,
every saved seat, and any saved liquidity commitment. This check needs no
live Manifold Guardian and does not repeat initial formation's all-seat health
and invite checks. A reopened federation is not checked again: its record was
written only after one of these checks, or with the confirmation of its
formation metadata.
If the saved invite is unreachable, the restore tries alternative invites from
the saved managers without waiting for every manager. It verifies the same
federation. After formation or a restore, the FA stores the guardian addresses
of the verified consensus as a read invite in its database, for later
liquidity and gateway reads. The read invite never enters a backup, and the
saved invite and committed request bytes are unchanged.

After formation, `propose_guardian_fees` changes only the rate through the generic
metadata verb. It does not resolve or resend recipient accounts. Directory and
recipient keys are formation-owned and generic writes carrying them revalidate
the stored signed directory and compiled split before voting.

## Public state and concurrency

`FaClient::state` and the `observe` watch expose one `FaState`: `Idle`;
`Restoring` while an opened restored-mnemonic FA has an unfinished backup
check; `Creating` with the formation id, name, size, DKG identity once bought
for, the picked slots with their per-seat progress, freshness, last error, and
one stage (`NeedsSeats` with the count of empty slots and any typed
reauthorization reason, `Buying`, or `Forming` with the formation phase and
invite); and `Active` for a formed or imported federation. The state is
derived from the stored slots; there is no separate purchase, pool, or
formation snapshot. The pool is the slots that hold a seat id. The run loop does the backup lookup as its first work and retries
failures; user actions fail as busy until it ends. The recovery requirement is
reconstructed on reopen from the caller's persisted mnemonic provenance and the
FA database's environment-scoped completion marker.

Formation phases are `PreparingDkg`, `DkgUnderway`, `DkgComplete`,
`PublishingSeatBindings`; a formed federation appears only as `Active`. State is
published through a watch channel independently of the run loop. Durable phases advance atomically
with their required recovery facts: `DkgComplete` saves every accepted seat's
quote, signed guardian-fee account, guardian setup code, and the common invite.
`PublishingSeatBindings` saves the exact directory and initial fee target before
submission; the formed state records its confirmation in consensus. Reopening preserves
these phases. Inconsistent storage fails closed before status is published.
The seat/fee-account pairing is
validated on load, so formation records persisted before signed fee-account
acceptance existed fail closed and must be reset rather than migrated because
they predate the production compatibility baseline. A stored seat quote is the
seat's authority for that quote's effect, and an accepted seat always holds
its quote. A paid quote whose wallet payment already
started is replayed exactly rather than refreshed, because its funds are
locked to that quote's blind nonces. Per-seat progress is
diagnostic and carries independent recovery facts. Public seat progress and
payment requirements expose the badge-vouched identity of their assigned MG,
and derive the two-word MG name from it locally
([ARCH-service-manifold-guardian](../../service-manifold-guardian/specs/ARCH-service-manifold-guardian.md)).
The identity is presentation material: the locator
remains the protocol-owned dialing and verification binding.

`cancel_creation` wipes the durable creation and purchase state under the run
guard while no seat is bought and no seat payment started. It asks the wallet
to recover every bound paid quote first. A `Prepared` payment refuses the
cancel with `AbandonUnavailable(PaymentOutputsStarted)`, because the run loop
must buy or refund that seat. Any accepted seat
refuses it with `AbandonUnavailable(SeatsBought)`: those seats stay with the
creation until its federation forms. Cancel contacts no MG, so free seats
already accepted server-side are forfeited, and the setup-payment policy
retention is preserved.

`decommission_seats` is a testing-only developer command sitting outside all of
that, not a second abandon. It reads the recorded formation, asks every MG
that accepted a seat to decommission it in one pass, and reports per seat
whether the MG decommissioned it, had already done so, or refused. It takes
no run guard and writes nothing, so durable FA state is unchanged and a
decommissioned federation still reads as formed. Seats are forfeited, never refunded,
and only development and staging MGs accept the underlying verb
([SPEC-fa-rpc](../../mg/specs/SPEC-fa-rpc.md)).

Formation operations return run futures; dropping one cancels local work only,
and reopening the same database, identity, and wallet then calling the
continuation API is the supported resume. Backup recovery and publication are
client-owned background tasks, separate from those formation runs. The
consumer opens at most one client for a database, and the client's in-memory
run guard serializes clones, the run loop, and user actions. A RocksDB-backed
database refuses a second open of the same path. A process restart ends the
old driver before persisted state is reopened. Schema 11 also kept a
database-wide driver lease; opening such an FA deletes it.
Exact durable operation recovery prevents a restarted consumer from submitting
a payment twice. Prepared payment work remains effect-free until it is polled. Parallel non-value seat
work uses narrow row updates so one completion cannot overwrite another seat's
recovery facts. The crate stays compatible with native mobile targets and
`wasm32-unknown-unknown`.

## Protocol ownership

The driver owns availability checks, signed-quote verification, per-seat
payment ordering, payment presentation, signed seat creation, DKG coordination,
status polling, and invite recovery (rejecting bearer API secrets and
requiring every MG's invite to identify the same federation before
persisting the deliverable). Every ordinary resume repeats the idempotent
`StartDkg` with the same durable guardian codes, so a crashed child starts a
fresh ceremony while an existing `DkgInProcess` ceremony keeps being polled.
A pool seat whose MG reports it decommissioned becomes an ended slot, and the
other cached codes go, while no seat can have started DKG. That holds while a
seat still lacks its cached code, because `StartDkg` follows only after every
code is cached, or while every other seat reports `New`. The FA paid for that
seat, so it is never dropped silently: the slot stays with phase `Ended`, the
FA logs a warning, and the creation needs a new seat with the `SeatEnded`
reason. Later previews exclude the ended slot's MG, service key, and holder,
as for every other slot. Ended slots go with the creation when the federation
forms.
Resume never selects the destructive `RestartDkg`: replacing a stuck ceremony
is an explicit, user-authorized action, and ordinary resume must not infer
that intent. Wire types,
envelopes, and the service trait come
from `crates/service-manifold-guardian` and must not be duplicated here; the
FA-facing verb contract is
[SPEC-fa-rpc](../../mg/specs/SPEC-fa-rpc.md) and the byte-level
authentication is
[SPEC-signed-envelopes](../../service-manifold-guardian/specs/SPEC-signed-envelopes.md).

## Post-formation metadata maintenance

`update_federation_metadata` exposes only the MG's compiled MVP policy as
fallibly constructed semantic values: display name, HTTP(S) icon URL, welcome
message/description, and a caller-supplied public HTTP(S) terms URL. The shared types
apply Guardianito's trim-for-validation/raw-for-submission rules and the
65,536-byte absolute raw resource cap before signing, connection, or
network effects; every MG repeats the same authoritative validation. The API
does not accept raw metadata keys, clear fields, upload image bytes, or widen
the MG allowlist. The operation requires
a formed federation record, reconstructs every FA-owned seat from
that record, and shares the formation driver's run guard, connector, identity, and real consensus-reader capabilities.

Fedi constructs `FederationMetadataUpdate::terms_of_service_url(url)` and passes
it to `FaClient::update_federation_metadata(update)` for both ready-made and
custom terms. The URL is stored
as `fedi:tos_url`; the result confirms fresh consensus adoption.

Consumers must rebuild for the value-bearing `TermsOfService` variant and the
removal of `GUARDIANITO_TERMS_OF_SERVICE_URL`. URL errors now use `InvalidUrl`
and `NonPublicUrlHost`, each carrying the field name. The signed `SetMetaField`
wire format and stored metadata format are unchanged; existing terms values
need no migration. Upgrade all participating MGs before enabling caller-chosen
URLs: older validators reject them, and a policy refusal can terminate the FA
operation even when other guardians could form a threshold. There is no
capability negotiation or fallback URL.

The complete consensus metadata object has a separate inclusive 1,048,576-byte
ceiling. The driver checks it immediately after every live read and returns a
typed terminal maintenance error before hashing, parsing, cloning, connecting,
signing, or fan-out. MG applies the same ceiling before parsing the current
object and before submitting the canonical target. Initial seat-binding
publication applies the same FA ceiling before hashing or fan-out and fails the
formation rather than amplifying an oversized pre-existing object.

The caller waits for a maintenance operation, so it alone keeps a limit: it
retries for at most 10 minutes, then returns `MaintenanceConvergence`.

Because the meta module votes on one opaque whole object, each retry reads the
live raw value, commits every FA-signed seat request to its exact
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
live MG and a threshold-live subset may finish while minority seats remain
offline.
Formation's initial metadata proposal runs through the same write loop in
`meta_write.rs`. Only its vote, target predicate, and error mapping differ:
any guardian refusal other than a stale base ends formation unless readback
shows adoption.

The MG's per-seat single-owner loop adds a live-occurrence pin: before that
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
wrong-state error before connector work; an intrinsic MG policy/lifecycle
refusal remains a typed terminal maintenance rejection; and retryable failures
that outlive the caller's bound return one maintenance-convergence error with
the unresolved seats and last sanitized consensus/guardian failures. A
concurrent winning change is therefore preserved when this mutation is rebased;
a single MG acknowledgement never masquerades as threshold adoption. The
operation itself stores no second mutable maintenance state: cancelling or
crashing is safe because a later identical request rereads consensus and either
observes success or replays the idempotent typed mutation.

## Modules

- `state` — public intent, status, and phase types.
- `ports` — consumer and transport capability traits.
- `db` — Fedimint-database recovery facts and checkpoints.
- `discovery` — read-only MG advertisement discovery and static
  admission.
- `selection` — ranked round-robin seat selection with lazy badge
  verification, and the read-only selection preview.
- `setup_payment_federations` — authenticated common-set refresh, durable
  high-water retention, and canonical selection.
- `formation` — formation, seat-by-seat purchase, exact per-quote recovery,
  seat leave reasons, cancel, and guardian-fee policy orchestration.
- `liquidity` — post-formation FLIP selection and signed allocation recovery,
  including exact-URL gateway attachment and its durable readback proof.
- `maintenance` — typed post-formation consensus maintenance orchestration.
- `unavailable` — deliberately unavailable adapters for consumer scaffolding.

## Testing

State-engine tests use fake ports to cover validation, default-name
persistence, per-seat quote binding, parallel narrow
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
typed priced-offer rejection, zero-balance admitted-payer listing, freshness
only until the first bound seat, re-binding of an unstarted paid quote, exact
replay of a prepared payment, the seat that leaves after rejection, refusal, or
insufficient funds, each accepted seat joining the pool at once, and the
cancel rules. The fa-cli's concrete reference FA payer adapter separately
covers exact foreign-output fee costing, quote-derived operation identity,
exact payment and change range recovery, unordered/duplicate rejected-input
refund recovery through spendable outputs, mint-v1 per-note refund fallback,
and terminal-proof enforcement. Its full funded-wallet path remains covered by the
paid `defe` E2E described below, which exports one large ecash note, reopens the
wallet to recover exact current operations without resubmission, and
independently reconciles setup prices, Fedimint transaction fees, and returned
spendable change. The
shared verifier's own behavior stays with
its focused crate tests. The `defe`
E2E split is intentional: a seven-MG free formation proves parallel ceremony
scale, and a separate seven-MG paid formation proves the real quote-bound
mint funding and claim path, including reuse of the same durable FA state and
wallet when a recoverable purchase failure requires one resume.
A third Linux `defe` E2E kills three real
`fedimintd` children after DKG starts, kills the FA process, reopens its durable
guardian-code preparation, and proves
that a fresh process repeats the idempotent `StartDkg` wave and resumes the
same seven-guardian formation; each killed guardian records its replacement
start while intact guardians may retain the first ceremony. The crash-recovery
process also retains one callback bearer and idempotency key across that FA
crash, then proves callback retry survives an MG restart and reaches
delivery on every guardian after an HTTP 500 endpoint recovers. The
paid E2E does not claim post-output or refund fault injection — exact recovery
and refund replay stay with the focused tests.

Maintenance tests split at the authorization boundary. The shared service and
MG suites own every Guardianito semantic boundary, compiled-key dispatch,
raw-resource rejection, whole-map preservation, and stale-base refusal before
child work. `fa-client` owns exact public-variant projection, pre-Formed and
oversized no-effect rejection, already-adopted/offline success, threshold-live
partial convergence, below-threshold bounded failure, transient consensus-read
retry, stale-wave rebase, cancellation-safe replay, and preservation of
unrelated consensus fields. Composition intentionally relies on the service and
MG suites for authoritative semantic rejection rather than duplicating those
validators in the client fake.

Liquidity tests use fake discovery, provider, MG, and consensus-reader ports
to cover durable completion replay before provider discovery, exact gateway-URL
proof invalidation, and signed completion-evidence validation. The
federation-preview unit test separately covers per-entry admission and
deduplication after upstream aggregation; module discovery, the live threshold
query, and network freshness remain that adapter's integration boundary. The
MG suite owns idempotent endpoint insertion, and the FA fake deliberately does
not stand in for either concrete boundary.
