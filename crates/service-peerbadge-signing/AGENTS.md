# PeerBadge signing protocol agent notes

- Read [SPEC-peerbadge-signing-protocol](specs/SPEC-peerbadge-signing-protocol.md)
  before changing DTOs, errors, the challenge digest, or the service trait.
- Keep Rust golden vectors and the credential-app TypeScript client synchronized.
  Byte vectors require `serde_bytes`; field order is part of the encoding.
- Keep daemon policy and issuer secret handling out of this shared crate.
- Read [SECURITY.md](SECURITY.md) before changing authentication or wire bounds.
