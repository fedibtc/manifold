//! Canonical public configuration for one Manifold deployment environment.
//!
//! This synchronous leaf crate assigns no consumer-specific meaning to its
//! relay routing. Production carries individually held PeerBadge issuer keys,
//! the deployment-owned setup-payment publisher identity, and the account
//! receiving Fedi's share of the federation guardian fee. See
//! `specs/SPEC-manifold-environment.md` and [`SECURITY.md`](../SECURITY.md).

#[cfg(test)]
mod tests;

use core::fmt;
use std::str::FromStr;

use bitcoin::Network;
use nostr::{PublicKey, RelayUrl};
use stability_pool_common::{Account, AccountType};
use url::Url;

const STAGING_NOSTR_RELAYS: &[&str] = &["wss://relay-staging.dev.fedibtc.com"];
const STAGING_ESPLORA_URL: &str = "https://mutinynet.com/api/";
const PRODUCTION_NOSTR_RELAYS: &[&str] = &[
    "wss://relay.dev.fedibtc.com",
    "wss://relay.primal.net",
    "wss://relay.damus.io/",
];
const TRUSTED_PEER_BADGE_TRUST_LEVEL: u64 = 9;

/// Revision of the built-in environment-to-profile mappings.
///
/// Bump this after profiles are released whenever a public deployment mapping
/// changes so independently released components can expose and compare what
/// they use. Adding the Fedi fee-account *field* consumed no revision, because
/// no released profile existed to observe the boundary. Populating the
/// production publisher and fee-account mappings did consume one, 7 -> 8: it
/// moves production from failing closed to collecting fees to a specific key,
/// so a build carrying it must be distinguishable from one that does not.
pub const MANIFOLD_ENVIRONMENT_PROFILE_REVISION: u32 = 8;

////////////////////////////////////////////////////////////////////////////////
// !!! SECURITY BLOCKER: THESE ARE DELIBERATELY UNSAFE TEST-ONLY ROOT KEYS !!!
//
// The real development and staging issuer and setup-payment publisher
// identities do not exist yet. These public keys are derived from the
// publicly known test secret keys 1 and 2 (issuers), 3 and 4 (publishers), and
// 5 and 6 (Fedi guardian-fee recipients).
// Anyone can impersonate them. They MUST be replaced before either
// environment treats PeerBadge results or the setup-payment federation list
// as a security decision.
//
// The committed issuer secret-key fixtures below extend this posture to the
// PBRSA issuance keys: development and staging issuance secrets are public by
// construction (this repository mirrors to a public one), so every issuer of
// test trust converges on one authority instead of racing replaceable relay
// events (the 2026-08-18 staging authority rotation; see
// crates/peer-badge-verifier/specs/SPEC-peer-badge-verifier.md).
//
// Production deliberately has NO placeholder identity of any role. Do not add
// a generated or known-secret production key: only keys whose secrets are held
// individually by their owners, or are deployment-owned with custody recorded
// out of band, may appear in the production constants below. Production likewise
// has NO committed issuance secret: `test_issuer_secret_keys` returns `None`
// so repository-built tooling cannot publish a production issuer authority.
// Do not remove or weaken this warning while test placeholders remain.
////////////////////////////////////////////////////////////////////////////////
const DEVELOPMENT_PLACEHOLDER_ISSUER: &str =
    "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const STAGING_PLACEHOLDER_ISSUER: &str =
    "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";
// Complete `IssuerSecretKeys` documents (identity secret + PBRSA issuance
// secret) for the placeholder issuers above, and the signed `IssuerAuthority`
// documents derived from them (revocation locations = the environment's
// canonical relays). Deliberately public test trust; see the SECURITY
// BLOCKER. Verifiers pin the authority documents; the test issuer signs with
// the secrets and publishes the committed documents verbatim, so both sides
// share one canonical authority by construction (a unit test enforces
// secret/document agreement). Whoever replaces a fixture MUST copy the
// deployment's canonical secret, not mint a fresh one: credentials issued
// before the swap stay verifiable only if the issuance key is unchanged, and
// no in-repo test can check the repository against the deployment.
const DEVELOPMENT_ISSUER_SECRET_KEYS_JSON: &str =
    include_str!("../fixtures/development-issuer-secret-keys.json");
