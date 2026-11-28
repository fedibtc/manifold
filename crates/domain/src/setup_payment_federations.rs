//! Wire content and semantic admission for the common setup-payment federation set.
//!
//! Contract: `specs/SPEC-setup-payment-federations.md`.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU32;
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
/// say: 1 GiB. Releases before 0.1.20 default to 1,229 MiB (about 1.2 GiB).
pub const DEFAULT_SEAT_MEMORY_BUDGET_MIB: u32 = 1_024;

/// Smallest and largest per-seat memory budget a publication may set, in MiB.
/// The bounds refuse a publisher mistake that would let MGs admit seats with
/// almost no memory each, or would stop every MG from admitting any.
pub const SETUP_PAYMENT_SEAT_MEMORY_BUDGET_MIB_RANGE: std::ops::RangeInclusive<u32> = 256..=16_384;

/// Smallest and largest per-seat CPU budget a publication may set, in the MG
/// CPU benchmark's iterations per second
/// (`crates/mg/specs/REQ-seat-capacity-default.md`). One iteration is one
/// second of a 10-guardian seat's consensus work, so 1 is the least a seat
/// can need; 100,000 is over a hundred cores of a current server per seat,
/// which would stop every MG.
pub const SETUP_PAYMENT_SEAT_CPU_ITERATIONS_PER_SEC_RANGE: std::ops::RangeInclusive<u32> =
    1..=100_000;

/// Network data one seat uses, in GB (10^9 bytes) per 30 days, when a
/// publication does not say: a little above the about 57 GB a guardian's
/// peer traffic averaged on nearly idle production federations.
pub const DEFAULT_SEAT_MONTHLY_DATA_GB: u32 = 65;

/// Smallest and largest per-seat monthly data figure a publication may set,
/// in GB (10^9 bytes) per 30 days. The bounds refuse a publisher mistake that
/// would let a data budget admit seats that cost almost nothing, or would
/// stop every MG with a budget from admitting any. A publication that omits
/// the figure gets [`DEFAULT_SEAT_MONTHLY_DATA_GB`].
pub const SETUP_PAYMENT_SEAT_MONTHLY_DATA_GB_RANGE: std::ops::RangeInclusive<u32> = 1..=10_000;

const DEFAULT_SEAT_MONTHLY_DATA: NonZeroU32 =
    NonZeroU32::new(DEFAULT_SEAT_MONTHLY_DATA_GB).expect("the default is nonzero");

/// Data filesystem size, in GB (10^9 bytes), an MG needs per seat when a
/// publication does not say. Disk size stands in for the drive's write
/// endurance, which the MG cannot read: 80 GB lets a 512 GB drive sell 6
/// seats and a 1 TB drive 12, after the filesystem's own overhead.
pub const DEFAULT_SEAT_DISK_GB: u32 = 80;

/// Smallest and largest per-seat disk figure a publication may set, in GB
/// (10^9 bytes). The bounds refuse a publisher mistake that would let a small
/// drive admit seats that cost almost nothing, or would stop every MG from
/// admitting any. A publication that omits the figure gets
/// [`DEFAULT_SEAT_DISK_GB`].
pub const SETUP_PAYMENT_SEAT_DISK_GB_RANGE: std::ops::RangeInclusive<u32> = 1..=100_000;

const DEFAULT_SEAT_DISK: NonZeroU32 =
    NonZeroU32::new(DEFAULT_SEAT_DISK_GB).expect("the default is nonzero");

/// Data filesystem size, in GB (10^9 bytes), an MG on btrfs needs per seat
/// when a publication does not say. btrfs writes far more per synced write
/// than ext4 or ZFS, so the same drive wears out several times sooner under
/// the same seats; twice [`DEFAULT_SEAT_DISK_GB`] halves the seats it carries.
/// The bounds are [`SETUP_PAYMENT_SEAT_DISK_GB_RANGE`].
pub const DEFAULT_SEAT_DISK_GB_BTRFS: u32 = 160;

