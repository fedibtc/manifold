# SPEC-advertisement: Nostr service advertisement

## Status

Advertisement availability depends only on a configured offer, physical
capacity, and the daemon's readiness checks. Setup-payment membership is
enforced when a priced quote is requested.

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

The operator explicitly requests a bounded query of at most 64 kind-37705
candidate events indexed to the FMan's own Nostr pubkey: `Check now` during
setup, or `Fetch new authorization` when renewing authorization afterward.
Relay tags are discovery hints only. Before retaining a candidate, the daemon
verifies the Nostr event signature, parses
its versioned content, requires the content holder id and authorization
statement holder id to equal the event author, verifies the holder's SDK
authorization proof, requires the authorization subject to equal this FMan's
Nostr pubkey, and requires the inline credential digest to equal the
authorization's credential digest. It also rejects a statement issued more than
one hour ahead of the receiver's clock. A fetched candidate must further pass
the shared verifier's offline issuance check: its badge names one of this
environment's trusted issuers, verifies against that issuer's pinned authority,
and was issued to the authorizing holder. Malformed, mismatched, or untrusted
candidates are skipped without logging candidate-controlled values.

One FMan identity retains exactly one complete authorization event, shared
across every federation it operates, and reverifies it at startup. A refresh
selects the valid candidate with the greatest signed authorization `issued_at`,
whatever its holder or credential, and it replaces the retained event only when
strictly later; an empty, failed, equal, or older relay answer never deletes or
rolls it back. Startup removes a retained event beyond the receiver-time bound
before reuse. The holder chooses `issued_at`, so any holder of a trusted badge
can publish a later-dated candidate that a refresh then selects in place of the
operator's.
The UI never polls for enrollment; after setup the operator can explicitly
check for renewed or replacement authorization. Refreshes update the live
authorization and trigger republication, while ordinary advertisement
publication performs no
Holder-authorization relay query. A relying consumer still performs fresh
issuer-policy, credential, and revocation verification; durable carriage is not
a claim that the backing credential remains valid.

The FMan does not check revocation or the relying-party minimum trust level,
and its issuance check is not a trust decision for anyone else. The FI must
repeat the authorization checks and perform the full issuer-policy checks
itself, as required
by the FI verification rules in
[SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md).

## Availability gates

The advertisement carries neither an availability boolean nor a count. Its
existence means the publication cycle observed that the FMan was accepting
seats: it had physical capacity after live seats — bounded by both the
operator's seat limit and the remaining lifetime port grid — the operator
had configured an offer, and the latest readiness check passed. Setup-payment
membership and opening a retained payment-federation client in the current
daemon process are not advertisement gates; RPC remains authoritative. A seat
offered at zero settles against nothing, which is the deployment bootstrap
where the first federation's guardians are given away because no ecash to pay
them with exists yet.
`GetAvailability` uses the same gated-slot calculation, but independent calls
can observe different settings epochs and live state.

## Readiness gate

A seat sold by an FMan that cannot be reached or cannot use Bitcoin becomes
the cause of a failed DKG, so the daemon admits new seats only while its
readiness checks pass. At startup and then every 10 minutes (every minute
while failing), it requires a connected home relay, its own discovery record
resolving through n0 pkarr or DNS with a relay it is connected to, and its
Bitcoin backend — read through the client `fedimintd` builds, with Bitcoin Core
required on its own rather than through its Esplora fallback — serving the
configured network, out of initial block download, with a fee estimate
(regtest waives the last two). An Esplora-only backend cannot report initial
block download and substitutes a default fee rate, so for it those two checks
pass vacuously. Failed checks are retried for up to a minute before a run
fails. Each Bitcoin check builds a fresh client and runs on a blocking thread
with a 10-second deadline, because the Core client blocks inside its async
calls; a check that outlives its deadline counts as unavailable, and the next
attempt waits on it rather than starting another. Local E2E skips the relay
and discovery checks.

The verdict is durable in `offer_state`, so a restart resumes it: a ready FMan
keeps selling and a failing one stays closed until a run passes. An FMan
created or upgraded before its first run starts ready, as it was before
readiness existed; a failing first run closes it like any other change of
verdict, so a quote issued before the upgrade is refused.

A failed verdict suppresses publication, makes `GetAvailability` report
`accepting_seats = false`, and makes `GetQuote` return `CapacityExhausted`, so
FIs treat it like a full FMan. Each change of verdict also draws a fresh offer
epoch in the same database write: a quote issued before a failure is refused
with `OfferChanged` and its refund instead of admitting a seat. Every run emits one shareable event with a
fixed code per check for telemetry, and the latest report is served to the
operator by the `ShowSeatReadiness` admin verb, which the operator UI's Health
page shows.
