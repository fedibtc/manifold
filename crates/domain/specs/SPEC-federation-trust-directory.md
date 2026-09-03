# SPEC-federation-trust-directory: FMan seat bindings in consensus metadata

## Record justification

The directory contract spans this crate's attestation types, FMan attestation
issuance and formation proposal validation, the FI's post-DKG write/readback,
and FLIP plus external verifiers that know only an invite code, so no single
implementation artifact can own it coherently.

## Goal and transports

Anyone holding only a federation invite code can verify which distinct FMan
identities operate the federation and whether each is currently trusted. Two
sources split the material, and the split is the point: one is durable and
expensive to change, the other mutable and cheap.

- **Consensus metadata** is authoritative for *which* FMan operates each
  guardian seat: signed `FmanPeerAttestation`s under the metadata key
  `fedi:fman_seat_bindings`. Changing it requires threshold guardians to submit
  byte-identical bytes, which is what makes it trustworthy and also what makes
  it unsuitable for anything that changes.
- **FMan-signed trust material** is authoritative for *whether* an FMan is
  currently trusted: its holder trust envelopes and current public endpoint,
  carried in an FMan-signed document with issue and expiry timestamps. A badge
  frozen into the directory could never be withdrawn, which is why standing
  lives outside it.
- **Nostr** remains authoritative for *revocation*: fresh kind-`37704` lookups
  at verification time
  ([SPEC-fman-nostr-events](../../nostr/specs/SPEC-fman-nostr-events.md),
  [SPEC-holder-trust-envelope](./SPEC-holder-trust-envelope.md)).

Trust material may be carried to a verifier by the party requesting something
of it rather than fetched by the verifier. That is sound because carriage is
not authorship: the directory decides which identities exist, each material
document is signed by the identity it describes, and a document is only ever
consulted for that identity. A carrier therefore controls only whether an
identity is answered for at all, and an unanswered identity is untrusted. A
verifier must bound the accepted validity window, because with no live
advertisement lookup nothing else reports that an FMan is still operating.

The FMan service identity is its stable restorable handle across restarts and
restores, and remains the key every attestation and material document is
resolved by.

## Attestation and container

`FmanPeerAttestation` (this crate) is a Schnorr signature by `fman_pubkey`
over the SHA-256 digest of the JCS-canonical, type-tagged statement
(`fman_pubkey`, `federation_id`, `federation_config_hash`, `peer_id`,
`guardian_identity`, the full single-signature `guardian_fee_account`,
`issued_at`) under domain
`fedi-fman-peer-attestation/v1\0`. `fman_pubkey` is the canonical
lowercase-hex Nostr serialization; alternate encodings (npub, NIP-21,
uppercase) are rejected. `peer_id` is the canonical decimal spelling of the
Fedimint peer id; `guardian_identity` is that peer's broadcast public key in
compressed lowercase hex, the key that signs the federation's consensus
session outcomes. `guardian_fee_account` is the public `BtcDepositor` account
the same seat returned in its earlier service-key-signed acceptance; including
it here gives every guardian a signed all-seat account source after DKG.

The service-key signature is not sufficient to prove that the FMan owns a
final configured guardian endpoint: an FI can invent a service key. Each
`GetPeerAttestation` response therefore also carries a `SeatEndpointProof`, an
Ed25519 signature by the seat endpoint key over
`fedi-fman-seat-endpoint-proof/v1\0 || attestation_statement_digest`. The
proof is bound to the exact attestation statement rather than a second
identity/account transcript.

When admitting `ProposeFormationMeta`, every FMan derives the API endpoint key
for each peer from its final client config, requires exactly one proof per
canonical directory peer, and verifies every signature. It also requires the
directory entry for its own peer id to name its own FMan attestation identity.
The proofs are formation-admission evidence: the FI persists them for exact
replay, but they are not written into consensus metadata.

`federation_config_hash` is SHA-256 over the domain separator
`fedi-federation-config-hash/v1\0` followed by the consensus encoding of the
final client config with `global.meta` cleared. Consensus metadata is excluded
deliberately: guardians sign their attestations before the FI publishes the
directory into metadata, so a hash covering metadata would invalidate every
attestation the moment the directory it belongs to is written. Everything else
in the config is bound, including the broadcast public keys — which the
Fedimint federation id, a hash of the API endpoints alone, does not cover.

