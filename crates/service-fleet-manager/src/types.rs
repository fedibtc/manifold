//! Fleet Manager protocol vocabulary: the newtypes and enums the message
//! modules share. Types with meaning beyond this protocol (federation ids,
//! invite codes, timestamps, federation names) come from the ecosystem-wide
//! `fedi_decentralized_domain` crate instead and are re-exported at the
//! crate root beside these.
//!
//! Some retained shapes (subscription plans, suspended statuses,
//! `ValidUntilDate`) are cross-program vocabulary the v1 daemon never emits;
//! they stay wire-compatible for future revisions.
use std::{fmt, str::FromStr};

use sha2::{Digest as _, Sha256};

/// Identity of a Federation Initiator.
///
/// Quotes bind this identity, and every allocating or seat-scoped signed
/// request uses the same key. Parsing is the validity gate: a value that
/// deserialized is a key someone could hold.
#[derive(
    serde::Deserialize, serde::Serialize, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub struct FiId(pub secp256k1::XOnlyPublicKey);

/// SemVer release of the Fedimint daemon requested or supported by an FM.
///
/// Parsing is the validity gate; the wire representation remains a string.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FedimintdVersion(semver::Version);

/// Three-number Fedimint release that must agree across one DKG.
///
/// FMan build suffixes such as `-fedi17` do not change this value. They name
/// different builds that may participate together when their core release is
/// the same.
#[derive(
    serde::Deserialize, serde::Serialize, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
#[serde(deny_unknown_fields)]
pub struct FedimintdVersionCore {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl fmt::Display for FedimintdVersionCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for FedimintdVersion {
    type Err = semver::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl fmt::Display for FedimintdVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FedimintdVersion {
    /// Return the three-number release used to group DKG-compatible builds.
    #[must_use]
    pub fn core(&self) -> FedimintdVersionCore {
        FedimintdVersionCore {
            major: self.0.major,
            minor: self.0.minor,
            patch: self.0.patch,
        }
    }
}

impl serde::Serialize for FedimintdVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for FedimintdVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// Number of guardians in a federation.
#[derive(
    serde::Deserialize, serde::Serialize, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub struct FederationSize(pub u16);

/// Out-of-band Fedimint ecash token supplied by an FI.
///
/// This is an opaque wrapper around the Fedimint OOB token encoding. It is
/// bearer money: implementations must not log it, and must not persist it after
/// successful receive/reissue except as a non-spendable hash.
#[derive(serde::Deserialize, serde::Serialize, Clone, Eq, PartialEq)]
pub struct OobEcashToken(pub String);

impl fmt::Debug for OobEcashToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("OobEcashToken").field(&"<redacted>").finish()
    }
}

/// FI signature over a request.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct FiSignature(pub secp256k1::schnorr::Signature);

/// FM signature over a request or response proof.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct ManagerSignature(pub secp256k1::schnorr::Signature);

/// Date until which a payment is valid.
#[derive(
    serde::Deserialize, serde::Serialize, Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub struct ValidUntilDate(pub u64);

/// Guardian display name used during DKG.
#[derive(
    serde::Deserialize, serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub struct GuardianName(pub String);

/// Non-default module configuration for a federation.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct ModuleConfig(pub Vec<u8>);

/// Guardian code pasted between guardians during DKG.
///
/// This is the bare upstream Fedimint base32 setup code. The newtype preserves
/// the Fleet Manager RPC vocabulary without adding a second envelope.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct GuardianCode(pub String);

/// Additional FM availability data useful to Fedi App.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct AvailabilityInfo(pub String);

/// Key for a federation metadata field.
#[derive(
    serde::Deserialize, serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd,
)]
pub struct MetaFieldKey(pub String);

/// Value for a federation metadata field.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct MetaFieldValue(pub String);

/// Fedimint's established federation display-name metadata key.
pub const FEDERATION_NAME_META_FIELD_KEY: &str = "federation_name";
/// Fedi's established federation icon URL metadata key.
pub const FEDERATION_ICON_URL_META_FIELD_KEY: &str = "fedi:federation_icon_url";
/// Fedi's established welcome-message metadata key, reused as wallet description.
pub const WELCOME_MESSAGE_META_FIELD_KEY: &str = "fedi:welcome_message";
/// Fedi's established terms-of-service URL metadata key.
pub const TERMS_OF_SERVICE_URL_META_FIELD_KEY: &str = "fedi:tos_url";

/// Domain separator for [`MetaConsensusBase`] preimages, so a base digest can
/// never collide with any other SHA-256 use in the protocol.
const META_CONSENSUS_BASE_DOMAIN: &[u8] = b"fedi-fman-meta-consensus-base/v1\0";

