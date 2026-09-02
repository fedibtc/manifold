# Security

This is a pure library crate: it performs no relay I/O and holds no state. It
owns the cross-program Nostr event kinds, tags, and signing constants shared
by decentralized-federation components, plus the one typed rendering of the
kind-37701 FMan advertisement document that the FMan-side publisher and the
FI consumer both depend on.

`verify_advertisement_self_signature` checks ONLY the document's own payload proof — the
FMan service-key Schnorr signature over the canonicalized payload. It is NOT
sufficient admission. Consumers must additionally authenticate the outer
Nostr event (id and signature), require the event author to equal the signed
payload's `fman_id_pubkey`, and verify every embedded trust envelope they
rely on through the shared PeerBadge verifier, binding the returned subject
to the event author. A document that passes `verify_advertisement_self_signature` alone
proves nothing about who published it or whether anyone vouches for them.

The v2 advertisement payload's `service_pubkey` — the FMan's
commitment-signing key, which locators carry and FIs verify signed FMan
responses against — is attested only by the chain above: it sits inside the
self-signed payload, whose signer the consumer pipeline binds to the
authenticated event author and to the badge subject the embedded holder
authorization vouches for. The claim "this Nostr identity's service key is
X" is thus made by the same authenticated identity the badge vouches for,
and by nobody else; a consumer that dials with that key trusts it exactly as
much as it trusts that identity.

The signature domain separators, the JCS canonicalization, and the payload
schemas rendered here are cross-program wire contracts governed by
[`specs/SPEC-fman-nostr-events.md`](./specs/SPEC-fman-nostr-events.md):
external programs (the Fedi app/SDK, Issuer tooling, FLIP implementations)
produce and verify these bytes. Any byte-level change — a constant, a field,
serde attributes, canonicalization behavior — is a coordinated wire break,
never a local refactor.

Report security-sensitive issues through the repository process in the root
[SECURITY.md](../../SECURITY.md).
