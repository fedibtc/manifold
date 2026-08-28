# Proof: Accepted setup-payment policy is publisher-controlled



Scope: `crates/domain/src/{lib,setup_payment_federations.rs}`,
`crates/domain/src/setup_payment_federations/tests.rs`,
`crates/nostr/src/{lib,setup_payment_federations.rs}`,
`crates/nostr/src/setup_payment_federations/tests.rs`,
`crates/fman/nostr/src/{lib,tests.rs}`,
`crates/nostr-clients/src/**`,
`crates/fman/core/src/{admin,db,fleet,service}.rs`,
`crates/fman/core/src/db/**`, `crates/fman/core/migrations/**`,
`crates/fman/core/tests/{db,fleet,service}.rs`,
`crates/fman/fedimint/src/{lib,setup_payment_policy.rs}`,
`crates/fman/fedimint/src/setup_payment_policy/tests.rs`,
`crates/fman/bin/src/main.rs`,
`crates/service-fleet-manager/src/**`,
`crates/manifold-environment/src/**`,
`SECURITY.md`,
`Cargo.toml`, `Cargo.lock`,
`crates/{domain,nostr,manifold-environment}/Cargo.toml`,
`crates/fman/{bin,core,fedimint,nostr}/Cargo.toml`

## Claim

For the official production Fleet Manager daemon and one data root, after the
database is initialized:

1. every federation ID which the FMan treats as a member of its accepted
   setup-payment set is derived from the content of a complete kind-37707
   Nostr event whose ID and signature verify, whose author is the
   deployment-pinned setup-payment publisher, and which passed all of the
   admission checks enumerated in L2;
2. no actor other than that publisher can add, remove, or replace an accepted
   member, including by operating the configured relay, authoring or replaying
   events under arbitrary keys, choosing an author in request data, or calling
   any FI or operator RPC; and
3. whenever replacement of the retained event removes at least one previously
   accepted federation, the removal, complete replacement event, replacement
   membership, and a newly drawn offer epoch become visible in one SQLite
   commit. No quote or fresh allocation decision can observe the removed
   membership with the preceding epoch, and an outstanding quote carrying that
   preceding epoch is refused rather than accepted.

“Treats as a member” covers every policy consequence in the daemon: selection
for a priced quote, the operator's `accepted` status, and wallet join
reconciliation. The join reconciler deliberately retains wallet
state for removed members, but that state is not acceptance and cannot make a
new quote name the removed member.

The adversary controls arbitrary Nostr relays and event authors, including
forged, replayed, stale, self-authorized, malformed, and oversized candidates;
arbitrary FI RPC bytes and verb concurrency; and crash points before, during,
or after durable writes. The adversary does not control the host, the database
files, the official daemon binary, or the deployment-pinned publisher key.

This claim is about authority over FMan policy and the epoch consequence of a
removal. It does not claim that the pinned publisher chooses a safe or live
federation, that relays deliver updates, that a wallet join succeeds, or that a
removed wallet and its balance are erased.

## Axioms (trusted, not checked here)

- **A1 Nostr cryptography:** Schnorr signatures are unforgeable and the Nostr
  event ID hash is collision/preimage resistant. `Event::verify` therefore
  binds the complete event's ID, author, timestamp, kind, tags, and content to
  the corresponding secret key.
- **A2 publisher pin and deployment integrity:** the Manifold environment
  selected by the official daemon supplies the intended setup-payment
  publisher public key without an attacker-controlled production override;
  the publisher key is uncompromised. Publisher malice or damaging
  misconfiguration is outside this claim, as documented in `SECURITY.md`.
- **A3 invite dependency semantics:** the pinned Fedimint invite parser
  faithfully rejects invalid invites, reports an embedded API bearer secret,
  and derives the canonical federation ID represented by an invite.
- **A4 SQLite and randomness:** SQLite/SQLx provides the stated transaction
  isolation, atomic commit, rollback, and crash durability; the schema and
  foreign/check constraints execute as written. `rand::random()` supplies an
  unpredictable 32-byte value which does not repeat a preceding live offer
  epoch in practice.
- **A5 official single-instance wiring:** the official daemon is the sole
  process with write access to the data root, holds its exclusive data-root
  lock, binds the data root to its selected Manifold environment before
  onboarding or loading fleet state, revalidates retained policy before exposing
  FI RPC, and starts its Nostr runtime once. Safe Rust privacy and module
  boundaries are not bypassed; test-only/direct library callers, memory
  corruption, a modified binary, and external database mutation are excluded.