const DEFAULT_SEAT_DISK_BTRFS: NonZeroU32 =
    NonZeroU32::new(DEFAULT_SEAT_DISK_GB_BTRFS).expect("the default is nonzero");

/// Seats one MG may host when a publication does not say.
pub const DEFAULT_MAX_SEATS_PER_MG: u32 = 12;

/// Largest per-MG seat limit a publication may set.
pub const SETUP_PAYMENT_MAX_SEATS_PER_MG: u32 = 256;

/// Smallest and largest maximum pressure a publication may set, in percent
/// of wall time. Zero would close every MG; pressure is a share of time, so no
/// average exceeds 100.
pub const SETUP_PAYMENT_MAX_PRESSURE_PERCENT_RANGE: std::ops::RangeInclusive<u8> = 1..=100;

/// Maximum pressure averages for one resource, in percent of wall time during
/// which at least one task of the MG's cgroup stalled on it (PSI `some`). Each
/// average is an exponential moving average with the named time constant; an
/// MG accepts new seats only while every average is at or below its maximum.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct MaxPressureWindows {
    /// Six-hour average: closes an MG for the hours a spike lasts.
    #[serde(rename = "6h")]
    pub six_hours: u8,
    /// 3.5-day average: closes an MG whose pressure stays high for days, and
    /// barely moves for a single spike.
    #[serde(rename = "3.5d")]
    pub three_and_a_half_days: u8,
}

/// Maximum pressure averages, in percent, for the resources an MG's seats
/// compete for: CPU, storage IO and memory. Memory also has its own seat
/// limit, from the per-seat memory budget.
///
/// Optional on the wire as a whole, per resource and per window: an omitted
/// window takes its [`Default`] value, so a publication can change one window
/// alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(from = "MaxPressurePercentWire")]
pub struct MaxPressurePercent {
    pub cpu: MaxPressureWindows,
    pub io: MaxPressureWindows,
    pub memory: MaxPressureWindows,
}

impl Default for MaxPressurePercent {
    fn default() -> Self {
        Self {
            cpu: MaxPressureWindows {
                six_hours: 15,
                three_and_a_half_days: 10,
            },
            io: MaxPressureWindows {
                six_hours: 25,
                three_and_a_half_days: 15,
            },
            memory: MaxPressureWindows {
                six_hours: 70,
                three_and_a_half_days: 70,
            },
        }
    }
}

/// [`MaxPressurePercent`] as published, before omitted windows take their
/// defaults; the defaults differ by resource, so the windows cannot default
/// on their own.
#[derive(serde::Deserialize)]
struct MaxPressurePercentWire {
    #[serde(default)]
    cpu: MaxPressureWindowsWire,
    #[serde(default)]
    io: MaxPressureWindowsWire,
    #[serde(default)]
    memory: MaxPressureWindowsWire,
}

#[derive(Default, serde::Deserialize)]
struct MaxPressureWindowsWire {
    #[serde(rename = "6h")]
    six_hours: Option<u8>,
    #[serde(rename = "3.5d")]
    three_and_a_half_days: Option<u8>,
}

impl From<MaxPressurePercentWire> for MaxPressurePercent {
    fn from(wire: MaxPressurePercentWire) -> Self {
        let defaults = Self::default();
        let windows =
            |wire: MaxPressureWindowsWire, default: MaxPressureWindows| MaxPressureWindows {
                six_hours: wire.six_hours.unwrap_or(default.six_hours),
                three_and_a_half_days: wire
                    .three_and_a_half_days
                    .unwrap_or(default.three_and_a_half_days),
            };
        Self {
            cpu: windows(wire.cpu, defaults.cpu),
            io: windows(wire.io, defaults.io),
            memory: windows(wire.memory, defaults.memory),
        }
    }
}

