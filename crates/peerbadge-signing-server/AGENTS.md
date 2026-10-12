# PeerBadge signing server agent notes

- Read [ARCH-peerbadge-signing-server](specs/ARCH-peerbadge-signing-server.md)
  and [SECURITY.md](SECURITY.md) before changing issuance or lifecycle policy.
- Wire types belong in `service-peerbadge-signing`; coordinate changes with its
  Linked Spec, golden vectors, and the credential-app TypeScript client.
- Never log or persist issuance-request contents beyond a SHA-256 digest.
- Keep the workspace Iroh pin and environment trust roots unchanged unless
  explicitly authorized. Development/Staging issuer fixtures are public test keys.
- Exercise the relay-free issuance/verifier integration and rejection tests.
