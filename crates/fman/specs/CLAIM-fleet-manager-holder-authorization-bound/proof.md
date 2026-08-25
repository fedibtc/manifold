# Proof: Admitted Holder authorizations are cryptographically bound



Scope: `crates/fman/nostr/src/{lib,tests}.rs`,
`crates/nostr/src/fman.rs`,
`crates/nostr-clients/src/{holder,nostr_relay_client}.rs`,
`crates/domain/src/trust_score.rs`,
`crates/fman/bin/src/{main,telemetry_registration}.rs`,
`crates/fman/core/src/**`,
`crates/fman/core/migrations/**`,
`crates/fman/core/tests/db.rs`,
`crates/service-fleet-manager/src/telemetry.rs`,
`crates/{domain,nostr,nostr-clients}/specs/**`,
`crates/fman/specs/{ARCH-fleet-manager-identity,SPEC-advertisement,SPEC-admin-socket,SPEC-fi-rpc,SPEC-guardian-telemetry-proxy}.md`,
`SECURITY.md`, `Cargo.toml`, `Cargo.lock`, `flake.nix`, `flake.lock`,
`crates/{domain,nostr,nostr-clients}/Cargo.toml`,
`crates/fman/{bin,core,nostr}/Cargo.toml`

## Claim

For every holder-authorization envelope the official Fleet Manager daemon
admits from Nostr, reports through its operator or FI-facing interfaces,
republishes in an advertisement, or sends in a telemetry registration, let `S` be
`envelope.holder_authorization.authorization` and let `C` be
`envelope.signed_credential.credential`. The envelope has all of these
properties:

1. its authorization proof verifies under `S.holder_id_pubkey` over the
   credential SDK's domain-separated canonical encoding of all of `S`;
2. `S.subject_pubkey` equals this daemon's Nostr service public key; and
3. `S.credential_digest` equals `C.digest()`.

Here **holder means only the public key in `S.holder_id_pubkey`**. “The holder
made the authorization” means that the envelope contains a signature verifying
under that key, subject to the cryptographic axioms below. It does not mean
that a named person controlled the key, that this key is the holder encoded in
`C`, or that `C` has a valid issuer proof. “This credential” means the
`Credential` payload `C`, which is what its digest covers, not the adjacent
`SignedCredential.proof` bytes.

The adversary controls the configured Nostr relay, omission and ordering of
its responses, and arbitrary event-author keys and event bytes. It may replay
authentic events and may return events that do not match the requested kind or
tags. It cannot forge a signature under an uncompromised key or find the hash
collisions excluded by A1. The claim covers the daemon's durable authorization cache, current in-memory
authorization vector, the operator's `AuthorizationObserved` projection, the
FI-facing `GetFederationTrustMaterial` response, kind-37701 advertisement
carriage, and guardian telemetry-registration carriage. It makes no claim about a relying consumer's acceptance of that
material.

## Axioms (trusted, not checked here)

- **A1 cryptography and canonicalization:** BIP-340 signatures are
  unforgeable; SHA-256 is collision- and second-preimage-resistant; and the
  pinned credential SDK faithfully implements its documented JCS encoding,
  holder-authorization signature domain, `HolderAuthorization::verify`, and
  `Credential::digest`. Thus a proof accepted by `verify` authenticates every
  field of `S` under `S.holder_id_pubkey`, and distinct credential payloads do
  not have the same digest. `nostr_sdk::Event::verify` faithfully checks the
  event ID and signature over the complete event.
- **A2 official-process integrity:** the official binary and its pinned
  dependencies execute the reviewed code without memory corruption or code
  injection. The daemon derives one Nostr service key from its fleet
  identity, gives it to `FleetManagerNostr`, and binds that concrete runtime's
  `NostrTrustMaterialSource` into `FleetManagerRpc`, passes that same runtime
  to the telemetry worker, and gives its durable store only to that runtime.
  Arbitrary library callers that inject another trust source, construct or
  mutate a store directly, or compose these public crates differently are
  outside the claim.
- **A3 value preservation:** Rust ownership, Serde/JSON round trips, SQLite
  parameter binding and committed row reads, and Tokio watch cloning preserve
  the typed fields of an accepted envelope between verification and each sink. An attacker cannot mutate an envelope behind a
  shared safe-Rust reference after its checks.

## Argument

