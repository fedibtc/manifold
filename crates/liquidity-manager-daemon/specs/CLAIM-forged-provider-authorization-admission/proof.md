# Current argument

## Current counterexample

`allocations.request_json` durably stores each accepted `RequestLiquidityRequest`, including its FMan-subject `fman_endorsement`. The claim quantifies over every durably retained envelope but requires every envelope subject to equal this FLIP provider key. The retained FMan envelope violates that conclusion by construction.

## Argument

**L1 (`code` + `test` + axiom) — one candidate cannot cross admission without
the four claimed bindings.** `verify_candidate` calls `Event::verify`, then
deserializes the content as `HolderAuthorizationEventContent`, whose `version`
is `ProtocolV1` and so rejects any other wire version. It requires the
content's holder string to equal the authenticated event author. It calls the
SDK `HolderAuthorization::verify`, requires the returned statement's holder to
equal that same author, and requires the statement's subject to equal the
`provider_pubkey` argument. It then computes `digest()` from the inline
`SignedCredential.credential` and requires equality with the signed statement
digest before returning. A1 gives these checks their cryptographic meaning.

`rejects_an_authorization_naming_another_provider`,
`rejects_an_authorization_republished_by_a_stranger`,
`rejects_a_different_badge_swapped_under_a_signed_authorization`,
`rejects_content_carrying_an_unsupported_wire_version`, and
`rejects_a_badge_bound_to_another_holder` pin the subject, author,
credential-payload, version, and holder-binding comparisons. The
statement-holder comparison remains on the `code` rung.

The function checks no event kind, timestamp, or tag. The fetch filter is an
untrusted relay hint and no part of this lemma relies on it: an off-kind or
wrongly tagged event still needs the valid inner authorization signature and
all four bindings to be returned.

**L2 (`enum` + `code`) — nothing surfaces from the durable store without
passing L1 again.** Regenerating the readers of `holder_authorization_events`
gives exactly two. `retained_count` runs `COUNT(*)` and verifies nothing; its
value reaches only `RefreshOutcome.retained`, which no Admin API response
carries and only tests read. Every other read is `load_verified`, which selects
each retained `event_json`, reparses it as an `Event`, and re-runs
`verify_candidate` against the currently installed provider key, dropping rows
that fail with a warning.

This lemma does not depend on enumerating writers. Because admission is re-run
at read time, a row is a relay answer this daemon once accepted rather than an
assertion, so the four bindings hold for anything surfaced regardless of how
the row reached the table — including rows placed there by `restore_backup`
replacing the data directory wholesale. Under A1 an adversary who can write the
table still cannot produce a row that surfaces without a valid signature
binding the statement to its author, this provider as subject, and an inline
credential payload issued to that same author.

**L3 (`enum` + `code`) — every report and republication sink is a projection of
L2.** Regenerating the call sites of the module's public functions outside its
own tests gives the complete official sink list:

- `advertisement::republish` calls `provider_trust_envelopes`, places the
  result in `LiquidityProviderAdvertisement.holder_authorizations`, signs the
  payload, and publishes it to the configured relays;
- `advertisement::public_readiness` calls `provider_trust_envelopes` and uses
  only whether the result is empty;
- `admin::get_holder_authorization_state` and
  `admin::refresh_holder_authorizations` call `status`, which projects
  `load_verified` through `status_of`;
- `holder_authorization::run_initial_read_task` calls `reconcile_now` once per
  runtime generation, which is the same `refresh` the operator's verb runs.

`provider_trust_envelopes` maps `load_verified` to its envelopes and applies no
further filter, so each carried envelope is exactly an L1 success re-derived at
read time. `test_support::enroll_provider_trust_envelope` also calls `refresh`,
but A2 scopes the claim to the official binary.

**L4 (`code` + `test`) — retention is one row per credential payload, advanced
only by a later-dated authorization.** Within one reconciliation, `refresh`
keeps the greatest signed `issued_at` per credential digest across every
answering relay. The `merge` upsert inserts a new digest, and replaces an
existing row only where the incoming signed `issued_at` is strictly greater;
`issued_at` is stored big-endian so SQLite's byte comparison is unsigned
numeric order across the whole `u64` range.
`merge_replaces_only_on_a_strictly_greater_issued_at` and
`merge_orders_by_unsigned_value_beyond_the_signed_range` pin both properties.
Replays, equal-dated events, and relay ordering therefore do not displace an
enrolled authorization or increase the stored count for that digest. Distinct
digests signed by one holder count separately, as do envelopes signed by
different holder keys.

