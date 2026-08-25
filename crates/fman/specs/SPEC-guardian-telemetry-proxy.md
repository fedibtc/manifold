# SPEC-guardian-telemetry-proxy: guardian telemetry over FMan Iroh

## Record justification

No single artifact can own this contract because the FMan journal and metrics
producers, service wire types, Iroh server, registration worker, and independent
collector must preserve one authorization and continuity boundary.

## Decision

An FMan exposes one authenticated telemetry service over the fixed
`fedi/fman/guardian-telemetry/1` Iroh ALPN. One 32-byte capability authorizes
all three surfaces owned by that FMan:

- discovery of its known seats and their optional public invite codes;
- default-deny projected access to a Running seat's loopback `fedimintd`
  Prometheus endpoint; and
- listing and incrementally reading the FMan and retained seat safe-event
  journals.

The capability is global to the FMan identity. It is derived with HKDF-SHA256
from the protected root mnemonic using
`fman/v1/telemetry/<generation>`. SQLite stores one nonnegative monotone
generation for the whole FMan, never bearer plaintext. There is no per-seat
authorization or acknowledgement state.

The `fedimintd` process remains the metrics producer. After authentication and
seat selection, FMan applies the exact compiled source profile and transports
only independently valid allowlisted families. The collector repeats the same
default-deny projection before adding collector-owned identity labels. During a
mixed rollout, new collectors therefore accept old raw FMan responses, while an
old collector accepts a new FMan response as a strict safe subset of source
metrics.

## Why this shape

The stakeholder decision in
[`docs/telemetry/telemetry.md`](../../../docs/telemetry/telemetry.md) chose
FMan-based enrollment and Prometheus endpoint access. A single FMan capability
matches that trust boundary: a party authorized to discover every seat on the
FMan is also authorized to scrape those seats and read explicitly shareable
journals. Seat ids select resources; they are not separate authorization
scopes.

The collector dials the existing FMan Iroh endpoint, so home-hosted deployments
need no public port, DNS, certificate, or reverse proxy. Public HTTPS guardian
endpoints, recurring metric uploads, FI relaying, and publishing Iroh bearer
material in Nostr remain rejected alternatives.

## Protocol and bounds

All requests carry `TelemetryCapability`. `list_guardian_telemetry_seats`
returns every seat known to the fleet, sorted by `SeatId`, as
`{ seat_id, invite_code: Option<InviteCode> }`. An unavailable or
not-yet-formed seat is still discoverable with no invite.

`scrape_guardian_metrics` additionally names a `SeatId`. After global
capability verification, FMan permits only a Running seat, connects only to that
child's fixed loopback metrics address, disables redirects, requires an
unencoded successful Prometheus text response, and returns canonical projected
text. Missing/wrong capability is `permission_denied`; an authorized request
for a non-Running or temporarily unavailable child is the same coarse retryable
`unavailable`.

The Iroh request frame is capped at 4 KiB, with at most eight concurrent
handlers and a five-second initial-frame deadline. Loopback connection and total
request timeouts are two and ten seconds. Responses are streamed into a hard
4 MiB maximum before serialization, and clients apply a bounded response limit
with envelope headroom.

`list_safe_event_journals` returns the global FMan journal followed by every
known retained seat journal. Each journal has a persisted UUIDv7 incarnation,
stable across ordinary restart and segment rotation and regenerated when that
journal's storage is recreated or replaced. Consumers compare the complete
incarnation value for identity; its embedded timestamp has no ordering authority.

Within one incarnation, cursor segment numbers form a durable monotone namespace:
the journal reserves each next number durably before using it and never reuses a
number after rotation, retention deletion, crash, or restart. After any reopen or
crash-tail repair, the writer durably creates a fresh segment before accepting
the first new record and never appends new bytes at offsets in a pre-reopen
segment. It fails closed rather than guessing an incarnation or next segment.

A supported restore excludes safe-event journals or creates a new incarnation
before exposing a restored journal. Restoring an entire volume can roll back both
the records and their in-volume incarnation/reservation state consistently; no
token on that volume can detect this. The operational restore procedure must force
a new incarnation after such a rollback.

`fetch_safe_event_journal` accepts one listed journal, its incarnation, and an
optional incarnation-bound `(segment, byte offset)` cursor, and echoes the
current incarnation. A different request or cursor incarnation produces the
typed `incarnation_changed` result with no data selected rather than applying
its segment coordinates to new storage. A current fetch returns at most 768 KiB
and ends only after a complete newline-terminated record. Missing segments,
invalid offsets, and
non-record-boundary offsets restart at the oldest retained record with
`continuity_gap = true`. A changed incarnation likewise requires the collector
to restart at the oldest retained record and record a continuity gap. An
incomplete concurrent tail remains for a later pull.

Capability comparison is constant-time and precedes seat lookup, child probing,
journal enumeration, or filesystem access. Capabilities, raw bodies, invite
codes, journal identifiers, incarnations, cursors, and seat identifiers never
appear in logs or metrics labels.

## Enrollment and recovery

Fedi's signed setup-payment policy contains only the credential-free HTTPS
registration URL. On startup, policy change, and every 15 minutes, FMan sends
one idempotent FMan-level registration containing its Iroh endpoint id, durable
capability generation, corresponding capability, and current Holder authorization.

Registration uses NIP-98 bound to the exact method, public endpoint URL, and
request body. The receiver performs current Holder credential, issuer-policy,
and revocation verification and requires the NIP-98 signer to equal the Holder
subject. No seat proof or federation preview is required: the Holder credential
authorizes the FMan, while the NIP-98 proof binds the submitted endpoint and
capability to that identity.

The strict standalone collector atomically admits the one encrypted target keyed
by FMan public key. A lower generation is rejected. The same generation is
accepted only with the same capability, while its signed endpoint may change; a
greater generation replaces both. Independently, an accepted NIP-98 `created_at`
may not move backward. Thus a fresh, correctly signed old body cannot roll
capability state back. Periodic idempotent registration recovers collector
database loss and endpoint changes without acknowledgements. Generation is part
of the exact NIP-98-bound body. Making it required is a deliberate pre-production
wire incompatibility: receivers reject registrations from older FMan builds
rather than accepting a body without rollback ordering. The owner-only local
`admin reenroll-telemetry` command durably advances the global generation,
immediately invalidates the prior bearer for discovery, metrics, and journals,
and wakes the worker to register the replacement. This is the explicit
operator recovery path when existing collector access should be revoked or a
receiver has lost its target. Downstream collectors first request the
authenticated seat list and then decide which returned seats or invites to
consume. Authentication proves which FMan made the response; it does not
attest that a returned seat id or invite names a real federation. Consumers
that need that assurance must independently verify the invite-derived
federation/config binding.

The generation is durable only with the complete data root. Mnemonic-only
restore starts from generation zero and can therefore re-derive a bearer used
before the lost database. Operators must treat telemetry access as needing
fresh recovery handling after such a restore; the mnemonic alone cannot prove
the lost generation high-water mark.

## Privacy inventory

The checked inventory is compiled into FMan and the collector. Each pinned
`fedimintd` and Manifold module-set change must inventory emitted metric names
and label dimensions before changing that profile. Both boundaries discard
known-denied, unknown, and family-local invalid input; neither has a raw
fallback. The current policy and the cloud ownership boundary are
[documented together](../../../specs/ARCH-cloud-fman-telemetry.md).
