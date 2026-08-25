# SPEC-signed-envelopes: Exact-byte request and commitment signatures

## Record justification

Shared signing code, typed service verbs, and separate FMan/FI consumers jointly
enforce this wire contract, so no single local artifact can own it coherently.

FI-authenticated requests and FMan commitment responses use typed envelopes that
sign the exact payload bytes received on the wire. The payload currently contains
serde-JSON bytes, but verification never parses and reserializes them to derive a
signature input. This avoids requiring canonical JSON and prevents serializer
differences from changing the authenticated message. It is the mechanism behind
the FI boundary described by
[`SPEC-fi-rpc`](../../fman/specs/SPEC-fi-rpc.md).

The signature algorithm is secp256k1 BIP-340 Schnorr in both directions;
signer identities are hex-encoded 32-byte x-only public keys. Algorithm
choice is governed by
[`ARCH-fleet-manager-identity`](../../fman/specs/ARCH-fleet-manager-identity.md)
(*Signature scheme*).

## Signed bytes and labels

The signed BIP-340 message is the 32-byte digest:

```text
SHA256( direction_domain || verb_label || NUL || exact_payload_bytes )
```

FI requests use the domain `fedi-fman-fi-request/v1\0`; FMan responses use
`fedi-fman-response/v1\0`. The `v1` versions the signing scheme itself —
matching the `fman/v1/*` key-derivation labels — not the product release; it
changes only on an incompatible scheme redesign. Distinct direction domains
prevent a request from being mistaken for a response. Per-verb labels prevent
replay as another verb whose payload happens to have a compatible shape.

Hashing before signing matches fedimint-core's convention for its own
Schnorr-signed documents (`SHA256(tag || bytes)` signed as a 32-byte
message), and a fixed 32-byte message keeps the scheme implementable from
secp256k1 bindings that only expose digest-input signing. BIP-340 makes the
two forms equivalent for a 32-byte input, so signatures interoperate with
digest-only verifiers.

FI request labels cover `create_seat`, `get_dkg_code`, `start_dkg`,
`restart_dkg`, `get_status`, `get_invite_code`, `get_peer_attestation`,
`propose_formation_meta`, `set_meta_field`, `register_gateway`, and
`get_fedimint_stats`. FMan
response labels cover `get_quote` and `create_seat`.

`start_dkg` signs its optional DKG-completion callback as part of the exact
payload bytes and rejects unknown fields. `restart_dkg` signs the complete
guardian-code set but has no callback: the first start choice is retained for
the whole formation. The embedded hook
URL is a bearer capability, not a second authentication mechanism: the
signature binds it to the FI and exact DKG attempt, while the receiving
deployment separately confines its destination.

### `propose_formation_meta` payload

`ProposeFormationMetaRequest` carries `ts`, `fi_id`, `seat_id`,
`expected_base`, structural `seat_bindings` entries that each pair one signed
FMan attestation with its endpoint proof, the FI's and deployment-pinned Fedi's complete
single-signature `BtcDepositor` accounts, and `send_ppm`. The signature therefore prevents rebasing,
substituting a directory or proof, redirecting the FI share, or changing the
rate without a fresh FI signature.

The structural list is capped at 64 entries while the signed payload is
deserialized. Each FMan constructs `FmanSeatBindings` once from the supplied
attestations; that construction canonically orders the directory, rejects
duplicate peers, and enforces the 65,536-byte canonical value cap before
attestation and endpoint-proof signature verification. The outer FI envelope
signature is verified before this payload is deserialized. The FI separately constructs the same canonical value
as its consensus-readback prediction, but that prediction is not wire input.

Each paired endpoint proof names the attestation's canonical peer id and contains an Ed25519 signature
under the domain `fedi-fman-seat-endpoint-proof/v1\0` over the corresponding
attestation statement digest. FMan derives the verification key from that
peer's final configured API endpoint. Recipients are not wire input: every
FMan derives the canonical FI=4, guardian=1, Fedi=1 list from the verified
directory and two request accounts. Recipient identity is the destination
account, ordered by account id; all destinations must be distinct. Before submitting any vote, FMan
requires the request's Fedi account to equal its own environment configuration;
the FI states the expected deployment value but cannot choose a replacement.
An absent local Fedi account and a mismatch are distinct typed refusals. Given
the same signed request, final config, and base metadata object, every guardian
that votes therefore derives byte-identical canonical metadata and recipients.
`ProposeFormationMetaResponse` is empty.

