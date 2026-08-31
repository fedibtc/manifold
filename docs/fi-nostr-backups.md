# FI Nostr backup and restore

## Status

`fi-client` implements a formed-only portable `FiBackup` and its encrypted
`EncryptedFiBackup` envelope. The consumer supplies one FI-scoped root;
`fi-client` derives its protocol, backup-author, and content keys. Encryption uses zstd,
random 8/16/32/64-KiB padding, and XChaCha20-Poly1305 bound to the provisional
kind-`37706` coordinate. The backup excludes setup-payment policy, driver
leases, and consumer secrets and restores atomically into an empty namespace
without external effects. Partial-formation backup is a possible future
iteration. Nostr event signing, publication, relay selection, refresh, and
remote restore remain a draft.

The JSON5 examples below describe the planned Nostr layer; they are not the
current `FiBackup` wire format. Sections that describe partial formation or use
the earlier `Reserve`/`ReservationId` protocol are not part of the current
formed-only implementation.


## Summary

The FI key remains deterministic from the app master seed. The backup is not a new authority and does not contain the master seed. Its job is to preserve one formed federation's non-derivable control and recovery state: the selected FMan locators, signed quote and seat authority, guardian recovery facts, formed invite, and FLIP request state. It is not a wallet, identity-secret, setup-payment-policy, or partial-formation backup.

Backups are published as encrypted addressable Nostr events authored by a separate deterministic backup key, not by the FI protocol key. Restore derives that backup key from the same master seed, queries the stable addressable backup coordinate across several relays, decrypts every valid candidate, and chooses the freshest valid snapshot. Nostr event ids are receipts and dedupe handles, not restore pointers.


## Goals

- Restore a fully formed federation into a fresh `fi-client` database.
- Let the restored app know which federation it initiated and which FMan seats it controls.
- Preserve the exact handles needed to reconcile formed seats and liquidity.
- Keep all federation-specific data encrypted before it reaches Nostr relays.
- Avoid linking backup events to the FI protocol pubkey unless the encrypted content is decrypted.
- Keep backups available on multiple Nostr relays and refresh them because relays are not durable storage.


## Non-goals

- Backing up the app master seed or any private key derivable from it.
- Backing up `Idle` or a partially formed federation.
- Backing up spendable ecash or replacing the Fedi wallet's own backup mechanism.
- Making Nostr a reliable archival storage provider. Redundancy and refresh reduce loss risk; they do not create a hard durability guarantee.
- Multi-device live sync. This is a disaster-recovery snapshot format. Concurrent writers need fork detection and conservative reconciliation, not silent last-writer-wins mutation.
- Proving current FMan trust from backup contents. Restore must re-fetch current trust material and revocations before taking new trust-sensitive actions.


## Existing design facts this depends on

From [`ARCH-fi-client`](../crates/fi-client/specs/ARCH-fi-client.md) and `docs/fedi-app/fedi-app.md`:

- The consumer supplies one stable FI-scoped root from its app key hierarchy.
- `fi-client` uses durable storage for session state and can resume from checkpoints.
- `fi-client` derives the protocol key and the separate environment-scoped
  backup key family; none of those secrets are exposed or persisted.
- FMan setup and maintenance are FI-pulled. There is no FMan push channel.

From `crates/fman/specs/SPEC-fi-rpc.md` and
`crates/service-fleet-manager`:

- FMan seat control after `CreateSeat` is by `SeatId` plus FI signatures by the
  same `FiId` that created the seat.
- `SeatId` is the canonical identity of the accepted quote and is not
  derivable from the FI key.
- Durable signed quotes, guardian codes, and the formed invite are needed to
  reconcile the current formed state safely.
- Running seats expose FI-scoped status, invite, attestation, maintenance, and
  statistics operations addressed by `SeatId`.

From [`SPEC-fman-nostr-events`](../crates/nostr/specs/SPEC-fman-nostr-events.md):

- Nostr events and tags are indexes only. Content must be parsed and verified.
- Addressable events are already used for mutable latest-state documents.
- Event kind numbers in the `377xx` range are provisional component-specific kinds.

The main consequence: a restored FI key alone is insufficient. Without the
backup, the app does not know which FMans and seats it controls or retain the
local facts needed to reconcile that formed federation.

The current portable layer backs up only `Formed`. Partial formation remains
ordinary local crash-recovery state and is outside the backup format.


## Key derivation

The app scopes its existing root to FI child 17 and passes that `DerivableSecret`
to `fi-client`. `fi-client` converts it directly to the protocol secp256k1 key,
preserving Fedi's existing identity, and uses its deterministic raw-byte output
as the input to environment-separated HKDF-SHA256 backup derivation.

Domain labels:

```text
fedi-fi-backup/nostr-author/v1
fedi-fi-backup/content-encryption/v1
```

Keys:

- **FI protocol key**: existing `FiId` key used for FMan and FLIP signatures.
- **Backup Nostr author key**: secp256k1 key used only to author backup events. It MUST NOT be the FI protocol key. Derive a valid scalar with rejection sampling so `1 <= scalar < secp256k1_order`.
- **Backup content key**: 32-byte symmetric key used only for encrypting backup documents.

