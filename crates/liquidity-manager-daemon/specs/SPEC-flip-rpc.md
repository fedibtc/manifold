# SPEC-flip-rpc: FLIP Public Liquidity API

## Status

Requester actor-binding is not yet enforced: the server verifies each request
signature against its declared `requester_pubkey`, but no rule yet binds that
key to the authenticated iroh transport actor (`auth.rs` reports this as a
tracked open item).

## Record justification

The contract spans the `service-liquidity-manager` wire types, the daemon's
acceptance and status logic, and the `fi-client` and Fedi-app consumers, so no
single implementation artifact can own it coherently.

The Public Liquidity API is the app-facing surface advertised over Nostr
([SPEC-flip-advertisement](./SPEC-flip-advertisement.md)). Transport is
`fedi-iroh-rpc` under ALPN `fedi/flip/public-liquidity/1`: one bidirectional
stream per call, one version-1 request frame, one bounded response frame.
Request and response payloads are signed over canonical CBOR bytes with
per-verb domains
([SPEC-flip-canonical-payloads](../../service-liquidity-manager/specs/SPEC-flip-canonical-payloads.md));
the declared author identity (`requester_pubkey` on requests,
`provider_pubkey` on responses) must be bound to the authenticated transport
actor by the transport profile — author keys are secp256k1 while
authenticated iroh node IDs are Ed25519, so the binding is a profile rule,
never key equality. Provider-side, the binding is that the requester reaches
the iroh node ID carried in the provider-signed advertisement
([SPEC-flip-advertisement](./SPEC-flip-advertisement.md)) and verifies
responses against that advertisement's `provider_pubkey`. A requester-side
binding rule is still pending (see Status); violations of an applicable
binding must be rejected before any allocation is created.

## Verbs

- `GetProviderInfo` — optional preflight repeating advertisement-derived
  state (sources, policy, endpoint id, negotiated API version) bound to
  `advertisement_hash`. It reveals no private details and may be skipped.
- `RequestLiquidity` — the acceptance boundary. The request carries the
  bounds (`gateway_min/max_amount`, `stability_min/max_amount`), the private
  `federation_details` (invite code, federation id/name/config hash, optional
  non-authoritative `fleet_seat_hints` and `revocation_locations` hints), the
  `fman_endorsement` admission gate, the `fman_trust_material` for every FMan
  identity operating the federation, the `details_payload_hash` commitment,
  and `expires_at`. The endorsement and trust material are each `Option` on
  the wire only so a request without one still deserializes and is answered
  with a signed `invalid_credentials` rejection; absence is never a bypass.
  Both are excluded from the commitment, because both are collected per
  attempt: a retry carrying freshly collected ones must resolve to the same
  allocation rather than read as a conflicting request.
- `GetAllocationStatus` — status polling for an accepted request, keyed by
  the caller's `requester_pubkey` plus `details_payload_hash`.

## Request shape and acceptance

A source type is requested only when its minimum is non-zero; at least one
minimum must be non-zero; a present maximum is an explicit cap and must be
`>=` the minimum; maximums must be absent for unrequested source types.
Requested source types are all-or-nothing: a request commits at most one
gateway/LN item and at most one stability-pool item, and if any requested
source cannot be fulfilled within bounds the whole request is rejected.
Committed amounts may exceed the minimum within the cap. Unrequested source
types must not appear in the accepted allocation.

`outcome = accepted` means the provider has already durably created
restart-recoverable allocation work for every committed item — never merely
parsed the request. `outcome = rejected` carries one machine-readable code
from the closed `PublicRejectionCode` set (`crates/service-liquidity-manager/src/public.rs`),
with these boundaries:

- structural/bounds problems: `invalid_amount_bounds`,
  `invalid_details_payload`, `request_expired`, `version_unsupported`,
  `unsupported_network`, `unsupported_source_type`; endpoint transport-policy
  refusals use `invalid_details_payload` with a sanitized reason rather than a
  new wire code
- trust failures: `invalid_credentials`, `invalid_seat_binding`,
  `policy_mismatch` (see
  [SPEC-flip-federation-trust](./SPEC-flip-federation-trust.md) for the exact
  mapping)
- dependency outage, including a required revocation lookup that cannot
  complete: `provider_unavailable`
- valid but uncommittable from available funds/capacity:
  `insufficient_capacity`
