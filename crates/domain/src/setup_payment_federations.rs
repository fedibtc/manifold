//! Wire content and semantic admission for the common setup-payment federation set.
//!
//! Contract: `specs/SPEC-setup-payment-federations.md`.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use fedimint_core::invite_code::InviteCode as FedimintInviteCode;

use crate::{FederationId, InviteCode, ProtocolV1, Url};

/// Maximum UTF-8 size of one public Fedimint invite.
pub const SETUP_PAYMENT_FEDERATION_INVITE_MAX_BYTES: usize = 16 * 1024;

/// Maximum number of federations in one publication.
pub const SETUP_PAYMENT_FEDERATIONS_MAX_COUNT: usize = 16;

/// Maximum UTF-8 size of one event's JSON content.
pub const SETUP_PAYMENT_FEDERATIONS_MAX_CONTENT_BYTES: usize = 128 * 1024;

/// Smallest guardian fee rate an FI may propose, in parts per million, when a
/// publication does not say: Fedi's 1,500-ppm (0.15%) product floor.
///
/// This is the wire default rather than a consumer-local fallback so that a
/// publication predating the field, a consumer that has never reached a relay,
/// and a consumer holding a retained event all enforce the same floor.
pub const DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM: u64 = 1_500;

/// Largest minimum a publication may set: the pinned Fedi payer's own
/// 210,000-ppm send-rate ceiling. A floor above that ceiling would leave no
/// proposable rate at all, so a publisher mistake is refused at admission
/// rather than quietly making every fee proposal impossible.
pub const SETUP_PAYMENT_MAX_MIN_FEE_PPM: u64 = 210_000;

fn default_min_fee_ppm() -> u64 {
    DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM
}

/// SemVer release of the Fleet Manager daemon.
///
/// Parsing is the validity gate; the wire representation is a string.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FmanVersion(semver::Version);

impl FromStr for FmanVersion {
    type Err = semver::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl fmt::Display for FmanVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl serde::Serialize for FmanVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for FmanVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// Version-1 Nostr content published by the setup-payment federation authority.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupPaymentFederationsContent {
    /// Wire-format version.
    pub version: ProtocolV1,

    /// Latest Fleet Manager release published by the authority.
    pub fman_version: FmanVersion,

    /// Public Fedimint invites forming the unordered common set.
    pub federations: Vec<InviteCode>,

    /// Fedi-operated endpoint where verified FMans privately register
    /// guardian telemetry access. This is the only telemetry locator carried
    /// by the public event; Iroh endpoint ids and bearer capabilities never
    /// appear on Nostr.
    pub telemetry_registration_url: Url,

    /// Smallest guardian fee rate, in parts per million, that a guardian will
    /// accept in a fee proposal. Optional on the wire: an event omitting it
    /// carries [`DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM`], which is what keeps
    /// `deny_unknown_fields` from being the only compatibility direction —
    /// older publications stay admissible, newer ones do not.
    #[serde(default = "default_min_fee_ppm")]
    pub min_fee_ppm: u64,
}

/// Semantically admitted common payment-federation set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedSetupPaymentFederations {
    /// Latest Fleet Manager release published with this set.
    fman_version: FmanVersion,

    /// Unique invites keyed by their canonical derived federation IDs.
    federations: BTreeMap<FederationId, InviteCode>,
    telemetry_registration_url: Url,
    min_fee_ppm: u64,
}