Environment separation:

- Production, signet, regtest, and local test builds MUST use distinct HKDF salt or context values.
- A production app MUST NOT query or publish test backup events with the same derived backup pubkey.


## Nostr event kind and coordinate

Use one addressable event kind and one stable replacement coordinate. V1 deliberately does not maintain a backup-history ring; the payload schema is versioned, and the compact recovery format only needs one latest snapshot per relay. Multiple relays still provide redundancy, but not rollback-proof archival history.

Provisional kind:

```text
37706 FI encrypted backup snapshot (addressable, provisional)
```

Stable addressable coordinate:

```text
kind: 37706
pubkey: BACKUP_NOSTR_PUBKEY
d: fedi-fi-backup:v1
```

Nostr wrapper:

```json5
{
  // Provisional addressable event kind for encrypted FI backups.
  kind: 37706,

  // Public key derived only for backup publication. This is not the FI protocol
  // key and not the app master seed.
  pubkey: "BACKUP_NOSTR_PUBKEY",

  tags: [
    // Addressable replacement coordinate for the latest encrypted FI backup.
    ["d", "fedi-fi-backup:v1"]
  ],

  // Canonical JSON string of `FiBackupCiphertextEnvelope`, documented below.
  // No federation or reservation data is visible before decrypting this.
  content: "CANONICAL_JSON_FI_BACKUP_CIPHERTEXT_ENVELOPE"
}
```

Do not include federation ids, FMan pubkeys, relay URLs, `FiId`, `ReservationId`, or other application data in tags. The event kind and `d` tag already reveal that this pubkey has a Fedi FI backup. Everything else stays encrypted.

A `t` hashtag is intentionally omitted in v1. Restore can query by exact author, kind, and `d` tags. Omitting a hashtag reduces global scrapeability by generic hashtag indexers. If operational tooling later needs a hashtag, it must be treated as an indexing hint only.


## Event ids

The event id is the normal Nostr NIP-01 id:

```text
sha256(json([0, pubkey, created_at, kind, tags, content]))
```

The event signature is the normal Nostr Schnorr signature by `BACKUP_NOSTR_PUBKEY` over that id.

Rules:

- Restore MUST NOT require a remembered event id. A fresh app only has the master seed, so it discovers backups by derived backup pubkey plus kind and the stable `d` tag.
- The same signed event bytes SHOULD be published to every relay in one publish attempt. That gives one event id replicated across multiple relays.
- A refresh republishes the current snapshot with a fresh `created_at`, fresh nonce, and fresh event id. The plaintext snapshot generation may be unchanged.
- `event.id` is stored locally as a relay receipt. It is not included in the v1 backup payload and is not an authority for restore.
- The addressable coordinate `37706:BACKUP_NOSTR_PUBKEY:fedi-fi-backup:v1` is the stable lookup handle.


## Ciphertext envelope

The Nostr `content` is canonical JSON for this minimal public envelope:

```json5
{
  // Envelope format version. Version 1 fixes the KDF, AEAD, compression,
  // padding, blob layout, and AEAD associated-data rules below. Keeping only a
  // version avoids leaking a detailed algorithm menu in every public event.
  version: 1,

  // Base64url without padding of:
  //   24-byte random XChaCha20-Poly1305 nonce
  //   followed by ciphertext and authentication tag.
  blob: "BASE64URL_UNPADDED_NONCE_THEN_CIPHERTEXT"
}
```

Version 1 parameters:

```text
KDF: HKDF-SHA256 with label fedi-fi-backup/content-encryption/v1
AEAD: XChaCha20-Poly1305
Compression: zstd
Padding: bucketed-compressed-v1
Blob layout: nonce[24] || ciphertext_and_tag
```

Plaintext preparation before encryption:

1. Build the canonical portable `FiBackup` bytes.
2. Compress those bytes with zstd.
3. Prefix the compressed bytes with a 4-byte big-endian compressed length.
4. Pad with random bytes to the next bucket.
5. Encrypt the framed, padded bytes once with XChaCha20-Poly1305 and a fresh random 24-byte nonce.

Padding buckets are measured on the framed compressed bytes before encryption. Padding is not meant to make all users indistinguishable; it primarily avoids leaking exact backup growth, such as when adding a seat, unresolved retry handle, or other recovery handle changes the compressed size by a recognizable amount. Suggested buckets:

```text
8 KiB, 16 KiB, 32 KiB, 64 KiB
```

If the framed compressed bytes do not fit the largest supported bucket, the implementation MUST fail the backup as too large or move to a future chunked format. It MUST NOT silently omit required recovery data. Non-essential audit and diagnostic history can be pruned before failing, using deterministic product policy. V1 does not need to record a pruning manifest.

Associated data for AEAD:

```text
"fedi-fi-backup/event-aead/v1\0"
|| lowercase-hex event_pubkey ASCII
|| big-endian u16 event_kind
|| UTF-8 d_tag
|| big-endian u16 envelope.version
```