**L5 (`enum`) — the reported counts are not an authorization gate.** The
production readers of `HolderAuthorizationStatus` are the two Admin API
responses in L3. Its non-observed states carry only timing and relay-failure
text, never envelope content, and `AuthorizationObserved` outranks them, so the
status cannot report fewer authorizations than are enrolled. No setup, allocation, funding, withdrawal, or public RPC verb
branches on it. `public_readiness` branches only on whether the envelope list
is empty, which gates publishing this daemon's own advertisement and grants no
requester anything.

**Conclusion.** L1 establishes the statement-signer, this-provider subject,
credential-payload, and credential-holder bindings at the only admission
function. L2 makes those
bindings a property of every read rather than of the write path, L3 enumerates
the sinks that consume those reads, and L4–L5 bound the counting and gating
consequences. Under A1–A3 an adversary cannot cause the official daemon to
admit, retain, report, or republish an envelope lacking any of the three
properties in the claim. ∎

## Residual windows

- **R1 historical, not current, authorization:** neither `verify_candidate` nor
  the SDK `verify` call checks the Nostr `created_at` or the signed
  `issued_at` against the clock. The merge prevents same-digest rollback after
  admission, but there is no expiry, holder retraction, or protection against a
  stale or future-dated event being the first admitted value for a digest. This
  satisfies the claim's historical signature predicate and disproves any
  stronger claim of current holder intent.
- **R2 no issuer validity:** a hostile key `H` holding any credential that
  names `H` in `blind_msg` can attach arbitrary issuer-proof bytes and drive
  enrollment, operator reporting, and advertisement carriage. The daemon checks
  the credential's holder binding (property 4) but not its issuer proof, schema,
  issuer trust, or revocation. This is deliberate; `SPEC-flip-advertisement`
  places complete envelope verification on the relying app. The authorization
  proof does not bind `SignedCredential.proof`, and
  `admits_either_issuance_of_one_badge_payload` pins that a re-issued credential
  with the same payload substitutes freely.
- **R3 arbitrary signers and nonconforming events:** there is no trusted-holder
  allowlist. Any key holding a credential bound to itself may create a valid
  statement naming this provider and so reach `AuthorizationObserved` and
  advertisement carriage. Property 4 stops one holder wearing another's badge;
  it does not stop an uninvited holder wearing its own. A hostile relay may
  supply it in an event with the wrong kind or wrong `d`, `t`, or `p` tags,
  because those are not local admission checks. The claim deliberately does not
  call admitted events protocol-conforming kind-37705 publications.
- **R4 relay-selected visibility:** each relay's answer is capped at 64
  candidates and the relay chooses what to return, so it can consume the cap
  with junk. Omission cannot empty enrolled state, because nothing deletes
  rows, but it can prevent initial enrollment or hide a later authorization
  indefinitely. The store therefore provides monotonic per-digest version
  carriage, not completeness or live validity.
- **R5 enrolled state is not withdrawable:** no verb deletes from
  `holder_authorization_events`. An operator who wants to stop advertising an
  enrolled authorization has no direct means; the available levers are
  disabling ready advertisement publication or restoring a backup. Accepted by
  FLIP developers, on the derivation that surfaced it. The
  accepted consequence is that enrollment is one-way within a data directory:
  a Holder cannot retract, and an operator can only stop advertising
  altogether. R1 already withholds any claim of current holder intent, so this
  narrows what enrollment means rather than what the claim proves.
- **R6 public library seams:** the module, its fetcher trait, and `Database`
  are public. Another library consumer may inject a fetcher or write the table
  directly. A2 quantifies only over the official binary; L2 additionally means
  such writes cannot surface a forged envelope.

## Weakest links

1. **L1 (`code` + A1):** the decisive checks are adjacent and simple, but the
   statement-holder comparison lacks a focused test and their meaning bottoms
   out in the pinned private credential SDK and cryptographic axioms.
2. **L2 (`enum`):** the "exactly two readers" enumeration is regenerated per
   check and enforced by nothing. A future reader that selects `event_json` and
   skips `load_verified` would break the claim silently. Promotion candidate:
   make the row type private to the module so no other query can construct one.
3. **L3 (`enum`):** the sink list is likewise a regenerated enumeration.
4. **L4 (`code` + `test`) and L5 (`enum`):** counting is not a security gate,
   and the absence of a gate on the status enum is not enforced by a type.
