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

/// Smallest guardian fee rate an FA may propose, in parts per million, when a
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

/// Available memory, in MiB, an MG needs per seat when a publication does not
/// say: about 1.2 GiB.
pub const DEFAULT_SEAT_MEMORY_BUDGET_MIB: u32 = 1_229;

/// Smallest and largest per-seat memory budget a publication may set, in MiB.
/// The bounds refuse a publisher mistake that would let MGs admit seats with
/// almost no memory each, or would stop every MG from admitting any.
pub const SETUP_PAYMENT_SEAT_MEMORY_BUDGET_MIB_RANGE: std::ops::RangeInclusive<u32> = 256..=16_384;

/// Seats one MG may host when a publication does not say.
pub const DEFAULT_MAX_SEATS_PER_MG: u32 = 12;

/// Largest per-MG seat limit a publication may set.
pub const SETUP_PAYMENT_MAX_SEATS_PER_MG: u32 = 256;

fn default_min_fee_ppm() -> u64 {
    DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM
}

/// Uptime scores, in percent, an MG needs to accept new seats. Each score is
/// an exponential moving average with the named time constant; 0 turns that
/// window's check off.
///
/// Optional on the wire as a whole and per window: an omitted window takes
/// its [`Default`] value, so a publication can change one window alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct MinUptimePercent {
    /// One-day score: holds an MG closed for some hours after a long outage.
    #[serde(rename = "1d")]
    pub day: u8,
    /// Seven-day score: closes an MG for about two days after a day down.
    #[serde(rename = "7d")]
    pub week: u8,
    /// Thirty-day score: closes an MG that is often down a little.
    #[serde(rename = "30d")]
    pub month: u8,
}

impl Default for MinUptimePercent {
    fn default() -> Self {
        Self {
            day: 70,
            week: 90,
            month: 93,
        }
    }
}

/// SemVer release of the Manifold Guardian daemon.
///
/// Parsing is the validity gate; the wire representation is a string.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MgVersion(semver::Version);

impl FromStr for MgVersion {
    type Err = semver::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl fmt::Display for MgVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl serde::Serialize for MgVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for MgVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// Default absence limit for guardian replacement when an older policy omits it.
pub const DEFAULT_MAX_GUARDIAN_OFFLINE_SECS: u64 = 24 * 60 * 60;

/// Version-1 Nostr content published by the setup-payment federation authority.
///
/// Consumers ignore unknown fields. Policy changes requiring consumer support
/// must use a new wire-format version instead.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SetupPaymentFederationsContent {
    /// Wire-format version.
    pub version: ProtocolV1,

    /// Latest Manifold Guardian release published by the authority.
    #[serde(rename = "fman_version")]
    pub mg_version: MgVersion,

    /// Public Fedimint invites forming the unordered common set.
    pub federations: Vec<InviteCode>,

    /// Fedi-operated endpoint where verified MGs privately register
    /// guardian telemetry access. This is the only telemetry locator carried
    /// by the public event; Iroh endpoint ids and bearer capabilities never
    /// appear on Nostr.
    pub telemetry_registration_url: Url,

    /// Smallest guardian fee rate, in parts per million, that a guardian will
    /// accept in a fee proposal. Optional on the wire: an event omitting it
    /// carries [`DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM`], so older publications
    /// stay admissible.
    #[serde(default = "default_min_fee_ppm")]
    pub min_fee_ppm: u64,

    /// Uptime scores below which an MG stops accepting new seats. Optional on
    /// the wire: absent, [`MinUptimePercent::default`] applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_uptime_percent: Option<MinUptimePercent>,

    /// Longest absence before a guardian qualifies for replacement, in seconds.
    /// Omitted by older policies; those use [`DEFAULT_MAX_GUARDIAN_OFFLINE_SECS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_guardian_offline_secs: Option<u64>,

    /// Nostr public key of Fedi support, which MG operators chat
    /// with over NIP-17. Optional on the wire: absent, the environment
    /// profile's support key applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_nostr_pubkey: Option<nostr::PublicKey>,

    /// Available memory, in MiB, an MG needs per seat. Optional on the
    /// wire: absent means [`DEFAULT_SEAT_MEMORY_BUDGET_MIB`]. Omitted when
    /// unset so that consumers that refuse unknown fields keep admitting
    /// publications that do not use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_memory_budget_mib: Option<u32>,

    /// Seats one MG may host, below its operator's own limit. Optional on the
    /// wire: absent means [`DEFAULT_MAX_SEATS_PER_MG`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_seats_per_mg: Option<u32>,

    /// Whether MGs start new guardian transfers. Optional on the wire:
    /// absent, the Manifold environment's default applies (on in development
    /// and staging, off in production). Transfers an MG already stored run to
    /// completion either way
    /// ([`SPEC-guardian-transfer`](../../mg/specs/SPEC-guardian-transfer.md)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guardian_transfer: Option<bool>,
}