The implementation has the outer event fields before decrypting, so this binds the ciphertext to the expected backup author, kind, and addressable coordinate. The nonce is authenticated by XChaCha20-Poly1305 as part of normal decryption because it is the AEAD nonce for this blob. The outer Nostr signature still MUST be verified before decryption. There is no second encryption layer: the event content is a single custom AEAD ciphertext envelope, not NIP-44 wrapping another encrypted payload.

Why not NIP-44 for v1 content:

- NIP-44 is optimized for encrypted messages between Nostr users. This backup is a seed-derived self-encrypted archive stored in an addressable event.
- NIP-44 would not hide the backup author, event kind, timestamp, or relay metadata; the privacy-sensitive FI data is already protected by the custom AEAD envelope.
- NIP-44 adds a 65535-byte plaintext limit and conversation-oriented semantics without a clear benefit for this design.

A future NIP-44-compatible backup profile is possible if a product requirement appears, but v1 should keep one directly specified AEAD envelope.


## Encrypted payload size estimate

This estimate is for compact v1 with one formed federation and one gateway operated by the FI. Gateway private keys, wallet state, liquidity, and runtime database state are not part of this FI backup; they belong in the gateway or wallet backup layer. If `fi-client` needs to remember that this FI controls a gateway for the federation, budget only a small gateway handle or endpoint hint here.

Assumed plaintext contents:

- top-level version, timestamps, sequence, FI pubkey, and no inner signature
- one formed federation record with `formation_id`, `state`, `display_name`, and `invite_code`
- four FMan seat records, each with `seat_id`, `fman_pubkey`, and `reservation_id`
- one FI-operated gateway handle or endpoint hint, about 300 plaintext bytes
- user trust omitted because it is empty
- no audit extension, no signed FMan responses, no old payment terms, no DKG history, no trust caches, no FLIP request payload bytes, and no ecash token hash/retry handles

Expected compact plaintext before compression:

| Case | Plain JSON | zstd framed bytes | Padding bucket | Approx Nostr event size |
| --- | ---: | ---: | ---: | ---: |
| 4 FMan seats, gateway handle | 2.5 KiB to 4 KiB | 1.5 KiB to 2.5 KiB | 8 KiB | 11 KiB to 12 KiB |
| 10 FMan seats, gateway handle | 4 KiB to 7 KiB | 2 KiB to 4 KiB | 8 KiB | 11 KiB to 12 KiB |
| Same, one unresolved ecash token hash/retry handle or exact retry payload | 5 KiB to 14 KiB | 3 KiB to 10 KiB | 8 KiB, sometimes 16 KiB or 32 KiB | 11 KiB to 45 KiB |

The event size is dominated by padding and base64url expansion, not by the small handle set. With the current bucket list:

| Padding bucket | Raw blob bytes | Base64url blob chars | Approx full Nostr event |
| ---: | ---: | ---: | ---: |
| 8 KiB | 8,232 | 10,976 | 11 KiB to 12 KiB |
| 16 KiB | 16,424 | 21,899 | 22 KiB to 23 KiB |
| 32 KiB | 32,808 | 43,744 | 44 KiB to 45 KiB |
| 64 KiB | 65,576 | 87,435 | 88 KiB to 89 KiB |

`Raw blob bytes` is `padding_bucket + 24-byte nonce + 16-byte AEAD tag`. The full event estimate adds the minimal ciphertext-envelope JSON plus normal Nostr fields, tags, id, and signature.

Operational target for v1: ordinary single-federation backups should stay in the 8 KiB padding bucket, producing about an 11 KiB to 12 KiB Nostr event. Hitting the 16 KiB bucket is acceptable for unusually large invite codes, many seats, or small unresolved retry material. Hitting 32 KiB should be treated as a warning that unresolved retry material or optional audit data is bloating the backup. Hitting 64 KiB should be treated as a product bug or future chunking requirement.

The 8 KiB minimum leaks one extra bit of coarse size information compared with a 16 KiB minimum, but it roughly halves normal event size. That tradeoff is acceptable for compact v1 because tags reveal no federation identifiers and ordinary payload size is mostly determined by seat count and optional unresolved retry material.


## Plaintext document shape

The decrypted object is a portable, schema-versioned export of `fi-client` state. It is not a raw SQLite or fedimint DB backup.

Top-level shape:

```json5
{
  // Backup payload schema version. Version 1 defines required fields and default
  // restore behavior. AEAD authenticates this plaintext, so v1 has no inner
  // payload signature.
  version: 1,

  // Unix timestamp when this payload generation was created. This is not an
  // anti-rollback guarantee; relays can withhold newer events.
  created_at: 1730000000,

  // Monotonic sequence local to this seed's backup stream. Used to choose the
  // newest valid candidate after decrypting relay candidates.
  snapshot_seq: 42,

  // FI protocol public key re-derived from the master seed. Restore rejects the
  // backup if this does not match the restored FI key.
  fi_pubkey: "LOWERCASE_HEX_NOSTR_PROTOCOL_PUBKEY",

  // One record per federation that this FI started or controls.
  federations: [],

  // User-added trust roots, pinned FMans, relays, and BYO state. App-shipped
  // defaults are not backed up here. Omit this field when empty.
  user_trust: {}
}
```

