# SPEC-peer-badge-verifier: shared PeerBadge authentication

## Record justification

FI, FLIP, push-gateway guardian telemetry, and the cloud FMan telemetry
collector must authenticate the same
credential and holder-authorization format. Keeping that security decision in one shared crate prevents relying
programs from acquiring subtly different authority, revocation, proof, or
schema rules. FMan verifies carriage integrity for its own HolderAuthorization
but does not judge its own badge.

This record governs `fedi-decentralized-peer-badge-verifier`. Environment
identity and public configuration are governed by
[SPEC-manifold-environment](../../manifold-environment/specs/SPEC-manifold-environment.md).
The credential and holder-authorization wire contract remains governed by
[SPEC-holder-trust-envelope](../../domain/specs/SPEC-holder-trust-envelope.md);
the authority and revocation Nostr event contract remains governed by
[SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md).

## Status

The shared-verifier contract is unconfirmed while the multi-relay FMan
transport amendment is consolidated. FMan's distinct advertisement and
authorization transport remains outside this record.

## Trust profile

The verifier's validated static configuration contains exactly:

- a non-empty set of issuer **identity** public keys;
- a schema-valid minimum PeerBadge trust level;
- a non-empty ordered set of issuer-authority relay URLs; and
- one **pinned authority** per committed authority document the profile
  carries (development and staging today; production pins none), admitted at
  construction.

Issuer identity keys are cryptographic trust roots in every environment. For
an issuer with a pinned authority, the issuance key and revocation locations
are additionally fixed at construction: the verifier parses the committed,
identity-signed `IssuerAuthority` document, verifies its proof, enforces the
revocation-location bound, and rejects construction when its identity is not
a configured root. Admitting the document involves no secret handling,
signing, or randomness. For every other issuer — production — issuance keys
and revocation locations come from a freshly fetched, identity-signed
`IssuerAuthority`; they are never pinned by the consuming application or
persisted by the verifier.

`ManifoldEnvironmentProfile` supplies typed issuer identities, canonical relay
routing, the schema-valid minimum trust level, and the committed authority
documents. The verifier copies those inputs into its validated non-empty or
bounded types. Development and staging issuer identities are conspicuously
marked known-secret placeholders; their complete secrets and the authority
documents signed with them are deliberately committed, and a
manifold-environment unit test enforces that each document, the committed
secret, and the canonical relays agree. Production supplies issuer-controlled
keys and commits neither secrets nor documents;
`PeerBadgeVerifier::try_from_profile` returns a typed
configuration error for any environment whose issuer list is empty or whose
committed document fails to parse, verify, respect bounds, or match a
configured root.

## Authority and revocation visibility

Authority replacements and revocations are not instantaneously or atomically
visible. Each completed relay read defines the state observed by that
verification. A concurrent or unpropagated publication affects that call only
when its response includes it; a later fresh verification may observe it
instead.

Under the relay assumptions below, a visible newer admitted authority
supersedes an older authority, and a visible matching valid revocation rejects
the credential. The verifier refetches revocation state on every call and
refetches authority state for unpinned issuers. It has no local authority or
revocation cache, high-water mark, or stale fallback. Consumers that need
stronger synchronization need a separate mechanism.

## Shared construction boundary

The environment adapter is the only default-feature public constructor;
relying production components cannot inject alternate issuer roots, relay
routing, or a minimum trust level. Construction validates roots, relay bounds,
and the schema range of the minimum once. The opaque verifier retains explicit
`PeerBadgeVerifierProvenance`, so composition roots can require the expected
source environment and profile revision.

The `test-support` feature exposes `PeerBadgeVerifier::new_for_test` solely for
defe-backed component tests that need ephemeral roots, relays, and an explicit
schema-valid minimum. Such values carry `ExplicitTestConfiguration` provenance
and can never report themselves as a canonical Manifold profile. Production
artifacts must not enable this feature or accept that provenance. FI receives the verifier directly in
`FiClient::open`; push-gateway telemetry and the cloud collector construct it from their selected
canonical profile; FLIP receives it in `run_daemon`, rejects non-matching
canonical provenance, and retains it in `DaemonContext`. FMan receives only
its resolved environment profile at `FleetManagerNostr::new` and does not
depend on this crate.

## Verification algorithm

Every call to `PeerBadgeVerifier::verify` performs the complete sequence
afresh:

1. Read the credential's issuer identity and reject it before network access
   unless it is in the configured root set.
2. Resolve the issuer's authority. A **pinned** issuer resolves to its
   construction-time authority with no relay lookup: no kind-37703 event —
   overwritten, missing, or malicious — can rotate its trust or deny
   verification. An **unpinned** issuer queries every configured authority
   relay for bounded kind-37703 candidates under the call's absolute deadline.
   Every relay must reach EOSE. Select the newest event across their combined
   results whose Nostr envelope has the exact author/kind/`d` role, then admit
   that selected authority only after verifying its JSON shape,
   issuer-identity match, and authority proof. An invalid newest authority
   fails closed rather than falling back to an older authority.
