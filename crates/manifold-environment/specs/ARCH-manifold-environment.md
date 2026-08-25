# ARCH-manifold-environment: Shared deployment profile boundary

`manifold-environment` provides deployment identity and canonical public
configuration as a small, synchronous leaf crate. It does not depend on
credential verification, storage, networking, or runtime implementations.

FI, FLIP, push-gateway guardian telemetry, and cloud FMan telemetry receive a
shared concrete PeerBadge verifier. FMan presents its own
`HolderAuthorization` and receives the resolved environment profile at its
Nostr boundary; it does not evaluate its own badge, issuer trust, or revocation
state. This keeps FMan independent of the verifier's credential SDK, relay
client, async runtime, and cryptographic API surface.

The profile owns PeerBadge issuer identities and a schema-valid minimum trust
level. Verifier-backed consumers and FLIP's request-carried-envelope pipeline
construct the shared domain policy from that value. The policy and its ordered
interpretation are defined by
[REQ-fman-trusted-peer-badge](../../../specs/REQ-fman-trusted-peer-badge.md).

The profile supplies routing data, not relay success semantics. Each consumer
defines the security and availability meaning of reads and writes through the
ordered canonical relay set. FMan publishes advertisements and reads
authorizations and policy through every profile relay, while backup and restore
use the first canonical relay only.

The profile also supplies one optional setup-payment federation-list publisher
identity for kind-37707 publications
([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)).
FMan consumes that identity directly. Development resolves its relay and
publisher overrides inside the profile; staging and production provide no
runtime or CLI override. Development and staging use distinct unsafe
known-secret placeholders for issuer, publisher, and fee-account data.
Production contains no placeholder: publisher and fee-account accessors remain
absent until deployment-owned values are supplied, so consumers fail closed.
The single-publisher contract and resolution precedence are defined by
[SPEC-manifold-environment](./SPEC-manifold-environment.md).

The profile owns the Bitcoin network and optional public default Esplora route
so independently released components do not assign different chains to the
same environment name. Private Bitcoin Core URLs and credentials remain
deployment capabilities. FMan may select such a backend but derives the
network from the profile.

## Remaining work

Define the FMan multi-relay transport contract separately; the profile must not
imply a general relay failover policy.
