# PeerBadge signing server security boundaries

## Issuer custody and trust

The issuer identity and PBRSA keys authorize issuance. Compromise bypasses the
signer allowlist, level ceilings, and rate limits. Protect key files, process
memory, host access, and backups accordingly. Development and Staging defaults
are deliberately public fixture secrets and must never secure real decisions.
Production requires an explicit key file; that does not make the key trusted
by any Manifold consumer. Adding a production identity and pinned authority
requires the existing [environment review and rollout](../manifold-environment/SECURITY.md).

The signer file is trusted startup configuration. Restart to change it. A
valid Iroh identity alone authorizes nothing: opening requires an allowlisted
Nostr identity's Schnorr signature binding a fresh nonce, requested level, and
issuer identity. Anyone with the session ID can redeem; handle offers and QR
codes as bearer capabilities.

## Blindness and audit

Only the holder knows the unblinded subject. The server processes blinded
issuance requests in memory and must never log or persist their JSON or SDK
errors that could include it. The audit may retain only a SHA-256 request
digest alongside signer, level, session, event time, and sanitized reasons.
Do not enable dependency tracing that could reveal request material.

The append-only `audit.jsonl` is confidential: open session IDs authorize
redemption and signer/session metadata links operator actions. Restrict its
file and directory access and protect retained copies. Audit failure refuses
further issuance. Operators own audit retention, disk capacity, and archival;
the file is not automatically rotated.

## Runtime boundary

Use one active process per issuer data directory. Challenges, sessions, and
hourly admission history are bounded and in memory. Restart loses offers and
resets limits; this is not a multi-instance or durable rate-limit design.
Request/frame limits and deadlines bound protocol work but do not replace host
and relay abuse controls. Redemption is irreversible if the response is lost:
a retry cannot obtain a second credential from the same session.

Nostr publication is an administrative network action. Revocation requires a
complete signed credential supplied by the operator; blinded issuance audit
records cannot reconstruct that credential. Offline issuance verification
proves issuance and holder binding only, not fresh revocation status.
