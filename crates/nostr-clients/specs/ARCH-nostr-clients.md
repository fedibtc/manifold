# ARCH-nostr-clients: role-scoped relay I/O boundary

`fedi-decentralized-nostr-clients` exposes hardened relay I/O primitives and
shared role-specific Nostr service traits for decentralized federation
components. `fedi-decentralized-nostr`
remains the protocol crate for event kinds, tag names, signature domains,
and small deterministic helpers
([SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md));
this crate owns relay I/O and the `nostr-sdk` implementation.

The public boundary provides hardened relay connection, publishing, and
hard-capped fetch primitives plus shared role clients. `HolderNostrClient`
publishes holder authorization events, while `FiNostrClient` fetches FMan
advertisements for pre-formation selection — one pinned author's latest
event, or the authorless kind-37701 enumeration bounded by
`FMAN_ADVERTISEMENTS_CANDIDATE_LIMIT` and the explicit
`FMAN_ADVERTISEMENTS_RETAINED_MAX_BYTES` aggregate byte cap —
and a bounded common setup-payment candidate set. FMan-specific relay
operations live in the deep `fman-nostr` component rather than
this shared crate.
`FlipNostrClient` supports FMan-ad lookups. Current FLIP federation eligibility
starts from the invite code, reads Fedimint consensus metadata, and queries the
public FMan APIs listed in `fedi:fman_api_urls`. Returning federation
eligibility to Nostr-ad resolution remains work (see the status in
[SPEC-flip-federation-trust](../../liquidity-manager-daemon/specs/SPEC-flip-federation-trust.md)).
`NostrRelayClient` is a cloneable shared SDK wrapper used by role clients, deep
Nostr components, and the production setup-payment publisher. Its unbounded
fetch helper stays crate-private.
`NostrPeerBadgeClient` is the bounded read-only adapter used by the shared
PeerBadge verifier: it queries configured canonical relays for issuer
authorities and the authenticated authority's own Nostr locations for
credential revocations. Its security-sensitive reads distinguish a complete
EOSE result from timeout, stream termination, `CLOSED`, notification failure,
or local truncation; only EOSE can complete a query. Authority candidates are
combined from every configured canonical relay, and revocation candidates are
combined from every authenticated Nostr location. Every query must complete
under the caller's one absolute deadline. It returns candidates only; the
complete admission algorithm belongs to
[SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md).

Fetched events are untrusted relay data. Nostr kinds and tags are indexing
hints only, so consumers must verify event signatures, content/tag
consistency, credential proofs, revocation status, and application-specific
subject bindings before acting on an event ([`SECURITY.md`](../SECURITY.md)).
For PeerBadge authority currentness only, the deployment separately assumes
its configured canonical relay answers with the current replacement event and
does not roll back after a component restart. An authenticated authority
delegates the corresponding negative-state completeness assumption to every
Nostr revocation relay it names: those operators must honestly retain and
return the issuer's current revocations. Neither assumption bypasses event or
content verification, and neither extends to a relay that is not configured
or named by the current authenticated authority.

The shared SDK client uses a stateless event database. The candidate-batch
fetches (holder-authorization candidates, setup-payment publications, the
FMan advertisement enumeration) need every bounded verified candidate for
semantic admission, so generic SDK replacement or deletion policy must not
suppress an older valid candidate after a newer role-invalid event. In
particular, the advertisement enumeration pins no author, so per-author
newest-wins replacement is the consumer's admission decision
([ARCH-fi-client](../../fi-client/specs/ARCH-fi-client.md)), not a
transport policy. `FiNostrClient::fetch_fman_advertisement` is the
exception: it requests a single latest event (`limit(1)`), so a newer
role-invalid advertisement can hide an older valid one until the FMan
republishes.