const STAGING_ISSUER_SECRET_KEYS_JSON: &str =
    include_str!("../fixtures/staging-issuer-secret-keys.json");
const DEVELOPMENT_ISSUER_AUTHORITY_JSON: &str =
    include_str!("../fixtures/development-issuer-authority.json");
const STAGING_ISSUER_AUTHORITY_JSON: &str =
    include_str!("../fixtures/staging-issuer-authority.json");
const DEVELOPMENT_PLACEHOLDER_SETUP_PAYMENT_PUBLISHER: &str =
    "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";
const STAGING_PLACEHOLDER_SETUP_PAYMENT_PUBLISHER: &str =
    "e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd13";
const DEVELOPMENT_PLACEHOLDER_FEDI_GUARDIAN_FEE_PUBLIC_KEY: &str =
    "022f8bde4d1a07209355b4a7250a5c5128e88b84bddc619ab7cba8d569b240efe4";
const STAGING_PLACEHOLDER_FEDI_GUARDIAN_FEE_PUBLIC_KEY: &str =
    "03fff97bd5755eeea420453a14355235d382f6472f8568a18b2f057a1460297556";

/// Deployment-owned setup-payment publisher identity, x-only hex, NO prefix.
///
/// Not a placeholder. Derived under NIP-06 `m/44'/1237'/0'/0/0` from a secret
/// independent of the fee key below and held outside the deployment.
/// Authorization and custody are recorded out of band, per `SECURITY.md`.
///
/// Possession, BIP-340 over
/// `sha256("manifold/v1/possession/setup-payment-publisher")`
/// = `228cd5fb8768324479e3866920f40e178f37a1687e50e8d628623e569685f163`:
/// `84a89997513b0fd3ea60e7433d0178498d10966ef646ec38e0fe43acd3aad3b3`
/// `5808dc98637df99c9d5c89bb557b0e10044537a982ea976ebe1166a9716347da`
const PRODUCTION_SETUP_PAYMENT_PUBLISHER: &str =
    "725cc60e9b9405acf48f27f8ec6e846dd499b7b5d1e0fff6c922da7dfa120f65";

/// Public key of the account receiving Fedi's share of the ongoing
/// per-transaction **federation guardian fee**, compressed hex, WITH the
/// `02`/`03` prefix. 66 characters, not 64.
///
/// Fedi is one weighted recipient of that fee, not a guardian earning one. The
/// MVP split gives the FI four shares, every guardian one, and this account one
/// ([REQ-guardian-fee-remittance](../../../specs/REQ-guardian-fee-remittance.md)).
/// Whether the share applies to every federation or only to Fedi-verified seat
/// quorums is a deferred product decision, so this describes the fee stream the
/// account is paid from rather than asserting what the share compensates.
///
/// **The parity byte is load-bearing.** `Account` commits to all 33 bytes and
/// the `AccountId` is the hash of that, so `02||X` and `03||X` are two
/// different accounts spendable by the same secret. Signature verification
/// drops parity and will not catch a normalised value, which would silently
/// strand every fee paid to it. These are exactly the bytes the key was
/// published as; never normalise to even-Y.
///
/// Derived under NIP-06 `m/44'/1237'/0'/0/0` from a secret independent of the
/// publisher key above. Derives to `AccountId`
/// `spd1luendqfafewqd2q8fkg9v6tvceurggtj06rxshw3rukt0c3x0ecsehzz39`, pinned by
/// test against an independently recorded value.
///
/// Possession, BIP-340 over
/// `sha256("manifold/v1/possession/fedi-guardian-fee-account")`
/// = `6871df32d2b4a0def4c2ffdf1ad9fb9f822e697f3ee95800d9c09cda6be94dea`:
/// `509f2ade29b72192d6dead5d4ad508a7ecc2c88e2e2d37ce3b5a6785abbbd1e8`
/// `d7937c368c223f3d3acff552a0bf7fd1b2349a9476db3dcde0a8b2d99d16fa84`
const PRODUCTION_FEDI_GUARDIAN_FEE_PUBLIC_KEY: &str =
    "0255c6b0de21aa9d5da41cdbb53be23e73dc5b3697f10b13f806c5ee7d18bd604d";