- existing allocation with different details, where the caller may be told the
  allocation exists: `request_conflict` (see [Idempotency](#idempotency) for
  which callers those are)
- unexpected provider-side failure after validation starts: `internal_error`

The admission gate locally authenticates the federation id encoded by the invite,
then charges its per-federation verification allowance before its first outbound
revocation lookup. The allowance never uses
the requester-declared `federation_id`, which remains a hint until the preview
joins it to the invite-derived identity.

## Idempotency

The federation is the allocation's identity: `federation_id` is the primary
key for allocation state, at most one allocation exists per federation at a time,
and there is no separate request id. `details_payload_hash` identifies the
request content. Idempotency is semantic, not byte replay — every response is
a freshly signed statement about current state:

- a repeat **from the allocation's own requester** whose federation already has
  an allocation with the same `details_payload_hash` is answered from that
  allocation's current state as `accepted`, without re-running verification
  (also after the original request's expiry);
- a different `details_payload_hash` **from that same requester** for the same
  federation is `request_conflict`;
- a caller that is **not** the allocation's requester is never answered from the
  allocation fast path, whatever `details_payload_hash` it supplies. Its request
  falls through to validation and verification. An endorsement-less caller gets
  an allocation-independent signed semantic response; timing is not part of this
  contract. Admission now authenticates the invite-derived federation id before the
  per-federation verification budget is charged, so an endorsement-less caller
  cannot probe that budget. A caller that then passes
  verification for that federation holds a valid unrevoked FMan endorsement for
  it, and may therefore learn that the federation has an allocation;
- rejections are stateless — nothing is persisted, and a retried rejected
  request is re-evaluated from scratch;
- concurrent duplicates create at most one allocation.

The supported live-restore path preserves accepted allocation identity: it refuses an
archive that omits or replaces an allocation accepted by the running generation.
An exact replay after that refusal is answered from the unchanged allocation
rather than accepted as new work. Fresh-host disaster recovery has no live newer
generation against which to establish this cross-generation property.

## Endorsement authorization and allocation ownership

Possession of a valid, unrevoked `fman_endorsement` naming a federation
authorizes a liquidity request for that federation. The endorsement is a public
bearer capability: it names neither a requester nor an expiry, and FLIP keeps no
requester allowlist, quota, or payment record. Adding requester binding or
expiry would require a separate credential or a change to the canonical signed
statement.

This model accepts that anyone with the federation invite and a self-generated
requester key can obtain the public endorsement and race for the allocation.
One allocation per federation bounds the exposure, but cannot identify the
legitimate FI. A verified requester may atomically replace another requester's
idle allocation; an authenticated operator may release an idle allocation with
an audited reason. Both paths require no in-flight item, delivered value, or
active wallet operation. They never cancel work or move capacity. Once an
allocation holds work or value, a different requester receives
`request_conflict`; remediation then uses the separate cancellation controls.

The requester-binding work in this record's Status concerns the authenticated
transport actor. It does not change the endorsement's bearer-capability model.

## Status and completion

There is deliberately no overall allocation status: consumers inspect each
item independently. Item states are `pending`, `running`, `action_required`
(retryable/cancellable, never selected by workers), and terminal `completed`,
`failed`, `cancelled`. There is no public app-side cancellation call.

Status lookup failures are service errors, not signed rejections: no
allocation under the caller's `requester_pubkey` + `details_payload_hash` is
`not_found` regardless of whether the federation has an allocation under
other keys (allocation existence is not exposed); actor mismatch is
`permission_denied`; malformed requests are `invalid_argument`; recovery or
dependency outage is `unavailable`.

Completion is a provider claim plus evidence hints (fulfilled amount,
observed gateway or stability-pool state, operation ids/txids when known),
returned only after the provider observed the committed item in place. The
gateway evidence additionally carries the shared canonical client-facing
`GatewayApiUrl`. FLIP derives it from gatewayd's own registrations, preferring
Iroh and otherwise accepting public HTTPS; it never publishes the admin URL.
Gateway readiness fails when gatewayd has no suitable registration, so an
allocation cannot be advertised and funded without an endpoint FI can later
register with guardians. The app verifies completion independently through its
own federation access;
app-side verification failure is not a FLIP protocol state. Completion
evidence must never include invite codes, credential bundles, or other
private federation details.