impl AdmittedSetupPaymentFederations {
    /// Parse bounded JSON content and derive one canonical federation ID per invite.
    ///
    /// An empty set is valid. Array order has no policy meaning.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized or malformed content, excessive entries,
    /// invalid, oversized, or secret-bearing invites, duplicate derived
    /// federation IDs, or a minimum fee rate above the payer's send-rate
    /// ceiling.
    pub fn parse(raw_content: &[u8]) -> Result<Self, SetupPaymentFederationsContentError> {
        if raw_content.len() > SETUP_PAYMENT_FEDERATIONS_MAX_CONTENT_BYTES {
            return Err(SetupPaymentFederationsContentError::ContentTooLarge);
        }

        let content: SetupPaymentFederationsContent = serde_json::from_slice(raw_content)
            .map_err(|_| SetupPaymentFederationsContentError::MalformedContent)?;
        if content.federations.len() > SETUP_PAYMENT_FEDERATIONS_MAX_COUNT {
            return Err(SetupPaymentFederationsContentError::TooManyFederations);
        }
        if content.min_fee_ppm > SETUP_PAYMENT_MAX_MIN_FEE_PPM {
            return Err(SetupPaymentFederationsContentError::MinFeePpmTooHigh);
        }

        let registration_url = url::Url::parse(&content.telemetry_registration_url.0)
            .map_err(|_| SetupPaymentFederationsContentError::InvalidTelemetryRegistrationUrl)?;
        if registration_url.scheme() != "https"
            || !registration_url.has_host()
            || !registration_url.username().is_empty()
            || registration_url.password().is_some()
            || registration_url.query().is_some()
            || registration_url.fragment().is_some()
        {
            return Err(SetupPaymentFederationsContentError::InvalidTelemetryRegistrationUrl);
        }

        let fman_version = content.fman_version;
        let mut federations = BTreeMap::new();
        for invite_code in content.federations {
            if invite_code.0.is_empty() {
                return Err(SetupPaymentFederationsContentError::InvalidInvite);
            }
            if invite_code.0.len() > SETUP_PAYMENT_FEDERATION_INVITE_MAX_BYTES {
                return Err(SetupPaymentFederationsContentError::InviteTooLarge);
            }

            let parsed = FedimintInviteCode::from_str(&invite_code.0)
                .map_err(|_| SetupPaymentFederationsContentError::InvalidInvite)?;
            if parsed.api_secret().is_some() {
                return Err(SetupPaymentFederationsContentError::SecretInvite);
            }
            let federation_id = FederationId(parsed.federation_id().to_string());
            if federations.insert(federation_id, invite_code).is_some() {
                return Err(SetupPaymentFederationsContentError::DuplicateFederation);
            }
        }

        Ok(Self {
            fman_version,
            federations,
            telemetry_registration_url: content.telemetry_registration_url,
            min_fee_ppm: content.min_fee_ppm,
        })
    }

    /// Return the latest Fleet Manager release published with this set.
    #[must_use]
    pub fn fman_version(&self) -> &FmanVersion {
        &self.fman_version
    }

    /// Return the number of unique admitted federations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.federations.len()
    }

    /// Return whether the admitted set stops all new paid setup.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.federations.is_empty()
    }

    /// Look up the signed public invite for one canonical federation ID.
    #[must_use]
    pub fn invite(&self, federation_id: &FederationId) -> Option<&InviteCode> {
        self.federations.get(federation_id)
    }

    /// Iterate over canonical federation IDs and their signed public invites.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&FederationId, &InviteCode)> {
        self.federations.iter()
    }

    /// Return the authenticated Fedi telemetry-registration endpoint.
    #[must_use]
    pub fn telemetry_registration_url(&self) -> &Url {
        &self.telemetry_registration_url
    }

    /// Return the smallest guardian fee rate a proposal may carry, in ppm.
    ///
    /// Never zero unless Fedi published zero: an event that omits the field
    /// yields [`DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM`].
    #[must_use]
    pub fn min_fee_ppm(&self) -> u64 {
        self.min_fee_ppm
    }
}

/// Failure while parsing or semantically admitting publication content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupPaymentFederationsContentError {
    /// Event content exceeds the pre-parse byte limit.
    ContentTooLarge,

    /// JSON is invalid or does not match the strict version-1 shape.
    MalformedContent,

    /// The publication contains more than the allowed number of entries.
    TooManyFederations,

    /// An invite is empty or not a valid Fedimint invite.
    InvalidInvite,

    /// An invite exceeds the explicit pre-parse byte limit.
    InviteTooLarge,

    /// An invite contains an API bearer secret and cannot be published.
    SecretInvite,

    /// Multiple invites derive the same canonical federation ID.
    DuplicateFederation,

    /// Telemetry registration must be a credential-free HTTPS URL with no
    /// query or fragment so NIP-98 can bind one unambiguous request target.
    InvalidTelemetryRegistrationUrl,

    /// The published minimum fee rate exceeds the payer's own send-rate
    /// ceiling, so no rate would satisfy both bounds.
    MinFeePpmTooHigh,
}

impl core::fmt::Display for SetupPaymentFederationsContentError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let message = match self {
            Self::ContentTooLarge => "setup-payment federation content is too large",
            Self::MalformedContent => "setup-payment federation content is malformed",
            Self::TooManyFederations => "setup-payment federation set has too many entries",
            Self::InvalidInvite => "setup-payment federation set contains an invalid invite",
            Self::InviteTooLarge => "setup-payment federation set contains an oversized invite",
            Self::SecretInvite => "setup-payment federation set contains a secret-bearing invite",
            Self::DuplicateFederation => {
                "setup-payment federation set contains a duplicate federation"
            }
            Self::InvalidTelemetryRegistrationUrl => {
                "setup-payment federation set contains an invalid telemetry registration URL"
            }
            Self::MinFeePpmTooHigh => {
                "setup-payment federation set sets a minimum guardian fee rate above the payer cap"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for SetupPaymentFederationsContentError {}
