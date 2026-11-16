# SPEC-advertisement: Nostr service advertisement

## Status

Advertisement availability depends only on a configured offer, physical
capacity, and the daemon's readiness checks. Setup-payment membership is
enforced when a priced quote is requested.

The MG publishes the kind-37701 advertisement and the kind-37709 v2
advertisement from each snapshot
([SPEC-mg-nostr-events](../../nostr/specs/SPEC-mg-nostr-events.md), *MG
advertisement v2*).

## Record justification

No single artifact can own the advertisement contract because daemon projection and Nostr publication must stay interoperable with service-wire availability shapes, holder authorization verification, and independent FA consumers.

When configured with a relay, the daemon immediately begins a periodic Nostr
publication cycle. It connects lazily, snapshots the fleet, and publishes only
when that snapshot says a seat would be accepted. An eligible cycle reads
durably enrolled Holder authorizations, signs the portable kind-37701 MG
advertisement and the kind-37709 v2 advertisement from that one snapshot, and
publishes each. Each publication is attempted even when the other fails. Cycles run every 30 minutes and promptly after
daemon-owned offer or capacity changes; repeated changes may coalesce into one
fresh snapshot. A failed cycle is logged and
retried at the next trigger or interval, and advertising failure never stops RPC
service. Every document is stamped with the current Unix time and expires after
60 minutes. The same relay client is reused after a successful connect.

This behavior implements the cross-program contract in
[SPEC-mg-nostr-events](../../nostr/specs/SPEC-mg-nostr-events.md)
(advertisement document, holder-published authorization event, and verifier
rules), including its
[additive-extensibility requirement](../../nostr/specs/REQ-extensible-mg-advertisement.md).
Kind numbers remain provisional in that contract.

## Published document

Advertisement publication and `GetAvailability` use the same gated availability
projection. A false projection suppresses publication; a true projection allows
the payload below. Advertisement and RPC calls are independent, so an ad is a
discovery hint rather than a capacity reservation. The payload contains:

- the MG's root-derived Nostr public key, which must also author the event;
- the MG's root-derived commitment-signing service pubkey — the exact
  value the printed locator carries, sourced from the same derivation
  ([ARCH-manifold-guardian-identity](./ARCH-manifold-guardian-identity.md)), so
  advertisement and locator can never disagree;
- issue and expiry times;
- one `iroh://` API endpoint containing the daemon's endpoint id;
- the release-pinned fedimintd version and supported federation sizes;
- the operator's current plans in the same `Plan` serialization used by
  `GetAvailability`; and
- the MG identity's bounded set of durably enrolled Holder authorizations and
  backing signed credentials, deduplicated by credential digest.

The v2 payload carries the same offer without the Nostr public key, which
the envelope signer states, and adds the uptime scores of the latest
readiness run (*Readiness gate*) once a run has recorded uptime.

The inner kind-37701 document is Schnorr-signed by the Nostr identity over
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
The MG publishes neither ad while it is not accepting seats. A previously published
ad remains visible until its signed expiry, at most 60 minutes after issue;
`GetAvailability` and `GetQuote` remain authoritative for races during that window.

The commitment-signing pubkey inside the signed payload is what binds the dialing identity to the Nostr identity: consumers build their dialing locator from it and the advertised endpoint, trusting it exactly as far as they trust the advertisement's badge-vouched author ([SPEC-mg-nostr-events](../../nostr/specs/SPEC-mg-nostr-events.md), trust-chain paragraph).

## Holder-authorization enrollment

The operator explicitly requests a bounded query of at most 64 kind-37705
candidate events indexed to the MG's own Nostr pubkey: `Check now` during
setup, or `Fetch new authorization` when renewing authorization afterward.
Relay tags are discovery hints only. Before retaining a candidate, the daemon
verifies the Nostr event signature, parses
its versioned content, requires the content holder id and authorization
statement holder id to equal the event author, verifies the holder's SDK
authorization proof, requires the authorization subject to equal this MG's
Nostr pubkey, and requires the inline credential digest to equal the
authorization's credential digest. It also rejects a statement issued more than
one hour ahead of the receiver's clock. A fetched candidate must further pass
the shared verifier's offline issuance check: its badge names one of this
environment's trusted issuers, verifies against that issuer's pinned authority,
and was issued to the authorizing holder. Malformed, mismatched, or untrusted
candidates are skipped without logging candidate-controlled values.

One MG identity retains exactly one complete authorization event, shared
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

The MG does not check revocation or the relying-party minimum trust level,
and its issuance check is not a trust decision for anyone else. The FA must
repeat the authorization checks and perform the full issuer-policy checks
itself, as required
by the FA verification rules in
[SPEC-mg-nostr-events](../../nostr/specs/SPEC-mg-nostr-events.md).

## Availability gates

The advertisement carries neither an availability boolean nor a count. Its
existence means the publication cycle observed that the MG was accepting
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

