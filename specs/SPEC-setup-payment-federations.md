# SPEC-setup-payment-federations: Common setup-payment federation publication

## Status

The reference content parser and complete Nostr admission helper implement this
contract. FA consumes and durably retains the event for paid formation. MG
consumes it as its only payment-federation policy source: the
`mg-nostr` boundary authenticates and fetches publications, retains
the admitted event through a daemon-owned store that also derives the accepted
membership, and the daemon's quote acceptance, paid availability, and wallet
join reconciliation all read that membership
([SPEC-locked-payment](../crates/mg/specs/SPEC-locked-payment.md)).
There is no operator-curated acceptance list or admin acceptance verb. The
production environment profile pins its publisher identity, but Fedi does not
yet publish kind 37707, so production paid setup remains unavailable until the
first publication. Development and staging use their profile placeholder
publishers.

A removed member's MG wallet survives a daemon restart: policy no longer
joins it, but listing and sweep reopen its retained client on demand. A
mnemonic restore rejoins it from the backed-up claim evidence
([SPEC-nostr-backup-restore](../crates/mg/specs/SPEC-nostr-backup-restore.md)),
so it is listed and sweepable there too.

No MG feature reads its [staged rollout](#staged-rollouts) state yet: MGs
only log it, so a schedule changes no behavior.

## Record justification

This contract spans Fedi's publisher, the shared domain parser, Nostr event
admission, and durable FA and MG state. No one implementation area can define
the signed wire shape, semantic invite validation, and cross-restart replacement
ordering coherently.

Fedi publishes the common set chosen by
[SPEC-locked-payment](../crates/mg/specs/SPEC-locked-payment.md)
as one Nostr addressable event. FA and MG pin the dedicated Fedi publisher key
through deployment configuration.

## Event

The version-1 event has:

```json5
{
  kind: 37707,
  pubkey: "<deployment-pinned Fedi publisher key>",
  created_at: 1730000000,
  tags: [
    ["d", "setup-payment-federations"]
  ],
  content: "{\"version\":1,\"fman_version\":\"0.2.0\",\"federations\":[\"<public Fedimint invite>\"],\"telemetry_registration_url\":\"https://telemetry.example/v1/telemetry/registrations\",\"min_fee_ppm\":1500}"
}
```

Kind `37707` is provisional. The event has exactly one `d` tag, with exactly the
value shown. Other tags do not affect admission.

The Nostr event signature is the only publication signature. Consumers retain
the complete event when they need offline verification; there is no detached
inner signature or JSON canonicalization layer.

## Publisher tooling

The production publisher accepts the complete event content as one JSON file,
deserializes it directly as `SetupPaymentFederationsContent`, and delegates
serialization, semantic validation, kind, and `d`-tag construction to the
shared Nostr event builder. It does not mirror content fields as CLI flags.
Consequently a rebuilt publisher consumes additions to the shared wire type
without a second producer field list; its complete-policy serialization fixture
fails when a required field or serialized default changes. An older binary
rejects fields it does not understand rather than signing unknown policy.

Publishing requires an independently supplied expected public key and reads
the matching secret only from a file or non-terminal standard input. The
custodian owns the complete signed-event receipts as the publisher-side durable
high-water mark and must retain the latest receipt independently of relay
storage.

An initial publication requires an explicit assertion that no prior publication
exists. An update requires and authenticates the latest prior receipt. Before
loading the secret, the tool completely queries the addressable author, kind,
and `d` selection on every Production-profile relay and refuses when that state
contradicts the asserted high-water mark. The new event's timestamp must
outrank the prior receipt under replacement ordering and remain within the
consumer future-time bound.

Publisher high-water selection authenticates the event ID/signature, author,
kind, and exact `d` tag, then treats content and timestamp as opaque. A newer
event remains the address high-water even when its schema is newer than the
running binary or its timestamp is temporarily beyond consumers' future skew
window. Full shared-schema semantic admission still applies to every new policy
before signing and to the supplied previous receipt.

The tool durably creates the new complete receipt without overwriting a file
before network access, publishes the same signed event to every relay in the
Production environment profile, reads that exact event back by ID, and also
requires it to be the current authenticated addressable selection. The saved
public receipt can be republished without the secret after a partial relay
failure, provided no higher publication has superseded it. An empty stop-set
requires a separate explicit acknowledgement.

Custody operations are serialized: exactly one publish or republish may run for
this key across all custodians and hosts. Relay preflight is not a distributed
lock. After each operation, the custodian retains the one receipt that wins
replacement ordering as the new shared high-water mark.

## Content and admission

`content` is strict JSON with these known fields:

- integer literal `version: 1`;
- `fman_version`, the latest supported Manifold Guardian release as a SemVer
  string;
- `federations`, an unordered array of public Fedimint invite strings;
- `telemetry_registration_url`, an absolute HTTPS URL with a host and no
  username, password, query, or fragment;
- `min_fee_ppm`, the smallest guardian fee rate an FA may propose, in ppm.
  Optional: absent means 1,500 (0.15%). It bounds *new* proposals
  only — each MG refuses one below it, while a rate a federation already
  adopted stays valid to carry forward and still reports as configured
  ([REQ-guardian-fee-remittance](./REQ-guardian-fee-remittance.md));
- `min_uptime_percent`, an object of uptime scores, each in percent from 0
  to 100, below which an MG stops accepting new seats: `1d`, `7d` and `30d`
  for the 1-, 7- and 30-day scores
  ([SPEC-advertisement](../crates/mg/specs/SPEC-advertisement.md#readiness-gate)).
  Optional as a whole and per window: an absent window means 70, 90 and 93
  respectively, and 0 turns a window off. Unknown windows are ignored.
  Consumers that admit unknown fields but predate this one ignore it and
  apply no uptime gate. Consumers older than unknown field admission reject
  a publication that sets it, so Fedi sets it only after those consumers have
  upgraded;
- `max_guardian_offline_secs`, a positive integer duration in seconds after
  which a guardian may qualify for replacement. Optional: absent means 86,400
  seconds (24 hours). FA replacement checks share this publication with the
  MG admission gate and reuse `min_uptime_percent.7d` and `.30d` for scores;
- `support_nostr_pubkey`, the Fedi support key MG operators chat with
  ([SPEC-mg-support-chat](../crates/mg/specs/SPEC-mg-support-chat.md)),
  as a Nostr public key. Optional: absent means the MG uses the key its
  environment profile pins. A present key overrides the profile key, so a
  newer publication with a different key rotates it. Consumers older than
  this field reject a publication that sets it, so Fedi sets it only after
  enough consumers have upgraded;
- `seat_memory_budget_mib_v2`, the RAM in MiB that each MG reserves per seat
  when it computes its memory seat limit, from 256 to 16,384. Optional:
  absent means 1,229 (about 1.2 GiB);
- `seat_cpu_iterations_per_sec`, the CPU capacity, in iterations per second
  of version 1 of the MG CPU benchmark, that each MG reserves per seat when
  it computes its CPU seat limit, from 1 to 100,000
  ([REQ-seat-capacity-default](../crates/mg/specs/REQ-seat-capacity-default.md#cpu-capacity)).
  Optional, with no default: absent means no CPU seat limit. A later benchmark
  version gets its own field, because its iterations are other work. Consumers that
  admit unknown fields but predate this one, which includes every MG and FA
  release from 0.1.1 to 0.1.15, ignore it and apply no CPU limit;
- `seat_monthly_data_gb`, the network data one seat uses, in GB (10^9 bytes)
  per 30 days, from 1 to 10,000. An MG whose operator set a monthly data
  budget sells at most that budget divided by this figure. Optional: absent
  means 65. Fedi sets it from the fleet's guardian peer transport counters.
  The field is new in 0.1.16, so it has no `_v2` twin. Consumers that admit
  unknown fields but predate it, which includes every MG and FA release
  from 0.1.1 to 0.1.15, ignore it and apply no data limit;
- `seat_disk_gb`, the size of the data filesystem, in GB (10^9 bytes), that each
  MG reserves per seat when it computes its disk seat limit, from 1 to
  100,000
  ([REQ-seat-capacity-default](../crates/mg/specs/REQ-seat-capacity-default.md#disk)).
  Optional: absent means 80. The field is new in 0.1.17, so it has no `_v2`
  twin. Consumers that admit unknown fields but predate it, which includes
  every MG and FA release from 0.1.1 to 0.1.16, ignore it and apply no disk
  limit;
- `max_seats_per_mg_v2`, a limit on total seats for each MG, from 0 to 256.
  Optional: absent means 12;
- `seat_memory_budget_mib` and `max_seats_per_mg`, the same two limits for
  MGs before 0.1.16, with the same ranges and defaults. MGs from 0.1.16 on
  ignore them and read only the `_v2` fields, with no fallback, so Fedi can
  publish separate limits for releases that run the newer readiness checks.
  A publication that wants the same limit for every release sets both;
- `max_pressure_percent`, an object of CPU, IO and memory pressure averages,
  each in percent of wall time from 1 to 100, above which an MG stops
  accepting new seats: `{"cpu": {"6h": ..., "3.5d": ...}, "io": {...},
  "memory": {...}}` for the averages with 6-hour and 3.5-day time constants.
  Pressure is the share of time in which a task of the MG's cgroup stalled
  waiting for the resource (PSI `some`)
  ([SPEC-advertisement](../crates/mg/specs/SPEC-advertisement.md#readiness-gate)).
  Optional as a whole, per resource and per window: an absent window means
  CPU 15 and 10, IO 25 and 15, and memory 70 and 70 respectively. Unknown resources and windows
  are ignored. An MG whose pressure is over a maximum, or cannot be read,
  stops advertising and quoting new seats and keeps running its seats, and
  the check does not mark uptime down. Consumers that admit unknown fields
  but predate this one, which includes every MG and FA release from 0.1.1 to
  0.1.15, ignore it and apply no pressure gate;
- `min_mg_version`, the oldest Manifold Guardian release that may advertise
  new seats, as a SemVer string no newer than `fman_version` by SemVer
  precedence. Optional: absent or `null` means no minimum, so a later
  publication that omits it or sets `null` lifts an earlier minimum. An MG
  whose release has lower precedence fails its readiness version check
  ([SPEC-advertisement](../crates/mg/specs/SPEC-advertisement.md#readiness-gate)):
  it stops advertising and quoting new seats and keeps running its seats,
  and the check does not mark uptime down. A value that is neither `null` nor
  a SemVer string, or one newer than `fman_version`, rejects the publication like any
  other invalid field, so consumers keep the policy they hold; the publisher
  refuses to sign it. MG and FA releases from 0.1.1 on admit unknown fields
  and ignore it, so a 0.1.8 MG keeps advertising;
- `recommended_min_mg_version`, the oldest Manifold Guardian release Fedi
  recommends, as a SemVer string no newer than `fman_version` by SemVer
  precedence. Optional: absent or `null` means no recommendation. An MG whose
  release has lower precedence tells its operator once, through the linked
  phone ([SPEC-guardian-link](../crates/mg/specs/SPEC-guardian-link.md)), and
  otherwise runs and sells seats as before; `min_mg_version` is the field
  that stops new seats. A value that is neither `null` nor a SemVer string,
  or one newer than `fman_version`, rejects the publication like any other
  invalid field. MG and FA releases from 0.1.1 on admit unknown fields and
  ignore it;
- `rollout`, an object of staged rollout schedules of MG features by feature
  name ([Staged rollouts](#staged-rollouts)). Optional: absent, or a feature
  absent from it, means that feature runs nowhere. MG and FA releases from
  0.1.1 to 0.1.16 admit unknown fields and ignore it; later ones reject a
  publication with an invalid schedule like any other invalid field;
- `guardian_link`, a boolean that turns on the MG's guardian link (mobile notifications)
  ([SPEC-guardian-link](../crates/mg/specs/SPEC-guardian-link.md#enabling-mobile-notifications)).
  Optional: absent means the Manifold environment's default, on in
  development and staging and off in production. Older MGs and FAs that
  admit unknown fields ignore it; and
- `guardian_transfer_v2`, a boolean that lets MGs start new direct-proof
  guardian transfers
  ([SPEC-guardian-transfer](../crates/mg/specs/SPEC-guardian-transfer.md#enabling-transfer)).
  Optional: absent means the Manifold environment's default, on in
  development and staging and off in production. MGs continue transfers they
  already stored regardless of the flag. Older MGs and FAs that admit unknown
  fields ignore it.
  The retired `guardian_transfer` field must never be set to `true` in
  production: MG 0.1.9 and older FA transfer readers use it to enable the old,
  incompatible `TransferSeat` shape. A production policy enabling the new
  flag must also set `min_mg_version` to the first release with the new shape.
  MGs that understand this minimum stop advertising new seats when below it;
  older readers may still advertise. Upgrade both transfer endpoints before
  activation; the minimum version is not a transfer protocol handshake or
  recovery mechanism for existing mixed-version transfers.

No seat field changes the operator's own maximum. Each MG sells up to
the lowest of the operator maximum and its runtime limits
([REQ-seat-capacity-default](../crates/mg/specs/REQ-seat-capacity-default.md)).
MG releases older than unknown-field tolerance reject a publication that sets
any of them, so Fedi sets them only after enough consumers have upgraded.

Consumers ignore unknown content fields so optional additions remain admissible
without a coordinated upgrade. A policy change that consumers must understand
uses a new `version`; consumers reject versions they do not support. Duplicate
known fields remain invalid.

The telemetry URL is required in version 1. It is a policy locator, not a
bearer capability. The event deliberately does not publish MG Iroh endpoint
ids, MG capabilities, invite codes, or collector locations. Each verified
MG supplies its own current locator and capability
to this URL under the separate authenticated registration contract. This keeps
global policy stable when an MG moves its endpoint and avoids making
sensitive MG access public Nostr data.

An empty array is valid and stops new paid setup. It does not itself erase
wallet state or invalidate already-issued payment or seat artifacts, whose
lifecycle remains governed by their own contracts.

Admission performs all of these checks before the set influences policy:

1. reject content larger than 128 KiB before event signature verification or
   JSON parsing;
2. verify the event ID and signature;
3. require the configured publisher key, kind `37707`, and exact `d` tag;
4. reject `created_at` more than 86,400 seconds ahead of the consumer clock;
5. reject malformed JSON, duplicate known object fields, schema versions
   other than 1, and an invalid `fman_version` SemVer;
6. reject more than 16 invites or an invite larger than 16 KiB;
7. parse every invite with the supported Fedimint parser and derive its
   canonical federation ID;
8. reject invites containing an API bearer secret;
9. reject multiple invites that derive the same federation ID.
10. reject a telemetry registration URL that is not credential-free HTTPS or
    contains a query or fragment;
11. reject a `min_fee_ppm` above the payer's 210,000-ppm send-rate ceiling,
    which would leave no proposable rate;
12. reject a `seat_memory_budget_mib` or `seat_memory_budget_mib_v2` outside
    256 to 16,384, a `seat_cpu_iterations_per_sec` outside 1 to 100,000, a
    `seat_monthly_data_gb` outside 1 to 10,000, a `seat_disk_gb` outside 1 to
    100,000, a `max_seats_per_mg` or
    `max_seats_per_mg_v2` above 256, or a `max_pressure_percent` window
    outside 1 to 100, where 0 would close every MG and no share of time
    exceeds 100;
13. reject a `min_uptime_percent` window above 100, which no MG can reach;
14. reject a `min_mg_version` that is neither `null` nor a SemVer string, or
    that has higher SemVer precedence than `fman_version`, which no MG could
    run; and the same for `recommended_min_mg_version`;
15. reject a `rollout` that is not an object, has more than 32 features, a
    feature name outside `[a-z0-9_]{1,64}` or a repeated one, or a schedule
    that is not 1 to 16 `[unix_secs, percent]` points with strictly
    increasing times and integer percentages from 0 to 100.

Array position carries no preference or fallback meaning. A consumer uses the
derived federation ID as the member identity and the signed invite as its join
material.

MG compares its Cargo package version to `fman_version` using SemVer
ordering. Its operator API exposes the running version, the published version,
and whether the running version is lower. An equal or newer local version is
accepted, which permits staged rollouts and development builds ahead of the
publication. This is a cheap rollout signal for operator consumers, not a
daemon kill switch.

## Staged rollouts

`rollout` lets Fedi turn an MG feature on for a growing share of the fleet
without a release per step. Each feature's schedule is a list of
`[unix_secs, percent]` points, for example
`{"aleph_backup_batch": [[1795000000, 0], [1795086400, 10]]}`. Between two
points the percentage moves linearly with time; before the first point it is
the first point's and after the last point the last point's, so an MG whose
clock is off still sees a percentage the publication meant to hold.

Each MG stores one private seed of 32 random bytes, created at its first start
and never derived from the root mnemonic, so rollout state is outside
[CLAIM-manifold-guardian-confines-secret-dependent-content](../crates/mg/specs/CLAIM-manifold-guardian-confines-secret-dependent-content.md).
A unit's draw for a feature is the first four bytes, big-endian, of SHA-256
over `fedi-manifold-guardian/rollout-draw/v1`, the seed, the unit and the
feature name; the unit is byte 1 and the 32 seat-id bytes for a seat, or byte
2 for the whole MG. The code that defines a feature chooses the unit: per seat
by default, per MG for a host-level feature; the publication cannot. A unit
runs a feature while `draw / 2^32 * 100` is below the schedule's percentage at
the MG's wall clock, computed exactly. Consequently:

- the units at a percentage are a subset of those at any higher percentage, so
  raising it only adds units and lowering it only removes them;
- draws are independent between features and seats, and stable across
  restarts of one data root; a new data root, such as a restored MG on a new
  host, draws anew;
- the publication cannot target or exclude a seat or MG, and Fedi does not
  learn which units a percentage selects except through their telemetry;
- a feature that the running MG does not know has no effect beyond its log,
  and an MG without any admitted publication runs no feature.

Each MG evaluates every published feature for every live seat on each newly
admitted publication and every 30 seconds, and logs the first state and each
change as a safe-to-share event with only the seat id, the feature name and
whether it is enabled. Fedi can therefore ramp a name that no code reads yet
and watch the fleet follow. That telemetry places a seat's draw on one side
of each percentage it crossed; publishing under a new feature name gives every
seat a new draw.

The publisher refuses a publication in which any feature's percentage at the
new event's `created_at` differs by more than one point from the previous
publication's at that time, where an unscheduled feature is at 0, unless the
operator acknowledges the jump explicitly. Every schedule change therefore
starts from where the previous schedule stands: pausing publishes one point
at the current percentage, and a rollback ramps down from it. An MG that
loses its relays keeps following the last schedule it admitted, so Fedi
publishes ramps of at most about a day and extends them with later
publications.

## Replacement and durable state

Consumers use NIP-01 addressable-event replacement order:

- greater `created_at` replaces lower `created_at`;
- at equal `created_at`, the lower event ID wins.

Each consumer atomically and durably retains the complete highest admitted
event. After restart it statically revalidates that trusted stored event before
using it as the opaque high-water mark. An event below that mark cannot replace
it after restart or relay change. Re-admitting the identical event is idempotent
even if the local clock moved backwards.

A release retains content it does not interpret, so a stored event can still
authenticate but fail a later release's content admission, for example a
`min_mg_version` that is not SemVer, retained by a release that ignored the
field. MG then starts with no admitted policy, as before its first admission,
keeps the stored event as the high-water mark, and admits the next higher
event that passes. Only authentication failures stop startup (see rotation
below).

Events do not expire. The last admitted event remains authoritative until a
higher event is admitted. This keeps publisher, relay, network, or clock outages
from disabling setup merely because time passed; it also means an isolated
consumer can retain policy that Fedi has since replaced.

A valid event up to 24 hours in the future is admissible and can temporarily
prevent normally timestamped replacements. The bound limits that damage from a
publisher clock mistake. Publisher-key rotation and recovery are out-of-band
deployment operations in version 1; the environment profile carries the
canonical publisher, overridable only in the Development profile. Rotation
invalidates every consumer's retained event: post-restart revalidation
against the new publisher fails loudly (in the MG daemon, startup refuses
before spawning its RPC router) until the retained publication is cleared as
part of the rotation procedure — fail closed, never silently trusting a stored
event the current profile no longer authenticates. MG additionally binds a
data root to its first Manifold environment before onboarding and refuses a
cross-environment restart before reading environment-derived policy
([SPEC-manifold-environment](../crates/manifold-environment/specs/SPEC-manifold-environment.md)).

This helper bounds event content before signature verification. Relay and
transport code must also bound the complete incoming event frame,
including tags, before invoking admission. FA relay and CLI transports reject a
normalized complete event larger than 64 KiB. Relay lookup observes at most 16
candidates and retains at most 1 MiB per common-set query.

## Convergence boundary

Publication is one-way and eventually consistent. FA and MG can temporarily
hold different admitted events during an update; this contract does not claim
simultaneous activation or uninterrupted payability across a set change.
FA retains last-known-good policy through fetch failures, treats an empty set as
a stop for new paid formation, filters the authenticated set by wallet
capability, and submits the canonical selected federation ID to `GetQuote`.
MG-local rejection remains an actionable quote failure rather than a
discovery-policy input. Joining, MG reconciliation, and publisher deployment
belong to their consuming components.

MG couples replacement to its quote pricing: it retains the admitted event,
replaces its derived accepted membership, and — whenever any previously
accepted member is no longer in the set — draws a fresh offer epoch, all in
one database transaction. Every outstanding quote is thereby refused (with its
refund) rather than settled against a removed federation, and a crash can
never retain a replacement without that effect. The MG wallet never leaves
a removed federation: received balances stay sweepable, and in-flight
refund settlement against it keeps working.