Validation after decrypting:

- `version` is supported.
- `fi_pubkey` matches the FI pubkey re-derived from the master seed.
- The outer `d` tag is the version-defined backup coordinate.
- `snapshot_id` computed from canonical plaintext bytes is used only for local dedupe and conflict detection.

V1 intentionally omits inner payload signatures, `parent_snapshot_id`, device
ids, backup policy mirrors, publication receipts, and pruning manifests. Those
can be added later as optional payload fields or by a version bump if an
implementation needs them. Writers SHOULD omit optional fields when empty, null,
or derivable from the version profile.

V1 is a compact recovery snapshot, not an audit log. Writers SHOULD omit
optional objects such as `restore_hints`, resolved `unresolved_operation`
records, cached advertisements, cached credentials, and diagnostic attestations
unless product policy explicitly enables them. Empty arrays and objects SHOULD
be omitted instead of serialized as placeholders. Required recovery handles MUST
NOT be omitted just to fit a relay limit.


## Federation record contents

Each federation record is a compact recovery index. It is not the formation transcript. Once a federation is formed, FMan, FLIP, and federation APIs are authoritative for mutable state.

```json5
{
  // Stable local federation-formation id allocated when the FI flow starts.
  // This is not the Fedimint federation id.
  formation_id: "BASE64URL_128_BITS",

  // Version of this federation record shape.
  record_version: 1,

  // Local FI lifecycle state for this federation record.
  state: "forming | formed | ended | abandoned",

  // Unix timestamps for local record creation and last material update.
  created_at: 1730000000,
  updated_at: 1730000000,

  // Optional user-visible label. Omit after restore if the app can recover a
  // name from federation config or let the user rename locally.
  display_name: "Friends federation",

  // Optional formation parameters. Keep only while formation has durable paid or
  // confirmed seats but no running federation. Omit once formed, unless product
  // wants this for UX.
  formation_intent: {
    federation_size: 10,
    fedimintd_version: "0.8.0",
    module_config: "BASE64URL_CBOR_OR_NULL"
  },

  // Formed federation invite code. This is small and useful redundancy. If it is
  // missing, restore can ask any running FMan seat for GetInviteCode.
  invite_code: "INVITE_CODE_OR_NULL",

  // Per-FMan seat handles. These are the critical opaque capabilities needed to
  // ask FMans for authoritative state after seed restore.
  seats: [],

  // Unresolved federation-level mutating operations, if any. Omit when empty.
  unresolved_operations: [],

  // Guardian-fee metadata write checkpoint. Omit after fields are written and
  // readable from federation metadata, unless product wants audit state.
  fee_arrangement: {},

  // FLIP request handles for this federation, if any. Omit when no liquidity
  // request exists.
  liquidity: [],

  gateway: {
    // Optional handle for a gateway operated by this FI for this federation. Omit
    // if gateway backup or normal gateway discovery can recover it. Never store
    // gateway private keys, wallet state, or runtime database state here.
    gateway_id: "LOCAL_GATEWAY_ID_OR_NULL",
    endpoint_hint: "GATEWAY_ENDPOINT_OR_NULL"
  }
}
```

Required fields for a formed federation:

- `formation_id`
- `state`
- `invite_code`, if already known
- every selected seat with `fman_pubkey` and `reservation_id`
- unresolved mutating operations that were sent but not authoritatively resolved
- FLIP request handles if liquidity was requested
- FI-operated gateway handle only if it is not recoverable from the gateway backup layer

Do not store federation id, config hash, network, final peer-set hash, current `fedi:fman_seat_bindings`, or final guardian-fee metadata by default. A restored app recovers those from invite-code preview, joined federation config, and federation metadata. Store hashes only as optional diagnostics.

A restored app can rejoin or display the federation from the invite code, but FI control of FMan seats requires the per-seat reservation handles.


## Seat record contents

Each FMan selected for a federation has one compact seat record.

```json5
{
  // Stable local id for this selected seat inside the FI backup.
  seat_id: "LOCAL_STABLE_ID",

  // FMan service or advertisement pubkey. Restore uses this to discover current
  // endpoints and to know which FMan to contact with the reservation id.
  fman_pubkey: "LOWERCASE_HEX_NOSTR_PUBKEY",

  reservation: {
    // FI-generated idempotency key for Reserve, if the canonical protocol
    // includes it. Needed only until the FMan-minted reservation id is known.
    request_id: "FI_GENERATED_RESERVE_IDEMPOTENCY_KEY_OR_NULL",

    // FMan-minted opaque reservation id. This is the critical non-derivable
    // handle needed to control the seat after seed restore.
    reservation_id: "FMAN_RESERVATION_ID_OR_NULL"
  },

  restore_hints: {
    // Optional contact hints. Omit when current discovery can find the FMan.
    // These hints are not trust evidence.
    relay_hints: ["wss://relay.example.invalid"],
    last_known_api_endpoints: ["iroh://ENDPOINT"]
  },

  unresolved_operation: {
    // Omit this object when no sent mutating RPC is unresolved.
    kind: "reserve | confirm_reservation | confirm_payment | start_dkg | restart_dkg",

    // FI-generated idempotency key or protocol request id, if that RPC has one.
    request_id: "REQUEST_ID_OR_NULL",

    // Hash of the exact request bytes, so retry can be checked before mutation.
    request_hash: "BASE64URL_HASH_OR_NULL",

    // Store only non-spendable retry material (token hash / wallet operation id)
    // while payment was made but FMan confirmation is not yet authoritatively
    // observed and the handle is not recoverable from the wallet backup layer.
    // Never store raw OOB ecash tokens here.
    payment_retry_handle: "BASE64URL_BYTES_OR_NULL",

    // Last local send time for UX and retry backoff.
    sent_at: 1730000000
  }
}
```

