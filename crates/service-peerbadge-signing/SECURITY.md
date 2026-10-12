# PeerBadge signing protocol security boundaries

These DTOs carry untrusted data, not proof of authorization. The server must
validate the signer, level, nonce, signature, and session lifecycle before
signing. Transport authentication identifies an Iroh peer, not an authorized
Nostr signer. The domain-separated challenge binds the requested level and
issuer identity; preserve its exact byte order.

Session IDs are bearer capabilities. Issuance JSON is blind-protocol material:
never log or persist its contents, including in Debug output or error details.
The only permitted audit representation is a SHA-256 digest. The holder's
unblinded identity never belongs in the signing exchange.

See [SPEC-peerbadge-signing-protocol](specs/SPEC-peerbadge-signing-protocol.md)
and the [server security notes](../peerbadge-signing-server/SECURITY.md).
