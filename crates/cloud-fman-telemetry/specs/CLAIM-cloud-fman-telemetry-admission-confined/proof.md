# Proof: Cloud FMan telemetry admission confinement

## Scope

This implementation-grounded proof covers:

- every Rust source file and migration under
  `crates/cloud-fman-telemetry/{src,migrations}` for the authority-writer
  enumeration, with the detailed handler and persistence argument concentrated
  in `admission.rs`, `auth.rs`, `cipher.rs`, `config.rs`, `server.rs`, and
  `store.rs`;
- the registration wire type in `crates/service-fleet-manager/src/telemetry.rs`;
- `crates/cloud-fman-telemetry/tests/daemon_e2e.rs`; and
- the assumed verifier contract in
  `crates/peer-badge-verifier/specs/SPEC-peer-badge-verifier.md`.

The protected durable authority tuple for one canonical FMan key is its
`fman_pubkey`, Iroh endpoint, capability, and capability generation. Lease,
authorization time, registration revision, operational status, display name,
metric state, and journal state are not authority fields. The endpoint and
capability are inside `TargetSecret`; the other two protected fields select and
bind that ciphertext.

## Model and quantifiers

The adversary chooses every request byte, header, connection identity, Nostr
identity, credential envelope, endpoint, capability, generation, arrival order,
and cancellation point. Requests for the same key may run concurrently. The
process may restart between any committed transactions. Targets may be active,
expired, or quarantined.

The durable predicate is a committed `targets` row containing a new protected
tuple. Holder currentness has exactly the imported verifier semantics:
credential and authorization time are evaluated at the verifier call's
requested time, while authority and revocation currentness mean the state
observed by each relevant completed relay read. A publication after its
corresponding completed read is not retroactive. The argument quantifies over
every production handler and internal call to the authority writer, and every
SQL writer of the protected tuple. It does not grant the requester filesystem,
SQLite, encryption-key, issuer, verifier-dependency, or process-code control.

## Assumptions

This proof treats each direct assumption in
[the claim](../CLAIM-cloud-fman-telemetry-admission-confined.md) as an axiom:
the pinned Nostr primitives and bounded clock verify signatures and freshness;
the peer-badge verifier completely enforces its linked contract or fails closed;
and SQLite transactions plus encrypted persistence satisfy their documented
contracts or fail detectably. These assumptions respectively bottom out the
request proof, Holder currentness, and durable/cryptographic operations below.

## Argument

1. **[type, code] The wire and route boundary is closed.**
   `GuardianTelemetryRegistrationRequest` has typed protocol, `u64` generation,
   32-byte capability, and Holder-envelope fields and denies unknown fields.
   `public_router` exposes only `POST /v1/telemetry/registrations`; the private
   router has no protected-authority mutation route. `register_inner` reads at most
   `MAX_GUARDIAN_TELEMETRY_REGISTRATION_BYTES`, parses that exact byte slice only
   after authenticating it, and rejects an endpoint that is not an `EndpointId`.
   Configuration restricts the signed public base to one HTTPS origin without
   credentials, path, query, fragment, or trailing slash, so `expected_url` is
   exactly that configured origin plus the fixed route. The claim deliberately
   protects this configured URL; correspondence with an external reverse-proxy
   route is a deployment concern outside this leaf.

2. **[code, test, assumption] NIP-98 establishes one signer and the claimed
   request commitments.**
   `auth::verify` accepts only the `Nostr ` scheme, decodes one Nostr HTTP-auth
   event, checks kind and canonical public-key representation, invokes the
   assumed signature verifier, and compares the first `u`, `method`, and
   `payload` tags with the configured URL, literal `POST`, and SHA-256 of the
   submitted body. The claim names this digest commitment rather than assuming
   collision resistance or asserting information-theoretic body equality. It
   accepts timestamps only in `[now - 60, now + 5]`. The named
   test `exact_url_body_signature_and_freshness_are_required` pins success and
   wrong-URL, changed-body, and stale rejection. The POST-only route pins the
   actual HTTP method; malformed/tampered header sanitization has its own named
   test.

3. **[schema, code] Replay cannot bypass the claimed commitments or later
   checks.**
   Before network verification, `reserve_auth` holds `BEGIN IMMEDIATE` and
   inserts the signed event id into the `auth_events` primary key. A retained
   duplicate loses the unique insert. Retention extends five seconds beyond the
   maximum accepted event age under the bounded-clock model. Even if a clock
   discontinuity later makes a pruned event acceptable again, the reused event
   must still carry the claimed configured URL, POST method, and submitted-body
   digest commitment, and the handler repeats complete Holder verification plus
   durable ordering. Reservation and admission are intentionally separate
   transactions, but no authority write occurs between them.

4. **[code, assumption] Holder verification and subject binding precede the
   writer.** After reservation, `register_inner` passes the body-carried
   `holder_authorization` to the assumed complete `PeerBadgeVerifier::verify`.
   Every error returns before admission. It extracts `VerifiedPeerBadge.subject`
   and requires its canonical key string to equal the NIP-98 signer. It then
   derives the display name from that same signer and constructs
   `TargetMaterial` with `fman_pubkey = auth.signer`; there is no request field
   from which a different FMan key can be selected.