/// Exact consensus metadata *occurrence* on which a field mutation was based.
///
/// The meta module reaches consensus over one opaque whole-object value rather
/// than merging independent field proposals. Binding a signed FI request to
/// this base lets every FMan reject a stale read-modify-write before casting a
/// guardian vote that could discard another field.
///
/// The digest covers the module's monotone consensus **revision** together
/// with the raw value bytes, so the base names one occurrence of a board
/// state, not its content: a board that returns byte-exactly to an old state
/// does so under a fresh revision and therefore under a fresh base. Stale
/// admission pins and stale delayed handlers from the earlier occurrence can
/// never re-match it.
#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[serde(tag = "kind", content = "sha256", rename_all = "snake_case")]
pub enum MetaConsensusBase {
    /// No consensus metadata value existed when the FI read the federation.
    Absent,

    /// SHA-256 over the domain separator, the meta module's big-endian
    /// consensus revision, and the exact raw consensus bytes the FI read.
    Sha256([u8; 32]),
}

impl MetaConsensusBase {
    /// Commit to one observed meta-module consensus occurrence: `None` when
    /// the federation had no consensus metadata value, otherwise the module's
    /// monotone revision paired with the exact raw bytes of that read.
    #[must_use]
    pub fn from_consensus(consensus: Option<(u64, &[u8])>) -> Self {
        match consensus {
            None => Self::Absent,
            Some((revision, value)) => {
                let mut digest = Sha256::new();
                digest.update(META_CONSENSUS_BASE_DOMAIN);
                digest.update(revision.to_be_bytes());
                digest.update(value);
                Self::Sha256(digest.finalize().into())
            }
        }
    }
}

/// Simple runtime stats for the single `fedimintd` this FMan runs for a seat.
///
/// An FMan hosts one guardian, not the whole federation, so these are
/// node-level stats, not federation-wide telemetry. Deliberately minimal for
/// MVP; richer telemetry is a post-MVP concern.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq)]
pub struct FedimintStats {
    /// Version of the `fedimintd` this seat runs.
    pub fedimintd_version: FedimintdVersion,

    /// Seconds the `fedimintd` process has been running.
    pub uptime_seconds: u64,

    /// Whether the seat's `fedimintd` reports consensus running.
    pub consensus_running: bool,

    /// Guardian peers currently connected, of the federation's set.
    pub connected_peers: u32,

    /// Client ecash backups this seat's `fedimintd` stores — fedimint's
    /// `BackupStatistics.num_backups`, read from `BACKUP_STATISTICS_ENDPOINT`.
    pub num_backups: u64,
}

/// Commercial and lifecycle terms for a seat, chosen by the FI when
/// requesting a quote from the plans the FM advertises in
/// `GetAvailabilityResponse`.
///
/// `InfiniteBestEffort` is the only plan implemented end to end and the only
/// one an operator can offer; `SubscriptionBased` renewal is post-MVP wire
/// vocabulary the v1 daemon refuses to offer. Free seats are not a plan: they
/// are an out-of-band admission path (an operator-issued quote), so nothing
/// an FMan advertises is ever free.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq, strum::Display)]
pub enum Plan {
    /// Paid once, never expires. The seat runs at the operator's discretion
    /// ("best effort"); only an operator decommission ends it.
    InfiniteBestEffort {
        /// One-time price, in millisatoshis — the same unit and type as
        /// [`QuoteTerms::price_msats`](crate::QuoteTerms::price_msats), so a
        /// plan and the quote priced from it cannot be read differently.
        price_msats: u64,
    },

    /// Paid upfront, then renewed each period. The renewal price may differ
    /// from the initial price.
    SubscriptionBased {
        /// Price for the first period, in millisatoshis.
        initial_price_msats: u64,

        /// Price charged for each renewal, in millisatoshis (may differ from
        /// `initial_price_msats`).
        renewal_price_msats: u64,

        /// Length of a paid period.
        period: String,
    },
}

/// Whether a seat's `fedimintd` child is currently serving — a health
/// dimension orthogonal to the lifecycle state (SPEC-seat-lifecycle).
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Eq, PartialEq, strum::Display)]
pub enum SeatHealth {
    /// Process alive and answering its status API.
    Healthy,

    /// Temporarily not serving (spawn failing, crash backoff in progress, or
    /// alive but unresponsive); expected to recover. Seat-needing verbs
    /// return [`crate::FleetManagerError::SeatUnavailable`] meanwhile.
    Unavailable,

    /// The Provisioner gave up (backoff ceiling reached, suspected
    /// corruption, or the seat's recorded `fedimintd` version is missing from
    /// the installed release); operator action required.
    Failed,
}

#[cfg(test)]
#[path = "types/tests.rs"]
mod tests;
