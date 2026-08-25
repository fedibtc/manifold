//! Shared `fedi-trust-score-v1.0` holder trust badge schema and the inline
//! holder-authorization trust envelope carried in FMan and FLIP advertisements.
//!
//! The badge schema is owned by the credential SDK's `schemas` crate and
//! re-exported here; this module adds inline envelope carriage plus the generic,
//! validated relying-party minimum-level policy. The envelope is the shared
//! advertisement convention: each entry embeds the
//! holder-signed authorization together with the backing issuer credential so
//! verifiers need no separate credential fetch.

#[cfg(test)]
mod tests;

use fedi_credential_sdk_protocol::{
    CredentialsError, HolderAuthorization, SignedCredential, VerificationContext,
};
use serde::{Deserialize, Serialize};

use crate::{Pubkey, Timestamp};

pub use fedi_credential_sdk_schemas::{
    TRUST_SCORE_LEVEL_MAX, TRUST_SCORE_LEVEL_MIN, TRUST_SCORE_SCHEMA_V1, TrustScoreBadgeV1,
    TrustScoreSchemaError, parse_trust_score_badge_v1, trust_score_blind_msg_v1,
    trust_score_info_v1,
};

/// Validated relying-party policy for the minimum accepted PeerBadge level.
///
/// This is application policy over an already authenticated and parsed badge,
/// not part of the credential schema itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerBadgeTrustPolicy {
    minimum_trust_level: u64,
}

impl PeerBadgeTrustPolicy {
    /// Construct a policy whose minimum lies inside the PeerBadge schema range.
    ///
    /// # Errors
    ///
    /// Returns [`PeerBadgeTrustPolicyConfigError`] when `minimum_trust_level`
    /// is outside [`TRUST_SCORE_LEVEL_MIN`] through [`TRUST_SCORE_LEVEL_MAX`].
    pub fn try_new(minimum_trust_level: u64) -> Result<Self, PeerBadgeTrustPolicyConfigError> {
        if !(TRUST_SCORE_LEVEL_MIN..=TRUST_SCORE_LEVEL_MAX).contains(&minimum_trust_level) {
            return Err(PeerBadgeTrustPolicyConfigError {
                minimum: TRUST_SCORE_LEVEL_MIN,
                maximum: TRUST_SCORE_LEVEL_MAX,
                actual: minimum_trust_level,
            });
        }
        Ok(Self {
            minimum_trust_level,
        })
    }

    /// Return the validated minimum accepted level.
    #[must_use]
    pub fn minimum_trust_level(self) -> u64 {
        self.minimum_trust_level
    }

    /// Require an authenticated, schema-valid badge to meet this policy.
    ///
    /// # Errors
    ///
    /// Returns [`PeerBadgeTrustPolicyError`] when the badge's level is below
    /// the configured minimum.
    pub fn require(&self, badge: &TrustScoreBadgeV1) -> Result<(), PeerBadgeTrustPolicyError> {
        if badge.trust_level < self.minimum_trust_level {
            return Err(PeerBadgeTrustPolicyError {
                minimum: self.minimum_trust_level,
                actual: badge.trust_level,
            });
        }
        Ok(())
    }
}

/// Invalid static PeerBadge relying-party policy configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerBadgeTrustPolicyConfigError {
    minimum: u64,
    maximum: u64,
    actual: u64,
}

impl PeerBadgeTrustPolicyConfigError {
    /// Smallest schema-valid policy value.
    #[must_use]
    pub fn minimum(self) -> u64 {
        self.minimum
    }

    /// Largest schema-valid policy value.
    #[must_use]
    pub fn maximum(self) -> u64 {
        self.maximum
    }

    /// Supplied invalid policy value.
    #[must_use]
    pub fn actual(self) -> u64 {
        self.actual
    }
}

impl core::fmt::Display for PeerBadgeTrustPolicyConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "PeerBadge minimum trust level must be between {} and {}, got {}",
            self.minimum, self.maximum, self.actual
        )
    }
}

