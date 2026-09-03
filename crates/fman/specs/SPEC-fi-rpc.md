# SPEC-fi-rpc: Federation Initiator RPC boundary

## Status

Paid quoting checks the authenticated common setup-payment membership and quotes a configured gross price; minimum-net-revenue pricing remains unimplemented.

## Record justification

The FI RPC contract spans service wire types, daemon handlers, wallet payment flows, and FI consumers, so no single implementation artifact can own it coherently.

The Fleet Manager exposes the FI control plane over the service protocol's
Iroh ALPN. Its advertised or out-of-band locator supplies the endpoint and
the commitment-verification key. The payment mechanism is
[SPEC-locked-payment](./SPEC-locked-payment.md); the byte-level
authentication mechanism is
[SPEC-signed-envelopes](../../service-fleet-manager/specs/SPEC-signed-envelopes.md).
There are no **FMan seat or capacity reservations**: unsigned quotes allocate
nothing, and a seat is created only after payment verification. Before any
ecash output is authorized, however, the FI's own payer wallet durably holds
the exact aggregate quote set, fee-aware held debit allocations, and required balance
floor. That local crash-recovery journal is the payer reservation described in
[SPEC-locked-payment](./SPEC-locked-payment.md); it grants no FMan capacity.

## Verbs and authentication

`GetAvailability` and `GetQuote` are unsigned; both are advisory reads
that allocate nothing. `GetAvailability` reports whether the FMan is accepting
seats, the release-pinned fedimintd version and federation sizes, and current plans using the same projection whose boolean result gates advertising.
It is a boolean and not a count because the verb is unsigned and
unauthenticated, so any dialer could otherwise poll an operator's occupancy.
Each call independently snapshots settings and later reads live state, so
separate calls may observe different epochs or a different answer.
These sizes are release capabilities rather than the FI product's accepted
intent range. The FI admits sizes 7 through 20 inclusive, then requires every selected
FMan to advertise the requested size before requesting a quote from that FMan.
Concurrent siblings may already have supplied quotes when another fails this
check, but formation cannot proceed. FMan 0.1 advertises every size from 7
through 20 inclusive; 7, 10, and 13 are consumer presets rather than server
capability limits.
It reports no payment-federation list. `GetQuote` refuses when no slot is
available and reads capacity, the durable offer epoch, plans, and its
local payment-federation policy in one database read transaction.
Capacity is stored with the offer, not supplied at daemon start. The operator
cannot lower it below active non-decommissioned seats; a real capacity change
rotates the epoch and a no-op preserves it. It names a
plan and, for a paid plan, the FI's chosen payment federation. The FI selects
that federation from the common set in
[SPEC-locked-payment](./SPEC-locked-payment.md).
The signed quote carries price and — for a paid plan — the payment terms
(the chosen payment federation and the issuance set;
per note: denomination, blinded nonce, and for mintv2 the public
random tweak). The quote also binds that offer epoch. Nothing else travels: the FMan re-derives every payment
secret from its wallet root at redemption
([SPEC-locked-payment](./SPEC-locked-payment.md)). The mint generation
is not on the wire: the payment federation's modules decide it (mintv2
if present), and the FI reads the choice off the payment-terms
variant. The quote binds the requesting `fi_id`; the FMan stores
nothing. The quote's identity is the SHA-256 of the signed response
payload — the FI presents those exact bytes back in `CreateSeat`, so
both sides hash the same bytes without any canonical form.

The pending pricing contract is
settings provide `minimum_net_revenue`, and the signed paid quote binds
that value, the smallest representable `gross_price` satisfying its
settlement policy, and the policy version. The issuance total equals
`gross_price`. The current wire and implementation still carry only a
configured gross price; this explicit divergence remains until the
settings, quote type, and planner are updated together.

`CreateSeat` is FI-signed and is the only allocating verb. It presents a quote
plus the aggregated blinded signatures paying for it — empty for a free quote,
which priced nothing. The mint protocol is read from the quote's signed payment
terms rather than restated by the presentation. Refund issuance and its
FI-chosen derivation nonce are in the signed quote request. It returns a signed commitment: either an accepted
seat — minting the seat id used by every later verb and committing the FMan's
full single-signature `BtcDepositor` guardian-fee account in that same signed
acceptance — or a refusal
carrying the signed refund transaction. Free-plan quotes present no payment and their
refusals carry no refund. `CreateSeat` is idempotent on the quote: replays
re-sign the same acceptance and refusals are recomputed from the quote offer
epoch, so neither commitment nor refusal is durable state. Idempotency is
semantic, not byte-level — the payload repeats, the signature does not — so an
FI verifies a replayed commitment instead of comparing it to the first. It
persists the seat id and guardian-fee account atomically before DKG; those
signed accepted-seat facts are the fee producer's guardian-recipient source.