/// Production PeerBadge issuer identities, in trust-roster order.
///
/// Each is a personal issuer key whose secret is held individually by its
/// owner rather than by the deployment, and is not a placeholder. The release
/// process must verify authorization and private-key possession before adding
/// an entry; see `SECURITY.md`. Each entry is the x-only hex encoding of the
/// `npub` in its comment.
const PRODUCTION_ISSUERS: &[&str] = &[
    // npub14jty0h6e4kjugvkw0lu6dta6c3g862wd7mkea8wjvqgwvs0lnt4s528jdn
    "ac9647df59ada5c432ce7ff9a6afbac4507d29cdf6ed9e9dd26010e641ff9aeb",
    // npub1mvml033envht57j7cr8cykx92eqxh7ypmv673lg0u9yew405s59s4qxa89
    "db37f7c6399b2eba7a5ec0cf8258c556406bf881db35e8fd0fe1499755f4850b",
    // npub13n05t087zy939r6ulu64kfxtsh65neuaapu65d5324c0m7d4pvpqx4psp0
    "8cdf45bcfe110b128f5cff355b24cb85f549e79de879aa36915570fdf9b50b02",
    // npub17mrmcy3wwjkw7cgp6t358flx3gx78kwunu6hc7f8vapmsmlak2ps06v0cw
    "f6c7bc122e74acef6101d2e343a7e68a0de3d9dc9f357c79276743b86ffdb283",
    // npub1jm4nvzweed7q0k5ztep607nnul5qyryen3w7qzenxdrkckxcgrjq2d9xz9
    "96eb3609d9cb7c07da825e43a7fa73e7e8020c999c5de00b3333476c58d840e4",
];

/// Deployment environment selecting one canonical Manifold profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifoldEnvironment {
    /// Developer and local-test deployments.
    Development,
    /// Internal staging deployments.
    Staging,
    /// Public production deployments.
    Production,
}

/// Development-only relay override: whitespace- or comma-separated relay URLs
/// replacing the built-in development relay routing.
pub const DEV_NOSTR_RELAYS_ENV: &str = "MANIFOLD_DEV_NOSTR_RELAYS";

/// Development-only setup-payment publisher override: one Nostr public key
/// replacing the built-in development placeholder publisher.
pub const DEV_SETUP_PAYMENT_PUBLISHER_ENV: &str = "MANIFOLD_DEV_SETUP_PAYMENT_PUBLISHER";

impl ManifoldEnvironment {
    /// Resolve this environment's public configuration, applying the
    /// development-only override environment variables.
    ///
    /// Override resolution lives here — not in consumer binaries — so every
    /// consumer behaves identically and no binary can grow a
    /// production-reachable override flag.
    ///
    /// # Errors
    ///
    /// Returns an error when an override variable is set for a staging or
    /// production profile (refused loudly rather than silently ignored), or
    /// when an override value does not parse.
    pub fn profile(self) -> Result<ManifoldEnvironmentProfile, ManifoldEnvironmentProfileError> {
        self.profile_with_env(|variable| std::env::var(variable).ok())
    }