/// Semantically admitted common payment-federation set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedSetupPaymentFederations {
    /// Latest Manifold Guardian release published with this set.
    mg_version: MgVersion,

    /// Unique invites keyed by their canonical derived federation IDs.
    federations: BTreeMap<FederationId, InviteCode>,
    telemetry_registration_url: Url,
    min_fee_ppm: u64,
    min_uptime_percent: MinUptimePercent,
    max_guardian_offline_secs: u64,
    support_nostr_pubkey: Option<nostr::PublicKey>,
    seat_memory_budget_mib: Option<u32>,
    max_seats_per_mg: Option<u32>,
    guardian_transfer: Option<bool>,
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
    /// federation IDs, a minimum fee rate above the payer's send-rate
    /// ceiling, or a minimum uptime above 100%.
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
        if content
            .seat_memory_budget_mib
            .is_some_and(|mib| !SETUP_PAYMENT_SEAT_MEMORY_BUDGET_MIB_RANGE.contains(&mib))
        {
            return Err(SetupPaymentFederationsContentError::SeatMemoryBudgetOutOfRange);
        }
        if content
            .max_seats_per_mg
            .is_some_and(|seats| seats > SETUP_PAYMENT_MAX_SEATS_PER_MG)
        {
            return Err(SetupPaymentFederationsContentError::MaxSeatsPerMgTooHigh);
        }
        let max_guardian_offline_secs = content
            .max_guardian_offline_secs
            .unwrap_or(DEFAULT_MAX_GUARDIAN_OFFLINE_SECS);
        if max_guardian_offline_secs == 0 {
            return Err(SetupPaymentFederationsContentError::ZeroOfflineDuration);
        }
        let min_uptime_percent = content.min_uptime_percent.unwrap_or_default();
        if [
            min_uptime_percent.day,
            min_uptime_percent.week,
            min_uptime_percent.month,
        ]
        .into_iter()
        .any(|percent| percent > 100)
        {
            return Err(SetupPaymentFederationsContentError::MinUptimeTooHigh);
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

        let mg_version = content.mg_version;
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
            mg_version,
            federations,
            telemetry_registration_url: content.telemetry_registration_url,
            min_fee_ppm: content.min_fee_ppm,
            min_uptime_percent,
            max_guardian_offline_secs,
            support_nostr_pubkey: content.support_nostr_pubkey,
            seat_memory_budget_mib: content.seat_memory_budget_mib,
            max_seats_per_mg: content.max_seats_per_mg,
            guardian_transfer: content.guardian_transfer,
        })
    }

    /// Return the latest Manifold Guardian release published with this set.
    #[must_use]
    pub fn mg_version(&self) -> &MgVersion {
        &self.mg_version
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

    /// Return the uptime scores an MG needs to accept new seats. An event
    /// that omits the field yields [`MinUptimePercent::default`].
    #[must_use]
    pub fn min_uptime_percent(&self) -> MinUptimePercent {
        self.min_uptime_percent
    }

    /// Return the absence limit for guardian replacement, in seconds.
    #[must_use]
    pub fn max_guardian_offline_secs(&self) -> u64 {
        self.max_guardian_offline_secs
    }

    /// Return the Fedi support key MG operators chat with, if published.
    #[must_use]
    pub fn support_nostr_pubkey(&self) -> Option<&nostr::PublicKey> {
        self.support_nostr_pubkey.as_ref()
    }

    /// Return the per-seat memory budget in MiB.
    #[must_use]
    pub fn seat_memory_budget_mib(&self) -> u32 {
        self.seat_memory_budget_mib
            .unwrap_or(DEFAULT_SEAT_MEMORY_BUDGET_MIB)
    }

    /// Return how many seats one MG may host.
    #[must_use]
    pub fn max_seats_per_mg(&self) -> u32 {
        self.max_seats_per_mg.unwrap_or(DEFAULT_MAX_SEATS_PER_MG)
    }

    /// Return the published guardian-transfer flag. `None` when the event
    /// omits it; the Manifold environment then supplies the default
    /// (`ManifoldEnvironment::guardian_transfer`).
    #[must_use]
    pub fn guardian_transfer(&self) -> Option<bool> {
        self.guardian_transfer
    }
}

/// Failure while parsing or semantically admitting publication content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupPaymentFederationsContentError {
    /// Event content exceeds the pre-parse byte limit.
    ContentTooLarge,

    /// JSON is invalid or does not match the known version-1 fields.
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

    /// The published per-seat memory budget is outside the admitted range.
    SeatMemoryBudgetOutOfRange,

    /// The published per-MG seat limit is above the admitted maximum.
    MaxSeatsPerMgTooHigh,

    /// A published minimum uptime is above 100%, which no MG can reach.
    MinUptimeTooHigh,
    /// An absence limit must allow a positive amount of time.
    ZeroOfflineDuration,
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
            Self::SeatMemoryBudgetOutOfRange => {
                "setup-payment federation set sets a per-seat memory budget outside the admitted range"
            }
            Self::MaxSeatsPerMgTooHigh => {
                "setup-payment federation set sets a per-MG seat limit above the admitted maximum"
            }
            Self::ZeroOfflineDuration => "guardian absence limit must be positive",
            Self::MinUptimeTooHigh => {
                "setup-payment federation set sets a minimum uptime above 100%"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for SetupPaymentFederationsContentError {}
