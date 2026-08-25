#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use fedi_credential_sdk_protocol::{
    CredentialDigest, CredentialsError, HolderId, IssuerAuthority, IssuerId, SignedRevocation,
    SubjectPubkey, VerificationContext,
};
use fedi_decentralized_domain::{
    HolderAuthorizationEnvelope, PeerBadgeTrustPolicy, PeerBadgeTrustPolicyConfigError,
    PeerBadgeTrustPolicyError, TrustScoreBadgeV1, TrustScoreSchemaError,
    parse_trust_score_badge_v1,
};
use fedi_decentralized_manifold_environment::{ManifoldEnvironment, ManifoldEnvironmentProfile};
use fedi_decentralized_nostr::attester::{
    CREDENTIAL_REVOCATION_EVENT_KIND, ISSUER_AUTHORITY_D_TAG, ISSUER_AUTHORITY_EVENT_KIND,
    credential_revocation_d_tag,
};
use fedi_decentralized_nostr_clients::{NostrClientError, NostrPeerBadgeClient};
use fedimint_core::runtime::Instant;
use nostr_sdk::{Event, EventId, Kind, PublicKey, RelayUrl, TagKind};

const PEER_BADGE_VERIFICATION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_AUTHORITY_RELAYS: usize = 4;
const MAX_REVOCATION_LOCATIONS: usize = 4;

/// Non-empty set of hardcoded issuer identity public keys.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PeerBadgeIssuerRoots(BTreeSet<PublicKey>);

impl PeerBadgeIssuerRoots {
    /// Validate a non-empty issuer identity set.
    ///
    /// # Errors
    ///
    /// Returns [`PeerBadgeVerifierConfigError::EmptyIssuerRoots`] when no
    /// issuer is supplied.
    fn new(
        issuers: impl IntoIterator<Item = PublicKey>,
    ) -> Result<Self, PeerBadgeVerifierConfigError> {
        let issuers: BTreeSet<_> = issuers.into_iter().collect();
        if issuers.is_empty() {
            return Err(PeerBadgeVerifierConfigError::EmptyIssuerRoots);
        }
        Ok(Self(issuers))
    }

    fn contains(&self, issuer: &PublicKey) -> bool {
        self.0.contains(issuer)
    }
}

/// Non-empty canonical relay set used to fetch current issuer authorities.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PeerBadgeAuthorityRelays(Vec<RelayUrl>);

impl PeerBadgeAuthorityRelays {
    /// Validate, normalize, and deduplicate authority relay URLs.
    ///
    /// # Errors
    ///
    /// Returns a typed configuration error when no relay is supplied or the
    /// deduplicated relay count exceeds the verifier's resource bound.
    fn new(
        relays: impl IntoIterator<Item = RelayUrl>,
    ) -> Result<Self, PeerBadgeVerifierConfigError> {
        let mut seen = BTreeSet::new();
        let mut normalized = Vec::new();
        for relay in relays {
            if seen.insert(relay.to_string()) {
                normalized.push(relay);
            }
        }
        if normalized.is_empty() {
            return Err(PeerBadgeVerifierConfigError::EmptyAuthorityRelays);
        }
        if normalized.len() > MAX_AUTHORITY_RELAYS {
            return Err(PeerBadgeVerifierConfigError::TooManyAuthorityRelays {
                maximum: MAX_AUTHORITY_RELAYS,
                actual: normalized.len(),
            });
        }
        Ok(Self(normalized))
    }

    /// Return the normalized relay URLs.
    #[must_use]
    #[cfg(test)]
    fn as_urls(&self) -> &[RelayUrl] {
        &self.0
    }
}

/// Invalid static configuration for a PeerBadge verifier.
#[derive(Debug, thiserror::Error)]
pub enum PeerBadgeVerifierConfigError {
    /// At least one issuer identity root is required.
    #[error("PeerBadge issuer identity roots must not be empty")]
    EmptyIssuerRoots,

