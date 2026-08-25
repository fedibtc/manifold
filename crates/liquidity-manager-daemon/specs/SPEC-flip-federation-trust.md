# SPEC-flip-federation-trust: Federation eligibility verification

## Record justification

The eligibility contract spans the shared `domain` attestation types, FMan
attestation issuance and metadata writes, the FI's post-DKG directory write,
this daemon's verification pipeline, and app-side gating, so no single
implementation artifact can own it coherently.

## Single verification profile

Every deployment runs the same authentication and trust pipeline; there is no
reduced-verification development mode in the acceptance path. Exactly one
external trust input is substitutable by boot-time opt-in fixtures
(`--trust-fixtures`, refused for Bitcoin mainnet): the invite-code federation
preview. FMan trust material is never substitutable — it arrives signed inside
the request and is verified in full — and the revocation fetch is always the
real relay path. Trust evaluation is local and fail-closed: signatures verify
against configured trusted issuer authorities, revocation lookups run fresh at
verification time, and a required lookup that cannot complete makes the
provider unavailable — there is no stale soft-pass.

## Seat bindings

A credential proves an attester trusts an FMan identity, not that the
identity operates this federation's peers. The binding contract — the
`FmanPeerAttestation` proof, the canonical `fedi:fman_seat_bindings`
container, the FI write/readback, and the generic verifier algorithm — is
[SPEC-federation-trust-directory](../../domain/specs/SPEC-federation-trust-directory.md).

FLIP applies it per request: preview the invite code for the authoritative
federation identity, config hash, peer set, network, and consensus threshold;
verify and match every seat binding to the preview; then, for each distinct
`fman_pubkey` the *directory* names, resolve that identity's trust material
from the request and verify its envelopes, issuer authority, and revocations.

The requester carries the trust material but does not author it, and the
ordering above is what makes that safe: the directory decides which identities
exist, each material document is signed by the identity it describes, and a
document is only consulted for that identity. The requester therefore controls
only whether an identity is answered for — and an unanswered identity is
untrusted. Material for an identity the directory does not name is ignored
rather than refused, since it can never be consulted; two documents for one
identity are refused, because resolving them by position would let list order
decide a trust outcome. Each document's own peer attestations are cross-checked
against the directory and must not contradict it. FLIP bounds the accepted
`expires_at - issued_at` window, which is the only remaining bound on how long
a withdrawn FMan's material outlives it.

The requester also embeds one guardian's `fman_endorsement` (attestation plus
holder-authorization envelope) in `RequestLiquidity`; FLIP verifies the
endorsement as an admission gate before previewing, but it is never
authoritative for per-guardian policy. Holding an endorsement is what
authorizes the request: the gate binds nothing to the requester and applies no
freshness window, as specified in
[SPEC-flip-rpc](./SPEC-flip-rpc.md). Request-supplied `fleet_seat_hints` and
`revocation_locations` are non-authoritative hints. Verification fails on any
binding that does not verify against the preview, any config peer without a
binding, extra bindings for non-peers, or missing/malformed/non-canonical
metadata. One FMan may operate multiple peers.

## Request-carried trust material

FLIP resolves every directory-selected FMan identity's standing from
FMan-signed trust material carried inside `RequestLiquidity`; the request path
does not look up standing over Nostr. Missing material, or missing material for
any identity selected by the directory, fails closed. Every document has a
bounded `expires_at - issued_at` interval, and revocation is resolved live at
verification time.

Carriage is not authorship. The seat-binding directory chooses the identities,
each document is signed by the identity it describes, and verification consults
it only for that identity. The requester can omit an identity and be rejected,
but cannot add an operator, substitute one operator's standing for another's, or
forge standing. Material remains replayable until its bounded expiry, and an
FMan that stops serving can remain trusted until then; live revocation can
withdraw it earlier.

## Policy evaluation

Each `accepted_attester_policies` entry (attester pubkey plus
`verification_requirement`) is evaluated independently over the **distinct**
`fman_pubkey` identities from accepted peer bindings; the federation is
eligible if at least one entry is satisfied. An identity is trusted for an
entry only when an envelope from its verified trust material validates against
that entry's attester, is unrevoked, and is unexpired. `all_trusted` requires
every distinct operating identity trusted; `consensus_majority_trusted`
requires at least the consensus-majority threshold derived from the final
peer set, counting each identity once regardless of how many peers it
operates. If the threshold cannot be determined, the request is rejected.

