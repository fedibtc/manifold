# CLAIM-fleet-manager-holder-authorization-bound: Admitted Holder authorizations are cryptographically bound

For every holder-authorization envelope the official Fleet Manager daemon
admits from Nostr, reports through its operator or FI-facing interfaces,
republishes in an advertisement, or sends in a telemetry registration, let `S` be
`envelope.holder_authorization.authorization` and let `C` be
`envelope.signed_credential.credential`. The envelope has all of these
properties:

1. its authorization proof verifies under `S.holder_id_pubkey` over the
   credential SDK's domain-separated canonical encoding of all of `S`;
2. `S.subject_pubkey` equals this daemon's Nostr service public key; and
3. `S.credential_digest` equals `C.digest()`.

Here **holder means only the public key in `S.holder_id_pubkey`**. “The holder
made the authorization” means that the envelope contains a signature verifying
under that key, subject to the cryptographic axioms below. It does not mean
that a named person controlled the key, that this key is the holder encoded in
`C`, or that `C` has a valid issuer proof. “This credential” means the
`Credential` payload `C`, which is what its digest covers, not the adjacent
`SignedCredential.proof` bytes.

The adversary controls the configured Nostr relay, omission and ordering of
its responses, and arbitrary event-author keys and event bytes. It may replay
authentic events and may return events that do not match the requested kind or
tags. It cannot forge a signature under an uncompromised key or find the hash
collisions excluded by A1. The claim covers the daemon's durable authorization cache, current in-memory
authorization vector, the operator's `AuthorizationObserved` projection, the
FI-facing `GetFmanTrustMaterial` response, kind-37701 advertisement
carriage, and guardian telemetry-registration carriage. It makes no claim about a relying consumer's acceptance of that
material.

## Assumptions

- **A1 cryptography and canonicalization:** BIP-340 signatures are
  unforgeable; SHA-256 is collision- and second-preimage-resistant; and the
  pinned credential SDK faithfully implements its documented JCS encoding,
  holder-authorization signature domain, `HolderAuthorization::verify`, and
  `Credential::digest`. Thus a proof accepted by `verify` authenticates every
  field of `S` under `S.holder_id_pubkey`, and distinct credential payloads do
  not have the same digest. `nostr_sdk::Event::verify` faithfully checks the
  event ID and signature over the complete event.
- **A2 official-process integrity:** the official binary and its pinned
  dependencies execute the reviewed code without memory corruption or code
  injection. The daemon derives one Nostr service key from its fleet
  identity, gives it to `FleetManagerNostr`, and binds that concrete runtime's
  `NostrTrustMaterialSource` into `FleetManagerRpc`, passes that same runtime
  to the telemetry worker, and gives its durable store only to that runtime.
  Arbitrary library callers that inject another trust source, construct or
  mutate a store directly, or compose these public crates differently are
  outside the claim.
- **A3 value preservation:** Rust ownership, Serde/JSON round trips, SQLite
  parameter binding and committed row reads, and Tokio watch cloning preserve
  the typed fields of an accepted envelope between verification and each sink. An attacker cannot mutate an envelope behind a
  shared safe-Rust reference after its checks.
