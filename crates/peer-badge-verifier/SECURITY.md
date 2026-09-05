# PeerBadge verifier security and reliability boundaries

`fedi-decentralized-peer-badge-verifier` is an async library used by FI, FLIP,
and push-gateway guardian telemetry. FMan presents its own HolderAuthorization but does not evaluate its own
badge. The verifier receives untrusted holder-authorization envelopes and
untrusted Nostr events, performs network lookups, and returns trusted typed
facts only after complete cryptographic and schema admission.

The environment crate supplies public routing, issuer identity and authority
data, and the minimum accepted PeerBadge trust level. Issuer identity public
keys are the trust roots, and canonical profiles additionally pin each root's
signed authority. An explicit test configuration without a pinned authority
uses its explicitly supplied authority relays to resolve it. Authority and revocation
events remain untrusted content and must pass local event, author, role,
signature, proof, digest, and schema checks.

Trust-level policy is evaluated only after the credential and holder
authorization authenticate and the PeerBadge schema parses. Every canonical
environment currently requires level 9 or greater. A lower level remains a
valid issuer credential but is rejected as insufficient for an FI, FLIP, or
push-gateway guardian-telemetry trust decision.

An authenticated `IssuerAuthority` delegates negative-state completeness trust
to every Nostr revocation relay it names. Accepting an issuer root therefore
also accepts that issuer's current choice of revocation relay operators: each
must honestly retain and return the issuer's current revocations. EOSE proves
query completion, not operator honesty. A compromised or mistakenly selected
revocation relay can conceal a revocation until the issuer replaces that
location in a newly signed authority.

Every verification fetches revocation state afresh, and authority state afresh
for issuers without a profile-pinned authority. Every canonical issuer derives
its authority from the environment's committed authority document at
construction and never reads kind-37703, so an overwritten or hostile
authority event cannot rotate its trust or deny verification
([SPEC-peer-badge-verifier](./specs/SPEC-peer-badge-verifier.md)). There is no cache, persisted high-water
mark, or stale fallback. Every configured authority relay (when fetched) and
supported Nostr revocation location must complete with EOSE; timeout,
disconnect, closed subscription, notification failure, or candidate/byte-bound
exhaustion fails closed. Any valid matching revocation rejects the badge.

**Relay visibility:** revocation publication is not instantaneously or
atomically visible. Each completed relay read defines the state observed by
that verification; concurrent or unpropagated updates can affect a later fresh
verification. Each later call refetches revocation state — and authority state
for unpinned test issuers — with no persistent local cache. Under the
documented honest-relay assumptions, an update visible in its relevant fresh
read must be applied. Canonical authority replacement instead requires a
coordinated profile release.

Authority relay configuration, authority-listed revocation locations, event
count, event size, and relay-I/O deadline are bounded. Dropping verification
cancels its reads and persists no state. As documented centrally in
`crates/nostr-clients/SECURITY.md`, the relay wrapper may briefly retain its
otherwise private client in a detached best-effort unsubscribe task; that
cleanup shares no state or capability with later verifications. Callers must
also bound the number and concurrency of verifications initiated from untrusted
advertisements. Credential cryptography is synchronous work inside the async
operation and must be revisited if badge volume becomes large enough to affect
runtime responsiveness.

`VerifiedPeerBadge.subject` states whom the holder authorized; it does not
prove that the presenter controls that key. Advertisement consumers must
verify the complete advertisement and require its authenticated author to
equal the returned subject.

The built-in development and staging issuer roots are public placeholder keys
with known secret keys. No verification result in those environments is
suitable for a security decision until those constants are replaced with
deployment-owned keys. Production has no placeholder root: its roots are the
profile's issuer identities, personal keys held individually by their owners,
and verifier construction fails closed if that list is ever empty.

A rooted identity signs its issuance key and revocation locations, while the
environment profile fixes which signed authority a release accepts. Compromise
of that identity cannot be contained by credential revocation or relay
freshness: the root must be removed from the environment profile and the new
revision deployed to every relying consumer. A production release must
therefore confirm the exact pinned authority with every retained issuer and
preflight every listed revocation location.
The `test-support` feature exposes explicit issuer-root, relay, and minimum
trust-level injection for defe-backed component tests. Those verifiers carry
`ExplicitTestConfiguration` provenance rather than impersonating a canonical
profile. Production artifacts must not enable this feature or accept that
provenance at a composition boundary.
Replacing roots or pinned authorities, changing relay-currentness assumptions,
changing issuer delegation of revocation relay trust, adding caches, changing
retry/fallback behavior, or consuming the verifier from a new network-facing
path requires renewed security review.
