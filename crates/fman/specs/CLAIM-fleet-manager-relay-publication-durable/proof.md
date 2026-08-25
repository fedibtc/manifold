# Proof: Fleet Manager relay publication is durable

## Scope and model

This is a compositional conditional argument for
[CLAIM-fleet-manager-relay-publication-durable](../CLAIM-fleet-manager-relay-publication-durable.md).
It covers the four required event classes, the meaning of a
durable-publication observation, and the operator's durability decision. It
does not inspect Nostr implementation, relay operation, publication source
code, or evidence for any premise.

The model quantifies over every configured Nostr relay and every required
advertisement, setup-payment, backup-document, and guardian-archive event.
It grants every immediate assumption exactly as an axiom. “Durable” means that
every configured relay accepted and retains the exact required event; it does
not mean that a publication attempt was sent or acknowledged best-effort.

## Assumption boundary

The first four assumptions respectively supply eventual acceptance and
retention for every required advertisement, setup-payment, backup-document,
and guardian-archive event. The observation assumption supplies an
event-specific predicate that establishes both relay acceptance and retention
for every configured relay, rather than a best-effort send result. The operator
gate supplies the prohibition on treating a best-effort publication as durable
before that predicate occurs.

No assumption uses the claim's conclusion or another assumption as its
justification. The operator gate refers to the observation predicate it gates,
but neither establishes the other. No premise depends on relay implementation
or on an observation inferred from the operator gate. Each is a direct external
or operational premise.

## Argument

1. **[assumption] Advertisement durability.** The first assumption supplies
   eventual acceptance and retention for every required advertisement event at
   every configured Nostr relay.
2. **[assumption] Setup-payment durability.** The second assumption supplies
   eventual acceptance and retention for every required setup-payment event at
   every configured Nostr relay.
3. **[assumption] Backup-document durability.** The third assumption supplies
   eventual acceptance and retention for every required backup-document event
   at every configured Nostr relay.
4. **[assumption] Guardian-archive durability.** The fourth assumption supplies
   eventual acceptance and retention for every required guardian-archive event
   at every configured Nostr relay.
5. **[assumption] Durable observation.** The fifth assumption makes a
   durable-publication observation establish acceptance and retention of the
   exact required event at every configured relay. A successful best-effort
   send alone cannot satisfy this predicate.
6. **[assumption] Operator gate.** The sixth assumption prevents the operator
   from treating a best-effort publication as durable before the
   event-specific durable-publication observation occurs.
7. **[logic] Joint sufficiency.** Steps 1 through 4 cover each required event
   class exactly once. Step 5 gives “observed” its acceptance-and-retention
   meaning rather than equating it with a send attempt, and step 6 applies that
   meaning before the operator makes a durability decision. Thus configured
   relays eventually accept and retain every required advertisement,
   setup-payment, backup-document, and guardian-archive event, and the
   operator does not treat best-effort publication as durable until this has
   been observed.

## Residuals

This claim does not establish that configured relays actually meet their
acceptance-and-retention premise, that a particular observation mechanism is
available, or that the operator follows the gate in practice. It excludes
event classes other than the four named required classes and relays outside the
configured set. Those cases are outside this conditional property, not harmless
outcomes.

## Weakest links

The relay acceptance-and-retention premises and the event-specific observation
predicate are external operational conditions. The operator gate is likewise an
operational condition: the proof establishes its consequence only when the
operator follows it.

## Additional current evidence

# Evidence: relay outage leaves advertisement stale




Scope: `crates/fman/core/src/fleet.rs`, `crates/fman/nostr/src/lib.rs`,
`crates/fman/bin/src/main.rs`,
`crates/nostr-clients/src/nostr_relay_client.rs`,
`crates/fman/specs/SPEC-advertisement.md`,
`crates/nostr/specs/SPEC-fman-nostr-events.md`, and `Cargo.lock`

## Claim

Under V1's fault model in the production-readiness fault model, loss or withholding of
all configured Nostr relays bounds the signed event/listing age, not every embedded field's semantic freshness: published advertisements expire at twice the one-hour
republish interval, failed publication cycles are warning-logged and cannot form a tight
self-loop: early wakes require a trusted operator-triggered enrollment change,
while SDK reconnect attempts use bounded adaptive backoff.
Publication resumes without manual intervention
after the relay recovers (A1, publication-loop A4, A5). Connection and publication run in a
background task and do not synchronously gate daemon startup or request paths.
This task-local isolation does not discharge row-wide A2; the trust-material
leaf falsifies that stronger property.

This claim is periodic, not change-triggered. `SetPrice` can leave an old but unexpired advertisement visible until periodic publication.

## Axioms (trusted, not checked here)

- **A-host/deps:** V1's A-host and A-deps-recover hold, with at most one daemon
  restart during the outage.
- **A-relay-client:** after relay recovery the pinned Nostr SDK reconnects a
  previously established client, or a later bounded initial connection attempt
  succeeds, and accepts an authentic advertisement.
