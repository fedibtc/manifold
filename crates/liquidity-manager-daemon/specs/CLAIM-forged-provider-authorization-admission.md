# CLAIM-forged-provider-authorization-admission: Forged provider authorization admission

For every holder-authorization envelope the official FLIP daemon admits from a
relay, retains durably, reports through its Admin API, or embeds in a published
advertisement, let `S` be `envelope.holder_authorization.authorization` and let
`C` be `envelope.signed_credential.credential`. The envelope has all of these
properties:

1. its authorization proof verifies under `S.holder_id_pubkey` over the
   credential SDK's domain-separated canonical encoding of all of `S`;
2. `S.subject_pubkey` equals the provider public key installed in this daemon's
   `provider_identity`;
3. `S.credential_digest` equals `C.digest()`; and
4. `C.blind_msg` parses as a public key equal to `S.holder_id_pubkey`.

Here **holder means only the public key in `S.holder_id_pubkey`**. "The holder
made the authorization" means the envelope contains a signature verifying under
that key, subject to the cryptographic axioms below. It does not mean a named
person controls the key, or that `C` has a valid issuer proof. Property 4 does
establish that `C`'s revealed holder binding names this same key, which is a
comparison of two fields rather than a judgement of the badge. "This credential" means the `Credential` payload
`C`, which is what its digest covers, not the adjacent `SignedCredential.proof`
bytes.

The adversary controls every configured Nostr relay, the omission and ordering
of their responses, and arbitrary event-author keys and event bytes. It may
replay authentic events and may return events that do not match the requested
kind or tags. It cannot forge a signature under an uncompromised key or find
the hash collisions excluded by A1. The claim covers the durable
`holder_authorization_events` table, the `provider_trust_envelopes` result, the
`get_holder_authorization_state` and `refresh_holder_authorizations` responses,
and kind-37702 advertisement carriage. It makes no claim about a relying
consumer's acceptance of that material.

## Status

Falsified: accepted requests durably retain FMan-subject authorization envelopes, so the claim’s provider-subject predicate does not hold for every retained envelope.

## Assumptions

- **A1 cryptography and canonicalization:** BIP-340 signatures are unforgeable;
  SHA-256 is collision- and second-preimage-resistant; and the pinned
  credential SDK faithfully implements its documented JCS encoding,
  holder-authorization signature domain, `HolderAuthorization::verify`, and
  `Credential::digest`. Thus a proof accepted by `verify` authenticates every
  field of `S` under `S.holder_id_pubkey`, and distinct credential payloads do
  not have the same digest. `nostr_sdk::Event::verify` faithfully checks the
  event ID and signature over the complete event.
- **A2 official-process integrity:** the official binary and its pinned
  dependencies execute the reviewed code without memory corruption or code
  injection. The daemon resolves the provider public key for every admission
  and every read from the `provider_identity` row through
  `identity::load_provider_identity` or `identity::find_provider_identity`, and
  binds one `HolderAuthorizationFetcher` into `DaemonContext` at construction.
  Library callers that compose these public items differently are outside the
  claim.
- **A3 value preservation:** Rust ownership, Serde/JSON round trips, and SQLite
  parameter binding and committed row reads preserve the typed fields of an
  accepted envelope between verification and each sink.
