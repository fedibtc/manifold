# ARCH-peerbadge-signing-server: Delegated blind issuer

The server is the PeerBadge issuer; allowlisted Nostr identities authorize
issuance but never receive its identity or PBRSA private keys. The shared
`service-peerbadge-signing` crate owns the RPC contract, independent of daemon
policy. Iroh authenticates transport peers; fresh Schnorr proofs authenticate
signers. Holders redeem a bearer session directly and finalize the blind
issuance locally through the SDK.

One server process owns issuer keys, a restart-loaded signer allowlist,
bounded in-memory challenges and sessions, per-signer hourly admission history,
and an append-only audit file. Session info is fixed when opening and passed
to `IssuerContext::issue_credential`; holder input cannot replace the approved
level. A serialized state transition prevents concurrent double redemption.
Expired sessions are swept and audited. Terminal state remains until 600 seconds
after the original expiry for polling, then is removed. This is not durable
session recovery. Restart invalidates every outstanding offer and resets
rate-limit history.

The RPC adapter bounds frame size, concurrent handlers, and initial request
read time. Signer, challenge, and session maps each hold at most 4,096 entries;
per-signer sliding-hour history has the same ceiling even if configured higher.
Saturation returns `RateLimited` rather than evicting live capabilities. These
single-process controls do not establish distributed rate limiting or
deployment-level denial-of-service protection.

Audit records contain event time, signer, level, session, and bounded reason
classes; redemption additionally commits to the request's SHA-256. Request
JSON, SDK errors containing input, and unblinded holder data never belong in
logs or persistent state. Audit I/O failure fails closed. The audit and issuer
keys remain confidential operator material.

The CLI owns environment selection, private files, Iroh endpoint/router
lifetime, and administrative Nostr publication. Development and Staging import
the environment's complete public issuer fixture and return its exact pinned
authority. Explicit issuer files support operator custody without modifying
consumer trust. Unpinned authorities use the SDK's authority digest with
deterministic BIP-340 signing, so publication and session offers contain
identical authority JSON across process restarts for unchanged keys and relays.
Pinned authority proofs are never regenerated. Authority publication and
credential revocation use SDK documents and established Nostr event contracts.
Iroh relay routing is separate from environment-owned Nostr routing.

See [SECURITY.md](../SECURITY.md),
[SPEC-peerbadge-signing-protocol](../../service-peerbadge-signing/specs/SPEC-peerbadge-signing-protocol.md),
and [REQ-peerbadge-signing-server](../../../specs/REQ-peerbadge-signing-server.md).
