# SPEC-guardian-telemetry-receiver: verified FMan telemetry admission

## Status

Registration, encrypted persistence, seat discovery, and the protected raw
metrics pull adapter are implemented. Push-gateway does not expose journal HTTP
routes, perform scheduled collection, or enforce generation and authentication
time ordering when replacing a registration. The standalone
[cloud collector](../../../specs/ARCH-cloud-fman-telemetry.md) owns strict
registration admission rather than consuming this transitional adapter.

## Record justification

No single artifact can own this contract because public registration routing,
NIP-98 and Holder verification, encrypted repository state, protected operator
routing, and the independently implemented FMan Iroh service must agree.

## Purpose and boundary

The Fedi-operated push-gateway deployment hosts a narrowly scoped guardian
telemetry control plane. A credentialed FMan registers one Iroh endpoint and
one current telemetry capability. The receiver stores that FMan-level target
encrypted. Protected operator endpoints can discover its seats, including an
optional invite code for each, and pull a selected Running guardian's live raw
Prometheus response.

This is endpoint access, not periodic metrics upload or scheduled collection.
FMan does not interpret or retain guardian metrics. The standalone collector,
not this receiver or its operator adapter, owns scrape selection, cadence,
latest metric snapshots, journal cursors, and journal archives. Prometheus owns
metrics history and long-term metric retention. The receiver does not forward
invites to an Observer.

## Registration admission

Fedi's signed setup-payment policy contains one credential-free HTTPS
`telemetry_registration_url`. FMan periodically sends:

- protocol version;
- stable Iroh endpoint id;
- durable FMan-wide capability generation;
- the FMan-wide 32-byte `TelemetryCapability`; and
- its current Holder authorization envelope.

`POST /v1/telemetry/registrations` accepts at most 64 KiB and requires NIP-98
authentication bound to the exact method, configured public URL, and body hash,
with a fresh valid signature and replay protection. Shared PeerBadge
verification checks the Holder authorization's complete authority chain,
revocation state, schema, selected environment issuer policy, and minimum trust
level. The NIP-98 signer must equal the verified Holder subject.

Holder authorization proves admission of the FMan identity. NIP-98 proves that
identity submitted the exact endpoint and capability. No caller-supplied
federation, peer, or seat assertion is trusted or required; seats are discovered
later through the authenticated FMan service.

The generation is a required part of the exact signed body. Adding it is a
deliberate pre-production wire incompatibility: this receiver rejects older FMan
registrations that omit it. This transitional receiver verifies generation's
signature but does not compare it with prior registrations; the standalone
collector owns rollback-safe generation and NIP-98 `created_at` admission.

Malformed endpoint ids, invalid/stale/below-minimum credentials, signer-subject
mismatch, invalid NIP-98 proofs, and replays fail with sanitized errors before
persistence. Production telemetry startup fails closed when the environment
cannot supply valid issuer roots or a valid minimum-trust policy.

## Durable state and recovery

The stable target key is the canonical FMan Nostr public key. AES-256-GCM
encrypts endpoint and capability with a fresh random nonce and target-specific
associated data. The deployment key is not stored in the database and must be
backed up separately. Database backups remain sensitive.

Registration is an idempotent replacement in this transitional receiver. FMan
repeats it at startup, after policy changes, and every 15 minutes, so receiver
database loss or endpoint change heals without per-seat acknowledgements. An
operator-triggered global rotation forces immediate replacement registration.
Until the next valid registration, a lost target
simply remains unavailable; authorization is never bypassed.

## Collector surface

The protected operator router exposes:

- `GET /v1/telemetry/fmans/{fman_pubkey}/seats`, which calls the authenticated
  FMan seat-list RPC and returns `{ seat_id, invite_code: Option<InviteCode> }`
  for every known seat. These values are authenticated as FMan assertions, not
  independently verified federation/config bindings; and
- `GET /v1/telemetry/fmans/{fman_pubkey}/seats/{seat_id}/metrics`, which calls
  the same FMan endpoint with the same capability and mirrors the selected
  guardian's HTTP status, content type, content encoding, and body bytes.

Both routes inherit the operator bearer/network boundary. They validate the
canonical FMan key and seat id, load and decrypt one FMan target, dial
`fedi/fman/guardian-telemetry/1`, and apply connection, request, and response
bounds. Missing targets return 404, unavailable Iroh/FMan/guardian dependencies
return a coarse 503, and corrupt ciphertext returns a sanitized 500.

The same target authorizes safe-event journal list/fetch RPCs at the FMan Iroh
boundary. Push-gateway exposes no HTTP routes for those verbs and is not the
journal collection path.

## Secrets, logs, and observability

The capability, Holder authorization, encryption key, Iroh locator, FMan
identity, seat id, invite, raw metric body, journal identifiers, and cursors
must not appear in logs, errors, traces, or metrics labels. DTO and config
`Debug` output redacts secrets. HTTP metrics use route templates only.

Low-cardinality operational state is limited to whether telemetry is configured
and the current FMan target count. The byte-preserving metrics layer is not a
privacy filter; source metrics must be fixed at their producer or governed by
explicit downstream scrape policy.