    fn profile_with_env(
        self,
        env: impl Fn(&'static str) -> Option<String>,
    ) -> Result<ManifoldEnvironmentProfile, ManifoldEnvironmentProfileError> {
        let relays_override = env(DEV_NOSTR_RELAYS_ENV);
        let publisher_override = env(DEV_SETUP_PAYMENT_PUBLISHER_ENV);
        if self != Self::Development {
            for (variable, value) in [
                (DEV_NOSTR_RELAYS_ENV, &relays_override),
                (DEV_SETUP_PAYMENT_PUBLISHER_ENV, &publisher_override),
            ] {
                if value.is_some() {
                    return Err(
                        ManifoldEnvironmentProfileError::OverrideOutsideDevelopment {
                            variable,
                            environment: self,
                        },
                    );
                }
            }
        }
        let (issuers, publisher, fedi_guardian_fee_key, relays, bitcoin_network, esplora_url) =
            match self {
                Self::Development => (
                    &[DEVELOPMENT_PLACEHOLDER_ISSUER][..],
                    Some(DEVELOPMENT_PLACEHOLDER_SETUP_PAYMENT_PUBLISHER),
                    Some(DEVELOPMENT_PLACEHOLDER_FEDI_GUARDIAN_FEE_PUBLIC_KEY),
                    STAGING_NOSTR_RELAYS,
                    Network::Regtest,
                    None,
                ),
                Self::Staging => (
                    &[STAGING_PLACEHOLDER_ISSUER][..],
                    Some(STAGING_PLACEHOLDER_SETUP_PAYMENT_PUBLISHER),
                    Some(STAGING_PLACEHOLDER_FEDI_GUARDIAN_FEE_PUBLIC_KEY),
                    STAGING_NOSTR_RELAYS,
                    Network::Signet,
                    Some(STAGING_ESPLORA_URL),
                ),
                Self::Production => (
                    PRODUCTION_ISSUERS,
                    Some(PRODUCTION_SETUP_PAYMENT_PUBLISHER),
                    Some(PRODUCTION_FEDI_GUARDIAN_FEE_PUBLIC_KEY),
                    PRODUCTION_NOSTR_RELAYS,
                    Network::Bitcoin,
                    None,
                ),
            };
        let peer_badge_issuer_identities = issuers
            .iter()
            .map(|issuer| PublicKey::parse(issuer).expect("built-in issuer identity is valid"))
            .collect();
        let setup_payment_publisher = match publisher_override {
            Some(value) => Some(PublicKey::parse(value.trim()).map_err(|err| {
                ManifoldEnvironmentProfileError::InvalidOverride {
                    variable: DEV_SETUP_PAYMENT_PUBLISHER_ENV,
                    detail: err.to_string(),
                }
            })?),
            None => publisher.map(|publisher| {
                PublicKey::parse(publisher).expect("built-in publisher identity is valid")
            }),
        };
        let nostr_relays = match relays_override {
            Some(value) => CanonicalNostrRelays::parse_override(&value)?,
            None => CanonicalNostrRelays::from_static(relays),
        };
        let fedi_guardian_fee_account = fedi_guardian_fee_key.map(|key| {
            Account::single(
                key.parse().expect("built-in Fedi fee public key is valid"),
                AccountType::BtcDepositor,
            )
        });
        Ok(ManifoldEnvironmentProfile {
            environment: self,
            nostr_relays,
            peer_badge_issuer_identities,
            minimum_peer_badge_trust_level: TRUSTED_PEER_BADGE_TRUST_LEVEL,
            setup_payment_publisher,
            fedi_guardian_fee_account,
            bitcoin_network,
            default_esplora_url: esplora_url
                .map(|url| Url::parse(url).expect("built-in default Esplora URL is valid")),
        })
    }
}

/// Failure resolving an environment profile from overrides.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifoldEnvironmentProfileError {
    /// A development override variable was set while resolving a staging or
    /// production profile.
    OverrideOutsideDevelopment {
        variable: &'static str,
        environment: ManifoldEnvironment,
    },
    /// An override variable was set but its value does not parse.
    InvalidOverride {
        variable: &'static str,
        detail: String,
    },
}

impl fmt::Display for ManifoldEnvironmentProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OverrideOutsideDevelopment {
                variable,
                environment,
            } => write!(
                formatter,
                "{variable} is set but the {environment} environment refuses development \
                 overrides; unset it or select the development environment"
            ),
            Self::InvalidOverride { variable, detail } => {
                write!(formatter, "invalid {variable}: {detail}")
            }
        }
    }
}