**L1 (code + test + axiom) — one candidate cannot cross admission without the
three claimed bindings.** `verify_candidate` first calls `Event::verify`, then
parses the version-1 `HolderAuthorizationEventContent`. It requires the
content's holder string to equal the authenticated event author. It calls the
SDK `HolderAuthorization::verify`, requires the returned statement holder to
equal that same author, and requires the statement subject to equal the
`fman_pubkey` argument. Finally it computes `digest()` from the inline
`SignedCredential.credential` and requires equality with the signed statement
digest before returning the envelope. A1 gives these checks their
cryptographic meaning.

`candidate_verification_accepts_our_authorizations_and_rejects_others` pins the
supported-version and cross-FMan subject cases. The remaining author, proof,
and credential-digest comparisons are on the `code` rung, not promoted to
`test` by that test.

The function rejects a signed authorization `issued_at` beyond the
receiver-computed one-hour future-skew bound. It does **not** locally check event
kind, Nostr `created_at`, or any tag. The fetch filter is an untrusted relay
hint, and no part of this lemma relies on it: an off-kind or wrongly tagged
event still needs the valid inner authorization signature and all three
bindings above to be returned.

**L2 (enum + code + axiom) — only L1 successes enter durable or runtime
authorization state.** Regenerating production writers finds one nonempty
admission route: the runtime's initial refresh and every later explicit
admin-triggered refresh call
`fetch_holder_authorizations`, which retains only successful L1 verifications,
selects the greatest admissible signed `issued_at` for each credential digest,
and passes the complete verified event to the durable store. In one immediate
SQLite transaction, the merge rejects a future-issued batch, inserts a new
digest only while the service-wide 64-row bound has room, or replaces an
existing row only with a greater signed `issued_at`; it does not synthesize or
modify an envelope under A3.

The refresh reloads the bounded retained event set and reruns L1 before
replacing the runtime watch. Startup first removes legacy rows outside the
aggregate or future-time bounds, then follows the same load-and-reverify path
before seeding that watch. A fetch error, equal/older replay, store failure, or
revalidation failure does not write a new runtime vector. An empty response
performs no merge, but its reload can remove a row that has moved beyond the
receiver-time bound after a backward wall-clock step. Thus every durable row
admitted through the official process was an L1 success, and every nonempty
runtime value is freshly reconstructed from L1 successes.

**L3 (enum + code + axiom) — every official report and republication sink is a
projection or clone of L2 state.** Regenerating consumers of the two watches
and `holder_authorizations()` gives the complete official sink list:

- `onboarding_status` derives its count and holder list from the verified local
  vector, and `AdminRequest::Onboarding` serializes only that projection;
- `run_advertisements` clones the authorization watch, `advertise_once` places
  that vector in the payload, and the daemon signs and publishes the complete
  advertisement; and
- the binary's concrete `NostrTrustMaterialSource::holder_authorizations`
  clones the same watch. The official binary binds that source once into the
  RPC service; `get_federation_trust_material` copies it into the response and
  signs the complete material.
- the telemetry worker clones the same runtime vector, deterministically selects
  one envelope, and passes it to `Fleet::telemetry_registration_requests`.
  Fleet clones it into every eligible seat request; the worker serializes the
  complete request and sends it to the deployment-pinned HTTP receiver.

The presence watch is updated immediately before the envelope watch, so the
operator projection and concrete carriage may transiently describe different
successful enrollments. Each value is nevertheless independently derived from an
L1-verified vector. The daemon publishes no standalone FMan-authored holder
authorization event. A2 excludes arbitrary trait implementations available to
non-official library users.

**L4 (code) — duplicate and multi-holder reporting cannot invent a holder.**
The refresh and durable merge retain at most one envelope for each credential
digest and at most 64 for the FMan identity, choosing only a strictly greater
admissible signed authorization `issued_at` for a replacement. Replays,
equal/older wrappers, and relay ordering therefore do not increase the stored or
reported authorization count for that digest. Different digests signed by one
holder count separately until the aggregate is full. Envelopes signed by
multiple holder keys also count separately unless their credential digests
collide; the first admitted L1-valid envelope for one digest remains until a
later-dated L1-valid envelope for that digest is enrolled or receiver-time
normalization removes it after a backward wall-clock step.

