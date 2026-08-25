# SPEC-flip-canonical-payloads: Canonical FLIP payload bytes and domains

## Record justification

The canonical byte and domain rules must produce identical results in this
crate's helpers, the daemon, and app-side implementations outside this
repository, so no single implementation artifact can own them coherently.

FLIP follows the FMan split between publication and RPC signing, but signs
canonical protocol payload bytes rather than exact wire bytes: transport
frame encoding (serde-compatible DTOs over `fedi-iroh-rpc` CBOR bodies) is
never the signing input. Every signed document is a typed `payload` plus a
`proof.signature` — a secp256k1 Schnorr signature by the author key over

```text
SHA256(domain_separator || canonical_payload_bytes)
```

Version, freshness (`issued_at`), and author identity live inside the typed
payload. Enum values on the wire are the canonical lower-snake-case strings;
Rust variant names are implementation-local. Amounts are unsigned integer
sats.

## Domains and encodings

Nostr-published portable documents (`LiquidityProviderAdvertisement`) use
canonical JSON (JCS) payload bytes under `fedi-flip-advertisement/v1\0`;
`advertisement_hash` is the same domain-tagged digest. RPC payloads use
canonical CBOR under per-verb request/response domains:

```text
fedi-flip-get-provider-info-request/v1\0
fedi-flip-get-provider-info-response/v1\0
fedi-flip-request-liquidity-request/v1\0
fedi-flip-request-liquidity-response/v1\0
fedi-flip-get-allocation-status-request/v1\0
fedi-flip-get-allocation-status-response/v1\0
```

Distinct per-verb and per-direction domains prevent cross-verb and
cross-direction replay. The `canonical` module owns the helpers; verifiers
must use them (or byte-identical implementations) and never hash ad hoc
reserialization.

## Details commitment

`details_payload_hash` binds `RequestLiquidity` and every later
request-scoped payload to the private details the provider evaluated:

```text
SHA256("fedi-flip-details-payload-hash/v1\0"
       || canonical_cbor(RequestLiquidityDetailsCommitmentV1))
```

Every commitment field (`version`, `requester_pubkey`, `provider_pubkey`,
`network`, `amounts`, `federation_details`, `expires_at`) is in the preimage.
Freshness and proof material are excluded by construction: `issued_at` is
signed-request metadata, and `details_payload_hash` and `proof` are derived.
The planned `fman_endorsement` admission gate rides alongside the request and
is likewise excluded, keeping the commitment stable for retry idempotency
(see [SPEC-flip-rpc](../../liquidity-manager-daemon/specs/SPEC-flip-rpc.md)).

Conformance fixtures must cover: JSON key-order-independent canonical bytes,
every commitment field changing the hash, `issued_at` not changing it, the
same payload under different domains producing different digests, signature
failure under a wrong verb domain, and provider-pubkey mismatch rejection.