5. **[enum, code] All production authority writes cross that boundary.** A fresh
   enumeration finds one non-test `Store::admit` call, in `register_inner`, and
   one non-test `TargetMaterial` constructor, at that call. `Store::admit`
   contains the only non-test `INSERT`/upsert of `targets`. The only other
   `UPDATE targets` is test-only `set_status`, which writes only status and
   registration revision. Migration SQL defines rows but supplies no authority
   values. Direct SQL mutation and other `admit` calls occur only inside
   `#[cfg(test)]` unit-test modules. The exported feature-gated test router still
   routes through the same `register_inner`; its explicit verifier remains
   subject to the verifier assumption.

6. **[schema, code, test] The transaction serializes the high-water decision and
   write.** `Store::admit` acquires `BEGIN IMMEDIATE` before reading the row keyed
   by the `fman_pubkey` primary key. Concurrent admissions therefore observe a
   total order. The transaction rejects a generation below the stored
   generation and an authentication timestamp below the stored timestamp. For
   the same generation it authenticates and decodes the current secret, then
   requires the capability to be byte-identical; only the endpoint may differ.
   For a greater generation it permits capability and endpoint rotation. The
   upsert conflict target is `fman_pubkey` and its update arm never assigns that
   column. `ordering_replay_expiry_and_restart_are_durable` pins lower-generation
   rejection, same-generation capability rejection, same-generation endpoint
   change, replay, expiry, quarantine preservation, and restart. The concurrent
   transaction shape is pinned by
   `concurrent_admissions_acquire_write_reservation_before_reading`.

7. **[schema, code] Lease and status cannot impersonate authority.** The schema
   closes status to `active|quarantined`, generation to nonnegative values, and
   registration revision to positive values. Admission refreshes `lease_until`
   but preserves an existing row's status. Expiry is a comparison, not an
   authority writer; renewal after expiry still runs all preceding checks.
   Test-only quarantine/reactivation changes status and revision only. Metric and
   journal paths read fenced authority material and update their own tables; the
   transactional exposition read may delete ineligible metric snapshots and bump
   their cache revision, but none writes a protected field.

8. **[code, test, assumption] Secret persistence binds deployment and row
   identity.** Startup accepts exactly 32 key bytes and, on Unix, rejects a key
   file readable by group or other. A sentinel authenticates the trust profile,
   key id, and key before the store opens existing state; the adjacent plaintext
   secret-format version is separately compared with the supported version.
   Target encryption uses AES-256-GCM with a nonce supplied by `OsRng`; nonce
   behavior belongs to the claim's encrypted-persistence assumption. Target AAD
   contains the format, secret kind, trust profile, key id, FMan key, random
   target id, generation, and authentication timestamp. Same-generation
   admission must authenticate the old secret under those values before
   comparison; worker resolution repeats authentication under the current row.
   Encryption, serialization, or
   authentication failure aborts the authority transaction. The named test
   `target_ciphertext_rejects_bound_metadata_and_deployment_transplants` changes
   target, generation, key id, and trust profile and observes fail-closed
   behavior. The restart test pins key mismatch. Its scan for an already
   replaced endpoint is only a limited confidentiality sanity check and is not
   evidence that current target material is absent from every SQLite file.

9. **[code, test, assumption] Failure and cancellation cannot expose a partial
   authority decision.** Generation conversion and transaction acquisition
   occur before any authority state changes. Every later fallible admission
   operation occurs inside one SQLite transaction, and `finish_immediate`
   commits only the complete success result and otherwise rolls back. Dropping a
   transaction rolls it back under the SQLite/sqlx assumption;
   `cancelled_immediate_transaction_does_not_poison_pool` pins connection reuse
   after cancellation. A timeout may make the HTTP outcome ambiguous at the
   commit boundary, but any committed row was computed only after lemmas 1--8.
   Restart reopens the same high-water row only after checking the environment/key
   sentinel. No startup, shutdown, polling, readiness, or response path
   reconstructs or mutates authority fields.

Together the handler closure, subject equality, sole-writer enumeration, and
serialized update rules imply that every committed protected tuple came from a
fresh request carrying the claimed configured-URL, POST-method, and submitted
body-digest commitments for that same FMan and a completely verified Holder
subject under the imported observation semantics. The high-water and
same-generation branches establish the additional replacement restrictions.

## Residuals

Compromise of an accepted FMan capability, issuer authority, verifier
implementation or dependencies, collector encryption key, process binary, or
collector database/backup is outside the adversary model. Availability failures
from source budgeting, replay-slot consumption, verifier relay failure,
admission saturation, or SQLite contention do not create authority state.

Revocation or authority publication after the corresponding completed relay
read is not retroactive under the imported verifier contract; a subsequent
registration performs fresh reads. A timeout or disconnect concurrent with a
successful commit may hide success from the caller, but it cannot change which
authority tuple committed.

## Weakest links

The Nostr primitives, wall clock, verifier completeness, and the SQLite plus
encrypted-persistence contract remain assumptions. The production call-site and
SQL-writer closure is an `enum` and must be regenerated after any scoped change.
Several NIP-98 rejection branches remain `code` rather than focused-test rungs.
The AEAD test attacks metadata transplants on the same-generation read path; the
higher-generation path intentionally replaces rather than authenticates the old
secret and relies on SQLite integrity, with database compromise excluded.