All seat-scoped requests after `CreateSeat` are FI-signed. The formation
surface is `GetDkgCode`, `StartDkg`, `RestartDkg`, `GetStatus`, and
`GetInviteCode` ([SPEC-seat-lifecycle](./SPEC-seat-lifecycle.md)).

`GetDkgCode` accepts only an optional leader federation name. Guardian display
names and leader-info/module-config structures are absent; the guardian name is
`fm-` followed by the first eight hexadecimal characters of the seat id. The
response is deterministic from the request and seat-derived public material
and causes no database or child mutation.

`StartDkg` supplies the complete guardian-code set and an optional completion
callback. Unknown fields are rejected. Validation, including byte-for-byte
recomputation of this seat's code, precedes the child request. Success means the
wire `DkgStarted` event was observed. No exact-set intent, request replay, or
interruption marker exists.

`RestartDkg` is the explicitly destructive retry verb. It stops and reaps the
current child, starts its replacement, and reads the replacement's `Hello`. A
`NeedsParams` replacement validates the supplied complete code set and starts a
fresh ceremony; `AlreadyConfigured` records the invite and returns `Running`
without starting another ceremony. Its response exposes that completion race
so the FI can decommission and replace the seat. Restart has no callback field:
the first `StartDkg` choice remains fixed for the formation. It accepts
`New` and `DkgInProcess`, while `StartDkg` continues to refuse an
in-flight ceremony.

Restart discards only the replaced child's staging state and never removes the
final data directory. Running, `DataLoss`, and
decommissioned seats refuse it. Standalone cancellation is deliberately absent;
operator `Decommission` is the only release path. The lifecycle status set is
`New`, `DkgInProcess`, `Running` (with the durable invite), `DataLoss`,
and `Decommissioned`. `GetStatus` reads a watchdog-maintained health
snapshot plus an inline final-directory stat and never contacts the child.
`GetInviteCode` reads only the durable formed record and therefore also works
while the child is unavailable. A formed child's `Hello` is the sole repair
path for a missing record; no FI read fetches or records an invite.

When present, `DkgCompletionCallback` is atomically installed in the dedicated
`completion_callbacks` row only after the seat has proved its existing child is
an idle `NeedsParams` child, and before the child request. The first `StartDkg`
choice is retained for the formation; `RestartDkg` carries no callback and
leaves it unchanged. The
callback URL is a bearer push-gateway hook and
the idempotency key is stable for formation. FMan accepts it only when the URL
is an exact `/hooks/{hook_id}/{hook_secret}` path under its configured gateway
origin. Delivery begins only after the formed record exists. Existing bounded
retry, operator-blocked, terminalization, idempotency, and bearer-clearing
semantics are unchanged. Delivery is owned by one fleet-wide relational worker:
it selects only callback rows with a formed row and no decommission row. The
FI's shared formation idempotency key deduplicates retries and other guardians
addressing the same hook. Delivery does not depend on a live ceremony, seat
process, or FI polling.

`GetPeerAttestation`, `ProposeFormationMeta`, `SetMetaField`, and
`RegisterGateway` are FI-signed and seat-scoped. All require a formed record
and a healthy watchdog snapshot, then make only the substantive API call the
verb needs; they never issue a request-scoped liveness probe. A stale
unavailable snapshot can refuse a recovered child for at most the watchdog's
five-second retry cadence. Conversely, if a healthy child fails after the
snapshot, the substantive call surfaces the normal child-unavailable error.
The first two contracts are
[SPEC-federation-trust-directory](../../domain/specs/SPEC-federation-trust-directory.md).

`GetPeerAttestation` returns a `FmanPeerAttestation` signed by the FMan's
service Nostr key — the same identity that authors its kind-37701
advertisement, because verifiers resolve an FMan's live trust by looking the
advertisement up by that key. The peer id it binds comes from the invite code
the seat's own `fedimintd` issues, which names that guardian's peer; the
federation id, config hash, and guardian identity are the shared `domain`
derivations over the config, never re-derived here. The statement also commits
to the mnemonic/seat-derived guardian-fee account returned in the earlier
signed acceptance; the FI must compare the two before publication. It stays a
diagnostic/recovery read for the FI: it is not FLIP's trust source, which
reads the directory from consensus metadata.

`GetPeerAttestation` pairs the FMan-signed attestation with an Ed25519
`SeatEndpointProof` made by this seat's configured API endpoint key over the
attestation statement digest. `ProposeFormationMeta` carries the complete
structural set of attestations paired with their endpoint proofs; each FMan
constructs the bounded canonical directory and verifies one paired proof per
final-config peer endpoint before voting. Guardian codes themselves are bare
upstream setup codes: the DKG peer handshake authenticates peer endpoint keys,
while `SeatEndpointProof` independently binds each post-DKG account attestation
to the final config. No DKG transcript is carried by `SetMetaField`.