The signed `CreateSeatResponse` acceptance includes the seat's complete
guardian-fee account beside its deterministic seat id. The account is therefore
quote-bound and replayable with the acceptance; an FI persists both together
and never needs an unsigned post-payment account lookup. After DKG the FMan
repeats that account inside its signed peer attestation. The FI rejects a
mismatch before publication. Its persisted paired entries are the exact replay
source; the separately persisted canonical-directory prediction is only for
consensus readback. Once adopted, the canonical directory gives every FMan the
authenticated complete account set for fee-policy validation.

## FI request verification

A request envelope contains an outer FI id, opaque payload bytes, and signature.
The outer id is deliberately available before parsing so the verifier can select
the public key without trusting attacker-controlled payload structure. The id
and signature are parsed types — an envelope with a malformed key or signature
fails deserialization and never constructs. Verification then proceeds in a
fixed order: verify the signature over the direction domain, verb label,
separator, and received bytes, parse the typed payload, require its inner
`fi_id` to equal the outer id, and finally check freshness.

The accepted freshness interval is inclusive `|request.ts - now| <= 3600`
seconds. This bounds replay but does not prevent it; there is no nonce or replay
store. The daemon's seat lookup separately enforces that `fi_id` equals the
identity bound at seat creation, since that recorded identity is not available
at envelope verification time.

Successful FI-request verification is represented by `VerifiedFiRequest<T>`,
whose payload `fi_id` *is* the verified signer: verification required it to
equal the envelope key the signature verified under.
Successful manager-response verification is represented separately by
`SignatureVerified<T>`, which carries the SHA-256 of the exact signed payload
bytes used to derive a quote id. Both proofs have private fields and only their
corresponding verification path can construct them, so downstream code cannot
manufacture an authenticated request or name an unverified quote. Envelope
fields are likewise private to prevent callers from parsing around the
verification boundary.

FI-signed types that target an existing seat implement
`SeatScopedFiRequest`, exposing their typed `SeatId`. The daemon's only
seat-selection path takes a verified request of such a type and compares its
signer against the seat's recorded owner; a request type without this marker
has no seat authority at all, so implementing it is the deliberate,
reviewable grant of (ownership-checked) seat access.

## Commitment creation, persistence, and replay

A response envelope contains opaque payload bytes and the manager signature. On
first service the response is serialized once and those bytes are signed. The
daemon keeps that shared response-envelope type intact through allocation and
replay; its store borrows and writes the exact `(payload, signature)` pair as two
columns with the seat transition. Startup reconstructs the envelope from those
columns. An idempotent retry returns it without serialization or re-signing, so
the caller receives byte-identical proof material. FI verification checks the
signature against the FMan commitment key before parsing the response.

Signed response payloads themselves bind commitments to their requests: a
quote echoes the request terms it prices, and a `CreateSeat` commitment or
refusal echoes the quote identity it answers. Persistence is part of the
commitment guarantee, not merely a response cache (quotes, being stateless,
are re-derived rather than persisted).

## Errors

Authentication errors distinguish bad signatures, invalid typed payloads,
inner/outer signer mismatch, stale timestamps, and local serialization
failures. Malformed keys and malformed signatures are unrepresentable: the
envelope's fields only parse as a valid x-only public key and a 64-byte
Schnorr signature. The shared mechanism exposes these distinctions
to trusted callers and logs, but an invalid-payload error carries only the
corresponding fixed verb label, never serde's attacker-influenced parse detail.
The daemon maps every incoming envelope failure to the coarse wire error
`Unauthorized`. This avoids both log-record injection and turning parsing or
cryptographic detail into a remote oracle. Errors creating a manager commitment
are internal service failures and are returned by the daemon as generic
`Other("internal error")` after detailed logging.