Required for seats that are paid, approved, confirmed, or running:

- `fman_pubkey`
- `reservation.reservation_id`
- optional contact hints only if the FMan is not reachable through bootstrap discovery

`reservation.request_id` is required only until `reservation_id` is known, and only after the canonical `Reserve` protocol adds it. Without either `reservation_id` in backup or an FMan enumeration API, a lost `ReserveResponse` remains unrecoverable from seed-only restore.

Do not store by default:

- plan terms
- signed Reserve or ConfirmReservation responses after FMan status confirms the seat
- old payment terms
- validity dates
- last status snapshots
- child health
- FMan peer attestations
- DKG attempt history or full guardian-code sets

Restore recovers those by calling FMan `GetStatus`, `GetInviteCode`, current public FMan trust APIs, and federation metadata. Signed responses and ecash token hash/retry handles are optional dispute evidence, not normal recovery material. If product wants dispute archives, store them under an explicit bounded audit extension, not in the compact v1 core.

In-flight DKG progress is soft state in the current FI design. Compact v1 should omit DKG code history by default. Restore must query FMan status before reusing any backed-up DKG hint and may use `RestartDKG` if the state is ambiguous.


## Fee arrangement state

`fi-client` owns the guardian-fee metadata write, but the formed federation metadata is the source of truth after write/readback succeeds.

```json5
{
  // Local write lifecycle for FI-owned fee metadata.
  write_state: "not_started | pending | written | failed | unknown",

  // Omit when writes are complete and readable from federation metadata.
  unresolved_set_meta_fields: [
    {
      // Metadata key, such as `fedi:guardian_fee_send_ppm` or
      // `fedi:guardian_fee_remittance_account`.
      key: "FEDERATION_META_KEY",

      // Store the value only while retry may be needed. Once written and read
      // back, restore reads the current value from federation metadata.
      value: "SERIALIZED_VALUE_OR_NULL",

      // Hash for idempotency and readback comparison.
      value_hash: "BASE64URL_HASH",

      // Hash of the SetMetaField request bytes, if already sent.
      request_hash: "BASE64URL_HASH_OR_NULL"
    }
  ]
}
```

The initiator fee account should be deterministically derived from the master seed and `federation_id`. This keeps the fee account recoverable from seed plus authoritative federation metadata, instead of making a random account secret another required backup item. If the implementation uses a random account secret instead, that secret becomes required encrypted backup material and the design is worse. The deterministic route is strongly preferred.

Do not store the final weighted recipient list or guardian-fee rate after they are written and readable from federation metadata. Store only unresolved write material needed to retry safely. Operator account source material should be recovered from FMan or federation metadata whenever the current specs make that possible.


## Liquidity state

If `fi-client` requested liquidity from a FLIP provider, the compact backup stores the provider and request handle. The provider is authoritative for allocation state after it accepts the request.

```json5
{
  // FLIP provider selected by FI.
  provider_pubkey: "LOWERCASE_HEX_PROVIDER_PUBKEY",

  // Optional contact hint. Omit if the provider can be rediscovered from the
  // current registry. Restore must refresh provider trust before mutation.
  provider_endpoint: "iroh://ENDPOINT",

  // FLIP's semantic retry/status identity for the exact request commitment.
  // This is required: it is not re-derivable from the FI key.
  details_payload_hash: "BASE64URL_SHA256",

  // Local allocation lifecycle hint. Restore confirms with the provider.
  status: "requested | completed | failed | unknown",

  unresolved_request: {
    // Omit once the provider has acknowledged the request and can answer by
    // details_payload_hash. Include only if exact retry may be needed.
    request_hash: "BASE64URL_HASH_OR_NULL",

    // Prefer not storing request bytes. If present, use only after provider
    // reconciliation says retry is safe.
    canonical_request_bytes: "BASE64URL_BYTES_OR_NULL"
  }
}
```

Do not store full `RequestLiquidity` or `FederationLiquidityDetails` by default.
Restore queries the provider by `requester_pubkey` plus
`details_payload_hash`; if the provider has no record, it refreshes current
provider/FMan trust before replaying an exact locally retained unresolved
commitment or asking for a new user intent. Canonical request material is only
for an unresolved exact retry.