- **A6 clock:** `Timestamp::now()` reflects the host's approximately correct
  wall clock, so the stated 24-hour future-timestamp check has its intended
  meaning.

## Argument

**L1 (schema + enum + code + test + axiom) — the active publisher and all
retained state belong to one deployment environment, and policy is validated
before FI RPC.** The official `serve` path resolves one
`ManifoldEnvironmentProfile`; no FI RPC or Nostr event supplies its publisher.
Before onboarding, identity access, wallet opening, or policy use, it calls
`Db::bind_manifold_environment`. `Fleet::open` repeats the binding check after
taking the exclusive data-root lock and before loading identity state.

The binding table admits one row whose value is constrained to the three typed
environments. Triggers reject every later insert/replacement, update, or delete.
The first-binding statement is conflict-safe: concurrent initial callers can
establish only one environment, and every loser rereads that winner and fails
on mismatch. The in-place migration rewrite deliberately changes the checksum
of preceding experimental databases, so an unbound old database fails
migration rather than acquiring whichever environment is selected after the
fact. `a_data_root_is_bound_to_its_first_manifold_environment`,
`concurrent_first_environment_bind_has_one_winner`, and
`a_fleet_refuses_a_different_environment_after_restart` pin these paths.

Within that bound environment, `main` obtains `setup_payment_publisher` only
from the resolved profile and passes the same value to retained restore and the
Nostr runtime. When it is `Some`, startup statically restores the retained
event under that publisher *before* constructing and spawning the FI RPC
router; a changed publisher therefore fails without serving the old policy.
When it is `None`, the official current environment mapping has no path that
admits membership for that bound environment, and a future positive price is
refused. A2/A5 cover the deployment profile and official wiring.

**L2 (code + test + axiom) — every inbound replacement passes the complete
kind-37707 admission predicate.** `refresh_setup_payment_federations` fetches
at most 64 candidates, then passes each candidate and the independently pinned
publisher to `admit_setup_payment_federations_event`. Its static admission,
before constructing the opaque admitted value:

1. rejects content over 128 KiB before event cryptography;
2. verifies the event ID and signature;
3. requires `event.pubkey` to equal the pinned publisher;
4. requires kind 37707;
5. requires exactly one two-element `d` tag equal to
   `setup-payment-federations`;
6. strictly decodes the version-1 JSON shape, rejects unknown/duplicate fields,
   more than 16 entries, empty/invalid invites, and invites over 16 KiB;
7. uses the Fedimint parser to reject every invite carrying an API bearer
   secret and derives its canonical federation ID; and
8. rejects duplicate derived federation IDs.

For a fetched candidate it additionally rejects timestamps more than 24 hours
ahead of `now` and requires strict NIP-01 replacement order over the current
durably admitted event (later timestamp, or lower event ID at equal timestamp).
Replaying the identical current event is idempotent. On restart,
`restore_durably_admitted_setup_payment_federations_event` repeats signature,
author, kind, exact-`d`, content, secret, and uniqueness checks on the trusted
atomic retained row; it deliberately does not apply a new clock or rollback
check to that already-admitted high-water mark.

`rejects_invalid_signature_publisher_kind_and_d_tag`,
`accepts_exact_future_bound_and_rejects_one_second_beyond`,
`enforces_nip01_replacement_order_and_allows_replay`, and
`content_bound_precedes_event_crypto` pin event admission. Domain tests
`wire_shape_is_version_and_invite_array_only`, `protocol_bounds_are_pinned`,
`rejects_duplicate_derived_federation_id`,
`rejects_invalid_and_oversized_invites`,
`rejects_secret_bearing_invite`, and
`rejects_oversized_malformed_unknown_and_duplicate_fields` pin content
admission. A1, A3, and A6 supply the external meanings.

`fman_nostr::verify_candidate` is not a second payment-policy path: it is
called for holder-authorization discovery and validates the holder envelope,
but it does **not** itself check kind 37705. Kind selection at that call site
rests on the relay subscription/filter behavior. This omission does not create
a kind-37707 policy writer: the kind-37707 path uses the shared admission
function above, which performs its own local kind check.

