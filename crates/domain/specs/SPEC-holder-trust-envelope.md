# SPEC-holder-trust-envelope: Holder authorization and trust badge carriage

## Record justification

The envelope and badge schema bind the external credential SDK, FCS/Attester
issuance, the Holder miniapp, the FMan and FLIP advertisers, and FI/FLIP
selection plus the push-gateway guardian-telemetry and cloud FMan telemetry
collector verifiers,
so no single implementation
artifact can own them coherently.

The `fedi-trust-score-v1.0` schema definition (issuance constructors,
verifier-side parser, and golden vectors) is owned by
`fedi-credential-sdk-schemas` in the credential-sdk repository; this crate's
`trust_score` module re-exports it and owns both the pure envelope helper
against a caller-supplied credential-SDK verification context and the generic,
validated relying-party `PeerBadgeTrustPolicy`. `crates/peer-badge-verifier`
owns the shared FI, FLIP, push-gateway guardian-telemetry, and cloud FMan
telemetry collector verifier that
fetches current authority and revocation state, runs the complete envelope
algorithm, and applies that policy
([SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)).
The `HolderAuthorization`, `SignedCredential`, and `IssuerAuthority` types and
their canonical serialization, digests, and proofs are owned by
`fedi-credential-sdk-protocol`. This is a cross-program contract shared with
programs outside this repository (the Fedi app/SDK and Issuer tooling);
byte-level changes must be coordinated with them, not made unilaterally.

## Auxiliary service identity

A service (FMan, FLIP provider) self-generates its long-lived identity key;
the owner's Holder key never operates the service and the service never holds
the Holder key. Trust attaches through an auxiliary-identity authorization:
the Holder signs a `HolderAuthorization` whose statement
(`holder_id_pubkey`, `subject_pubkey`, `credential_digest`, `issued_at`)
authorizes the service pubkey to present one concrete trust badge. This
scopes a service-host compromise to the service identity instead of the
operator's holder key, which is the decisive reason identity sharing was
rejected. Subject possession is proven separately by the service's own
signatures (advertisement proof, RPC authentication).

The authorization request shown to the Holder (QR/deep-link, not a Nostr
event) is the SDK `HolderAuthorizationRequest` carrying only the subject
pubkey; the Holder app selects the badge locally, signs, and publishes the
kind-`37705` event
([SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md)).

## Trust badge schema `fedi-trust-score-v1.0`

The badge spans `Credential.info` and `Credential.blind_msg`
(`TRUST_SCORE_SCHEMA_V1` in `trust_score`):

- `info` (attester-visible, attester-attested): `schema =
  "fedi-trust-score-v1.0"` and a numeric `trust_level` (public result of
  attester-private scoring) in the documented `1..=12` trust model. Unknown
  extra keys are tolerated for additive revisions; the schema is exactly what
  the shipped PeerBadge issuer produces. `fedi-trust-score-v1.0` badges attest
  holders; a credential about any other subject requires a new schema string.
- `blind_msg` (hidden from the attester during blind issuance, revealed at
  presentation): the holder pubkey as canonical lowercase hex, binding the
  badge to the Holder.

The attester never learns the service pubkey at issuance and must not be able
to link a published advertisement back to a signing session. The service
pubkey is bound only later by the `HolderAuthorization`.

**Future work — `issuance_epoch`.** Version `v1.0` defines neither
`issuance_epoch` nor `subject_type`. A later version may add a coarse
`issuance_epoch` batch id as a policy input, but never a unique issuance
timestamp, to preserve the holder anonymity set. Version-string batching keeps
all `v1.0` badges in their own cohort so policy can age them out or reissue them
as a batch when epoch-aware policy arrives.

## Inline envelope and verifier semantics

Advertisements carry trust inline as `{holder_authorization,
signed_credential}` envelope entries — the FMan and FLIP carriage convention —
so verifiers need no separate credential fetch. An envelope is authentic only
when all of:

- the authorization verifies under the SDK proof API and yields its signed
  `subject_pubkey`;
- `credential_digest` equals `Credential::digest()` of the inline backing
  credential (domain-separated SHA-256 over the JCS-canonical `Credential`,
  covering `info` and revealed `blind_msg`, excluding the proof signature);
- the backing `SignedCredential` verifies against a trusted `IssuerAuthority`
  under this schema, and its `blind_msg`
  equals the authorization's `holder_id_pubkey`;
- `issued_at` passes freshness policy, and no valid `SignedRevocation` from
  the credential's attester matches the digest (fresh, fail-closed lookup).

A valid envelope means exactly: *a trusted attester attested a holder trust
badge, the badge is bound to this Holder, and this Holder authorized this
service pubkey to present it*. It is an input to verifier policy (trust
level, issuer, epoch), never trust by itself, and it is separate from any
future direct issuer-signed service credential. Subject possession is a
separate event-level fact: a complete advertisement verifier authenticates the
Nostr event and requires its author to equal the subject returned from envelope
verification
([SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)).

`PeerBadgeTrustPolicy` is the shared validated representation of the caller's
minimum-level policy. It rejects a configured minimum outside the schema range
and rejects an authenticated badge whose numeric level is below that minimum;
it does not alter the envelope wire format or credential schema.

The verifier's relay-observation model does not change this envelope contract.
Its visibility and stronger-synchronization boundary are governed by
[SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md).
