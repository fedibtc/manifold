# SPEC-setup-payment-federations: Common setup-payment federation publication

## Status

The reference content parser and complete Nostr admission helper implement this
contract. FI consumes and durably retains the event for paid formation. FMan
consumes it as its only payment-federation policy source: the
`fman-nostr` boundary authenticates and fetches publications, retains
the admitted event through a daemon-owned store that also derives the accepted
membership, and the daemon's quote acceptance, paid availability, and wallet
join reconciliation all read that membership
([SPEC-locked-payment](../crates/fman/specs/SPEC-locked-payment.md)).
There is no operator-curated acceptance list or admin acceptance verb. Fedi
does not yet publish kind 37707, and the production environment profile
deliberately carries no publisher identity, so production paid setup remains
impossible until the deployment-owned key exists; development and staging use
their profile placeholder publishers.

FMan's intended removed-member retention has a known restart deviation: its
durable client prefix and balance remain, but the restarted daemon does not
reopen that scope. Its supported listing retains the scope with an unknown
wallet projection, but configured-destination sweep cannot reach it. See the falsified
[CLAIM-fleet-manager-payment-continuation-progresses](../crates/fman/specs/CLAIM-fleet-manager-payment-continuation-progresses.md).

## Record justification

This contract spans Fedi's publisher, the shared domain parser, Nostr event
admission, and durable FI and FMan state. No one implementation area can define
the signed wire shape, semantic invite validation, and cross-restart replacement
ordering coherently.

Fedi publishes the common set chosen by
[SPEC-locked-payment](../crates/fman/specs/SPEC-locked-payment.md)
as one Nostr addressable event. FI and FMan pin the dedicated Fedi publisher key
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

`content` is strict JSON with exactly:

- integer literal `version: 1`;
- `fman_version`, the latest supported Fleet Manager release as a SemVer
  string;
- `federations`, an unordered array of public Fedimint invite strings;
- `telemetry_registration_url`, an absolute HTTPS URL with a host and no
  username, password, query, or fragment; and
- `min_fee_ppm`, the smallest guardian fee rate an FI may propose, in ppm. The
  only optional field: absent means 1,500 (0.15%). It bounds *new* proposals
  only — each FMan refuses one below it, while a rate a federation already
  adopted stays valid to carry forward and still reports as configured
  ([REQ-guardian-fee-remittance](./REQ-guardian-fee-remittance.md)).

The telemetry URL is required in version 1. It is a policy locator, not a
bearer capability. The event deliberately does not publish FMan Iroh endpoint
ids, FMan capabilities, invite codes, or collector locations. Each verified
FMan supplies its own current locator and capability
to this URL under the separate authenticated registration contract. This keeps
global policy stable when an FMan moves its endpoint and avoids making
sensitive FMan access public Nostr data.

An empty array is valid and stops new paid setup. It does not itself erase
wallet state or invalidate already-issued payment or seat artifacts, whose
lifecycle remains governed by their own contracts.

Admission performs all of these checks before the set influences policy:

1. reject content larger than 128 KiB before event signature verification or
   JSON parsing;
2. verify the event ID and signature;
3. require the configured publisher key, kind `37707`, and exact `d` tag;
4. reject `created_at` more than 86,400 seconds ahead of the consumer clock;
5. reject malformed JSON, unknown or duplicate object fields, schema versions
   other than 1, and an invalid `fman_version` SemVer;
6. reject more than 16 invites or an invite larger than 16 KiB;
7. parse every invite with the supported Fedimint parser and derive its
   canonical federation ID;
8. reject invites containing an API bearer secret;
9. reject multiple invites that derive the same federation ID.
10. reject a telemetry registration URL that is not credential-free HTTPS or
    contains a query or fragment;
11. reject a `min_fee_ppm` above the payer's 210,000-ppm send-rate ceiling,
    which would leave no proposable rate.

Array position carries no preference or fallback meaning. A consumer uses the
derived federation ID as the member identity and the signed invite as its join
material.

FMan compares its Cargo package version to `fman_version` using SemVer
ordering. Its operator API exposes the running version, the published version,
and whether the running version is lower. An equal or newer local version is
accepted, which permits staged rollouts and development builds ahead of the
publication. This is a cheap rollout signal for operator consumers, not a
daemon kill switch.

## Replacement and durable state

Consumers use NIP-01 addressable-event replacement order:

- greater `created_at` replaces lower `created_at`;
- at equal `created_at`, the lower event ID wins.

Each consumer atomically and durably retains the complete highest admitted
event. After restart it statically revalidates that trusted stored event before
using it as the opaque high-water mark. An event below that mark cannot replace
it after restart or relay change. Re-admitting the identical event is idempotent
even if the local clock moved backwards.

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
against the new publisher fails loudly (in the FMan daemon, startup refuses
before spawning its RPC router) until the retained publication is cleared as
part of the rotation procedure — fail closed, never silently trusting a stored
event the current profile no longer authenticates. FMan additionally binds a
data root to its first Manifold environment before onboarding and refuses a
cross-environment restart before reading environment-derived policy
([SPEC-manifold-environment](../crates/manifold-environment/specs/SPEC-manifold-environment.md)).

This helper bounds event content before signature verification. Relay and
transport code must additionally bound the complete incoming event frame,
including tags, before invoking admission. FI relay and CLI transports reject a
normalized complete event larger than 256 KiB. Relay lookup observes at most 16
candidates and retains at most 4 MiB across one common-set query.

## Convergence boundary

Publication is one-way and eventually consistent. FI and FMan can temporarily
hold different admitted events during an update; this contract does not claim
simultaneous activation or uninterrupted payability across a set change.
FI retains last-known-good policy through fetch failures, treats an empty set as
a stop for new paid formation, filters the authenticated set by wallet
capability, and submits the canonical selected federation ID to `GetQuote`.
FMan-local rejection remains an actionable quote failure rather than a
discovery-policy input. Joining, FMan reconciliation, and publisher deployment
belong to their consuming components.

FMan couples replacement to its quote pricing: it retains the admitted event,
replaces its derived accepted membership, and — whenever any previously
accepted member is no longer in the set — draws a fresh offer epoch, all in
one database transaction. Every outstanding quote is thereby refused (with its
refund) rather than settled against a removed federation, and a crash can
never retain a replacement without that effect. The FMan wallet never leaves
a removed federation: received balances stay sweepable, and in-flight
refund settlement against it keeps working.