The backup should not treat a stored FLIP endpoint or credential as current
trust. On restore, refresh the provider advertisement, read current
`fedi:fman_seat_bindings` metadata, fetch fresh signed trust material from every
operating FMan's public API, refresh revocations, and re-evaluate policy before
sharing private federation details or retrying a request. There is no FMan
advertisement fallback in the post-formation trust path.


## User trust and pinned material

The app ships default issuer trust roots, so they do not need to be backed up. User-added trust roots and pinned participation do need backup.

```json5
{
  // User-added issuer authorities. App-shipped default authorities are not
  // included.
  user_added_issuer_authorities: [],

  // User-pinned FMan identities or endpoints for BYO/BYFriends flows.
  user_pinned_fmans: [],

  // User-added backup or discovery relays. These help only after first decrypt.
  user_added_relays: [],

  // State for any holder authorization or BYO-FMan flow initiated by this FI.
  byo_fman_authorization_state: []
}
```

Any user-added issuer authority or pinned FMan entry restored from backup is configuration, not proof. It still must pass the normal parser and validation before use.


## Recoverable from authoritative sources

Compact v1 deliberately avoids storing data that can be recovered after the app has the master seed, the FI pubkey, and the small set of backup handles.

Recover from FMan, using `fman_pubkey` plus `reservation_id`:

- current reservation lifecycle state
- plan terms
- service validity date
- pending or current payment requirement
- running invite code
- DKG status or the need to restart DKG
- current public FMan trust material and API endpoints

Recover from the federation, using `invite_code` or a rejoined wallet:

- federation id
- final config hash
- network
- peer set
- `fedi:fman_seat_bindings`
- `fedi:guardian_fee_send_ppm`
- `fedi:guardian_fee_remittance_account`

Recover from current discovery and trust evaluation:

- FMan and FLIP advertisements
- credentials
- revocation state
- endpoint freshness
- peer attestations used for diagnostics

Recover from the FLIP provider, using `provider_pubkey` plus `request_id`:

- allocation status
- accepted or rejected result
- provider-side request state

Recover from deterministic app derivation:

- FI protocol key
- backup Nostr author key
- backup content key
- FI-owned guardian-fee account, if the deterministic design is used

Recover from the wallet or gateway backup layer, not this FI backup:

- spendable ecash
- wallet secrets
- ordinary payment history and retry handles, unless a retry handle is specifically needed for an unresolved FI retry and is not recoverable elsewhere
- FI-operated gateway private keys, liquidity, routing state, and runtime database state

The tradeoff is intentional: if every relevant FMan, FLIP, and federation is gone or refuses service, a large transcript in Nostr would not restore operational control anyway. The compact backup should therefore preserve handles needed to ask authoritative systems, not mirror their state.


## What not to back up

Do not include by default:

- app master seed or mnemonic
- FI private key, backup Nostr private key, or content encryption key
- spendable ecash notes or wallet secrets owned by the app wallet backup layer
- raw `fi-client` database bytes as the canonical recovery format
- telemetry upload state
- unbounded registry caches
- default trust roots that ship with the app
- Nostr relay authentication secrets, unless a future relay policy explicitly requires them and product accepts the restore-time risk
- FMan advertisements, credentials, revocations, or peer attestations
- FMan plan, status, health, validity date, or renewal snapshots
- signed FMan responses after authoritative status confirms the seat
- old payment terms and ecash token hash/retry handles after the operation is resolved
- DKG attempt history and full guardian-code sets
- federation id, config hash, network, peer-set hash, or metadata values that can be read from the federation
- FLIP request payload bytes after the provider acknowledges the request and can answer by `request_id`
- FI-operated gateway private keys, liquidity, routing state, or runtime database state
- Nostr publication receipts or old event ids

These may exist as explicit bounded audit or diagnostic extensions, but they are not part of the compact v1 recovery core.

Derived private keys should be regenerated from the master seed after restore. Runtime storage may cache them locally according to the app's normal secret-storage policy, but the Nostr backup must not contain them.


## Required versus soft FI state

The current FI design persists hard checkpoints and reconstructs soft state. This backup format follows that split and leans on authoritative recovery.

Required compact backup state:

- One federation record for any federation that reached a durable external checkpoint.
- For each durable seat: `fman_pubkey` and `reservation_id`.
- Formed federation `invite_code` when known, as small redundancy.
- Unresolved mutating operation material only while retry may be needed: request id, request hash, and ecash token hash/retry handle only if the retry handle is not recoverable from the wallet backup layer.
- Fee metadata values only while the FI has started a write that is not yet read back from federation metadata.
- FLIP `provider_pubkey` and `request_id` once liquidity is requested; request bytes only for unresolved exact retry.
- FI-operated gateway handle only when separate gateway recovery cannot rediscover it.
- User-added trust roots, pinned FMans, user relays, and BYO authorization state.

Best-effort or omitted backup state:

- Pre-payment FI intent and unpaid reservations.
- In-flight DKG details before a hard checkpoint.
- Cached advertisements, credentials, revocations, relay cursors, diagnostic attestations, and last status snapshots.
- Historical payment terms, signed responses, ecash token hash/retry handles, and audit records after the corresponding operation is resolved.

Restore may use best-effort state for UX and reconciliation, but must be safe if it is absent or stale. In particular, it may drop unpaid reservations and re-reserve, and it must query FMan status before reusing DKG hints.


## Backup creation triggers

Create a new material snapshot after each compact recovery handle changes:

- `formation_id` allocated for a flow the product wants to recover
- FMan `reservation_id` learned for a paid, approved, confirmed, or otherwise durable seat
- a mutating RPC is sent and becomes unresolved
- an unresolved mutating RPC is authoritatively resolved and can be pruned
- invite code learned
- fee metadata write starts, succeeds, fails, or is read back and can be pruned
- FLIP `request_id` learned
- FLIP request is acknowledged, completed, failed, or prunable
- user-added trust roots, pinned FMans, relays, or BYO authorization state changes

Do not create a new material snapshot only because a cache changed. This includes ads, credentials, revocations, status polling, health polling, peer attestations, relay cursors, and other recoverable observations.

Debounce rapid updates, but the app should aim to publish within a few minutes of each material handle change while online. A formed federation snapshot should be published immediately after the invite code and all required seat handles are known.


## Publication, redundancy, and refresh

Relay sets:

- The app ships a bootstrap backup relay set, separate from discovery relays if product wants that separation.
- Every material snapshot MUST be published to the bootstrap relay set. Otherwise seed-only restore has nowhere deterministic to query.
- The user may add relays, but encrypted user relay configuration is only available after the first valid backup is found.
- V1 does not store relay receipt history. It may store user-added relay configuration after first decrypt, but bootstrap relays remain the only seed-only discovery path.

Publication rule:

1. When backup content changes, increment `snapshot_seq`, build the new plaintext snapshot, and publish it as soon as practical while online. This is a material update, not a refresh.
2. Build and sign one event for the stable addressable coordinate.
3. Publish identical event bytes to every relay in the active backup relay set, which is the bootstrap set plus any locally configured user relays and previously successful relays.
4. Treat the snapshot as durably published only after at least `required_publish_relay_successes` relays accept it. Suggested value: 3.
5. Keep retrying failed relays in the background with bounded exponential backoff.
6. Surface a warning if fewer than 2 relays have accepted the latest material generation.

Refresh rule:

- Refresh is only for keeping the same plaintext snapshot available on relays when no backup content has changed.
- At app start, refresh if the newest successful publication is older than 7 days.
- While the app is active, refresh the current snapshot at least every 7 days.
- Refresh means republishing the current plaintext generation with the same `snapshot_seq`, a new Nostr `created_at`, new nonce, and new event id.

Relay behavior assumptions:

- Relays may delete old events, reject large events, lie by omission, or return stale events.
- Relays cannot forge a valid backup without the derived keys.
- Multiple relays reduce data-loss and rollback risk but do not eliminate it.


## Restore algorithm

After master-seed restore:

1. Derive the FI protocol pubkey, backup Nostr key, and content key.
2. Build the backup `d` tag.
3. Query product bootstrap backup relays, plus any relay URLs the user explicitly supplies during restore outside this encrypted backup. Do not assume encrypted user relay config is available before discovery.
4. Use a bounded filter:

```json5
{
  // Backup event kind.
  kinds: [37706],

  // Derived backup Nostr pubkey. This exact author filter is what makes restore
  // possible without unencrypted identifiers in tags.
  authors: ["BACKUP_NOSTR_PUBKEY"],

  // Query the stable addressable backup coordinate.
  "#d": ["fedi-fi-backup:v1"],

  // Relay-side hint only. The client must still enforce a local candidate cap.
  limit: 12
}
```

5. For every candidate event:
   - verify the Nostr signature;
   - verify `event.pubkey` equals the derived backup pubkey;
   - verify kind and `d` tag;
   - parse the ciphertext envelope;
   - decrypt using the derived content key and AEAD associated data;
   - verify the payload FI pubkey equals the derived FI pubkey;
   - verify the coordinate binding and supported version.
6. Group valid snapshots by `snapshot_id` and keep relay receipt metadata.
7. After the first valid decrypt, optionally query relay URLs listed inside the decrypted backup for additional candidates, then repeat validation once.
8. Choose the valid snapshot with the highest `snapshot_seq` among retained candidates. If several candidates have the same highest sequence and different `snapshot_id` values, treat that as a writer conflict. Use the highest-sequence snapshot as primary, but scan all valid candidates for formed federation records missing from the primary and present them as reconcile-only candidates. Active setup, renewal, fee, and liquidity operations from any conflicting candidate require FMan-authoritative or FLIP-authoritative reconciliation before mutation.
9. Rehydrate `fi-client` storage from the structured export.
10. Reconcile before acting:
   - fetch current FMan advertisements for every `fman_pubkey`;
   - re-verify credentials and revocations under current policy;
   - call `GetStatus` for every non-terminal reservation;
   - call `GetInviteCode` and optionally `GetPeerAttestation` if diagnostic state is missing and the seat is running;
   - preview the invite code, verify `federation_id` and config hash, and read back `fedi:fman_seat_bindings`;
   - refresh FLIP provider state before any liquidity action;
   - re-arm renewal timers from FMan-authoritative `valid_until_date` values.
