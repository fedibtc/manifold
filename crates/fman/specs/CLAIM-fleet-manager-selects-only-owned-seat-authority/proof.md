# Proof: FI requests select only their owner's seat authority

## Stale proof

The numbered argument does not describe the complete current source. The signed
service has twelve verbs; SQLite admission uses `admit_seat` rather than the
described accepted-quote index and allocation lock; and the
`ecash_claims` worker replaces the named settlement and refund tasks. That
worker also supplies the claim's current
[counterexample](falsification-claim-worker-seat-id.md). Regenerate the route,
writer, admission, replay, and continuation enumerations before relying on this
argument.

## Scope and model

This proof supports
[CLAIM-fleet-manager-selects-only-owned-seat-authority](../CLAIM-fleet-manager-selects-only-owned-seat-authority.md).
It covers the official daemon's signed FI service, request verification, seat
persistence and registry, allocation, replay, payment continuations, and
crash/restart behavior. It permits arbitrary valid attacker-signed inputs,
known victim IDs, replay, concurrency with trusted local principals, client
disconnect, and interruption at any process boundary.

The protected capability is Fleet Manager-local seat authority: a `Seat`, its
registry or database identity, local API credential or port, supervisor or
process handle, key, data path, or stored creation commitment. Ordinary public
Fedimint protocol traffic carries none of these capabilities.

## Assumption boundary

The proof grants the claim record's five assumptions. They bottom out request
and quote cryptography, durable and exclusive persistence, official production
wiring and trusted local principals, and detached-task runtime behavior. The
argument does not establish those premises.

## Argument

**S1 (enum + code, historical) — last-reviewed production routing validated,
then authorized first, and the service file was the whole FI surface.**
The historical rosters yielded ten signed verbs: `CreateSeat` and
the nine post-creation types `GetDkgCode`, `StartDkg`, `RestartDkg`,
`GetStatus`, `GetInviteCode`, `GetPeerAttestation`, `SetMetaField`,
`ProposeGuardianFees`, and `GetFedimintStats`. Those nine—and only those
nine—implement
`SeatScopedFiRequest`; `CreateSeat` deliberately does not. The service trait
also has three unsigned verbs. Workspace implementations are the generated
client proxy, production `FleetManagerRpc`, and test-only `TestFleetManager`;
production `main`'s `Serve` branch installs `FleetManagerRpc`. The other two network-facing
tasks `main` wires are the operator admin socket (a trusted principal, A4)
and the Nostr advertiser, which reads only the availability snapshot and
publishes advertisements — no seat authority or FI verb enters through
either. The hidden/argv[0] bundled-fedimintd entries run fedimintd without
constructing a `Fleet`, `Seat`, or FI service; manual invocation is operator
action under A4, not another FI-signed route.

Every signed trait method's first act is envelope verification (`validate`),
and every post-creation method's first fleet call is `Fleet::authorize` on its
own verified request. `service.rs` is one small file containing the entire FI
surface, and no method in it calls an operator verb (listing, decommission,
shutdown), calls `Seat::start`, or opens/reads the seat database directly —
its only seat sources are `Fleet::authorize` and `Fleet::create_seat`. Trait
completeness catches a wholly missing method, but the
authorize-first ordering and those absences are per-method scans of
that file; this is `enum + code`, not a type proof. A4 excludes alternate
implementations.

**S2 (type + code) — successful verification establishes K.**
`SignedRequest::verify` verifies the exact payload bytes under the outer key
and direction/verb-separated digest, parses the typed payload, requires inner
`fi_id == outer fi_id`, and checks freshness. It is the sole constructor of
private-field `VerifiedFiRequest<T>`, whose payload `fi_id` therefore *is* the
verified signer K. With A1 and S1, no handler processes an unverified payload,
and no later layer can manufacture the proof.

**S3 (schema + enum + code, historical) — a durable seat ID/owner binding could
not change.** The last-reviewed sole production seat-row insertion was
`Db::admit_seat`; the sole load was
`Db::list_seats`; the only production seat-row update changes
`decommissioned_at_ms`. `SeatFacts` is built only from successful insertion or
startup decoding. Registry entries are inserted at startup or after fresh
durable creation and are never removed/replaced.