A seat sold by an MG that cannot be reached or cannot use Bitcoin becomes
the cause of a failed DKG, so the daemon admits new seats only while its
readiness checks pass. At startup and then once in every 10-minute uptime
slot (every minute while failing), it requires a connected home relay, its own discovery record
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

Each run also requires this MG's release to be at least the setup-payment
policy's `min_mg_version` by SemVer precedence (a pre-release ranks below its
release; build metadata does not count); without one the check does not
apply. An MG below it stops selling new seats until it is updated, but it
still serves its seats, so the version check is not a prerequisite and never
marks a slot down. A newly admitted policy starts a run at once, so raising
the minimum closes `GetAvailability` and `GetQuote` and stops publishing the
advertisement without waiting for the next slot, and lowering or removing it
reopens them and publishes again. An advertisement already on relays stays
visible until its signed expiry, as above.

Each run also requires CPU and IO pressure averages of at most the
setup-payment policy's `max_pressure_percent` (CPU 15% over 6 hours and 10%
over 3.5 days, IO 25% and 15%, when absent or before a policy is admitted;
[SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)).
Each run reads the cumulative pressure stall (PSI `some`) counters of the
cgroup the daemon shares with its seats, for CPU and for IO, and divides the
stall time added since the previous reading by the wall time elapsed. It
folds that share into bias-corrected exponential moving averages with 6-hour
and 3.5-day (84-hour) time constants: the uncorrected average and its weight
each move `1 - exp(-elapsed / tau)` of the way to the share and to 1, and the
average compared is their quotient. A young history is therefore the
time-weighted mean of the intervals measured so far, so the busy minute
while seats start after an upgrade does not close the host for days, and a
long history is the plain moving average. The first measured interval sets
every average to its own share, so a fresh install measures before it
sells. An interval says
nothing about pressure, and leaves the averages unchanged, when a counter
went backwards, as it does when a restart recreates the cgroup; when the
clock did not advance; or when it is longer than 30 minutes, since runs come
every 10 minutes and a longer interval spans downtime that would otherwise
count as time without pressure. The last reading, the averages and their weights
are durable. The averages compare at the 0.1% telemetry reports, and a failure
names the shorter failing window, `host_pressure_6h_high` before
`host_pressure_3_5d_high`; a host steady just over a 3.5-day maximum stays
closed, while a spike of a few hours closes it only while its 6-hour average
is over. Pressure is a share of time, so unlike system load it does not
depend on CPU count or speed, and it counts only waits of this MG's tasks.
Counters that cannot be read fail the check (`host_pressure_unavailable`),
and so does the first run before any interval is measured
(`host_pressure_measuring`): every MG release that journals resource samples
reported both counters when this check was added. Like the version check, a
failing pressure check stops new seats but the MG still serves its seats, so
it is not a prerequisite and never marks a slot down. Local E2E, whose MGs
all share one busy test host's cgroup, skips the pressure check as it skips
the relay and discovery checks.

Each run also requires three uptime scores of at least the setup-payment
policy's `min_uptime_percent` (1 day 70%, 7 days 90%, 30 days 93% when
absent or before a policy is admitted; 0 turns a window off;
[SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)).
Each run records its 10-minute slot, up when the run's prerequisites passed;
the first run in a slot decides it. The daemon keeps durable exponential
moving averages over slots with 1-, 7- and 30-day time constants and compares
each at the 0.1% telemetry reports; a failure names the shortest failing
window. Slots no daemon recorded, such as while it was stopped, count as
down. History starts at 95% at the first up slot, so setup before an MG first
passes its prerequisites costs it nothing, while a new MG has less margin
than one with a good record. Only the prerequisites decide whether a slot is
up, so a low uptime alone never keeps itself low, and failed prerequisites
are retried but the uptime check is not. With the defaults, an outage of up
to about 8.5 hours costs nothing, 12 hours down closes the MG for about 6
hours (1 day), one day down for about two days (7 days), and an MG down
about 2 hours every day stays closed (30 days).

The verdict is durable in `offer_state`, so a restart resumes it: a ready MG
keeps selling and a failing one stays closed until a run passes. A fresh,
restored, or upgraded MG starts closed.

A failed verdict suppresses publication, makes `GetAvailability` report
`accepting_seats = false`, and makes `GetQuote` return `CapacityExhausted`, so
FAs treat it like a full MG. Each change of verdict also draws a fresh offer
epoch in the same database write: a quote issued before a failure is refused
with `OfferChanged` and its refund instead of admitting a seat. Every run emits one shareable event with a
fixed code per check, the three uptime scores, the minimums they were held to,
and the pressure averages and thresholds for telemetry, and the latest report is served to the
operator by the `ShowSeatReadiness` admin verb, which the operator UI's Health
page shows. The report includes the same three scores and minimums, so the page
names the window that fails and by how much.