11. Publish a fresh backup from the new device only after reconciliation, with `snapshot_seq` greater than the restored highest sequence. V1 does not write a parent pointer.

Safety rule after restore:

- Never automatically mint or submit payment from restored backup state alone.
- Never resend a mutating RPC until the app has either queried authoritative status or confirmed that the stored idempotency key and payload are exactly the intended retry.
- Never trust restored advertisement or credential caches for current selection decisions.


## Fork and rollback handling

Rollback risk:

- A relay can withhold the newest snapshot and return only an older valid event.
- The app cannot prove freshness from Nostr alone after total local state loss.

Mitigations:

- Publish to several relays.
- Refresh periodically so relays that prune old events see a recent replacement event.
- On a non-restored app with local state, refuse to replace local state with any fetched snapshot whose `snapshot_seq` is lower than the local last-published sequence.
- Treat same-sequence snapshots with different `snapshot_id` values as a writer-conflict signal.

Fork risk:

- Two devices restored from the same master seed can both publish backups.
- Two writers can publish different payloads with the same or increasing `snapshot_seq`.

MVP stance:

- Treat this as a detected conflict, not as normal sync.
- Restore can show the union of formed federations because those are mostly immutable discovery records.
- Active setup, renewal, fee, or liquidity operations from conflicting heads must be reconciled against FMans and FLIPs before mutation.
- The product should strongly steer users to one active FI device per seed until a real sync protocol exists.


## Privacy notes

Publicly visible to relays and observers:

- backup Nostr pubkey
- event kind `37706`
- backup `d` tag
- backup envelope version
- event timestamps
- ciphertext size bucket
- relay set contacted by the app

Encrypted:

- FI pubkey
- federation ids and invite codes
- FMan pubkeys and endpoints
- reservation ids
- unresolved ecash token hash/retry handles
- optional diagnostic peer attestations
- liquidity `details_payload_hash` handles
- user trust choices

Important choices and limitations:

- Backup events are not authored by the FI protocol key.
- With a separate backup author key and encrypted payload, public Nostr data does not directly link the backup to the FI protocol identity or a specific federation. That link appears only after decryption, or through external correlation.
- Tags carry no federation or reservation identifiers.
- Padding hides exact plaintext size, but the size bucket can still leak coarse growth.
- Refresh cadence can reveal that the app is active. This is inherent to relay-backed backups.
- Relays can observe network metadata for clients that connect to them, including IP address unless the app uses Tor, a proxy, or another relay-access privacy layer. The backup format does not solve IP-address leakage.

Deletion is best-effort only. Nostr relays may ignore delete requests or retain old data. Since ciphertext is encrypted under a seed-derived key, losing the seed is the practical erasure boundary.


## Verification checklist

A restored backup is acceptable only if all of these pass:

- Nostr event signature under derived backup pubkey
- expected kind and `d` tag
- ciphertext envelope version is supported
- `blob` base64url-decodes and is long enough to contain the version-defined nonce plus ciphertext/tag layout
- AEAD decrypts with associated data bound to author, kind, and addressable coordinate
- zstd decompresses and canonical JSON parses
- payload FI pubkey matches derived FI pubkey
- payload version is supported
- no required fields are missing for each record's state
- every restored FMan or FLIP action is reconciled before new mutation


## Implementation notes

Suggested constants for a protocol crate:

```rust
pub const FI_BACKUP_EVENT_KIND: u16 = 37706;
pub const FI_BACKUP_D_TAG: &str = "fedi-fi-backup:v1";
pub const FI_BACKUP_AEAD_DOMAIN: &[u8] = b"fedi-fi-backup/event-aead/v1\0";
pub const FI_BACKUP_SNAPSHOT_ID_DOMAIN: &[u8] = b"fedi-fi-backup/snapshot-id/v1\0";
```

Suggested role trait additions:

```text
trait FiBackupNostrClient

fetch_fi_backup_candidates(FetchFiBackupCandidatesRequest) returns events or error
publish_fi_backup_snapshot(PublishFiBackupSnapshotRequest) returns event id or error
```

The fetch path must enforce a local candidate cap. Relay `limit` is only a hint.


## Open questions

- Final event kind number.
- Final app root-secret derivation profile.
- Maximum backup event size accepted by the chosen relay set.
- Whether the initiator guardian-fee account is definitely deterministic. If not, its secret must be backed up encrypted.
- Exact pruning policy for old signed payment and renewal retry handles if an audit extension is enabled.
- Whether FI persistence semantics should change to make any currently soft setup state durable.
- Product stance for multi-device FI use with the same seed.
- Whether the app should offer manual export of the same encrypted backup document outside Nostr.
