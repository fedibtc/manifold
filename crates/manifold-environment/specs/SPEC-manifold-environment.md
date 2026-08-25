# SPEC-manifold-environment: canonical deployment profiles

`fedi-decentralized-manifold-environment` owns the environment names and the
public, immutable configuration that independently released Manifold
components resolve from them. It is a synchronous, network-free,
storage-free, browser/WASM-safe leaf crate.

## Record justification

The profile definition and its coordinated use by FI, FMan, FLIP, and telemetry
consumers form one distributed deployment contract that no consumer owns alone.

`ManifoldEnvironment` accepts `development`/`dev`, `staging`, and
`production`/`prod`. It has no `Default`; production-capable programs require
an explicit value, while development-only `fi-cli` explicitly supplies its own
development default.

Each value resolves to a `ManifoldEnvironmentProfile` containing:

- its environment identity;
- a non-empty ordered, deduplicated set of typed canonical Nostr relay URLs;
- one or more typed PeerBadge issuer identity public keys;
- a PeerBadge minimum trust level;
- at most one typed setup-payment federation-list publisher public key;
- at most one complete single-sig `BtcDepositor` account for Fedi's canonical
  share of ongoing guardian-fee remittances;
- the Bitcoin network on which the environment forms federations;
- an optional typed public default Esplora URL; and
- for development and staging only, the committed complete `IssuerSecretKeys`
  JSON of the environment's placeholder issuer (`test_issuer_secret_keys()`;
  `None` for production) together with the identity-signed `IssuerAuthority`
  document derived from it (`pinned_issuer_authorities()`; empty for
  production).

The profile exposes `profile_revision()`, currently `8`. Revision `8`
populates the production setup-payment publisher and Fedi fee-account mappings,
which moves production from failing closed to collecting fees to a specific key;
a build carrying those keys must be distinguishable from one that does not.
Every change to an
environment's relay, issuer-identity, committed issuer secret, PeerBadge
minimum trust level, setup-payment-publisher, Fedi fee-account,
Bitcoin-network, or default-Esplora mapping must increment the shared
revision.
Operators must roll out such a change across
independently released FI, FMan, FLIP, push-gateway telemetry, and cloud FMan
telemetry collector components as one coordinated deployment and use the revision in diagnostics to identify a
mixed-profile rollout. FMan and FLIP log the selected environment and revision
at normal startup; the opaque verifier passed into FI and FLIP and the concrete
verifier constructed by push-gateway and the cloud FMan telemetry collector
also exposes both values to embedding diagnostics.

Development and staging use `wss://relay-staging.dev.fedibtc.com`.
Production uses, in order, `wss://relay.dev.fedibtc.com`,
`wss://relay.primal.net`, and `wss://relay.damus.io/`.
Development and staging have distinct unsafe test issuer identities despite
sharing relay routing. Each commits its issuer's complete secret — identity
and PBRSA issuance key — and the authority document signed with it as
repository fixtures, so every repository-built issuer signs under one
canonical authority and PeerBadge verifiers pin the committed document
instead of trusting the newest replaceable kind-37703 event
([SPEC-peer-badge-verifier](../../peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)).
The staging secret fixture must be the
deployment's previously canonical key file — copied, never regenerated — or
credentials issued before the fixture landed stop verifying; no in-repo test
can enforce that against the deployment, so fixture replacement is a
review-time obligation. Production commits no issuer secret and no authority
document.

Development forms regtest federations and has no public default chain backend.
Staging forms Mutinynet federations: Fedimint receives the generic `signet`
network value and uses `https://mutinynet.com/api/` as the public default
Esplora backend. Production forms Bitcoin-mainnet federations and has no public
default backend; deployments must supply their operator-owned Bitcoin Core
connection. A consumer may replace a default Esplora route with Bitcoin Core,
but it must not replace the environment-owned network. Moving an environment
to a different chain is a data-incompatible environment change, not an
operator override.

Production carries personal PeerBadge issuer keys, each secret held
individually by its owner rather than by the deployment. They are listed in
trust-roster order and stored as x-only hex alongside the `npub` encoding of
each key. Production must never receive a generated or known-secret
placeholder. `PeerBadgeVerifier` construction still fails closed when an
environment's issuer list is empty, so removing the last production
issuer disables production verification rather than widening trust. Changing
the production roster is a profile-revision change shipped in a coordinated
component release, never a runtime or operator-supplied value.

Every environment requires an authenticated `fedi-trust-score-v1.0`
PeerBadge trust level of at least `9`, admitting the schema range `9..=12` as
Trusted-or-higher. The external credential app's exact-9 tier label is display
metadata; Manifold treats the numeric schema value as an ordered relying-party
minimum, as recorded in
[REQ-fman-trusted-peer-badge](../../../specs/REQ-fman-trusted-peer-badge.md).
This is relying-party policy rather than credential-schema validation: lower
schema-valid levels are authentic credentials but are not admitted by FI,
FLIP, push-gateway guardian telemetry, or the cloud FMan telemetry collector.
Keeping the minimum in the
environment profile makes policy changes explicit coordinated profile
revisions rather than consumer-local thresholds.

`setup_payment_publisher()` returns the identity that authenticates
kind-37707 setup-payment federation publications
([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)).
This section is the canonical statement of the single-publisher rule and the
publisher resolution precedence; other documents summarize and link here.
Each deployment deliberately trusts exactly one publisher at a time, never a
list: the publication is one authoritative federation list, and multiple
concurrently trusted publisher keys would create list-merge ambiguity
between differing signed lists.

The profile value is the only resolution source outside development.
`profile()` resolves development-only overrides inside this crate: in the
Development profile, `MANIFOLD_DEV_NOSTR_RELAYS` (whitespace- or
comma-separated relay URLs) and `MANIFOLD_DEV_SETUP_PAYMENT_PUBLISHER` (a
Nostr public key) replace the built-in defaults, letting local harnesses point
components at leased test relays and test publisher keys without any
per-binary CLI surface. In Staging and Production, `profile()` fails when
either variable is set rather than silently ignoring or honoring it — no
deployed binary can have its relay routing or trust root redirected through
the environment. The Fedi app likewise consumes the profile default only —
end-user devices carry no trust-root override surface, the same rule that
keeps the issuer channel shrink-only. Rotation, routine or emergency, ships a
new profile default in a component release.

Development and staging carry distinct unsafe test publisher identities,
also distinct from the issuer identities. Production carries no publisher —
the accessor returns typed `None` — until the real deployment-owned key
exists; production consumers must fail closed rather than substitute a
default.

`fedi_guardian_fee_account()` is the canonical recipient for Fedi's one share
in the fixed Manifold 4:1:1 policy. Development and staging carry distinct
unsafe known-test accounts. Production returns typed `None` until the
deployment-owned account exists. FI policy construction and FMan validation
both fail closed on absence; consumers must not generate a fallback.

The canonical relay list does not itself define consumer policy. PeerBadge
verification interprets it as its issuer-authority lookup set; FMan uses it
for advertisement, HolderAuthorization discovery, and setup-payment
publication refresh (currently through the first listed relay).

Tests pin parsing/display aliases, exact relay ordering, development/staging
sharing, the minimum PeerBadge trust level, distinct test issuer identities,
distinct test publisher identities
disjoint from the union of the development and staging issuer identities,
distinct full development/staging Fedi fee accounts, the exact production
issuer roster and its disjointness from every known-secret placeholder, the
exact environment-to-Bitcoin-network mapping, the
Mutinynet staging Esplora default, and the intentionally absent production
publisher, Fedi fee account, and chain-backend default.