`SetMetaField` relays a metadata proposal only for a key in a compiled,
typed validator set; an unregistered key is refused as `MetaKeyRefused`
without reaching `fedimintd`, and a registered key whose raw value exceeds its
per-key cap is likewise refused before any child probe. A bounded value that
fails its semantic validator is `MetaValueInvalid`. The set contains the guardian-fee rate and the narrow Guardianito-compatible
FI maintenance keys documented in
[SPEC-fi-metadata-maintenance](./SPEC-fi-metadata-maintenance.md). The formation-owned trust-directory and fee-recipient keys are refused by this
generic verb.

The request commits to the exact consensus merge occurrence (`Absent`, or a
domain-separated SHA-256 over the meta module's monotone consensus revision
and the raw value from the same read — so byte-identical content readopted
later is a different base). The FMan reads its own current object and returns
the retryable
`MetaConsensusChanged` without submitting when the base is stale. A base this
process already admitted for a different whole-object target returns the
distinct `MetaTargetConflict` instead: rereading cannot clear it, and the
caller must wait for consensus to move or the process to restart
([SPEC-fi-metadata-maintenance](./SPEC-fi-metadata-maintenance.md)).
Otherwise it
replaces exactly the validated string field, preserves all other fields,
canonicalizes the whole object, and submits it with the seat's admin auth.
**Success means submitted, not live:** the module promotes a value only once
threshold guardians submit byte-identical bytes, so the FI serializes
all whole-object operations—including guardian-fee proposals—and confirms each
write by reading consensus back, never from this response.

`ProposeFormationMeta` is the only verb that can install the trust directory
and guardian-fee recipient list. Its FI signature covers the exact merge base,
paired attestation/proof entries, FI fee account, initial rate,
timestamp, FI id, and seat id. FMan checks the rate floor before child access,
refuses an already-consensus directory as `FormationMetaAlreadyPublished`,
verifies the constructed directory and paired proofs against the final config,
and checks its own seat identity. It then derives the fixed recipient list—FI
at weight four, every guardian at weight one, and the Guardian Verification Fee
at weight one—and submits all three formation fields as one guarded target.
Success means submitted, not live.

After formation, `SetMetaField` may change only the guardian-fee rate. Every
whole-object generic vote carrying fee fields revalidates the fixed recipient
policy from the stored signed directory and live config. Endpoint proofs are
not stored in the directory and are not rechecked during maintenance.

`RegisterGateway` carries the shared canonical `GatewayApiUrl` returned in
signed FLIP completion evidence. The type accepts only public HTTPS or
identity-shaped Iroh endpoints and rejects credentials, query strings,
fragments, private/loopback hosts, and unsupported transports before a request
can be signed or decoded. After the normal envelope and seat-owner checks, FMan
requires the guardian to be consensus-running and calls its admin-authenticated
LNv2 `add_gateway` endpoint. It stores the URL; it does not probe it or claim
independent reachability/trust validation. The boolean response distinguishes a
new insertion from an idempotent replay.

The unsigned `GetFederationTrustMaterial` serves this FMan's signed trust
material for one federation: the peer attestations for every seat it runs
there, the Holder authorizations durably enrolled through the operator flow,
and its current `iroh://` endpoint, signed by the service Nostr key that also signs
the kind-37701 advertisement and every peer attestation. It is unauthenticated
because the material is what any verifier holding an invite code is meant to
be able to fetch, so there is no requester to authorize and no seat the caller
names — the request names a federation instead. An FMan running no seat in
that federation answers with an empty attestation list rather than an error,
since that is a fact about the federation and not a failure to serve. Its
response carries an issue and expiry timestamp and the relying verifier
applies its own upper bound on that window. It also performs fresh issuer-policy
and revocation checks over every retained envelope. Before a Nostr relay is
configured the FMan cannot enroll a Holder authorization, so the verb
answers `UnsupportedVerb` rather than an empty document, which would let a
verifier read "not participating" as "participating but untrusted".

The protocol also declares `GetFedimintStats`; this daemon profile returns
`UnsupportedVerb` for it. For each signed seat-scoped verb it first verifies
the envelope and checks seat ownership; a missing or wrong-owner seat
therefore remains `UnknownSeat`, while an owned seat reaches
`UnsupportedVerb`. Unsupported operations do not create an authentication
bypass, signature oracle, or seat-existence oracle.

The signed commitment response roster is `GetQuote` and `CreateSeat`.

## Boundary validation and policy