An empty policy list can satisfy no federation and is invalid provider setup;
FLIP must not reach Ready or publish an advertisement until at least one
accepted attester policy is configured.

After SDK verification, FLIP still applies its application checks: schema
`fedi-trust-score-v1.0`, subject binding, and the selected environment's
validated minimum trust level (currently `9`, admitting Trusted-or-higher).
The same shared domain `PeerBadgeTrustPolicy` governs both the request
endorsement and every per-operator envelope; the SDK does not decide policy
semantics.

## Invite endpoint transport policy

The endorsement authenticates the federation id, not the guardian API URLs in
the requester-carried invite. The production preview provider therefore checks
every advertised peer endpoint before it dials, and FLIP's target-client join
and gateway-attach paths repeat the check before their later independent dials.
One valid endpoint never blesses another peer in the same invite.

WebSocket endpoints remain location-bearing input. The pinned connector resolves
and follows redirects internally, so a check outside it cannot bind an address
verdict to the final socket. The production `GlobalOnly` policy therefore
rejects every `ws` and `wss` endpoint before DNS or connector work. This narrows
compatibility to Manifold's intended production Iroh transport rather than
retaining a resolve-then-dial gap. The explicit `AllowPrivate` operator setting
remains the local/non-mainnet harness escape hatch documented by the
endpoint-policy claim; it is the only policy that admits WebSockets. The
mainnet refusal currently runs only at generation startup: applying mainnet
setup through Admin to an already-running unconfigured generation started with
the allowance retains it. That live-apply exception remains outside the
supported production envelope.

Iroh guardian endpoints have a different boundary: the URL names an
authenticated Ed25519 endpoint identity rather than a socket address. FLIP
accepts only Fedimint's canonical `iroh://<node-id>` form, where `node-id` is a
valid lowercase-hex Iroh endpoint id and the URL has no credentials, port,
path, query, or fragment. The pinned Fedimint connector consumes only that id
and obtains its transport addressing through the configured Iroh discovery
stack or operator connector overrides. Alternate encodings and any additional
URL material fail closed so a future connector cannot silently give meaning to
fields this release ignored. Discovery- and override-provided addresses are a
separate pinned dependency boundary; accepting an Iroh locator is not a claim
that any resulting address is globally routable. A requester with a valid,
unrevoked endorsement can use an Iroh node id it owns and publish a direct
address or relay URL for that node. Before Iroh authenticates the node end to
end, the pinned transport may send its encrypted discovery Ping and QUIC
handshake to that UDP address, or open a relay TCP connection and send a TLS
ClientHello or the fixed empty `GET /relay` upgrade request. The published
relay name appears in SNI and `Host`, and a successful handshake may continue
with protocol-generated client and relay frames. The requester cannot select
an arbitrary method, path, body, header set, or subsequent frame contents,
receive the destination response, or use FLIP as a general-purpose proxy. FLIP developers
accept this constrained blind-reachability residual.
Deployments where FLIP can reach sensitive internal services should enforce an
egress boundary outside the process.

## Rejection mapping

An identity the request carries no trust material for is untrusted;
structurally valid but insufficient distinct trusted identities are
`policy_mismatch`. Malformed, unverifiable, revoked, expired, below-minimum,
wrong-subject, or wrong-federation credential material is
`invalid_credentials`, as are a
request carrying no trust material at all, duplicate documents for one
identity, and material contradicting the directory. Seat-binding or config
mismatch, missing or extra bindings, or malformed metadata is
`invalid_seat_binding` (or `invalid_details_payload` for the details
themselves). An invite endpoint refused by the transport policy is also
`invalid_details_payload`, with the sanitized reason `invite endpoint rejected
by transport policy`; the public response does not expose the endpoint or
FLIP's resolver result. A required revocation lookup that cannot complete,
including an installed issuer authority with no Nostr revocation location or
unreachable relays, is `provider_unavailable`. A failed admission gate rejects
without previewing.