impl MaxPressureWindows {
    fn admissible(self) -> bool {
        SETUP_PAYMENT_MAX_PRESSURE_PERCENT_RANGE.contains(&self.six_hours)
            && SETUP_PAYMENT_MAX_PRESSURE_PERCENT_RANGE.contains(&self.three_and_a_half_days)
    }
}

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
///
/// Deliberately not `Ord`: `semver::Version`'s ordering also ranks build
/// metadata, which SemVer precedence ignores. Compare releases with
/// [`MgVersion::is_older_than`]; equality stays exact, so a changed build
/// still reads as a changed policy.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MgVersion(semver::Version);

impl MgVersion {
    /// Whether this release has lower SemVer precedence than `other`:
    /// pre-releases rank below their release, build metadata is ignored.
    #[must_use]
    pub fn is_older_than(&self, other: &Self) -> bool {
        self.0.cmp_precedence(&other.0) == core::cmp::Ordering::Less
    }
}

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

    /// Frozen version-1 compatibility projection for readers without feature configuration.
    #[serde(flatten)]
    pub legacy_features: FeaturePolicy,

    /// Public Fedimint invites forming the unordered common set.
    pub federations: Vec<InviteCode>,

    /// Fedi-operated endpoint where verified MGs privately register
    /// guardian telemetry access. This is the only telemetry locator carried
    /// by the public event; Iroh endpoint ids and bearer capabilities never
    /// appear on Nostr.
    pub telemetry_registration_url: Url,

    /// Fedi-operated endpoint where MGs without a Holder authorization
    /// register instead, with a collector apart from the badged one.
    /// Optional on the wire: absent means those MGs do not register.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unbadged_telemetry_registration_url: Option<Url>,

    /// Smallest guardian fee rate, in parts per million, that a guardian will
    /// accept in a fee proposal. Optional on the wire: an event omitting it
    /// carries [`DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM`], so older publications
    /// stay admissible.
    #[serde(default = "default_min_fee_ppm")]
    pub min_fee_ppm: u64,
}

/// A complete feature snapshot. It has no authority over payment policy.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FeatureConfigContent {
    /// Version of the independent feature event schema.
    pub version: ProtocolV1,
    /// Complete replacement, not a patch to previous snapshots.
    #[serde(flatten)]
    pub features: FeaturePolicy,
}