An incoming signed envelope carries its FI key and signature as parsed
types (a malformed key or signature fails envelope deserialization and
never constructs). Verification then proceeds: verify the signature over
the exact received payload bytes, parse the typed payload, require its
inner `fi_id` to equal the envelope `fi_id`, then require its timestamp
to be within one hour of the daemon clock. Only a validated request
reaches fleet behavior. There is no nonce or replay table; within-window
duplicates rely on verb idempotency and state checks.

The implementation keeps that ordering easy to verify.
`SignedRequest::verify` is the sole constructor of `VerifiedFiRequest<T>`, so
no handler runs on an unauthenticated payload. Request types that target an
existing seat implement `SeatScopedFiRequest`, and `Fleet::authorize` — the
fleet's only crate-visible seat-selection path — takes such a verified
request and returns the seat only after the ownership comparison. Every
existing-seat verb calls it as its first fleet call, and no FI verb calls an
operator verb (listing, decommission, shutdown); the service module is the
entire FI surface and is kept small and scannable for those two facts.
Adding a wire verb without a trait method fails compilation.

`GetQuote` applies release policy before quoting: the requested
fedimintd version must equal the release's sole supported version, the
federation size must be between 7 and 20 inclusive, and the selected plan must
exactly match a currently offered plan. For a paid plan, the named payment
federation must be a member of the accepted common setup-payment set — read
from the same database snapshot as the offer epoch, so a quote can never
outlive a removal without an epoch change — and its wallet must already be
joined. A federation outside the accepted set fails with
`PaymentFederationNotAccepted`; an accepted federation whose local client is
still joining or unavailable fails with the retryable
`PaymentFederationUnavailable`.

`CreateSeat` verifies the echoed manager signature, re-checks quote coherence,
and verifies payment offline (including distinct finalized note nonces), then
reads the accepted quote index and epoch from one SQLite snapshot. An existing
seat or already-stale absent quote completes from that snapshot. An absent,
current candidate asks one immediate transaction to recheck both facts before
accepting, so a committed acceptance always wins over a later settings change.
A current live quote is asserted to have capacity; the seat insert and
last-slot epoch replacement commit atomically
([SPEC-locked-payment](./SPEC-locked-payment.md)). The current offer is
not consulted separately: equality of the epoch proves the quote settings
are unchanged. A quote binds the `fi_id` it was issued to; presented
under any other signer it fails as `InvalidPayment` — the quote is
simply not valid payment material for that identity.

Before any seat-specific policy result or fleet DKG behavior, lookup enforces
the seat identity rule: the signed request's `fi_id` must equal the identity
that created the seat. Both a missing seat and an identity mismatch appear as
`UnknownSeat`, avoiding an ownership oracle. Only the successful comparison
inside `Fleet::authorize` yields the seat; handlers have no other
seat-selection path. After the ownership check, federation display names must be 1..=128
UTF-8 bytes, contain at least one non-whitespace character, and contain no
Unicode control characters; invalid names return `InvalidDkgInput` before
storage or child calls. The raw wire wrappers remain unvalidated so a malformed
authenticated request receives that typed policy error rather than failing
transport decoding. The live/decommissioned-state gate follows policy
validation, preserving policy errors for an owned seat without exposing them to
another FI.

## Errors and information disclosure

Expected policy and lifecycle failures retain typed wire errors,
including `UnsupportedVersion`, `UnsupportedFederationSize`,
`PlanNotOffered`, `PaymentFederationNotAccepted` (both at `GetQuote` only),
and retryable `PaymentFederationUnavailable`,
`InvalidPayment`, `UnknownSeat`, `WrongState`,
`SeatUnavailable`, `InvalidDkgInput`, and `FederationIsRunning`.
A refusal of a paid presentation is not an error: it is a signed `CreateSeat`
refusal carrying the refund transaction, and `OfferChanged` — a quote presented
under a superseded offer epoch — is the only reason one is issued.

Every envelope failure is logged with its internal authentication reason
but is returned only as `Unauthorized`. Unexpected fleet, database,
signing, or child errors are logged with their cause chain but returned
only as `Other("internal error")`. Callers must not receive sensitive
internal detail. `InvalidDkgInput` carries locally-authored text plus,
when fedimintd rejects a peer setup code, fedimintd's rejection detail —
the FI supplied the rejected input and fedimintd response text carries no
daemon secret content under the `fedimintd` content-discretion assumption in
[CLAIM-fleet-manager-confines-secret-dependent-content](CLAIM-fleet-manager-confines-secret-dependent-content.md).

Client-side RPC transport failures are not Fleet Manager errors and have no
serialized wire variant. A consumer that needs to distinguish definite local
stream failure from a remote service response retains the generated client's
outer RPC result in its connector adapter. In particular, FI retry and
reselection policy must never be triggered by an FMan-authored service error.
