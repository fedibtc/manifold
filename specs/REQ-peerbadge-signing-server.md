# REQ-peerbadge-signing-server: Delegated blind badge issuance

The credential-app signing-server integration requires one company-operated
PeerBadge issuer to serve authorized Nostr signers without distributing the
issuer secret to their devices. A signer authorizes a trust level and presents
one offer; the holder redeems that offer directly and retains sole knowledge
of its unblinded holder identity.

The server must authenticate signers using a fresh single-use Schnorr challenge,
enforce a configured allowlist, level ceiling, and hourly session limit, and
issue at most one credential per expiring bearer session. Issuance must use the
PeerBadge SDK and remain compatible with ordinary Holder authorization and
Manifold verification. Signers can poll completion without seeing the holder.

The TypeScript client and native server must share a stable CBOR contract,
including byte-exact golden vectors. See
[SPEC-peerbadge-signing-protocol](../crates/service-peerbadge-signing/specs/SPEC-peerbadge-signing-protocol.md).

Audit records may identify the signer, level, and session, but must never retain
issuance-request contents beyond their SHA-256 digest. Development and Staging
reuse their complete public issuer fixtures and pinned authorities; this does
not establish secure trust. Production requires explicit key custody and a
separately authorized trust-root rollout; no production roots change here.

Acceptance includes policy/lifecycle rejection tests, audit checks, and a
relay-free native Iroh issuance flow whose Holder authorization passes the
Development profile's offline issuance verifier. Local CLI key generation,
authority publication, ticket output, and administrative revocation are in
scope; container images and cluster deployment are not.

Architecture: [ARCH-peerbadge-signing-server](../crates/peerbadge-signing-server/specs/ARCH-peerbadge-signing-server.md).
