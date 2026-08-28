# SPEC-advertisement: Nostr service advertisement

## Status

Advertisement availability depends only on a configured offer and physical
capacity. Setup-payment membership is enforced when a priced quote is requested.

## Record justification

No single artifact can own the advertisement contract because daemon projection and Nostr publication must stay interoperable with service-wire availability shapes, holder authorization verification, and independent FI consumers.

When configured with a relay, the daemon immediately begins a periodic Nostr
publication cycle. It connects lazily, snapshots the fleet, and publishes only
when that snapshot says a seat would be accepted. An eligible cycle reads
durably enrolled Holder authorizations, signs a portable kind-37701 FMan
advertisement, and publishes it. Cycles run every 30 minutes and promptly after
daemon-owned offer or capacity changes; repeated changes may coalesce into one
fresh snapshot. A failed cycle is logged and
retried at the next trigger or interval, and advertising failure never stops RPC
service. Every document is stamped with the current Unix time and expires after
60 minutes. The same relay client is reused after a successful connect.

This behavior implements the cross-program contract in
[SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md)
(advertisement document, holder-published authorization event, and verifier
rules), including its
[additive-extensibility requirement](../../nostr/specs/REQ-extensible-fman-advertisement.md).
Kind numbers remain provisional in that contract.

## Published document

Advertisement publication and `GetAvailability` use the same gated availability
projection. A false projection suppresses publication; a true projection allows
the payload below. Advertisement and RPC calls are independent, so an ad is a
discovery hint rather than a capacity reservation. The payload contains:

- the FMan's root-derived Nostr public key, which must also author the event;
- the FMan's root-derived commitment-signing service pubkey — the exact
  value the printed locator carries, sourced from the same derivation
  ([ARCH-fleet-manager-identity](./ARCH-fleet-manager-identity.md)), so
  advertisement and locator can never disagree;
- issue and expiry times;
- one `iroh://` API endpoint containing the daemon's endpoint id;
- the release-pinned fedimintd version and supported federation sizes;
- the operator's current plans in the same `Plan` serialization used by
  `GetAvailability`; and
- the FMan identity's bounded set of durably enrolled Holder authorizations and
  backing signed credentials, deduplicated by credential digest.

The inner document is Schnorr-signed by the Nostr identity over
`SHA256(fedi-fman-advertisement-domain || JCS(payload))`; the Nostr event supplies
its own signature as well. The payload identity is required to match the signing
key. Availability fields are discovery hints, not trust claims. Setup-payment
federation identities and join material appear in neither content nor relay
tags; consumers obtain them from
[SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md).

Fleet runtime and advertisement publication do not begin until onboarding has
retained at least one structurally verified, subject-bound Holder authorization.
The relying consumer still owns issuer trust, revocation, and policy evaluation;
onboarding does not turn structural verification into an issuer endorsement.
The ad is not published when the FMan is not accepting seats. A previously published ad remains visible
until its signed expiry, at most 60 minutes after issue; `GetAvailability` and
`GetQuote` remain authoritative for races during that window.

The commitment-signing pubkey inside the signed payload is what binds the dialing identity to the Nostr identity: consumers build their dialing locator from it and the advertised endpoint, trusting it exactly as far as they trust the advertisement's badge-vouched author ([SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md), trust-chain paragraph).

## Holder-authorization enrollment

The operator's `Check now` action invokes a bounded query of at most
64 kind-37705 candidate events indexed to the FMan's own Nostr pubkey while it
waits for setup to complete. Relay tags are discovery hints only. Before
retaining a candidate, the daemon verifies the Nostr event signature, parses
its versioned content, requires the content holder id and authorization
statement holder id to equal the event author, verifies the holder's SDK
authorization proof, requires the authorization subject to equal this FMan's
Nostr pubkey, and requires the inline credential digest to equal the
authorization's credential digest. It also rejects a statement issued more than
one hour ahead of the receiver's clock. Malformed or mismatched candidates are
skipped without logging candidate-controlled values.

Accepted complete events are retained in the FMan database by credential
digest and reverified at startup. One FMan identity has one authorization set
shared across every federation it operates, and retains at most 64 distinct
credential digests so the whole set remains representable in one public
trust-material response. A later signed authorization statement for the same
credential may advance the retained value even at that limit; an empty, failed,
equal, or older relay answer never deletes or rolls it back. A new digest is
ignored when the set is full rather than evicting an existing valid row in
response to attacker-controlled churn. Because the daemon has no trusted-holder
allowlist, arbitrary signers may fill this one service-wide set and deny later
new digests; the operator enrollment flow must not treat first admission as
issuer trust. Startup removes legacy rows beyond the aggregate or receiver-time
bounds before reuse. Once the UI observes enrollment it stops requesting
refreshes, and ordinary advertisement publication performs no
Holder-authorization relay query. A relying consumer still performs fresh
issuer-policy, credential, and revocation verification; durable carriage is not
a claim that the backing credential remains valid.

The FMan does not decide whether the credential issuer is trusted, verify the
backing credential's PBRSA proof, or check revocation. The FI must repeat the
authorization checks and perform those issuer-policy checks itself, as required
by the FI verification rules in
[SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md).

## Availability gates

The advertisement carries neither an availability boolean nor a count. Its
existence means the publication cycle observed that the FMan was accepting
seats: it had physical capacity after live seats — bounded by both the
operator's seat limit and the remaining lifetime port grid — and the operator
had configured an offer. Setup-payment membership and opening a retained
payment-federation client in the current daemon process are not advertisement
gates; RPC remains authoritative. A seat offered at zero settles against
nothing, which is the deployment bootstrap where the first federation's
guardians are given away because no ecash to pay them with exists yet.
`GetAvailability` uses the same gated-slot calculation, but independent calls
can observe different settings epochs and live state.