/// Release, resource limits and feature switches shared with legacy readers.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FeaturePolicy {
    /// Latest Manifold Guardian release published by the authority.
    #[serde(rename = "fman_version")]
    pub mg_version: MgVersion,

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

    /// Available memory, in MiB, an MG before 0.1.16 needs per seat. This
    /// release ignores it and reads [`Self::seat_memory_budget_mib_v3`], so
    /// Fedi can give older MGs a different budget. Admission still checks
    /// its range, which older consumers enforce. Omitted when unset so that
    /// consumers that refuse unknown fields keep admitting publications that
    /// do not use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_memory_budget_mib: Option<u32>,

    /// Network data one seat uses, in GB (10^9 bytes) per 30 days. An MG
    /// whose operator set a monthly data budget sells at most the budget
    /// divided by this. Optional on the wire: absent means
    /// [`DEFAULT_SEAT_MONTHLY_DATA_GB`]. Omitted when unset, like
    /// [`Self::seat_memory_budget_mib_v3`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_monthly_data_gb: Option<u32>,

    /// Data filesystem size, in GB (10^9 bytes), an MG needs per seat. An MG
    /// sells at most its data filesystem's size divided by this. Optional on
    /// the wire: absent means [`DEFAULT_SEAT_DISK_GB`]. Omitted when unset,
    /// like [`Self::seat_monthly_data_gb`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_disk_gb: Option<u32>,

    /// [`Self::seat_disk_gb`] for an MG whose data filesystem is btrfs.
    /// Optional on the wire: absent means [`DEFAULT_SEAT_DISK_GB_BTRFS`],
    /// whatever [`Self::seat_disk_gb`] says. Omitted when unset, like
    /// [`Self::seat_disk_gb`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_disk_gb_btrfs: Option<u32>,

    /// Seats one MG before 0.1.16 may host. This release ignores it and
    /// reads [`Self::max_seats_per_mg_v2`]. Admission still checks its bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_seats_per_mg: Option<u32>,

    /// Available memory, in MiB, an MG from 0.1.16 to 0.1.19 needs per seat.
    /// This release ignores it and reads [`Self::seat_memory_budget_mib_v3`],
    /// so Fedi can give older MGs a different budget. Admission still checks
    /// its range, which older consumers enforce. Omitted when unset, like
    /// [`Self::seat_memory_budget_mib`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_memory_budget_mib_v2: Option<u32>,

    /// Available memory, in MiB, an MG needs per seat. Optional on the
    /// wire: absent means [`DEFAULT_SEAT_MEMORY_BUDGET_MIB`], whatever
    /// [`Self::seat_memory_budget_mib`] and
    /// [`Self::seat_memory_budget_mib_v2`] say. Omitted when unset, like
    /// [`Self::seat_memory_budget_mib`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_memory_budget_mib_v3: Option<u32>,

    /// Seats one MG may host, below its operator's own limit. Optional on the
    /// wire: absent means [`DEFAULT_MAX_SEATS_PER_MG`], whatever
    /// [`Self::max_seats_per_mg`] says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_seats_per_mg_v2: Option<u32>,

    /// CPU capacity, in iterations per second of version 1 of the MG CPU
    /// benchmark, an MG needs per seat. A later benchmark version needs its
    /// own field. Optional on the wire: absent means no CPU seat limit. Unlike
    /// the memory budget it has no default, so an MG applies no CPU limit
    /// until Fedi publishes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_cpu_iterations_per_sec: Option<u32>,

    /// CPU, IO and memory pressure averages above which an MG stops accepting
    /// new seats. Optional on the wire: absent, [`MaxPressurePercent::default`]
    /// applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pressure_percent: Option<MaxPressurePercent>,

    /// Whether MGs start new direct-proof guardian transfers. Optional on the wire:
    /// absent, the Manifold environment's default applies (on in development
    /// and staging, off in production). Transfers an MG already stored run to
    /// completion either way
    /// ([`SPEC-guardian-transfer`](../../mg/specs/SPEC-guardian-transfer.md)).
    ///
    /// The retired `guardian_transfer` field must never be true in production:
    /// older consumers use it to enable the incompatible proofless RPC shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guardian_transfer_v2: Option<bool>,

    /// Whether MGs offer the guardian link: the dashboard's Mobile notifications page
    /// and notifications to a linked phone. Optional on the wire: absent, the
    /// Manifold environment's default applies (on in development and staging,
    /// off in production)
    /// ([`SPEC-guardian-link`](../../mg/specs/SPEC-guardian-link.md)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guardian_link: Option<bool>,

    /// Oldest Manifold Guardian release that may advertise new seats.
    /// Optional on the wire: absent means no minimum. An MG below it keeps
    /// running its seats but stops selling new ones until it is updated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_mg_version: Option<MgVersion>,

    /// Oldest Manifold Guardian release Fedi recommends. Optional on the
    /// wire: absent means no recommendation. An MG below it tells its
    /// operator to update by notifying the linked phone once
    /// ([`SPEC-guardian-link`](../../mg/specs/SPEC-guardian-link.md)) and
    /// otherwise runs as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_min_mg_version: Option<MgVersion>,
}

/// Semantically admitted common payment-federation set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedSetupPaymentFederations {
    /// Unique invites keyed by their canonical derived federation IDs.
    federations: BTreeMap<FederationId, InviteCode>,
    telemetry_registration_url: Url,
    unbadged_telemetry_registration_url: Option<Url>,
    min_fee_ppm: u64,
    features: AdmittedFeaturePolicy,
}