    /// At least one authority relay is required.
    #[error("PeerBadge authority relays must not be empty")]
    EmptyAuthorityRelays,

    /// The configured minimum is outside the PeerBadge schema's legal range.
    #[error(transparent)]
    InvalidMinimumTrustLevel(#[from] PeerBadgeTrustPolicyConfigError),

    /// The configured relay list exceeds the verification resource bound.
    #[error("too many PeerBadge authority relays: maximum {maximum}, got {actual}")]
    TooManyAuthorityRelays {
        /// Maximum supported relay count.
        maximum: usize,
        /// Supplied relay count after deduplication.
        actual: usize,
    },

    /// The selected environment has no configured PeerBadge issuer identity.
    #[error("{environment} PeerBadge issuer identities are not configured")]
    EnvironmentIssuerRootsUnavailable {
        /// Environment whose verifier cannot yet be constructed.
        environment: ManifoldEnvironment,
    },

    /// A committed authority document failed to parse or verify, exceeded
    /// verifier bounds, or names an issuer outside the identity roots.
    #[error("committed issuer authority is invalid: {detail}")]
    InvalidCommittedAuthority {
        /// Human-readable failure description.
        detail: String,
    },
}

/// Provenance of a verifier's immutable trust configuration.
///
/// Relying composition roots can distinguish canonical deployment profiles
/// from the explicit configuration available only to component-test builds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerBadgeVerifierProvenance {
    /// Issuer roots, authority relays, and minimum trust level came from one
    /// canonical profile.
    ManifoldProfile {
        /// Selected deployment environment.
        environment: ManifoldEnvironment,
        /// Revision of the canonical profile.
        profile_revision: u32,
    },

    /// Explicit roots, relays, and minimum trust level supplied by the
    /// `test-support` constructor.
    ExplicitTestConfiguration,
}

/// Complete, verified facts carried by one authentic holder-authorization envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedPeerBadge {
    issuer: IssuerId,
    holder: HolderId,
    subject: SubjectPubkey,
    credential_digest: CredentialDigest,
    badge: TrustScoreBadgeV1,
}

impl VerifiedPeerBadge {
    /// Trusted issuer identity that signed the backing credential authority.
    #[must_use]
    pub fn issuer(&self) -> &IssuerId {
        &self.issuer
    }

    /// Holder identity bound into the credential and authorization.
    #[must_use]
    pub fn holder(&self) -> &HolderId {
        &self.holder
    }

    /// Service identity authorized to present the badge.
    #[must_use]
    pub fn subject(&self) -> &SubjectPubkey {
        &self.subject
    }

    /// Digest binding the authorization to the backing credential.
    #[must_use]
    pub fn credential_digest(&self) -> &CredentialDigest {
        &self.credential_digest
    }

    /// Typed `fedi-trust-score-v1.0` claims.
    #[must_use]
    pub fn badge(&self) -> &TrustScoreBadgeV1 {
        &self.badge
    }
}

/// Failure while authenticating a holder-authorization envelope.
#[derive(Debug, thiserror::Error)]
pub enum PeerBadgeVerificationError {
    /// The badge names an issuer identity outside the hardcoded root set.
    #[error("PeerBadge issuer identity is not trusted: {issuer}")]
    UntrustedIssuer {
        /// Credential issuer identity.
        issuer: PublicKey,
    },