impl std::error::Error for PeerBadgeTrustPolicyConfigError {}

/// An authentic PeerBadge did not meet the relying party's minimum level.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerBadgeTrustPolicyError {
    minimum: u64,
    actual: u64,
}

impl PeerBadgeTrustPolicyError {
    /// Minimum required trust level.
    #[must_use]
    pub fn minimum(self) -> u64 {
        self.minimum
    }

    /// Authentic badge's actual trust level.
    #[must_use]
    pub fn actual(self) -> u64 {
        self.actual
    }
}

impl core::fmt::Display for PeerBadgeTrustPolicyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "PeerBadge trust level {} is below required minimum {}",
            self.actual, self.minimum
        )
    }
}

impl std::error::Error for PeerBadgeTrustPolicyError {}

/// Inline trust envelope shared by FMan and FLIP advertisements.
///
/// The wire shape is exactly
/// `{ "holder_authorization": ..., "signed_credential": ... }`: the
/// holder-signed authorization binding a subject service pubkey to a trust
/// badge, together with the backing issuer credential, embedded inline so the
/// verifier needs no separate credential fetch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HolderAuthorizationEnvelope {
    /// Holder-signed authorization for the subject pubkey.
    pub holder_authorization: HolderAuthorization,

    /// Issuer credential backing the holder's trust badge.
    pub signed_credential: SignedCredential,
}

/// Error returned when verifying a [`HolderAuthorizationEnvelope`].
#[derive(Debug)]
pub enum HolderTrustEnvelopeError {
    /// SDK credential/authorization verification failed: untrusted issuer,
    /// invalid proof, revoked credential, holder-binding mismatch, digest
    /// mismatch, or a not-yet-valid authorization.
    Credential(CredentialsError),

    /// The authorization's `subject_pubkey` does not match the expected
    /// subject.
    SubjectMismatch,

    /// The backing credential does not match `fedi-trust-score-v1.0`.
    Schema(TrustScoreSchemaError),
}

impl core::fmt::Display for HolderTrustEnvelopeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Credential(error) => write!(f, "credential verification failed: {error}"),
            Self::SubjectMismatch => {
                f.write_str("holder authorization subject does not match the expected pubkey")
            }
            Self::Schema(error) => write!(f, "trust badge schema check failed: {error}"),
        }
    }
}

impl std::error::Error for HolderTrustEnvelopeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Credential(error) => Some(error),
            Self::SubjectMismatch => None,
            Self::Schema(error) => Some(error),
        }
    }
}

/// Verify an inline holder trust envelope for an expected subject pubkey.
///
/// Runs the SDK chain (trusted issuer, credential proof, revocation set,
/// authorization proof, holder binding, credential digest binding, and
/// authorization freshness at `now`), then checks the authorization subject
/// binding and the `fedi-trust-score-v1.0` schema. Trust-level acceptance
/// remains caller policy.
///
/// # Errors
///
/// Returns a [`HolderTrustEnvelopeError`] naming the failing stage.
pub fn verify_holder_trust_envelope(
    verifier: &VerificationContext,
    envelope: &HolderAuthorizationEnvelope,
    expected_subject_pubkey: &Pubkey,
    now: Timestamp,
) -> Result<TrustScoreBadgeV1, HolderTrustEnvelopeError> {
    verifier
        .verify_credential_authorization_at_time(
            &envelope.signed_credential,
            &envelope.holder_authorization,
            now.0,
        )
        .map_err(HolderTrustEnvelopeError::Credential)?;

    let subject = Pubkey(
        envelope
            .holder_authorization
            .authorization
            .subject_pubkey
            .0
            .to_string(),
    );
    if &subject != expected_subject_pubkey {
        return Err(HolderTrustEnvelopeError::SubjectMismatch);
    }

    parse_trust_score_badge_v1(&envelope.signed_credential.credential)
        .map_err(HolderTrustEnvelopeError::Schema)
}