- **A-consumer:** a conforming consumer enforces the signed advertisement
  `expires_at` rule in `SPEC-fman-nostr-events`.
- **A-runtime/time:** the advertisement task continues to be polled and Tokio's
  one-hour sleeps elapse according to the monotonic-clock contract. The wall
  clock used in advertisement timestamps is after the Unix epoch.

## Argument

**L1 (code) — Nostr connection and publishing are isolated in a background
task.** `FleetManagerNostr::new` performs no IO. `start` spawns `Inner::run`,
whose initial connection loop and subsequent advertisement loop run after the
fleet, RPC router, and local services have been constructed. Initial connection
failure logs a warning and sleeps for `REPUBLISH_INTERVAL`; it does not return
an error into daemon startup. A daemon restart during the outage recreates the
same background loop while durable seats and the other services open normally.

**L2 (code + axiom) — signed event age has an explicit bound.** Every successful
cycle snapshots the fleet, stamps `issued_at` with the current Unix time, and
sets `expires_at` to twice the one-hour `REPUBLISH_INTERVAL`. If the relay then
withholds all traffic indefinitely, it can retain old bytes but A-consumer rejects the signed advertisement after that two-hour validity window.
The outage can therefore remove discovery, but cannot make one signed event
remain valid without bound. This says nothing about semantic freshness of
fields inside a newly issued event.

**L3 (code) — failures are visible and attempts remain paced.** Before initial
connection, one bounded attempt is followed by a one-hour sleep and warning.
After connection, every failed advertisement cycle logs a warning. The normal
sleep is one hour. A successful holder-authorization enrollment can wake the
publisher early. Enrollment runs once at runtime start and after later
owner-local Admin requests, and wakes the watch only when the durably retained
vector changes; relay input alone cannot schedule another pass. The hourly timer and a pending trusted watch change
can produce a two-attempt burst, but publication cannot create its own tight
retry loop. Independently, the pinned SDK reconnect loop uses adaptive
10–60-second backoff with jitter and bounded 60-second connection attempts;
those transport attempts are not advertisement cycles.

**L4 (code + axiom) — the next post-recovery cycle republishes.** If recovery
precedes initial connection, the next hourly connection attempt succeeds under
A-relay-client and immediately starts `run_advertisements`, whose first action
is a publication cycle. If outage follows an established connection, the SDK
reconnects under A-relay-client and the next cycle publishes a newly sampled,
newly timestamped advertisement. No manual signal or persisted dirty bit is
needed because publication is unconditional and periodic. Recovery just after
an attempt may wait approximately one interval plus bounded connection or
publish time; V1 states no tighter recovery-time objective.

L1–L4 establish bounded signed-event age, resumption, publication-loop pacing,
visibility, and
task-local isolation for the advertisement portion of V1; they do not
establish row-wide A2. ∎

## Residual windows

- `advertisement-retains-old-price-after-set-price.md` shows that an offer
  change does not wake this publisher, so an old price may remain advertised
  until the next cycle. That is inside the periodic envelope asserted here,
  although it falsifies the bug-hunt root's stronger immediate-coherence claim.
- This leaf bounds publication-cycle and SDK reconnect pacing, not
  cross-cutting log-sink growth or backpressure. Cycle and reconnect warnings
  can continue for the outage's duration; the root's separately parked
  log-volume owner must analyze retention and sink behavior.
- Expiry bounds how long old data is represented as current; it does not
  preserve discoverability. During a long outage there is necessarily no valid
  advertisement, which is V1's allowed advertisement degradation.
- Holder-authorization enrollment is operator-triggered and monotonic by
  credential digest. Relay withholding can prevent initial or replacement
  enrollment but cannot erase retained authorizations or independently wake
  publication. This leaf still proves event age and publication resumption,
  not live issuer/revocation validity of embedded authorization content.

## Weakest links

A-relay-client is the weakest link because established-client reconnection is
implemented by the pinned SDK, not an explicit outer reconnect loop. L2's bound
also relies on consumers enforcing signed `expires_at`; the daemon cannot make
a withholding relay delete old bytes. The one-hour retry cadence is a source constant; the spec's phrase
“configured republish interval” is inaccurate, and no
test drives a real relay through outage and recovery.

# Evidence: relay outage stops backup publication




Scope: `crates/fman/core/src/{backup,backup_queue,fleet,seat}.rs`,
`crates/fman/bin/src/main.rs`,
`crates/fman/nostr/src/backup.rs`,
`crates/nostr-clients/src/nostr_relay_client.rs`, and `Cargo.lock`

## Claim

Under V1's fault model in the production-readiness fault model, loss or withholding of
all configured Nostr relays stalls F-backup. Failures alone are retried at a
paced cadence rather than in a tight loop (A4), every durable seat is
re-enqueued for a fresh attempt after at most one daemon crash/restart during
the outage, and publication attempts resume and complete without manual
intervention after the relay recovers (A1). Relay awaits occur only in the
background publisher, and failed work is warning-logged (A5). This leaf does
not discharge row-wide A2;
The interaction-security proof owns the separate trust-material nonemptiness condition.