`seats_creation_immutable` rejects updates to every creation column, including
`seat_id`, `fi_id`, `quote_id`, and `port_base`; `seats_no_delete` rejects
deletion. `Db::open` enables recursive triggers on every pooled connection, so
implicit `INSERT/UPDATE OR REPLACE` deletes are rejected too. UPSERT identity
changes hit the immutable-update trigger and duplicate plain inserts hit
primary/unique constraints. With A3, rename/reinsert, delete/reinsert,
replacement, release/reuse, crash, and restart cannot change the binding.

**S4 (type + code) — FI seat selection is one owner-comparing gate.**
`Fleet::authorize` is the only crate-visible getter for an *existing
registered* seat: `seat_by_id` is module-private to `fleet.rs`, and no other
`pub`/`pub(crate)` `Fleet` method or field returns a registered seat.
It takes `&VerifiedFiRequest<T: SeatScopedFiRequest>`, resolves that
request's typed id, compares `seat.facts().fi_id` with that same request's
verified signer K, and returns `UnknownSeat` on absence/mismatch before
consulting any other seat state; the resolved authority is dropped on
mismatch. Visibility plus the private `VerifiedFiRequest` constructor
therefore prevent FI code from *fetching* a registered seat without a
verified, seat-scoped request passing this comparison, and the signature
forces both ids to come from one request object. What the compiler does not
prevent, and S1's scan therefore must exclude, is FI code invoking the
operator methods that select or affect seats internally
(`admin_seat_status`, `decommission_seat`, `seat_summaries`, `shutdown`),
calling `pub(crate)` `Seat::start` to *construct* a parallel seat object
from caller-supplied facts, or reopening the database to read seat rows.
Independent operator use of those methods is A4's trusted-principal premise.
`map_seat_error` preserves `UnknownSeat` on the wire.

**S5 (enum + code) — `CreateSeat` cannot cross identities.** The handler
retains K, verifies the manager-signed quote, and requires its `fi_id == K`
before allocation. For a paid request it also invokes offline `verify_locked`
before entering fleet allocation. Under the allocation lock:

- accepted replay indexes the exact verified quote id; fresh creation and
  startup build that index from a row containing the same quote id, K, and
  exact signed commitment, so A2/A3 permit only K's commitment;
- refusal replay returns no seat authority;
- fresh admission counts non-decommissioned seats in the accepted-quote index
  under the allocation lock (the claim's capacity aggregate: a read-only
  liveness scan that selects no seat and reads nothing else of a sibling),
  commits a row with `fi_id = K`, then constructs/registers/starts that row's
  `Seat`.

Offline verification and the later `spawn_settlement`,
`spawn_accepted_claim`, and `spawn_refund_submit` continuations carry only the
official wallet, quote/federation/payment or refund material, and an
independently cloned quote-level `settling` gate. The concrete wallet has no
`Fleet`, `Seat`, seat DB/id, local seat API authority, supervisor/process
handle, or seat path/key field. Claim/refund methods can emit only ordinary
Fedimint client traffic under the claim definition. A4 excludes substituting a
malicious alternate `EcashWallet` implementation into the official daemon.

**S6 (code + axiom) — interruption does not open a selection gap.** Creation
holds the allocation lock from the idempotency lookup through registry
insertion. Decommission enters the selected seat's command loop, durably
writes only `decommissioned_at_ms`, publishes that state, stops the child, and
closes command admission before replying and exiting; once accepted, the
independently owned loop completes even if its caller is cancelled. Terminal
startup creates no replacement loop. Creation handlers remain detached under
A5. Process death rebuilds the registry and accepted-quote index from
`list_seats` alone, so every selection fact after any interruption derives
from durable rows S3 keeps immutable.

