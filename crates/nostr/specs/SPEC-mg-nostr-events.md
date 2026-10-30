# SPEC-mg-nostr-events: MG discovery and attester Nostr events

## Status

The kind-`37701` advertisement does not meet the additive-field compatibility
required by
[REQ-extensible-mg-advertisement](./REQ-extensible-mg-advertisement.md).
Its typed consumers discard unknown payload fields during deserialization and
reconstruct the payload for signature verification, so an otherwise valid
advertisement with a newly added signed field fails its proof in those
clients. The MG advertisement v2 (kind `37709`) meets it: readers verify the
exact signed bytes and then ignore unknown fields.

The MG publishes both advertisements, and the in-repo FA reads both, as the
v2 section below describes.

## Record justification

These event schemas are produced by the MG advertiser, the Holder trust-badge
miniapp, and Attester tooling, and verified by FA, FLIP, and programs outside
this repository, so no single implementation artifact can own them coherently.

This crate owns the shared kind, tag, and signing constants, plus the one
typed rendering of the kind-37701 advertisement document (`mg` module) that
the MG-side publisher and the FA consumer both depend on; the MG-side
producer is `crates/mg/nostr` (its `run_advertisements` loop,
driven by the `fleet-manager` daemon;
[SPEC-advertisement](../../mg/specs/SPEC-advertisement.md)). The
schemas are cross-program contracts shared with programs outside this
repository (the Fedi app/SDK, Issuer tooling, FLIP implementations):
byte-level changes must be coordinated with those consumers, not made
unilaterally, and where this repo's types render a schema the record is
authoritative — a mismatch is a bug on this side.

The contract is constrained by
[REQ-extensible-mg-advertisement](./REQ-extensible-mg-advertisement.md):
coordinating a semantic change does not permit additive fields to break older
clients that support the same schema version.

## Event kinds

All kinds are provisional, addressable events for mutable latest-state
documents. Tags are indexing hints only: every verifier parses and verifies
event content, and a tag is never trusted unless it matches the signed payload.

```text
37701 MG advertisement                (d = "fman-ad", t = "fedi-fman")
37709 MG advertisement v2             (d = "mg-ad", t = "fedi-mg")
37703 Attester issuer-authority mirror  (d = "issuer-authority", t = "fedi-attester-issuer")
37704 Attester credential revocation    (d = "credential-revocation:<digest>", t = "fedi-credential-revocation")
37708 MG encrypted backup             (d = "mg-backup-seat:<seat_id>")
37705 Holder-published MG authorization
      (d = "fman-authorization:<fman_pubkey>:<credential_digest>",
       t = "fedi-fman-authorization", p = <fman_pubkey>,
       issuer = <issuer_pubkey>, credential = <credential_digest>,
       schema = <badge_schema>)
```

Kind `37708` is the one exception to this record's cross-program framing: its
content is encrypted to the publishing MG's own backup identity and has no
external consumer, so it carries no shared schema to coordinate. Only the kind
and `d`-tag allocation belong here; the documents themselves are owned by
[SPEC-nostr-backup-restore](../../mg/specs/SPEC-nostr-backup-restore.md).

Related kinds owned elsewhere: `37702` is the FLIP provider advertisement
([SPEC-flip-advertisement](../../liquidity-manager-daemon/specs/SPEC-flip-advertisement.md)),
`37706` is reserved for the draft FA encrypted backup snapshot
([`docs/fa-nostr-backups.md`](../../../docs/fa-nostr-backups.md)), and `37707`
is the Fedi-authored setup-payment federation set
([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)).
Kind-`37701` events and relay tags never carry an MG-local payment set.

## MG advertisement (37701)

The event `content` is a portable signed document (Nostr is only one
publication transport): a `payload` plus a `proof.signature` — a Schnorr
signature by the MG service key over
`SHA256("fedi-fman-advertisement/v1\0" || JCS(payload))`. The MVP identity
rule is `MG id == MG Nostr pubkey`: the event author must equal
`payload.fman_id_pubkey`, and this is the MG's self-generated service
identity, never the operator's holder identity.

Payload fields (v1, rendered by this crate's `mg::AdvertisementPayload` and
published by `mg-nostr`'s advertisement loop):

- `version`, `fman_id_pubkey` (canonical lowercase hex), `issued_at`,
  `expires_at` (must comfortably exceed the republish interval or MGs flap
  out of discovery);
- `service_pubkey` — the MG's commitment-signing public key (x-only
  secp256k1, lowercase hex; `fman/v1/service-sign` derivation,
  [ARCH-manifold-guardian-identity](../../mg/specs/ARCH-manifold-guardian-identity.md)),
  the same key a `service-manifold-guardian` `Locator` carries, against which FAs
  verify signed MG responses. Distinct from `fman_id_pubkey`, the Nostr
  service identity (`fman/v1/service-nostr`);