This is a liveness/resumption claim about the queue. It deliberately does not
claim that the reconstructed document has the latest or complete guardian
content; the separate known risks are that restart can reconstruct the wrong archive requirement and that an older in-flight document can win NIP-01 replacement ordering.

## Axioms (trusted, not checked here)

- **A-host/deps:** V1's A-host and A-deps-recover hold, including at most one
  daemon restart during the outage. Durable database seat facts survive it.
- **A-relay-client:** once the configured relay recovers, the pinned Nostr SDK
  reconnects its existing client or a later bounded connection attempt
  succeeds; accepted authentic events are served back to the confirming read.
- **A-runtime:** a spawned Tokio task which is not cancelled continues to be
  polled, timers fire, and the recovered relay remains available long enough
  for the finite queued publications to finish.

## Argument

**L1 (code + test) — foreground work never awaits the relay.** Every backup-relevant lifecycle transition calls `BackupQueue::mark`, which changes an in-memory dirty bit under
a synchronous mutex, notifies the publisher, and returns. The single publisher
task alone awaits document assembly and `BackupSink::publish`. The relay sink
also connects lazily on its first publication, so opening an existing fleet
does not await the failed relay. The test `marking_a_seat_never_waits_on_the_relay`
exercises this isolation. Thus the outage necessarily stalls F-backup but adds
no relay wait to the other service functions.

**L2 (code + test) — failures alone are retained and paced.** `take_dirty` clears a
mark only while taking work. Every assembly, connection, publication, or
read-back error calls `remark`, emits `failed to publish the seat's recovery
document` at warning level, and causes one fixed 15-second sleep before the
next batch. A fresh mark may cut that sleep short, including marks induced by
external requests. The queue coalesces storage by seat in a bounded `HashMap`,
so failures alone neither append work nor retry in a tight loop. This does not
prove a bound against request-driven wakeups; V8 owns per-request amplification. `a_failed_publication_is_retried_until_it_lands`
tests fail-then-recover convergence, while
`a_mark_during_a_publication_is_published_again` tests an in-flight mark.

**L3 (code) — restart re-enqueues every durable seat.** Queue state
is in memory and dies with the process, but `Fleet::open_with_wallet` enumerates
every durable seat at startup and unconditionally calls `backup.mark_dirty`.
The newly spawned publisher therefore receives one fresh dirty mark for every
seat, including one whose prior attempt was in flight or failing when the
daemon died. The queue state and exact `requires_archive` obligation do not
survive; this lemma establishes a fresh seat-level attempt only. The prior guardian-archive records show that this reconstruction
can choose wrong content or lose ordering; they do not show that it fails to
make another attempt.

**L4 (code + axiom) — recovery drains the finite dirty set.** A failed lazy
connect is not cached, so each later retry attempts a new connection. After a
successful connection, the SDK client is reused and reconnects under
A-relay-client. For every document, `publish_confirmed` first publishes and
then fetches the exact event id; only both successes let the queue leave that
seat clean. Under A-runtime and relay recovery, each finite publication
eventually returns `Ok`, and all reconstructed marks drain. Failure attempts
remain visible in warning logs until then.

L1–L4 establish A1, failure-driven A4 pacing, and visible bounded attempts for
queue liveness under V1, subject to the stated SDK progress premise. They do
not establish row-wide A2 or request-driven amplification bounds. ∎

## Residual windows

- Restart can rebuild the archive requirement incorrectly for some formed or decommissioned seats, and an old pre-crash request can later win NIP-01 replacement. These counterexamples concern recovered content and freshness, not retry or relay-return liveness.
- The failure cadence is pacing, not exponential backoff. Bounded seat
  cardinality and coalesced marks rule out an unbounded queue, while failures
  alone wait 15 seconds. External marks can cut the wait short, so this leaf
  does not rule out request-driven retry amplification; no claim is made that 15
  seconds is an optimal traffic policy.
- This leaf bounds queue storage and retry pacing, not cross-cutting log-sink
  growth or backpressure. Failed seat attempts warn indefinitely; the root's
  separately parked log-volume owner must analyze retention and sink behavior.
- Sequential publication means one withholding operation delays later seats in
  the same batch. A request/connection timeout bounds each failed attempt; this
  record states no numeric recovery-time objective.
- If the relay accepts a request just before process death, that request may
  arrive after restart. The prior delayed-publication records own the resulting
  replacement-order hazards. Startup still queues a fresh attempt.

## Weakest links

The pinned dependency is the weakest link: initial connection is bounded at 15
seconds; SDK event send waits ten seconds for `OK` (with bounded transport and
authentication waits), read-back is bounded at 15 seconds, and the default
reconnect loop uses an adaptive 10–60-second interval. Reconnection is still
delegated to that SDK rather than explicitly driven by `RelayBackupSink`. L3 is a code-rung seat enumeration, but it proves only a
new attempt, not correct reconstruction of `requires_archive`. L2 establishes only failure-driven A4 pacing; request-driven wakeup
amplification remains unproved and belongs to V8.