impl std::error::Error for ManifoldEnvironmentProfileError {}

impl fmt::Display for ManifoldEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Development => "development",
            Self::Staging => "staging",
            Self::Production => "production",
        })
    }
}

impl FromStr for ManifoldEnvironment {
    type Err = ParseManifoldEnvironmentError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "development" | "dev" => Ok(Self::Development),
            "staging" => Ok(Self::Staging),
            "production" | "prod" => Ok(Self::Production),
            _ => Err(ParseManifoldEnvironmentError(value.to_owned())),
        }
    }
}

/// Error returned for an unknown deployment environment name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseManifoldEnvironmentError(String);

impl fmt::Display for ParseManifoldEnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown Manifold environment `{}`; expected development, staging, or production",
            self.0
        )
    }
}

impl std::error::Error for ParseManifoldEnvironmentError {}

/// Immutable public configuration resolved for one deployment environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifoldEnvironmentProfile {
    environment: ManifoldEnvironment,
    nostr_relays: CanonicalNostrRelays,
    peer_badge_issuer_identities: Vec<PublicKey>,
    minimum_peer_badge_trust_level: u64,
    setup_payment_publisher: Option<PublicKey>,
    fedi_guardian_fee_account: Option<Account>,
    bitcoin_network: Network,
    default_esplora_url: Option<Url>,
}

impl ManifoldEnvironmentProfile {
    /// Return the revision of the built-in profile mappings.
    #[must_use]
    pub fn profile_revision(&self) -> u32 {
        MANIFOLD_ENVIRONMENT_PROFILE_REVISION
    }

    /// Return the environment identity that produced this profile.
    #[must_use]
    pub fn environment(&self) -> ManifoldEnvironment {
        self.environment
    }

    /// Return the canonical environment relay routing.
    #[must_use]
    pub fn nostr_relays(&self) -> &CanonicalNostrRelays {
        &self.nostr_relays
    }

    /// Return environment-configured PeerBadge issuer identities.
    ///
    /// Every environment returns a non-empty set: development and staging
    /// return their known-secret placeholders, production returns its
    /// individually held issuer keys.
    #[must_use]
    pub fn peer_badge_issuer_identities(&self) -> &[PublicKey] {
        &self.peer_badge_issuer_identities
    }

    /// Return the minimum authenticated PeerBadge trust level accepted by
    /// relying components in this environment.
    #[must_use]
    pub fn minimum_peer_badge_trust_level(&self) -> u64 {
        self.minimum_peer_badge_trust_level
    }