`onboarding_status.authorizations` is exactly the retained vector length. Its
`holders` are copied from each signed statement, sorted, and deduplicated, so
one holder appears once even when it has several credentials. The status is
`AuthorizationObserved` precisely when that vector is nonempty. These count
semantics can be influenced by any hostile key willing to sign, but they cannot
attribute an entry to a different statement holder without breaking L1.

**L5 (enum + code) — onboarding observation is not itself a verb
authorization gate.** The production readers of `DirectoryPresence.onboarding`
are the admin status renderer and its watch plumbing. No fleet, seat, payment,
setup, or FI verb branches on `AuthorizationObserved`. The telemetry worker
instead reads and selects a concrete L1-verified envelope from the authorization
vector; its registration side effect is the third carriage sink enumerated in
L3, not authority granted by the presence enum.

**Conclusion.** L1 establishes the statement-signer, this-FMan subject, and
credential-payload bindings at the only admission function. L2 preserves them
in all nonempty daemon state, and L3–L5 enumerate the report, republication,
telemetry carriage, counting, and presence-gate consequences. Under A1–A3 an adversary cannot cause
the official daemon to admit, report, or republish an envelope lacking any of
the three properties in the claim. ∎

## Residual windows (outside this claim)

- **R1 historical, not current, authorization:** admission rejects a signed
  authorization more than one hour ahead of the receiver clock, but does not
  check Nostr `created_at`, impose a lower freshness bound, or provide holder
  retraction/revocation. The durable store prevents same-digest rollback after
  admission. These rules satisfy the claim's historical signature predicate
  but disprove any stronger claim of current holder intent.
- **R2 no credential-holder or issuer validity:** a hostile key `H` can sign an
  authorization for a credential whose `blind_msg` names victim `V`, attach
  arbitrary issuer-proof bytes, and drive observation, public carriage, and
  deterministic telemetry-envelope selection. The daemon
  checks neither the credential holder binding nor its PBRSA issuer proof,
  schema, issuer trust, or revocation. The authorization proof does not bind
  `SignedCredential.proof`: a hostile event author may choose arbitrary proof
  bytes, and a relay may select among holder-signed variants. Complete trust
  verification belongs to relying
  consumers under root residual R3; this leaf proves only that `H` signed the
  stated subject and credential-payload digest.
- **R3 arbitrary signers and nonconforming events:** there is no trusted-holder
  allowlist. Any key may create a valid statement and trigger
  `AuthorizationObserved`, FI response carriage, advertisement carriage, and
  telemetry registration attempts for eligible seats.
  A hostile relay may also supply it in an event with the wrong kind or wrong
  `d`, `p`, `t`, `issuer`, `credential`, or `schema` tags because those are not
  local admission checks. The claim deliberately does not call admitted events
  protocol-conforming kind-37705 publications.
- **R4 relay-selected visibility:** the relay controls omission and can consume
  the 64-candidate cap with junk. Omission cannot empty enrolled state, but it can prevent initial enrollment or
  hide a later authorization indefinitely. A returned greater-`issued_at`
  same-digest event does replace the prior exact envelope. The cache therefore
  provides monotonic per-digest version carriage, not append-only history or completeness
  or live issuer/revocation validity; relying consumers must check the latter.
  Arbitrary valid signers can also fill the 64-row service-wide durable set.
  Admission then preserves existing rows and same-digest updates but refuses
  every new digest; there is no holder or federation sub-pool because all rows
  authorize the same FMan service identity.
- **R5 public library seams:** `TrustMaterialSource`, the Nostr runtime
  constructor, and the durable store are public. Tests or another library
  consumer may inject or persist arbitrary bytes, or compose different sinks.
  A2 and the claim quantify only over the official binary wiring.

## Weakest links

1. **L1 (`code` + A1):** the decisive checks are adjacent and simple, but most
   lack focused regression tests and their meaning bottoms out in the pinned
   credential SDK and cryptographic axioms.
2. **L2/L3 (`enum` + `code` + A2/A3):** the absence of another official writer
   or sink is a scoped enumeration, while the public trust-source trait makes
   the official-binary boundary essential.
3. **L5 (`enum`):** the absence of a presence-enum authorization gate is not
   enforced by a capability type or lint and must be regenerated when the
   scoped daemon code changes.
4. **L4 (`code` + `test`):** counting is not a security gate. The database
   regression test pins monotonic merge and restart retention, while exact
   holder counting remains local code.
