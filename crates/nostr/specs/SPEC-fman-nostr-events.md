# SPEC-fman-nostr-events: FMan discovery and attester Nostr events

## Status

The additive-field compatibility required by
[REQ-extensible-fman-advertisement](./REQ-extensible-fman-advertisement.md) is
not yet implemented. Current typed consumers discard unknown payload fields
during deserialization and reconstruct the payload for signature verification,
so an otherwise valid advertisement with a newly added signed field fails its
proof in those clients.

## Record justification

These event schemas are produced by the FMan advertiser, the Holder trust-badge
miniapp, and Attester tooling, and verified by FI, FLIP, and programs outside
this repository, so no single implementation artifact can own them coherently.

This crate owns the shared kind, tag, and signing constants, plus the one
typed rendering of the kind-37701 advertisement document (`fman` module) that
the FMan-side publisher and the FI consumer both depend on; the FMan-side
producer is `crates/fman/nostr` (its `run_advertisements` loop,
driven by the `fleet-manager` daemon;
[SPEC-advertisement](../../fman/specs/SPEC-advertisement.md)). The
schemas are cross-program contracts shared with programs outside this
repository (the Fedi app/SDK, Issuer tooling, FLIP implementations):
byte-level changes must be coordinated with those consumers, not made
unilaterally, and where this repo's types render a schema the record is
authoritative — a mismatch is a bug on this side.

The contract is constrained by
[REQ-extensible-fman-advertisement](./REQ-extensible-fman-advertisement.md):
coordinating a semantic change does not permit additive fields to break older
clients that support the same schema version.

## Event kinds

All kinds are provisional, addressable events for mutable latest-state
documents. Tags are indexing hints only: every verifier parses and verifies
event content, and a tag is never trusted unless it matches the signed payload.

```text
37701 FMan advertisement                (d = "fman-ad", t = "fedi-fman")
37703 Attester issuer-authority mirror  (d = "issuer-authority", t = "fedi-attester-issuer")
37704 Attester credential revocation    (d = "credential-revocation:<digest>", t = "fedi-credential-revocation")
37708 FMan encrypted backup             (d = "fman-backup-seat:<seat_id>")
37705 Holder-published FMan authorization
      (d = "fman-authorization:<fman_pubkey>:<credential_digest>",
       t = "fedi-fman-authorization", p = <fman_pubkey>,
       issuer = <issuer_pubkey>, credential = <credential_digest>,
       schema = <badge_schema>)
```

Kind `37708` is the one exception to this record's cross-program framing: its
content is encrypted to the publishing FMan's own backup identity and has no
external consumer, so it carries no shared schema to coordinate. Only the kind
and `d`-tag allocation belong here; the documents themselves are owned by
[SPEC-nostr-backup-restore](../../fman/specs/SPEC-nostr-backup-restore.md).

Related kinds owned elsewhere: `37702` is the FLIP provider advertisement
([SPEC-flip-advertisement](../../liquidity-manager-daemon/specs/SPEC-flip-advertisement.md)),
`37706` is reserved for the draft FI encrypted backup snapshot
([`docs/fi-nostr-backups.md`](../../../docs/fi-nostr-backups.md)), and `37707`
is the Fedi-authored setup-payment federation set
([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)).
Kind-`37701` events and relay tags never carry an FMan-local payment set.

## FMan advertisement (37701)

The event `content` is a portable signed document (Nostr is only one
publication transport): a `payload` plus a `proof.signature` — a Schnorr
signature by the FMan service key over
`SHA256("fedi-fman-advertisement/v1\0" || JCS(payload))`. The MVP identity
rule is `FMan id == FMan Nostr pubkey`: the event author must equal
`payload.fman_id_pubkey`, and this is the FMan's self-generated service
identity, never the operator's holder identity.

Payload fields (v1, rendered by this crate's `fman::AdvertisementPayload` and
published by `fman-nostr`'s advertisement loop):

- `version`, `fman_id_pubkey` (canonical lowercase hex), `issued_at`,
  `expires_at` (must comfortably exceed the republish interval or FMans flap
  out of discovery);
- `service_pubkey` — the FMan's commitment-signing public key (x-only
  secp256k1, lowercase hex; `fman/v1/service-sign` derivation,
  [ARCH-fleet-manager-identity](../../fman/specs/ARCH-fleet-manager-identity.md)),
  the same key a `service-fleet-manager` `Locator` carries, against which FIs
  verify signed FMan responses. Distinct from `fman_id_pubkey`, the Nostr
  service identity (`fman/v1/service-nostr`);
- `api_endpoints` — `{transport, url}` entries for pre-formation FI setup RPC;
- `availability` — typed scalar `fedimintd_version` and `federation_sizes`;
  the version's SemVer build metadata is the exact DKG vendor identity, and FI
  admits only `+fedi`. The event is published only while the
  FMan is accepting seats, so the payload carries neither a boolean nor a count;
- `plans` — the `service-fleet-manager` `Plan` enum in its own canonical serde
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
precisely as much as it trusts the badge-vouched FMan identity that asserted
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

Advertisements serve pre-formation FI discovery. They are **not** the
post-formation trust source: a verifier holding an invite code resolves each
seat-binding identity's standing from FMan-signed trust material fetched from
that FMan directly, because the advertisement names no federation and so cannot
be bound to the one being evaluated
([SPEC-federation-trust-directory](../../domain/specs/SPEC-federation-trust-directory.md)).
The embedded envelopes remain a real trust input for discovery, under the same
verifier checklist above.

## Holder-published FMan authorization (37705)

During setup the Holder publishes one addressable event per authorization,
authored by the holder pubkey, whose content carries `{version,
holder_id_pubkey, holder_authorization, signed_credential}` — the
holder-signed authorization plus the backing trust badge inline, so FMan
ingestion needs no second fetch. The `p` tag is the index the FMan queries
during operator-driven enrollment; the `issuer`, `credential`, and `schema` tags mirror envelope
content for consumer-side filtering and, like every tag, must be cross-checked
against the signed content before use.

The FMan verifies before embedding: event author equals `holder_id_pubkey`
and the statement's holder pubkey; the SDK authorization proof;
`subject_pubkey` equals its own service pubkey; `issued_at` is no more than one
hour ahead of its receiver clock; and the credential digest binding. It does
not judge the badge itself — the badge's PBRSA proof, its
holder binding, issuer trust, and revocation state are not consumed by the
current FMan onboarding path. FMan's Nostr boundary receives the resolved
deployment environment profile rather than a `PeerBadgeVerifier`; complete
advertisement verification belongs to relying consumers
([ARCH-manifold-environment](../../manifold-environment/specs/ARCH-manifold-environment.md),
[SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)).
FMans must never publish standalone FMan-authored authorization events —
embedding in the advertisement is the only FMan-authored carriage. Diagnostic
enumerators must preserve the unverified-claim boundary. Every relying
advertisement flow must authenticate the complete event, verify an envelope
through the shared verifier within its documented work bound, and bind the
returned subject to the event author before producing a trust conclusion.

## Attester events (37703, 37704)

`37703` is the addressable distribution event for canonical
`fedi-credential-sdk-protocol::IssuerAuthority` content. Every canonical
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
