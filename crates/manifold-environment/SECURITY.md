# Manifold environment security and reliability boundaries

`fedi-decentralized-manifold-environment` is a pure synchronous configuration
library shared by browser/WASM FI code and native FMan/FLIP daemons plus the
native push-gateway guardian-telemetry receiver. It
performs no network I/O, starts no tasks, persists no state, and contains no
secrets or private-key material.

Production-capable programs must select `ManifoldEnvironment` explicitly.
Development-only tools may explicitly default their own CLI boundary to
development. Development and staging currently share relay endpoints, so
environment identity must never be inferred from a relay URL.
Any relay, issuer-identity, PeerBadge minimum-trust-level,
setup-payment-publisher, Fedi fee-account, Bitcoin-network, or default-Esplora
mapping change must bump the public profile revision and be rolled out across
FI, FMan, FLIP, and push-gateway telemetry as one coordinated deployment; mixed revisions can
otherwise make components disagree while displaying the same environment
name.

The Bitcoin network is environment identity, while the default Esplora URL is
public routing configuration. An operator-owned Bitcoin Core connection may
replace that route but never the network. Staging's `signet` value specifically
means Mutinynet and its default is `https://mutinynet.com/api/`; selecting an
ordinary signet backend would be a chain mismatch. Development and Production
have no public default backend.

Relay URLs are public routing configuration. This crate does not assign event
authenticity, authority-currentness, negative-state completeness, publication,
or availability semantics to them. Reusing the same endpoint set across FI,
FMan, FLIP, and push-gateway telemetry does not make those consumers' relay
policies interchangeable.
FMan publishes to and reads from every canonical profile relay for
advertisements, holder authorizations, and setup-payment policy; backup and
restore use the first canonical relay only. Development may replace that
routing through the profile-owned `MANIFOLD_DEV_NOSTR_RELAYS` override;
Staging and Production refuse it.

The development and staging PeerBadge issuer identities, setup-payment
publisher identities, and Fedi guardian-fee accounts are deliberately unsafe placeholders derived from
publicly known test secrets; the publisher placeholders use different test
secrets from the issuer placeholders so the two roles cannot silently
collapse into one key. Production has no placeholder identity in any role: its
PeerBadge issuer identities are personal keys, each secret held individually by
its owner rather than by the deployment. Its setup-payment publisher and the
account receiving Fedi's share of the federation guardian fee are
deployment-owned keys derived from two independent secrets held outside this
repository. Each publishes a BIP-340 possession signature over a fixed
documented digest beside its constant in `src/lib.rs`, so any reader can check
that a published key corresponds to a held secret without that secret reaching
a networked machine. Custody and provisioning records stay out of this
repository, exactly as they do for issuer roots. Adding a
production issuer is a trust-root change requiring renewed security review; a
generated or known-secret key must never be listed.

Each production issuer root is independently sufficient to authenticate an
issuer authority, including its issuance key and revocation locations. Before
adding a root, the release owner must obtain independent confirmation that the
key is authorized and that its custodian possesses the corresponding private
key. The internal custodian record, custody and recovery details, and incident
contacts must remain outside this public repository. Before rollout, release
owners must also verify that every retained root has a valid current authority
on every canonical production authority relay and complete supported
revocation locations.

Loss, suspected compromise, or custodian offboarding triggers an emergency
profile revision removing that root and a tracked rollout to every FI, FLIP,
and push-gateway telemetry consumer. Older deployed binaries continue to trust the removed root until
updated, so credential revocation alone does not contain a root compromise.
Setup-payment consumers must fail closed until the deployment-owned publisher
identity is configured. Fee-policy producers and validators likewise fail
closed until the production Fedi account is configured; generating a fallback
would redirect Fedi's share.

The `manifold-test-issuer` devmon binary deliberately operationalizes the
public development and staging issuer secrets for local workflows. It signs
credentials with the complete issuer secret committed in this profile —
identity and PBRSA issuance key — and publishes the profile's committed
authority document verbatim, so every run, on any machine, distributes the
one canonical authority that verifiers pin; it verifies the committed
identity against this profile's roots and refuses Production, which commits
neither secret nor document. (Before 2026-08-18 each run minted a per-machine
issuance key, which let two runs silently rotate the shared staging
authority; fedibtc/manifold#401.) Material from this command is test trust,
not an approximation of deployment-owned production credentials. It issues
the selected profile's minimum level so local and staging workflows exercise
the same relying-party gate as deployed consumers.

Adding the Fedi fee-account *field* was covered by the prelaunch exception and
consumed no revision, because no released profile existed to observe the
boundary. That exception is spent. Populating the production publisher and
fee-account mappings consumed revision `8` under the normal rule: it moves
production from failing closed to collecting fees to a specific key, so a build
carrying those keys must be distinguishable from one that does not. Any later
change follows the same revision and coordinated-rollout rule.

The setup-payment publisher key authenticates the kind-37707 federation list
that decides which federations paid setup uses, so whoever holds its secret
controls that policy. Each environment deliberately trusts exactly one
publisher at a time; the rationale is stated canonically in
[SPEC-manifold-environment](specs/SPEC-manifold-environment.md). The profile
is the only source outside Development. Development may replace the publisher
through the profile-owned
`MANIFOLD_DEV_SETUP_PAYMENT_PUBLISHER` environment variable; Staging and
Production refuse it. FMan consumes the resolved publisher directly to
authenticate retained and fetched setup-payment policy. The Fedi app consumes
the profile value only and must never expose a runtime publisher override:
end-user devices carry no trust-root override surface, the same rule that keeps
the issuer channel shrink-only. Routine or emergency rotation ships a new
profile value in a release and causes retained policy under the old key to fail
revalidation.

The crate must remain browser/WASM-safe and must not depend on an async
runtime, network client, credential implementation, storage, or private-key
material. Adding further trust roots or publisher identities, overrides outside
the Development profile, implicit defaults, or relay roles requires renewed
security review. Development overrides remain centralized in this crate so
deployed profiles cannot acquire a consumer-specific bypass.