- `api_endpoints` — `{transport, url}` entries for pre-formation FA setup RPC;
- `availability` — typed scalar `fedimintd_version` and `federation_sizes`;
  the version's SemVer build metadata is the exact DKG vendor identity, and FA
  admits only `+fedi`. The event is published only while the
  MG is accepting seats, so the payload carries neither a boolean nor a count;
- `plans` — the `service-manifold-guardian` `Plan` enum in its own canonical serde
  form, identical to `GetAvailability`, so advertisement and RPC cannot
  disagree. Prices are millisatoshis as JSON numbers, the same unit and type
  a quote's `price_msats` uses, so no consumer parses or reconciles a price
  spelling;
- `holder_authorizations` — embedded verified trust envelopes
  ([SPEC-holder-trust-envelope](../../domain/specs/SPEC-holder-trust-envelope.md)).

### Additive evolution

Advertisement payload objects are open to additive fields within a schema
version. Consumers ignore fields they do not understand when interpreting an
advertisement, but signature verification canonicalizes the complete payload
received from the publisher, including unknown fields. Consumers must not
deserialize into a type that drops unknown fields and then verify a
reconstructed payload. Consequently, changing or removing any received signed
field, whether understood by the consumer or not, invalidates the proof.

This rule applies to the payload and protocol-owned objects nested within it.
An additive field cannot change the meaning of an existing field or become
necessary to interpret an existing field safely. A change that requires new
client behavior for trust, authorization, or endpoint safety uses an
incompatible schema version instead.

`version` is strictly `1`; other values and a missing `service_pubkey` do not
parse as this schema.

`service_pubkey` sits inside the self-signed payload, so its attestation
rides the advertisement's existing trust chain: the payload is signed by the
key it names in `fman_id_pubkey`, the consumer pipeline binds that key to
the authenticated event author, and the embedded holder authorization
vouches for exactly that author as its badge subject. The claim "this Nostr
identity's commitment-signing key is `service_pubkey`" is therefore attested
by the same authenticated identity the badge vouches for — the service key
gets no independent vouching, and none is claimed: a consumer trusts it
precisely as much as it trusts the badge-vouched MG identity that asserted
it.

Availability, plans, and endpoints are non-trust hints;
only verified trust envelopes are trust inputs. A purely diagnostic
enumerator may return statically admitted advertisements only when its API
marks their envelopes as unverified claims. Any relying flow that turns an
advertisement into a trust conclusion must reject when the event author and
payload pubkey differ, the proof is invalid, `issued_at` is too far in the
future, `expires_at` has passed, or no examined embedded envelope passes the
trust checks with `subject_pubkey == fman_id_pubkey`. Such a relying flow may
examine a documented bounded envelope prefix and verify lazily in selection
order, but it must never treat an unexamined envelope as trusted. Consumers
that dial the advertised endpoints must additionally reject an advertisement
offering no acceptable endpoint or a malformed `service_pubkey`.

Advertisements serve pre-formation FA discovery. For post-formation trust,
a verifier holding an invite code resolves each seat-binding identity's
standing from MG-signed trust material fetched from that MG directly. An
advertisement names no federation, so the verifier cannot bind it to the
federation under evaluation
([SPEC-federation-trust-directory](../../domain/specs/SPEC-federation-trust-directory.md)).
The embedded envelopes remain a trust input for discovery, under the same
verifier checklist above.

## MG advertisement v2 (37709)

The v2 advertisement carries the same offer as kind `37701`, and adds signed
fields, under the exact-payload envelope of
[SPEC-exact-payload-signatures](../../../specs/SPEC-exact-payload-signatures.md).
The event `content` is that envelope. The purpose tag is
`fedi-signed-json/mg-advertisement/v1\0`. The signer is the MG Nostr service
key, which must equal the event author. The trust chain, the non-trust status
of availability, plans and endpoints, and every relying-flow check of the
legacy section apply unchanged. They read the signer wherever the legacy
section reads `fman_id_pubkey`.

The payload has the legacy fields with the same meanings and serde shapes:
`service_pubkey`, `issued_at`, `expires_at`, `api_endpoints`, `availability`,
`plans` and `holder_authorizations`. It drops `version`, because the purpose
tag versions the schema, and it drops `fman_id_pubkey`, because the envelope
signer is the identity. It adds one field:

- `uptime_permille`: an optional object `{"1d", "7d", "30d"}` of integers in
  `0..=1000`. The values are the scores that the MG's latest readiness run
  compared against the uptime minimums, not the minimums themselves, rounded to
  per-mille ([SPEC-advertisement](../../mg/specs/SPEC-advertisement.md),
  *Readiness gate*). The field is absent until a readiness run has recorded
  uptime, and absent means unreported, not zero. The MG computes the values
  from its own clock. Signing makes them attributable to the MG, but it does
  not make them true, so FAs must not use them to admit, reject, rank or select
  an MG. FAs may display them.