    /// Return the committed `IssuerSecretKeys` JSON for this environment's
    /// known-secret placeholder issuer, or `None` for production.
    ///
    /// Development and staging placeholder trust is public by construction
    /// (see the SECURITY BLOCKER above), and committing the complete secret —
    /// identity *and* PBRSA issuance key — makes the issuer authority a
    /// function of the repository instead of a race between replaceable relay
    /// events: every repository-built issuer converges on one issuance key,
    /// and verifiers pin the authority derived from it rather than trusting
    /// whichever kind-37703 event was published last. Production returns
    /// `None`; its issuer secrets are held individually by their owners, and
    /// repository tooling must fail closed rather than publish a production
    /// authority.
    #[must_use]
    pub fn test_issuer_secret_keys(&self) -> Option<&'static str> {
        match self.environment {
            ManifoldEnvironment::Development => Some(DEVELOPMENT_ISSUER_SECRET_KEYS_JSON),
            ManifoldEnvironment::Staging => Some(STAGING_ISSUER_SECRET_KEYS_JSON),
            ManifoldEnvironment::Production => None,
        }
    }

    /// Return the committed, identity-signed `IssuerAuthority` JSON documents
    /// pinned for this environment's issuers.
    ///
    /// PeerBadge verifiers trust these documents instead of the newest
    /// kind-37703 relay event, so a replaceable-event overwrite can neither
    /// rotate a pinned issuer's trust nor deny verification. The documents
    /// are plain signed public material: pinning them adds no runtime
    /// signing, secret handling, or randomness to verifiers. Production pins
    /// none yet; its authorities stay relay-fetched until issuer-supplied
    /// documents enter through the release process (`SECURITY.md`).
    #[must_use]
    pub fn pinned_issuer_authorities(&self) -> &'static [&'static str] {
        match self.environment {
            ManifoldEnvironment::Development => &[DEVELOPMENT_ISSUER_AUTHORITY_JSON],
            ManifoldEnvironment::Staging => &[STAGING_ISSUER_AUTHORITY_JSON],
            ManifoldEnvironment::Production => &[],
        }
    }

    /// Return the environment-default setup-payment federation-list
    /// publisher identity authenticating kind-37707 publications.
    ///
    /// This is the deployment default; the single-publisher rule and the
    /// flag-over-default resolution precedence are stated canonically in
    /// `specs/SPEC-manifold-environment.md`. Production intentionally
    /// returns `None` until its real publisher identity exists, so
    /// production consumers must fail closed rather than substitute a
    /// default.
    #[must_use]
    pub fn setup_payment_publisher(&self) -> Option<&PublicKey> {
        self.setup_payment_publisher.as_ref()
    }

    /// Return the deployment-owned recipient of Fedi's one guardian-fee share.
    ///
    /// This is a full single-signature `BtcDepositor` account because Fedi
    /// validates the complete descriptor before remitting. Production returns
    /// `None` until the real deployment account is supplied; every fee-policy
    /// path must fail closed rather than accept caller-provided replacement
    /// material.
    #[must_use]
    pub fn fedi_guardian_fee_account(&self) -> Option<&Account> {
        self.fedi_guardian_fee_account.as_ref()
    }

    /// Return the Bitcoin network on which this environment forms federations.
    #[must_use]
    pub fn bitcoin_network(&self) -> Network {
        self.bitcoin_network
    }

    /// Return the environment's public default chain backend, when one exists.
    ///
    /// Deployments may select an operator-owned Bitcoin Core backend instead;
    /// that changes routing, never the environment-owned network.
    #[must_use]
    pub fn default_esplora_url(&self) -> Option<&Url> {
        self.default_esplora_url.as_ref()
    }
}

/// Non-empty, ordered, deduplicated canonical Nostr relay routing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalNostrRelays(Vec<RelayUrl>);

impl CanonicalNostrRelays {
    fn from_static(relays: &[&str]) -> Self {
        let mut parsed = Vec::new();
        for relay in relays {
            let relay = RelayUrl::parse(relay).expect("built-in relay URL is valid");
            if !parsed.contains(&relay) {
                parsed.push(relay);
            }
        }
        assert!(!parsed.is_empty(), "built-in relay set must not be empty");
        Self(parsed)
    }

    fn parse_override(value: &str) -> Result<Self, ManifoldEnvironmentProfileError> {
        let mut parsed = Vec::new();
        for relay in value.split([',', ' ', '\t', '\n']) {
            let relay = relay.trim();
            if relay.is_empty() {
                continue;
            }
            let relay = RelayUrl::parse(relay).map_err(|err| {
                ManifoldEnvironmentProfileError::InvalidOverride {
                    variable: DEV_NOSTR_RELAYS_ENV,
                    detail: format!("{relay:?}: {err}"),
                }
            })?;
            if !parsed.contains(&relay) {
                parsed.push(relay);
            }
        }
        if parsed.is_empty() {
            return Err(ManifoldEnvironmentProfileError::InvalidOverride {
                variable: DEV_NOSTR_RELAYS_ENV,
                detail: "no relay URLs".to_owned(),
            });
        }
        Ok(Self(parsed))
    }

    /// Return canonical relay URLs in preference order.
    #[must_use]
    pub fn as_urls(&self) -> &[RelayUrl] {
        &self.0
    }
}