    /// At least one canonical authority relay could not complete.
    #[error("current issuer authority is unavailable")]
    AuthorityUnavailable(#[source] Box<NostrClientError>),

    /// The relay answered but had no authority event for the issuer.
    #[error("current issuer authority is missing")]
    MissingAuthority,

    /// No returned authority candidate passed complete admission.
    #[error("current issuer authority is invalid")]
    InvalidAuthority,

    /// At least one signed revocation location could not complete.
    #[error("credential revocation state is unavailable")]
    RevocationUnavailable(#[source] Box<NostrClientError>),

    /// The authenticated authority provides no supported revocation source.
    #[error("issuer authority provides no Nostr revocation location")]
    MissingRevocationLocation,

    /// A non-empty completed revocation result contained no valid exact match.
    #[error("credential revocation result contains no valid exact match")]
    InvalidRevocation,

    /// A valid issuer revocation matched the credential.
    #[error("PeerBadge credential has been revoked")]
    CredentialRevoked,

    /// Credential or holder-authorization cryptography/binding failed.
    #[error("PeerBadge envelope is invalid")]
    InvalidEnvelope(#[source] CredentialsError),

    /// Credential claims do not match the PeerBadge schema.
    #[error("PeerBadge schema is invalid")]
    InvalidSchema(#[source] TrustScoreSchemaError),

    /// The authentic badge does not meet the configured relying-party policy.
    #[error(transparent)]
    InsufficientTrustLevel(#[from] PeerBadgeTrustPolicyError),
}

/// Cloneable shared verifier for authentic PeerBadge holder-authorizations.
#[derive(Clone)]
pub struct PeerBadgeVerifier {
    inner: Arc<PeerBadgeVerifierInner>,
}

struct PeerBadgeVerifierInner {
    provenance: PeerBadgeVerifierProvenance,
    issuer_roots: PeerBadgeIssuerRoots,
    trust_policy: PeerBadgeTrustPolicy,
    /// Authorities admitted at construction from the environment's committed,
    /// identity-signed authority documents, keyed by issuer identity. A
    /// pinned issuer is verified against this authority; no kind-37703 relay
    /// lookup happens for it, so a replaceable-event overwrite cannot rotate
    /// its trust (`SPEC-peer-badge-verifier`).
    pinned_authorities: BTreeMap<PublicKey, IssuerAuthority>,
    source: Arc<dyn PeerBadgeEventSource>,
}

impl PeerBadgeVerifier {
    /// Construct a verifier with explicit test-only roots, authority relays,
    /// and minimum trust level.
    ///
    /// This exists only under the `test-support` feature for defe-backed
    /// component tests. Production consumers must resolve immutable roots and
    /// relays through [`Self::try_from_profile`].
    ///
    /// # Errors
    ///
    /// Returns a typed configuration error when the roots or relays are empty,
    /// the relay count exceeds its bound, or the minimum is outside the
    /// PeerBadge schema's legal range.
    #[cfg(feature = "test-support")]
    pub fn new_for_test(
        issuer_roots: impl IntoIterator<Item = PublicKey>,
        authority_relays: impl IntoIterator<Item = RelayUrl>,
        minimum_trust_level: u64,
    ) -> Result<Self, PeerBadgeVerifierConfigError> {
        Ok(Self::new_with_provenance(
            PeerBadgeIssuerRoots::new(issuer_roots)?,
            PeerBadgeAuthorityRelays::new(authority_relays)?,
            PeerBadgeTrustPolicy::try_new(minimum_trust_level)?,
            BTreeMap::new(),
            PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        ))
    }

    /// Construct the shared verifier from one resolved deployment profile.
    ///
    /// # Errors
    ///
    /// Returns a typed configuration error when the environment has no issuer
    /// identity configured or its canonical data violates verifier bounds or
    /// the PeerBadge schema's trust-level range.
    pub fn try_from_profile(
        profile: &ManifoldEnvironmentProfile,
    ) -> Result<Self, PeerBadgeVerifierConfigError> {
        Self::try_from_profile_parts(
            profile.environment(),
            profile.profile_revision(),
            profile.peer_badge_issuer_identities().iter().copied(),
            profile.nostr_relays().as_urls().iter().cloned(),
            profile.minimum_peer_badge_trust_level(),
            profile.pinned_issuer_authorities().iter().copied(),
        )
    }

    /// Return the source of this verifier's immutable trust configuration.
    #[must_use]
    pub fn provenance(&self) -> PeerBadgeVerifierProvenance {
        self.inner.provenance
    }

    fn try_from_profile_parts<'a>(
        environment: ManifoldEnvironment,
        profile_revision: u32,
        issuer_roots: impl IntoIterator<Item = PublicKey>,
        authority_relays: impl IntoIterator<Item = RelayUrl>,
        minimum_trust_level: u64,
        committed_authorities: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, PeerBadgeVerifierConfigError> {
        let issuer_roots =
            PeerBadgeIssuerRoots::new(issuer_roots).map_err(|error| match error {
                PeerBadgeVerifierConfigError::EmptyIssuerRoots => {
                    PeerBadgeVerifierConfigError::EnvironmentIssuerRootsUnavailable { environment }
                }
                other => other,
            })?;
        let authority_relays = PeerBadgeAuthorityRelays::new(authority_relays)?;
        let trust_policy = PeerBadgeTrustPolicy::try_new(minimum_trust_level)?;
        let mut pinned_authorities = BTreeMap::new();
        for document in committed_authorities {
            let (identity, authority) = pin_committed_authority(document, &issuer_roots)?;
            if pinned_authorities.insert(identity, authority).is_some() {
                return Err(PeerBadgeVerifierConfigError::InvalidCommittedAuthority {
                    detail: format!("duplicate committed authority for issuer {identity}"),
                });
            }
        }
        Ok(Self::new_with_provenance(
            issuer_roots,
            authority_relays,
            trust_policy,
            pinned_authorities,
            PeerBadgeVerifierProvenance::ManifoldProfile {
                environment,
                profile_revision,
            },
        ))
    }

    fn new_with_provenance(
        issuer_roots: PeerBadgeIssuerRoots,
        authority_relays: PeerBadgeAuthorityRelays,
        trust_policy: PeerBadgeTrustPolicy,
        pinned_authorities: BTreeMap<PublicKey, IssuerAuthority>,
        provenance: PeerBadgeVerifierProvenance,
    ) -> Self {
        let mut authority_relays = authority_relays.0.into_iter();
        let first_authority_relay = authority_relays
            .next()
            .expect("validated authority relay collection is non-empty");
        let source = NostrPeerBadgeClient::new(first_authority_relay, authority_relays);
        Self::with_source_and_provenance(
            issuer_roots,
            trust_policy,
            pinned_authorities,
            Arc::new(source),
            provenance,
        )
    }

    #[cfg(test)]
    fn with_source(
        issuer_roots: PeerBadgeIssuerRoots,
        source: Arc<dyn PeerBadgeEventSource>,
        profile: &ManifoldEnvironmentProfile,
    ) -> Self {
        Self::with_source_and_provenance(
            issuer_roots,
            PeerBadgeTrustPolicy::try_new(profile.minimum_peer_badge_trust_level())
                .expect("canonical profile minimum trust level is valid"),
            BTreeMap::new(),
            source,
            PeerBadgeVerifierProvenance::ManifoldProfile {
                environment: profile.environment(),
                profile_revision: profile.profile_revision(),
            },
        )
    }

    fn with_source_and_provenance(
        issuer_roots: PeerBadgeIssuerRoots,
        trust_policy: PeerBadgeTrustPolicy,
        pinned_authorities: BTreeMap<PublicKey, IssuerAuthority>,
        source: Arc<dyn PeerBadgeEventSource>,
        provenance: PeerBadgeVerifierProvenance,
    ) -> Self {
        Self {
            inner: Arc::new(PeerBadgeVerifierInner {
                provenance,
                issuer_roots,
                trust_policy,
                pinned_authorities,
                source,
            }),
        }
    }

    /// Resolve the issuer's authority, fetch fresh revocation state, and
    /// authenticate an envelope.
    ///
    /// A pinned issuer (development/staging committed authority) resolves to
    /// its construction-time authority with no kind-37703 lookup; an unpinned
    /// issuer's authority is fetched fresh. Every invocation fetches
    /// revocation state afresh via sequential bounded relay I/O under one
    /// absolute deadline and never returns cached or stale trust state — a
    /// pinned authority is configuration, not cache.
    /// Dropping the future cooperatively cancels its relay reads and leaves no
    /// verifier state. A best-effort unsubscribe on the private ephemeral relay
    /// client may briefly outlive the dropped future.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the issuer is not rooted, a complete current
    /// authority or revocation result cannot be obtained, any signed object or
    /// binding is invalid, the credential is revoked, the PeerBadge schema is
    /// invalid, or the authenticated trust level is below configured policy.
    pub async fn verify(
        &self,
        envelope: &HolderAuthorizationEnvelope,
    ) -> Result<VerifiedPeerBadge, PeerBadgeVerificationError> {
        let now = fedimint_core::time::duration_since_epoch().as_secs();
        let deadline = Instant::now() + PEER_BADGE_VERIFICATION_TIMEOUT;
        self.verify_at(envelope, now, deadline).await
    }

    async fn verify_at(
        &self,
        envelope: &HolderAuthorizationEnvelope,
        now: u64,
        deadline: Instant,
    ) -> Result<VerifiedPeerBadge, PeerBadgeVerificationError> {
        let issuer = envelope
            .signed_credential
            .credential
            .issuer_id_pubkey
            .clone();
        if !self.inner.issuer_roots.contains(&issuer.0) {
            return Err(PeerBadgeVerificationError::UntrustedIssuer { issuer: issuer.0 });
        }

        // A pinned issuer is verified against the authority derived from the
        // environment's committed secret: no kind-37703 lookup happens, so a
        // replaceable-event overwrite can neither rotate its trust nor deny
        // verification. Unpinned issuers (production) keep the fetch-fresh
        // rule.
        let authority = match self.inner.pinned_authorities.get(&issuer.0) {
            Some(pinned) => pinned.clone(),
            None => {
                let authority_candidates = self
                    .inner
                    .source
                    .fetch_issuer_authority_candidates(issuer.0, deadline)
                    .await
                    .map_err(|error| {
                        PeerBadgeVerificationError::AuthorityUnavailable(Box::new(error))
                    })?;
                admit_current_authority(authority_candidates, &issuer)?
            }
        };

        let credential_digest = CredentialDigest(
            envelope
                .signed_credential
                .credential
                .digest()
                .map_err(PeerBadgeVerificationError::InvalidEnvelope)?,
        );
        self.check_revocation(&issuer, &authority, &credential_digest, deadline)
            .await?;

        let mut verifier = VerificationContext::new();
        verifier
            .add_issuer_authority(&authority)
            .map_err(PeerBadgeVerificationError::InvalidEnvelope)?;
        verifier
            .verify_credential_authorization_at_time(
                &envelope.signed_credential,
                &envelope.holder_authorization,
                now,
            )
            .map_err(|error| match error {
                CredentialsError::CredentialRevoked => {
                    PeerBadgeVerificationError::CredentialRevoked
                }
                error => PeerBadgeVerificationError::InvalidEnvelope(error),
            })?;
        let badge = parse_trust_score_badge_v1(&envelope.signed_credential.credential)
            .map_err(PeerBadgeVerificationError::InvalidSchema)?;
        self.inner.trust_policy.require(&badge)?;
        let authorization = &envelope.holder_authorization.authorization;

        Ok(VerifiedPeerBadge {
            issuer,
            holder: authorization.holder_id_pubkey.clone(),
            subject: authorization.subject_pubkey.clone(),
            credential_digest,
            badge,
        })
    }

    async fn check_revocation(
        &self,
        issuer: &IssuerId,
        authority: &IssuerAuthority,
        credential_digest: &CredentialDigest,
        deadline: Instant,
    ) -> Result<(), PeerBadgeVerificationError> {
        if authority.issuer.revocation.len() > MAX_REVOCATION_LOCATIONS {
            return Err(PeerBadgeVerificationError::InvalidAuthority);
        }
        let mut seen = BTreeSet::new();
        let relay_urls = authority
            .issuer
            .revocation
            .iter()
            .filter(|location| {
                location.protocol
                    == fedi_decentralized_nostr::attester::NOSTR_REVOCATION_LOCATION_PROTOCOL
            })
            .map(|location| {
                RelayUrl::parse(&location.location)
                    .map_err(|_| PeerBadgeVerificationError::InvalidAuthority)
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|relay| seen.insert(relay.to_string()))
            .collect::<Vec<_>>();
        if relay_urls.is_empty() {
            return Err(PeerBadgeVerificationError::MissingRevocationLocation);
        }

        let digest = credential_digest_wire_string(credential_digest)?;
        let candidates = self
            .inner
            .source
            .fetch_revocation_candidates(issuer.0, &digest, &relay_urls, deadline)
            .await
            .map_err(|error| PeerBadgeVerificationError::RevocationUnavailable(Box::new(error)))?;
        admit_revocation_candidates(candidates, issuer, credential_digest, &digest)
    }
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
trait PeerBadgeEventSource: Send + Sync {
    async fn fetch_issuer_authority_candidates(
        &self,
        issuer: PublicKey,
        deadline: Instant,
    ) -> Result<Vec<Event>, NostrClientError>;

    async fn fetch_revocation_candidates(
        &self,
        issuer: PublicKey,
        credential_digest: &str,
        relay_urls: &[RelayUrl],
        deadline: Instant,
    ) -> Result<Vec<Event>, NostrClientError>;
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl PeerBadgeEventSource for NostrPeerBadgeClient {
    async fn fetch_issuer_authority_candidates(
        &self,
        issuer: PublicKey,
        deadline: Instant,
    ) -> Result<Vec<Event>, NostrClientError> {
        self.fetch_issuer_authority_candidates(issuer, deadline)
            .await
    }

    async fn fetch_revocation_candidates(
        &self,
        issuer: PublicKey,
        credential_digest: &str,
        relay_urls: &[RelayUrl],
        deadline: Instant,
    ) -> Result<Vec<Event>, NostrClientError> {
        self.fetch_revocation_candidates(issuer, credential_digest, relay_urls, deadline)
            .await
    }
}

/// Admit one environment-committed, identity-signed authority document.
///
/// The document is public signed material: admitting it involves no secret
/// handling, signing, or randomness, so this path stays portable everywhere
/// the verifier compiles (including wasm). Its revocation locations are the
/// environment's canonical relays, so revocation stays enforced — and stays
/// updatable by publishing kind-37704 revocations to those relays — while the
/// issuance key becomes a function of the repository instead of the newest
/// kind-37703 event.
fn pin_committed_authority(
    document: &str,
    issuer_roots: &PeerBadgeIssuerRoots,
) -> Result<(PublicKey, IssuerAuthority), PeerBadgeVerifierConfigError> {
    let invalid =
        |detail: String| PeerBadgeVerifierConfigError::InvalidCommittedAuthority { detail };
    let authority: IssuerAuthority = serde_json::from_str(document)
        .map_err(|error| invalid(format!("parse IssuerAuthority: {error}")))?;
    authority
        .verify()
        .map_err(|error| invalid(format!("verify committed authority: {error}")))?;
    if authority.issuer.revocation.len() > MAX_REVOCATION_LOCATIONS {
        return Err(invalid(format!(
            "committed authority exceeds the revocation location bound of {MAX_REVOCATION_LOCATIONS}"
        )));
    }
    let identity = authority.issuer.issuer_id_pubkey.0;
    if !issuer_roots.contains(&identity) {
        return Err(invalid(format!(
            "committed authority issuer {identity} is not a configured identity root"
        )));
    }
    Ok((identity, authority))
}

fn admit_current_authority(
    mut candidates: Vec<Event>,
    issuer: &IssuerId,
) -> Result<IssuerAuthority, PeerBadgeVerificationError> {
    if candidates.is_empty() {
        return Err(PeerBadgeVerificationError::MissingAuthority);
    }
    candidates.retain(|event| {
        event.verify().is_ok()
            && event.pubkey == issuer.0
            && event.kind == Kind::Custom(ISSUER_AUTHORITY_EVENT_KIND)
            && has_exact_d_tag(event, ISSUER_AUTHORITY_D_TAG)
    });
    candidates.sort_by(newest_event_first);
    let event = candidates
        .into_iter()
        .next()
        .ok_or(PeerBadgeVerificationError::InvalidAuthority)?;
    let authority = serde_json::from_str::<IssuerAuthority>(&event.content)
        .map_err(|_| PeerBadgeVerificationError::InvalidAuthority)?;
    if authority.issuer.issuer_id_pubkey != *issuer || authority.verify().is_err() {
        return Err(PeerBadgeVerificationError::InvalidAuthority);
    }
    Ok(authority)
}

fn admit_revocation_candidates(
    candidates: Vec<Event>,
    issuer: &IssuerId,
    credential_digest: &CredentialDigest,
    digest_wire: &str,
) -> Result<(), PeerBadgeVerificationError> {
    let expected_d_tag = credential_revocation_d_tag(digest_wire);
    if candidates.iter().any(|event| {
        is_valid_matching_revocation(event, issuer, credential_digest, &expected_d_tag)
    }) {
        return Err(PeerBadgeVerificationError::CredentialRevoked);
    }
    if candidates.is_empty() {
        Ok(())
    } else {
        Err(PeerBadgeVerificationError::InvalidRevocation)
    }
}

fn is_valid_matching_revocation(
    event: &Event,
    issuer: &IssuerId,
    credential_digest: &CredentialDigest,
    expected_d_tag: &str,
) -> bool {
    if event.verify().is_err()
        || event.pubkey != issuer.0
        || event.kind != Kind::Custom(CREDENTIAL_REVOCATION_EVENT_KIND)
        || !has_exact_d_tag(event, expected_d_tag)
    {
        return false;
    }
    let Ok(revocation) = serde_json::from_str::<SignedRevocation>(&event.content) else {
        return false;
    };
    if revocation.proof.issuer_id_pubkey != *issuer {
        return false;
    }
    let Ok(verified) = revocation.verify() else {
        return false;
    };
    &verified.credential_digest == credential_digest
}

fn credential_digest_wire_string(
    digest: &CredentialDigest,
) -> Result<String, PeerBadgeVerificationError> {
    match serde_json::to_value(digest)
        .map_err(CredentialsError::from)
        .map_err(PeerBadgeVerificationError::InvalidEnvelope)?
    {
        serde_json::Value::String(digest) => Ok(digest),
        _ => Err(PeerBadgeVerificationError::InvalidEnvelope(
            CredentialsError::VerificationFailed,
        )),
    }
}

fn has_exact_d_tag(event: &Event, expected: &str) -> bool {
    let mut d_tags = event
        .tags
        .as_slice()
        .iter()
        .filter(|tag| tag.kind() == TagKind::d());
    let Some(d_tag) = d_tags.next() else {
        return false;
    };
    let d_tag = d_tag.as_slice();
    d_tag.len() == 2 && d_tag[0] == "d" && d_tag[1] == expected && d_tags.next().is_none()
}

fn newest_event_first(left: &Event, right: &Event) -> std::cmp::Ordering {
    right
        .created_at
        .cmp(&left.created_at)
        .then_with(|| event_id_order(&left.id, &right.id))
}

fn event_id_order(left: &EventId, right: &EventId) -> std::cmp::Ordering {
    left.as_bytes().cmp(right.as_bytes())
}