**Conclusion.** S1/S2 establish the official route and K. S3 keeps durable and
in-memory ownership stable. S4 covers every post-creation verb and establishes
the required `UnknownSeat` ordering. S5/S6 cover allocation, replay, the
capacity aggregate, and interruption. Therefore no invocation reaches a
wrong-owner local seat authority or commitment, and no non-Seat payment
continuation supplies an uncovered local seat-selection path. ∎

### Signer-attribution limitation (inside the claim's domain)

A captured, still-fresh victim envelope can be retransmitted because requests
have no nonce/replay store. It remains attributed to the victim K and satisfies
the ownership predicate; this limits actor attribution but is not an
outside-claim residual.

## Residual windows (accepted, outside the claim)

- **R1 — timing:** missing-seat and owner-mismatch paths need not have
  constant response latency despite their identical semantic result.
- **R2 — trusted local control:** operator/admin and startup/supervision may
  independently select every seat under A4; they cannot change ownership or
  the FI authorize-first ordering.

## Weakest links

In order: S1's per-handler authorize-first and
no-operator-verb scan of `service.rs`; A2 quote/hash identity; S3/S5's
regenerated writer, replay, and wallet/settlement enumerations; S4's
comparison/error code; then A1/A3/A4/A5. S4's authorize gate (the sole
`Arc<Seat>`-yielding getter, private `VerifiedFiRequest` construction) and
S3's schema constraints are the strongest rungs — noting S4 leans on S1's
scan to exclude FI use of the operator methods. Tests exercise regressions
but do not establish the claim.

## Regression attack

To attack this argument independently:

1. Enumerate every signed service verb, signing/seat-marker implementation,
   client/server dispatch arm, `FleetManagerService` implementation, production
   wiring, and trait-method body. Try to find a production signed route that
   reaches fleet or seat behavior without verification or, for a post-creation
   verb, without `Fleet::authorize` first. Also search every crate-visible
   `Fleet` method and field for a second seat-selection path, and every FI
   verb for an operator-verb call (listing, decommission, shutdown).
2. Attack signer attribution with attacker signature/victim inner id, victim
   outer id/attacker signature, cross-verb bodies, malformed typed payloads,
   and freshness edges. Attempt to forge or construct `VerifiedFiRequest`,
   obtain an `Arc<Seat>` without `Fleet::authorize`, pass ids from two
   different requests through one authorization, or exploit visibility,
   serialization, deref, clone, or unsafe code.
3. For all nine post-creation verbs, submit a valid attacker-owned envelope
   naming a victim seat, including policy-invalid and unsupported cases.
   Attempt to obtain policy/liveness/unsupported output or handler entry before
   `UnknownSeat`; race decommission, lifecycle verbs, and process death around
   authorization.
4. Enumerate every seat-row writer/load, `SeatFacts` construction, registry and
   accepted-quote insertion/removal/replacement, and capacity-count site. Attack
   every creation column with UPDATE, rename/reinsert, DELETE, plain INSERT,
   same-key/cross-unique/rowid REPLACE, UPSERT, and UPDATE OR REPLACE. Check the
   recursive-trigger setting on every pool connection and restart after each
   boundary.
5. Attack `CreateSeat` with another FI's quote, quote collision/reuse, accepted
   and refusal replay, concurrent duplicate/fresh calls, capacity boundaries,
   decommission races, disconnect, and process death before/during/after the
   durable insert and registry insert. Enumerate offline wallet verification and
   every accepted/refund settlement spawn, captured value, official wallet
   field/dependency, path, and network exit; search for any `Fleet`/`Seat` or
   local seat resource that crosses into those continuations. Search
   specifically for any per-seat scan or returned commitment not fixed by K's
   quote/row.
6. A counterexample is a verified invocation attributed to K that constructs,
   registers, starts, returns the commitment of, or reaches seat-specific
   behavior with local seat S while durable `seats.fi_id != K`, or a verified valid
   missing/wrong-owner post-creation request that produces another semantic
   result before `UnknownSeat`, or any CreateSeat payment continuation that
   obtains a FMan-local seat authority omitted from the selected-seat branch.
