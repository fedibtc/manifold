# Security

Nostr relays are untrusted. This crate only publishes events and returns discovery candidates; it does not decide whether an event is valid for trust decisions.

The shared PeerBadge verifier makes one narrower deployment assumption: the
canonical authority relays configured for an environment return the current
issuer-authority replacement event and do not roll it back after a component
restart. A valid issuer authority then delegates negative-state completeness
trust to each Nostr revocation relay it names. Those relay operators are
trusted to retain and return the issuer's current revocations; EOSE alone does
not establish their honesty. The assumptions remove the need for persisted
authority or revocation high-water marks, but do not authenticate event
content. `NostrPeerBadgeClient` therefore still returns only bounded
candidates, and the verifier checks the Nostr signature, exact event role,
authority proof, fresh revocation state, credential proof, holder
authorization, and badge schema before use. A missing authority, missing
supported revocation location, or unavailable lookup fails closed.

PeerBadge results require EOSE. Timeout, notification-stream end or failure,
relay `CLOSED`, and candidate/byte-bound exhaustion are incomplete queries and
fail closed. Every configured authority relay and every Nostr revocation
location in the admitted authority is queried under the same absolute
verification deadline; an empty earlier location cannot hide a newer
authority or valid revocation at a later location.

Dropping a bounded complete-query future runs its subscription cleanup and
cancels the pending read. Cleanup sends a best-effort unsubscribe from a
detached task because `Drop` cannot await it; SDK connection shutdown is
likewise not joined to the caller's deadline. FI registry queries use a unique
subscription id on the caller-owned shared pool, so their cleanup can briefly
contend with later pool use but cannot unsubscribe another query. PeerBadge
queries use a private ephemeral client. Neither cleanup path persists state or
holds admission, lease, reservation, or selection authority.

Callers must verify every fetched event before use:

- the Nostr event signature and expected author;
- kind, `d`, `p`, `t`, issuer, credential, and schema tags against event content;
- canonical payload signatures where the protocol defines them;
- `fedi-credential-sdk-protocol` credential and holder-authorization proofs;
- revocation state and local issuer trust policy.

FI common-set lookup pins the publisher, kind, and addressable-event identifier
in its relay filter and enforces a local candidate-count cap. Those are query
hints and resource bounds, not authentication. `fi-client` verifies complete
kind-37707 events and owns durable replacement-order rollback protection.
The shared relay config rejects every normalized event larger than 256 KiB and
locally enforces subscription filters before signature verification, database
retention, or role notification. The bounded collector also
observes at most 16 common-set candidates and retains at most 4 MiB, so
oversized events and tag arrays cannot expand the returned batch. The
authorless FMan advertisement enumeration is likewise locally bounded to
2048 observed candidates and an explicit 16 MiB aggregate byte cap;
per-author replacement and every trust decision stay with the consumer.
That enumeration is complete-or-capped: a relay answer is accepted at EOSE
or once a local resource bound — the candidate ceiling or the aggregate
byte cap — is reached (both bounds complete the query with the retained
prefix, since they guard resources against publisher volume rather than
proving completeness), and an answer that stalls, ends, or is closed with
neither bound reached fails closed as a typed incomplete query rather than
a silently truncated list.
Its SDK database is deliberately stateless: it returns every locally
filter-matching, signature-valid candidate without applying NIP-01 replacement
suppression. Semantic admission must see an older valid event even when a relay
first sends a newer publisher-signed event with invalid role content.

FMan holder-authorization discovery returns a bounded candidate set instead of a single newest event because a newer invalid event with matching tags must not hide an older valid authorization. Publishing fails only when no configured relay accepts the event: one acceptance is success, and other relays' rejections are logged rather than returned, so a caller that needs every relay to hold the event must use a single-relay client per relay (as the setup-payment publisher does).

Legacy FLIP FMan advertisement lookup likewise returns bounded untrusted
candidates, but current FLIP federation-eligibility verification must not use
Nostr as the authoritative FMan trust-material directory. FLIP starts from the
invite code, reads `fedi:fman_api_urls` consensus metadata, queries public FMan
APIs, verifies signed `GetFederationTrustMaterial` responses, verifies returned
`FmanPeerAttestation` objects against the final config, and performs fresh
required revocation lookups before accepting liquidity. Relay `Filter::limit`
values are only hints: any role-specific hard cap must be enforced before using
helpers that accumulate all matching events or event IDs. The legacy bounded
manual subscription still unsubscribes as soon as the local cap or timeout is
reached.