**L3 (enum + code + test; FALSIFIED by relay withholding) — only the
highest admitted event can reach the policy store.** `admit` first decodes and
statically restores the complete stored event as the current high-water mark. It folds candidates through L2;
an invalid, stale, or unordered candidate is ignored, while each accepted
candidate becomes the new current value. A store replacement is requested
only if the winning event ID differs from the retained ID, and it carries the
serialized complete winning event and the membership cloned from that same
opaque `AdmittedSetupPaymentFederationsEvent`. A corrupt or no-longer-valid
retained event fails startup/refresh rather than degrading to network-chosen
state once that restore runs. But the high-water mark is only local. If the
publisher signs `E1` and then `E2` above retained `E0`, a malicious relay can
withhold `E2` and reveal historical `E1`; `E1 > E0` passes local rollback
checking and its membership is installed. With no retained event the same
attack can install any otherwise-admissible historical publisher event.
Therefore the relay can select which publisher-authorized historical
membership is activated, falsifying item 2's stale/replay case both during and
after bootstrap. Nostr supplies no global currentness proof or trusted
checkpoint.
`setup_payment_admission_retains_only_the_highest_admitted_event` pins
the winner, idempotent restore, and publisher-rotation failure behavior.

**L4 (enum + code + schema + test + axiom) — event, derived membership, and
removal epoch have one production writer and one atomic commit.** Regenerating
all statements naming `nostr_setup_payment_federations` and
`setup_payment_federation_members` finds only their empty schema creation and
`Db::replace_setup_payment_policy`: one upsert of the complete event, deletion
of the old membership, and insertion of each ID derived in L3. Its sole
production caller is `FleetSetupPaymentPolicyStore::replace`, reached from the
L3 retention branch. The store does not accept raw network IDs: it converts
only the opaque admitted set's canonical IDs.

In that same SQLite transaction the method snapshots every previous member,
computes whether any is absent from the replacement, and, if so, writes a
fresh random `offer_epoch` before commit. Thus crashes expose either the old
event/membership/epoch or the complete new triple. The single-row primary keys
and membership primary key reject duplicate physical state.
`setup_payment_policy_replacement_bumps_the_epoch_only_on_removal` pins initial
emptiness, addition without a bump, removal with a bump, idempotent membership,
and the empty stop-set removal. A4 supplies commit and epoch freshness.

**L5 (enum + code + axiom) — the other epoch writers cannot conceal a
removal.** The complete production enumeration of `offer_epoch` writes is: the
initial migration draw; `Db::set_offered_price`, which changes price and epoch
in one transaction; L4's removal branch; and
`Db::admit_seat`, which advances the epoch in the seat-insertion
transaction when the last slot is consumed. All runtime writers are serialized
with L4 by the same `Fleet::admission` mutex. A concurrent price change or
last-slot allocation can only install another fresh epoch, not restore the
pre-removal epoch. Under A4/A5 no concurrent production writer can make a
removed membership visible with the preceding epoch.

**L6 (enum + code) — every policy decision reads authenticated derived state,
not caller assertions.** Regenerating readers yields the following exhaustive
list:

- `Db::offer_snapshot` reads membership, epoch, price, and capacity in one
  SQLite transaction. `Fleet::quote_offer` supplies it to `get_quote`, which
  requires a priced request's selected ID to be in that membership before
  asking the wallet to quote. The FI chooses among admitted IDs, not whether an
  ID is admitted.
- `Fleet::availability_snapshot` reads the same snapshot, but availability and
  kind-37701 advertisement discovery depend only on offer and capacity. They do
  not publish or synthesize setup-payment membership; `get_quote` remains the
  membership-enforcing boundary.
- `Fleet::payment_federation_statuses` labels as `accepted` only IDs from that
  snapshot. Joined wallet-only leftovers are listed with `accepted: false`.
  `AdminRequest::ListPaymentFederations` only renders this view.
- the wallet join reconciler reads the watch value produced only after L3/L4
  successfully commit. It joins invites in that opaque admitted set and never
  writes acceptance rows. Removal intentionally leaves an already joined
  wallet in place.
- startup alone reads the retained event, and does so through L2's static
  restore. No payment decision reads unvalidated `event_json` directly.

No other use of the admitted-set type, membership table, membership field, or
policy watch makes a payment-policy decision in the scoped production code.
In particular, arbitrary FI verbs cannot write any of these sources.