/// Semantically checked feature snapshot. Contains no payment fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedFeaturePolicy {
    /// Latest Manifold Guardian release published with this set.
    mg_version: MgVersion,

    min_uptime_percent: MinUptimePercent,
    max_guardian_offline_secs: u64,
    support_nostr_pubkey: Option<nostr::PublicKey>,
    seat_memory_budget_mib: u32,
    seat_monthly_data_gb: NonZeroU32,
    seat_disk_gb: NonZeroU32,
    seat_disk_gb_btrfs: NonZeroU32,
    max_seats_per_mg: u32,
    seat_cpu_iterations_per_sec: Option<NonZeroU32>,
    max_pressure_percent: MaxPressurePercent,
    guardian_transfer_v2: Option<bool>,
    guardian_link: Option<bool>,
    min_mg_version: Option<MgVersion>,
    recommended_min_mg_version: Option<MgVersion>,
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
    /// ceiling, a minimum uptime above 100%, or a minimum or recommended
    /// MG release above the latest one.
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
        for url in std::iter::once(&content.telemetry_registration_url)
            .chain(&content.unbadged_telemetry_registration_url)
        {
            check_telemetry_registration_url(url)?;
        }

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
            federations,
            telemetry_registration_url: content.telemetry_registration_url,
            unbadged_telemetry_registration_url: content.unbadged_telemetry_registration_url,
            min_fee_ppm: content.min_fee_ppm,
            features: content.legacy_features.admit()?,
        })
    }

    /// Read the non-payment projection of this legacy event.
    pub fn features(&self) -> &AdmittedFeaturePolicy {
        &self.features
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

    /// Return the Fedi endpoint where MGs without a Holder authorization
    /// register, if the policy names one.
    #[must_use]
    pub fn unbadged_telemetry_registration_url(&self) -> Option<&Url> {
        self.unbadged_telemetry_registration_url.as_ref()
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

/// Require a credential-free HTTPS URL with a host and no query or fragment.
fn check_telemetry_registration_url(url: &Url) -> Result<(), SetupPaymentFederationsContentError> {
    let registration_url = url::Url::parse(&url.0)
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
    Ok(())
}

impl FeaturePolicy {
    /// Validate feature limits and release relationships and resolve defaults.
    pub fn admit(self) -> Result<AdmittedFeaturePolicy, SetupPaymentFederationsContentError> {
        let content = self;
        if [
            content.seat_memory_budget_mib,
            content.seat_memory_budget_mib_v2,
            content.seat_memory_budget_mib_v3,
        ]
        .into_iter()
        .flatten()
        .any(|mib| !SETUP_PAYMENT_SEAT_MEMORY_BUDGET_MIB_RANGE.contains(&mib))
        {
            return Err(SetupPaymentFederationsContentError::SeatMemoryBudgetOutOfRange);
        }
        let seat_cpu_iterations_per_sec = content
            .seat_cpu_iterations_per_sec
            .map(|iterations| {
                SETUP_PAYMENT_SEAT_CPU_ITERATIONS_PER_SEC_RANGE
                    .contains(&iterations)
                    .then(|| NonZeroU32::new(iterations))
                    .flatten()
                    .ok_or(SetupPaymentFederationsContentError::SeatCpuBudgetOutOfRange)
            })
            .transpose()?;

        // The range starts at 1, so an admitted figure is never zero.
        let seat_monthly_data_gb = match content.seat_monthly_data_gb {
            None => DEFAULT_SEAT_MONTHLY_DATA,
            Some(gb) if SETUP_PAYMENT_SEAT_MONTHLY_DATA_GB_RANGE.contains(&gb) => {
                NonZeroU32::new(gb).expect("the range starts at 1")
            }
            Some(_) => return Err(SetupPaymentFederationsContentError::SeatMonthlyDataOutOfRange),
        };
        let seat_disk = |figure: Option<u32>, default| match figure {
            None => Ok(default),
            Some(gb) if SETUP_PAYMENT_SEAT_DISK_GB_RANGE.contains(&gb) => {
                Ok(NonZeroU32::new(gb).expect("the range starts at 1"))
            }
            Some(_) => Err(SetupPaymentFederationsContentError::SeatDiskOutOfRange),
        };
        let seat_disk_gb = seat_disk(content.seat_disk_gb, DEFAULT_SEAT_DISK)?;
        let seat_disk_gb_btrfs = seat_disk(content.seat_disk_gb_btrfs, DEFAULT_SEAT_DISK_BTRFS)?;
        if [content.max_seats_per_mg, content.max_seats_per_mg_v2]
            .into_iter()
            .flatten()
            .any(|seats| seats > SETUP_PAYMENT_MAX_SEATS_PER_MG)
        {
            return Err(SetupPaymentFederationsContentError::MaxSeatsPerMgTooHigh);
        }
        let max_pressure_percent = content.max_pressure_percent.unwrap_or_default();
        if !max_pressure_percent.cpu.admissible()
            || !max_pressure_percent.io.admissible()
            || !max_pressure_percent.memory.admissible()
        {
            return Err(SetupPaymentFederationsContentError::InvalidMaxPressure);
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

        let mg_version = content.mg_version;
        if content
            .min_mg_version
            .as_ref()
            .is_some_and(|minimum| mg_version.is_older_than(minimum))
        {
            return Err(SetupPaymentFederationsContentError::MinMgVersionAboveLatest);
        }
        if content
            .recommended_min_mg_version
            .as_ref()
            .is_some_and(|recommended| mg_version.is_older_than(recommended))
        {
            return Err(SetupPaymentFederationsContentError::RecommendedMinMgVersionAboveLatest);
        }
        Ok(AdmittedFeaturePolicy {
            mg_version,
            min_uptime_percent,
            max_guardian_offline_secs,
            support_nostr_pubkey: content.support_nostr_pubkey,
            seat_memory_budget_mib: content
                .seat_memory_budget_mib_v3
                .unwrap_or(DEFAULT_SEAT_MEMORY_BUDGET_MIB),
            seat_monthly_data_gb,
            seat_disk_gb,
            seat_disk_gb_btrfs,
            max_seats_per_mg: content
                .max_seats_per_mg_v2
                .unwrap_or(DEFAULT_MAX_SEATS_PER_MG),
            seat_cpu_iterations_per_sec,
            max_pressure_percent,
            guardian_transfer_v2: content.guardian_transfer_v2,
            guardian_link: content.guardian_link,
            min_mg_version: content.min_mg_version,
            recommended_min_mg_version: content.recommended_min_mg_version,
        })
    }
}

impl AdmittedFeaturePolicy {
    /// Select one complete snapshot, using the legacy projection only before feature admission.
    pub fn resolve(
        snapshot: Option<Self>,
        legacy: Option<&AdmittedSetupPaymentFederations>,
    ) -> Option<Self> {
        snapshot.or_else(|| legacy.map(|payment| payment.features().clone()))
    }

    /// Return the latest Manifold Guardian release published with this set.
    #[must_use]
    pub fn mg_version(&self) -> &MgVersion {
        &self.mg_version
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
    }

    /// Return the per-seat CPU budget in benchmark iterations per second, or
    /// `None` when the publication sets no CPU seat limit.
    #[must_use]
    pub fn seat_cpu_iterations_per_sec(&self) -> Option<NonZeroU32> {
        self.seat_cpu_iterations_per_sec
    }

    /// Return the network data one seat uses, in GB per 30 days.
    #[must_use]
    pub fn seat_monthly_data_gb(&self) -> NonZeroU32 {
        self.seat_monthly_data_gb
    }

    /// Return the data filesystem size one seat needs, in GB.
    #[must_use]
    pub fn seat_disk_gb(&self) -> NonZeroU32 {
        self.seat_disk_gb
    }

    /// Return the data filesystem size one seat needs on btrfs, in GB.
    #[must_use]
    pub fn seat_disk_gb_btrfs(&self) -> NonZeroU32 {
        self.seat_disk_gb_btrfs
    }

    /// Return how many seats one MG may host.
    #[must_use]
    pub fn max_seats_per_mg(&self) -> u32 {
        self.max_seats_per_mg
    }

    /// Return the maximum CPU, IO and memory pressure averages for new seats; an
    /// event that omits the field yields [`MaxPressurePercent::default`].
    #[must_use]
    pub fn max_pressure_percent(&self) -> MaxPressurePercent {
        self.max_pressure_percent
    }

    /// Return the published direct-proof guardian-transfer flag. `None` when the event
    /// omits it; the Manifold environment then supplies the default
    /// (`ManifoldEnvironment::guardian_transfer_v2`).
    #[must_use]
    pub fn guardian_transfer_v2(&self) -> Option<bool> {
        self.guardian_transfer_v2
    }

    /// Return the published guardian-link flag. `None` when the event omits
    /// it; the Manifold environment then supplies the default
    /// (`ManifoldEnvironment::guardian_link`).
    #[must_use]
    pub fn guardian_link(&self) -> Option<bool> {
        self.guardian_link
    }

    /// Return the oldest MG release that may advertise new seats, or `None`
    /// when the event sets no minimum.
    #[must_use]
    pub fn min_mg_version(&self) -> Option<&MgVersion> {
        self.min_mg_version.as_ref()
    }

    /// Return the oldest MG release Fedi recommends, or `None` when the
    /// event recommends none.
    #[must_use]
    pub fn recommended_min_mg_version(&self) -> Option<&MgVersion> {
        self.recommended_min_mg_version.as_ref()
    }
}

/// Failure while parsing or semantically admitting publication content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyContentError {
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

    /// The published per-seat CPU budget is outside the admitted range.
    SeatCpuBudgetOutOfRange,

    /// The published per-seat monthly data figure is outside the admitted range.
    SeatMonthlyDataOutOfRange,

    /// The published per-seat disk figure is outside the admitted range.
    SeatDiskOutOfRange,

    /// The published per-MG seat limit is above the admitted maximum.
    MaxSeatsPerMgTooHigh,

    /// A published maximum pressure is outside the admitted range.
    InvalidMaxPressure,

    /// A published minimum uptime is above 100%, which no MG can reach.
    MinUptimeTooHigh,

    /// The published minimum MG release is above the latest release the
    /// same event names, which no MG can run.
    MinMgVersionAboveLatest,

    /// The published recommended MG release is above the latest release the
    /// same event names, which no MG can update to.
    RecommendedMinMgVersionAboveLatest,
    /// An absence limit must allow a positive amount of time.
    ZeroOfflineDuration,
}

/// Backward-compatible name for payment-content callers.
pub use PolicyContentError as SetupPaymentFederationsContentError;

impl core::fmt::Display for PolicyContentError {
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
            Self::SeatCpuBudgetOutOfRange => {
                "setup-payment federation set sets a per-seat CPU budget outside the admitted range"
            }
            Self::SeatMonthlyDataOutOfRange => {
                "setup-payment federation set sets a per-seat monthly data figure outside the admitted range"
            }
            Self::SeatDiskOutOfRange => {
                "setup-payment federation set sets a per-seat disk figure outside the admitted range"
            }
            Self::MaxSeatsPerMgTooHigh => {
                "setup-payment federation set sets a per-MG seat limit above the admitted maximum"
            }
            Self::InvalidMaxPressure => {
                "setup-payment federation set sets a maximum pressure outside 1-100%"
            }
            Self::ZeroOfflineDuration => "guardian absence limit must be positive",
            Self::MinUptimeTooHigh => {
                "setup-payment federation set sets a minimum uptime above 100%"
            }
            Self::MinMgVersionAboveLatest => {
                "setup-payment federation set sets a minimum MG release above the latest one"
            }
            Self::RecommendedMinMgVersionAboveLatest => {
                "setup-payment federation set sets a recommended MG release above the latest one"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for PolicyContentError {}