Unknown payload fields follow the envelope's rule: readers verify the exact
bytes and then ignore the unknown fields. The rule covers the objects this
section and the legacy section define, including the holder-authorization
envelope objects. The embedded PeerBadge credential's `info` and `blind_msg`
keep the PeerBadge SDK's own parsing rules
([SPEC-exact-payload-signatures](../../../specs/SPEC-exact-payload-signatures.md),
*Unknown fields*). The payload is bounded by the shared
64 KiB per-event cap, and the 65,536-byte decoded-payload limit applies as
well. The FA keeps the v1 bound on examined holder authorizations.

**Publication.** While deployed FAs that read only kind `37701` remain
supported, the MG publishes both advertisements in every cycle. Both come from
the same snapshot, with the same `issued_at` and `expires_at`, and each is
attempted even when the other fails. The kind-`37701` document stays
byte-for-byte what a v1-only MG would publish. The two events have distinct
addressable coordinates, so neither replaces the other. When the MG stops
accepting seats, it stops publishing both, and both lapse at their signed
expiry. Retiring `37701` publication is a separate, approved change.

**Reading.** An FA queries the two kinds separately, and each query has its
own copy of the legacy candidate and retained-byte bounds. While every MG
publishes kind `37701`, the legacy query alone decides whether the enumeration
is complete. A failed v2 query leaves every author on its legacy
advertisement. For each author, it
statically admits each format. The legacy format is admitted as today. A v2
event must pass the event signature, the envelope verification, the payload
parse and the author binding. The FA then applies the existing freshness rule
to the signed `issued_at` and `expires_at`. It uses the newest fresh admitted
v2 advertisement, by the existing newest-per-author order, when one exists. Otherwise it uses the newest fresh admitted
legacy advertisement, and that candidate has no uptime report. The FA never
merges fields across the two formats. A fresh legacy advertisement never
displaces a fresh v2 advertisement, so uptime does not appear and disappear
between runs. If an author's v2 advertisement is invalid or stale, the FA falls
back to its legacy one. This covers an MG downgraded to a v1-only release,
whose last v2 advertisement lapses within its expiry. Eligibility and
selection then run on the chosen document exactly as in v1.

## Holder-published MG authorization (37705)

During setup the Holder publishes one addressable event per authorization,
authored by the holder pubkey, whose content carries `{version,
holder_id_pubkey, holder_authorization, signed_credential}` — the
holder-signed authorization plus the backing trust badge inline, so MG
ingestion needs no second fetch. The `p` tag is the index the MG queries
during operator-driven enrollment; the `issuer`, `credential`, and `schema` tags mirror envelope
content for consumer-side filtering and, like every tag, must be cross-checked
against the signed content before use.

The MG verifies before embedding: event author equals `holder_id_pubkey`
and the statement's holder pubkey; the SDK authorization proof;
`subject_pubkey` equals its own Nostr pubkey; `issued_at` is no more than one
hour ahead of its receiver clock; and the credential digest binding. Through
the shared offline issuance check, the MG also requires a trusted issuer,
valid issuance under that issuer's pinned authority, and holder binding.
Enrollment does not check revocation or the relying party's minimum trust
level, so relying consumers must still run complete verification.
MG's Nostr boundary receives the resolved
deployment environment profile rather than a `PeerBadgeVerifier`; complete
advertisement verification belongs to relying consumers
([ARCH-manifold-environment](../../manifold-environment/specs/ARCH-manifold-environment.md),
[SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)).
MGs must never publish standalone MG-authored authorization events on Nostr;
they carry authorizations in their advertisements instead. Direct
guardian-transfer proof separately carries the envelope and the subject's
co-signature, as specified in
[SPEC-signed-envelopes](../../service-manifold-guardian/specs/SPEC-signed-envelopes.md).
Diagnostic
enumerators must preserve the unverified-claim boundary. Every relying
advertisement flow must authenticate the complete event, verify an envelope
through the shared verifier within its documented work bound, and bind the
returned subject to the event author before producing a trust conclusion.

## Attester events (37703, 37704)

`37703` is the addressable distribution event for canonical
`peerbadge-protocol::IssuerAuthority` content. Every canonical
profile pins its committed authority documents, so the shared verifier performs
no 37703 lookup for those issuers. The event is discovery only: no consumer may
treat the newest 37703 event as a replacement for a profile-pinned authority.
For an explicit test issuer without a pinned authority, the verifier fetches
37703 afresh from every configured authority relay and rejects it unless the
event author equals `issuer.issuer_id_pubkey`, `IssuerAuthority::verify()`
succeeds, and the issuer is trusted by local policy
([SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)). `37704` carries a
canonical `SignedRevocation` authored by the issuer identity key. The shared
verifier queries every Nostr location listed in
`IssuerAuthority.issuer.revocation` afresh and rejects a credential when any
location returns a matching valid revocation. Revoking a published credential
does not link it to its blind-signing session. Both kinds are shared with
FLIP's trust pipeline
([SPEC-flip-advertisement](../../liquidity-manager-daemon/specs/SPEC-flip-advertisement.md)).