The `fedi:fman_seat_bindings` value is strict canonical JSON:
`{"version":1,"seat_bindings":[...]}` — non-empty, exactly one attestation per
final-config guardian seat, ordered by ascending numeric `peer_id` with no
`peer_id` repeated, every entry's `federation_id` and `federation_config_hash`
equal to the values derived from the final config, at most 64 entries and 65536
canonical bytes (provisional caps). Two entries claiming one `peer_id` are a
conflict about who operates that seat, not a duplicate to collapse. The
fee accounts must be unique single-signature `BtcDepositor` accounts; one
account cannot silently stand in for two guardian entitlements. The container
carries binding claims and public fee destinations only — badges and authorizations arrive as
FMan-signed trust material, revocations from Nostr, and both remain untrusted
until verified. Every reader enforces the same canonical directory rules. The formation-only
`ProposeFormationMeta` handler additionally verifies exact endpoint-proof
coverage against the live final config and the self-seat FMan identity before
voting. `SetMetaField` cannot write the directory after formation.

The stored directory is not permanently self-verifying for endpoint ownership:
`verify_for_federation` verifies attestation signatures, federation/config
binding, peer ids, guardian identities, and accounts, but the directory does
not contain the admission-time endpoint proofs. A hostile guardian threshold
can therefore adopt hostile metadata and can misattribute an excluded,
non-colluding guardian if it controls enough other seats to adopt the target;
it still cannot forge that excluded guardian's endpoint proof to include the
honest endpoint under a false identity during admission. Misattributing an
included hostile guardian adds no funds-control power beyond the threshold the
attackers already possess.

## FI write and readback

After DKG succeeds and an invite code exists, and even when liquidity is
skipped, `fi-client` collects one attestation and endpoint proof from each
FMan. It checks each fee account against the signed acceptance paired with that
seat and requires each proof to name the attestation's peer. It assembles the
complete formation metadata target — canonical-directory readback prediction,
paired attestation/proof entries, FI account, derived recipients, and initial rate — and persists it before the
first proposal wave. Recovery replays that exact target rather than refetching
attestations or re-resolving accounts and policy.

Before a proposal wave FI reads exact raw consensus metadata, derives the
occurrence-bound `MetaConsensusBase`, and signs one `ProposeFormationMeta`
request per seat containing the same paired attestation/proof entries, the FI
account, the deployment-pinned Guardian Verification Fee account, and the
initial rate. Each FMan independently constructs the bounded canonical
directory, verifies the complete proposal, and derives the fixed recipient
list, then submits the directory, recipients, and rate as one whole-object
target. Before voting, it requires the stated Guardian Verification Fee account
to equal its own configuration and distinctly refuses a missing configuration
or mismatch. The FI states the deployment-pinned value; it does not select it.
Every submitted target is consequently a pure function of identical signed
request inputs. `MetaConsensusChanged` causes a fresh read, rebase, signature,
and byte-identical semantic replay.

After every wave the FI previews the federation until consensus-metadata
readback exactly equals the expected value. A threshold may adopt the target
while slower guardians from that same wave return a late stale-base response;
fresh readback proving the exact directory won takes precedence over those
late responses. The readback source is consensus metadata, never an FI
assertion or one FMan's memory; attestations bind the post-DKG
`federation_config_hash`, which the mutable metadata write does not change.
The complete shared whole-object protocol is
[`SPEC-fi-metadata-maintenance`](../../fman/specs/SPEC-fi-metadata-maintenance.md).

The preview query may be performed by an `fi-client` consumer rather than by
`fi-client` itself, which keeps the crate free of the Fedimint client stack.
What may not move is the judgement: the peer-set derivation, the attestation
signature checks, and the equality comparison stay inside `fi-client`, so the
consumer supplies raw material and never a conclusion. Because
`get_consensus` earns its guarantee from the caller performing threshold
agreement and returns no signatures, a consumer-performed query is an
obligation `fi-client` cannot verify; see
[`ARCH-fi-client`](../../fi-client/specs/ARCH-fi-client.md).

## Verifier algorithm

From the invite code: download final config and consensus metadata; derive
the authoritative federation id, config hash, peer set, guardian identities,
network, and consensus threshold; parse the canonical container; verify every
attestation signature and match each to exactly one final-config guardian by
federation id, config hash, peer id, and guardian identity (missing, extra,
or conflicting bindings are invalid); resolve each distinct `fman_pubkey`'s
trust material, verifying that it is signed by that exact directory-selected
identity and lies within the verifier's accepted validity window; run fresh
fail-closed revocation checks; and apply policy over
distinct trusted identities — one FMan may operate several seats but counts
once.

The identity set comes from the directory, never from the material, and the
order matters: a verifier that enumerated identities from the material it was
handed would let whoever supplied it decide who the federation's operators are.
Material naming an identity the directory does not name can never be consulted
and needs no special handling. The live document carries no seat claims: the
consensus directory is the sole source for federation membership. FLIP's policy
profile and rejection mapping are
[SPEC-flip-federation-trust](../../liquidity-manager-daemon/specs/SPEC-flip-federation-trust.md).
