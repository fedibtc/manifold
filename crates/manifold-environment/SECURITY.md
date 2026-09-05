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
Any relay, issuer-identity, issuer-authority, PeerBadge minimum-trust-level,
setup-payment-publisher, Guardian Verification Fee account, Bitcoin-network,
or default-Esplora
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
publisher identities, and Guardian Verification Fee accounts are deliberately
unsafe placeholders derived from
publicly known test secrets; the publisher placeholders use different test
secrets from the issuer placeholders so the two roles cannot silently
collapse into one key. Production has no placeholder identity in any role: its
PeerBadge issuer identities are personal keys, each secret held individually by
its owner rather than by the deployment. Its setup-payment publisher and
Guardian Verification Fee account are deployment-owned public keys. Adding a
production issuer is a trust-root change requiring renewed security review; a
generated or known-secret key must never be listed.

Each production issuer root is independently sufficient to authenticate an
issuer authority, including its issuance key and revocation locations. Before
adding a root, the release owner must obtain independent confirmation that the
key is authorized and that its custodian possesses the corresponding private
key. The internal custodian record, custody and recovery details, and incident
contacts must remain outside this public repository. Before rollout, release
owners must also confirm that every retained custodian recognizes the exact
authority committed for its root and can issue with its stated issuance key.
Each authority proof must verify and every listed revocation location must be
complete and available.

The public authority document is release-pinned. Reinstalling an issuer app
with only the identity secret can generate a different issuance key while
keeping the same identity. Replacing the pinned authority with that document
makes every badge signed by the previous issuance key unverifiable. Recover
the complete old issuer secret when possible; otherwise coordinate reissuance
before rolling out the replacement.

Loss, suspected compromise, or custodian offboarding triggers an emergency
profile revision removing that root and a tracked rollout to every FI, FLIP,
and push-gateway telemetry consumer. Older deployed binaries continue to trust the removed root until
updated, so credential revocation alone does not contain a root compromise.
Consumers must fail closed if either deployment-owned key is absent; they must
not generate a fallback.

The `manifold-test-issuer` devmon binary deliberately operationalizes the
public development and staging issuer secrets for local workflows. It signs
credentials with the complete issuer secret committed in this profile —
identity and PBRSA issuance key — and publishes the profile's committed
authority document verbatim, so every run, on any machine, distributes the
one canonical authority that verifiers pin; it verifies the committed
identity against this profile's roots and refuses Production, which commits no
issuer secret and only signed public authority documents. (Before 2026-08-18 each run minted a per-machine
issuance key, which let two runs silently rotate the shared staging
authority; fedibtc/manifold#401.) Material from this command is test trust,
not an approximation of deployment-owned production credentials. It issues
the selected profile's minimum level so local and staging workflows exercise
the same relying-party gate as deployed consumers.

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
material. Adding or replacing production trust roots or authorities, adding
publisher identities, overrides outside the Development profile, implicit
defaults, or relay roles requires renewed security review. Development
overrides remain centralized in this crate so deployed profiles cannot acquire
a consumer-specific bypass.