3. Derive the inline credential digest and read Nostr revocation locations only
   from that authenticated authority. Every supported Nostr location must
   return a complete EOSE result; an empty first location cannot hide a
   revocation at a later one. A missing supported location, unavailable or
   incomplete lookup, local truncation, excessive location count, or malformed
   matching revocation candidate fails closed. A valid issuer-signed
   revocation for the digest rejects the credential.
4. Install the admitted authority into a new credential-SDK
   `VerificationContext` and verify the complete `SignedCredential` plus
   `HolderAuthorization` at the requested time. This covers issuance proof,
   credential digest binding, holder signature, holder/blind-subject binding,
   and authorization timestamp.
5. Parse the credential as the typed `fedi-trust-score-v1.0` PeerBadge schema
   and reject an authentic badge whose `trust_level` is below the configured
   environment minimum.

Dropping verification cooperatively cancels its pending relay reads and leaves
no verifier state. The relay layer can briefly retain its private ephemeral
client in a detached best-effort unsubscribe task; that cleanup is not joined
by the caller's deadline and has no durable state or capability shared with a
later verification.

The verifier has no cache, database, persisted high-water mark, or stale
fallback. Revocation state — and, for unpinned issuers, authority state — is
fetched on every call under one shared nominal ten-second relay-I/O deadline;
this does not enforce a hard end-to-end duration bound. A pinned authority is
not a cache: it is construction-time configuration admitted from the
committed authority document, identical on every call, and revocation stays
fetched fresh against its locations. Authority relays, authenticated revocation locations,
candidate count, individual event bytes, and aggregate candidate bytes have
hard bounds. An unavailable or incomplete lookup produces a typed error rather
than reusing earlier state.

Success returns `VerifiedPeerBadge`, containing the issuer, holder, authorized
subject public key, credential digest, and typed badge. The call intentionally
has no `expected_subject` parameter: authenticating an envelope establishes
which subject the Holder authorized. A later complete advertisement verifier
must separately verify its Nostr event and require the returned subject to
equal that event's authenticated author before treating the advertisement as
belonging to the FMan or FLIP.

All canonical environments currently require `trust_level >= 9`, admitting
`9..=12` as Trusted-or-higher under the interpretation in
[REQ-fman-trusted-peer-badge](../../../specs/REQ-fman-trusted-peer-badge.md).
A lower schema-valid level produces the typed `InsufficientTrustLevel` error.
Explicit test verifiers
must also supply a schema-valid minimum and cannot silently bypass this policy.

## Relay assumption

The verifier requires every configured authority relay and every Nostr
revocation relay named by an authenticated authority to reach EOSE before its
absolute deadline. It combines candidates only from those complete responses.
An incomplete result fails verification; it is never evidence that no newer
authority or revocation exists. This deliberately favors omission resistance
over availability.

Configured authority relays are deployment trust configuration. They are
assumed to answer with the authority replacement visible to the relevant fresh
read and not roll an issuer back to an older authority after a component
restart. The verifier therefore does not persist historical authority state.

This currentness assumption does not make relay event content
cryptographically trusted. Every returned event and embedded signed object
still passes the complete admission checks above. Components must resolve the
same canonical environment profile, while each component owns the semantics of
its relay operations. FMan's own relay transport (pooled for its liveness
paths, single-relay for backup and restore) is unchanged by this verifier
contract.

An authenticated authority delegates negative-state completeness trust to
every Nostr revocation relay it names. Each such relay operator must honestly
retain and return the issuer's revocations visible to the relevant fresh read;
EOSE proves completion of the response, not operator honesty. This trust
follows the current signed authority so the issuer can replace a revocation
location without coordinated component releases. A compromised or mistakenly
chosen delegated relay can conceal a revocation until the issuer replaces it.

## Intentional omissions and change constraints

This record does not introduce `FiTrustConfig`.

Before changing all-relay completion to first-responder or quorum behavior,
accepting partial results, adding a hardcoded revocation-relay allowlist, or
persisting relay high-water marks, explicitly reassess the availability,
omission-resistance, and delegated-trust consequences.

## Verification coverage

Focused verifier tests use real credential-SDK issuance and signatures with
fixed test-only issuer keys. They prove typed success, behavioral authority
and revocation changes between calls, invalid-newest-authority rejection,
pre-network rejection of untrusted issuers, resource bounds, fail-closed
revocation absence/unavailability, signed revocation rejection, and
holder-authorization tamper rejection, below-minimum rejection, and acceptance
at and above the configured minimum. Nostr-client contract tests separately
prove that only EOSE completes a negative result, timeout/stream end/`CLOSED`
and cap exhaustion fail closed, and every revocation location contributes to
the combined result. The `defe`-backed
`holder_trust_badge_to_concrete_fi_selection_flow` component test publishes a
current authority, complete holder credentials/authorizations, and dialable
advertisements to a real relay, then adds the concrete verifier to
`FmanRegistryQuery` via `with_verifier` to obtain `FmanSelectionQuery`, and
checks the selected issuer, subject, and badge facts.