**L7 (type + enum + code) — the operator has no membership mutation verb.**
The complete `AdminRequest` enumeration has `ListPaymentFederations` but no
add, remove, replace, import, or publisher-selection variant. Dispatch for the
list calls only `payment_federation_statuses` and serializes its result. The CLI
subcommand vocabulary likewise has only `payment-federations list`. The
operator can set price and withdraw balances, neither of which changes
membership. Safe Rust exhaustiveness makes adding an enum variant fail to
compile until dispatch and CLI mapping are considered; the no-writer result
itself remains an `enum` obligation.

**L8 (code + axiom) — after successful restore, a removal cannot race an
old-epoch allocation acceptance.** `get_quote` copies the epoch from the same snapshot that admitted
the selected federation into the signed quote. `Fleet::create_seat` rejects an
epoch mismatch once before acquiring `Fleet::admission`, then acquires that
same mutex used by L4 and rereads an atomic offer snapshot. It refuses with
`OfferChanged` if the quote epoch differs; only after that guarded recheck can
it sign acceptance and insert the seat. Therefore either allocation wins the
mutex and commits before removal, or removal commits its new epoch first and
allocation refuses. A crash during removal has L4's old-or-new atomic outcome;
a crash during allocation cannot bypass the guarded durable recheck.

`a_priced_quote_refuses_a_federation_outside_the_accepted_set` pins quote
selection, while `quote_epoch_changes_only_when_quote_settings_change` and
`create_is_atomic_idempotent_and_rebuilds_as_live` pin generic stale-epoch
refusal for price/capacity changes. They do not exercise a concurrent
policy-removal/allocation race, so this lemma rests on the code ordering and
A4/A5 rather than a `test` rung.

**Conclusion (falsified).** L1 repairs the two startup counterexamples
found by the first check. L2 and L4–L8 establish the remaining mechanisms: a
candidate presented to kind-37707 admission is fully checked, and under the
bound, successfully restored publisher the event/membership/removal-epoch
transaction and allocation lock are sound. They still do not establish the
absolute claim. L3 lets a relay select a historical publisher event by
withholding a newer one whenever the local high-water mark lags, including but
not limited to empty bootstrap. That is an in-claim counterexample, not a
residual window. ∎

## Residual windows (accepted, outside the claim)

- **R1 publisher authority:** the pinned publisher may intentionally or
  accidentally publish an empty stop set, attacker-controlled public
  endpoints, or otherwise unsafe federation choices. This is the authority
  the claim distinguishes from attackers; `SECURITY.md` assigns compromise or
  damaging misconfiguration to out-of-band key/configuration recovery.
- **R2 future timestamp blocking:** an authenticated event up to 24 hours in
  the future can outrank normally timestamped publisher updates until time
  catches up. Only the pinned publisher can create it under A1/A2, so it does
  not give another actor policy authority.
- **R3 resource exhaustion:** the FMan content parser bounds signed content and
  the fetch caps candidate count, but this record does not prove a complete
  relay frame or all relay work is resource-bounded. Availability and compute
  exhaustion are outside this policy-integrity claim.
- **R4 removed wallet retention:** removing a member stops new quotes and
  invalidates outstanding ones, but does not delete its wallet database or
  balance and does not prevent operator withdrawal. Retained wallet state is
  explicitly not accepted membership.
- **R5 additions preserve outstanding quotes:** adding members does not draw a
  new epoch because no prior member was invalidated. An outstanding quote
  against an unchanged member remains valid; the claim requires a fresh epoch
  for removals, not every publication change.

## Weakest links

1. **L3 (`code`) — broken by relay withholding:** replacement order protects
   only relative to the locally retained event; it cannot establish that any
   relay-supplied publisher event is globally current.
2. **L4/L5/L8 (`enum` + `code` + A4/A5):** conditional removal safety depends
   on writer enumeration, the shared mutex, and SQLite transaction boundaries
   rather than a schema constraint or direct removal/concurrency test.
3. **L6/L7 (`enum` + `code`):** absence of a second policy source or mutation
   verb is maintained by scoped enumeration, not a lint or capability type.
4. **L2 (`code` + `test` + A1/A3/A6):** event admission is strong, but crypto,
   invite-dependency semantics, and the clock bottom out in axioms.
5. **L1 (`schema` + `code` + `test` + A2/A5):** the repaired environment binding
   is durable and tested, while the environment-to-publisher mapping and
   official startup wiring remain deployment/code premises.
