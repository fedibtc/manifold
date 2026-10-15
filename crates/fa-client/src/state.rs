//! Public FA intent and progress state.

use fedi_decentralized_service_manifold_guardian::{
    FederationName, FederationSize, FedimintdDkgVersion, FedimintdVersion, FedimintdVersionCore,
    GuardianCode, InviteCode, Locator, MgName, Plan, SeatId,
};
use nostr_sdk::PublicKey;

use crate::{FaError, FaErrorCode, FaResult};

/// Smallest product-supported federation size.
pub const MIN_FEDERATION_SIZE: u16 = 7;
/// Largest product-supported federation size.
pub const MAX_FEDERATION_SIZE: u16 = 20;
/// Exclusive upper bound for a product-supported federation size.
pub const MAX_FEDERATION_SIZE_EXCLUSIVE: u16 = MAX_FEDERATION_SIZE + 1;
/// Largest guardian fee the FA proposes, in parts per million (21%): the
/// pinned Fedi payer's own ceiling, not a separate product cap.
pub const MAX_GUARDIAN_FEE_PPM: u32 = 210_000;

/// FA-approved half-open range of three-number Fedimint releases.
///
/// Prerelease and build metadata are intentionally outside these bounds. This
/// policy controls which exact releases the FA accepts; DKG compatibility is
/// separately based on major/minor/vendor and may span patches in the range.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "UncheckedFedimintdVersionRange")]
pub struct FedimintdVersionRange {
    minimum: FedimintdVersionCore,
    maximum_exclusive: FedimintdVersionCore,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedFedimintdVersionRange {
    minimum: FedimintdVersionCore,
    maximum_exclusive: FedimintdVersionCore,
}

impl TryFrom<UncheckedFedimintdVersionRange> for FedimintdVersionRange {
    type Error = FaError;

    fn try_from(value: UncheckedFedimintdVersionRange) -> FaResult<Self> {
        Self::from_cores(value.minimum, value.maximum_exclusive)
    }
}

impl FedimintdVersionRange {
    /// Construct `[minimum, maximum_exclusive)` from two Fedimint versions.
    ///
    /// Any prerelease suffixes on the bounds are ignored.
    pub fn new(minimum: FedimintdVersion, maximum_exclusive: FedimintdVersion) -> FaResult<Self> {
        Self::from_cores(minimum.core(), maximum_exclusive.core())
    }

    /// Construct a range directly from three-number releases.
    pub fn from_cores(
        minimum: FedimintdVersionCore,
        maximum_exclusive: FedimintdVersionCore,
    ) -> FaResult<Self> {
        let range = Self {
            minimum,
            maximum_exclusive,
        };
        range.validate()?;
        Ok(range)
    }

    fn validate(&self) -> FaResult<()> {
        if self.minimum >= self.maximum_exclusive {
            return Err(FaError::InvalidIntent(
                "fedimintd version range must have a lower minimum than maximum".to_owned(),
            ));
        }
        Ok(())
    }

    /// Range containing exactly one patch release.
    pub fn one_core(core: FedimintdVersionCore) -> FaResult<Self> {
        let maximum_exclusive = FedimintdVersionCore {
            major: core.major,
            minor: core.minor,
            patch: core.patch.checked_add(1).ok_or_else(|| {
                FaError::InvalidIntent("fedimintd release patch cannot be ranged".to_owned())
            })?,
        };
        Self::from_cores(core, maximum_exclusive)
    }

    /// Return the sole patch release when this range contains exactly one.
    #[must_use]
    pub fn only_core(&self) -> Option<FedimintdVersionCore> {
        Self::one_core(self.minimum)
            .ok()
            .filter(|single| single.maximum_exclusive == self.maximum_exclusive)
            .map(|_| self.minimum)
    }

    /// Inclusive lower release bound.
    #[must_use]
    pub fn minimum(&self) -> FedimintdVersionCore {
        self.minimum
    }

    /// Exclusive upper release bound.
    #[must_use]
    pub fn maximum_exclusive(&self) -> FedimintdVersionCore {
        self.maximum_exclusive
    }

    /// Whether one exact MG build lies inside this release range.
    #[must_use]
    pub fn contains(&self, version: &FedimintdVersion) -> bool {
        self.contains_core(version.core())
    }

    /// Whether one three-number release lies inside this range.
    #[must_use]
    pub fn contains_core(&self, core: FedimintdVersionCore) -> bool {
        self.minimum <= core && core < self.maximum_exclusive
    }

    /// Whether this range contains any patch from one DKG major/minor line.
    #[must_use]
    pub fn overlaps_dkg(&self, dkg: &FedimintdDkgVersion) -> bool {
        let line = dkg.major_minor();
        let minimum_line = (self.minimum.major, self.minimum.minor);
        let maximum_line = (self.maximum_exclusive.major, self.maximum_exclusive.minor);
        minimum_line <= line
            && (line < maximum_line || (line == maximum_line && self.maximum_exclusive.patch > 0))
    }
}

/// Stable identifier for one formation record.
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FormationId(pub String);

/// Consumer choice of the currently implemented commercial plan families.
///
/// One variant, because MGs offer one plan: free seats left the plan
/// vocabulary and became an out-of-band admission path. Kept as an enum so
/// the choice has somewhere to land when a second plan ships.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanPreference {
    /// One-time paid, best-effort service.
    #[default]
    InfiniteBestEffort,
}

impl PlanPreference {
    /// Whether an MG-offered plan satisfies this preference. The one
    /// definition: selection (picking a plan out of an advertisement) and
    /// recovery (checking a persisted quote still matches the intent) must
    /// never disagree about what "the intended plan" means.
    pub(crate) fn matches(self, plan: &Plan) -> bool {
        matches!(
            (self, plan),
            (
                PlanPreference::InfiniteBestEffort,
                Plan::InfiniteBestEffort { .. }
            )
        )
    }
}

/// Reject a federation name that `create_federation` would refuse.
pub fn validate_federation_name(name: &FederationName) -> FaResult<()> {
    if name.0.is_empty()
        || name.0.len() > 128
        || name.0.trim().is_empty()
        || name.0.chars().any(char::is_control)
    {
        return Err(FaError::InvalidIntent(
            "federation name must contain 1..=128 UTF-8 bytes, include a non-whitespace \
             character, and contain no Unicode control characters"
                .to_owned(),
        ));
    }
    Ok(())
}

/// What every quote of a running purchase must match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedSeatPurchase {
    /// Number of seats, and the federation size every quote is priced for.
    pub(crate) federation_size: FederationSize,
    /// Requested MG plan family.
    pub(crate) plan: PlanPreference,
    /// FA-approved Fedimint release range.
    pub(crate) fedimintd_versions: FedimintdVersionRange,
    /// Major/minor/vendor identity shared by every bought seat.
    pub(crate) fedimintd_dkg_version: FedimintdDkgVersion,
}

/// Consumer-observed phase of one formation, from pool seats to a formed
/// federation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormationPhase {
    /// Guardian codes are being collected from the seats.
    PreparingDkg,
    /// Every guardian code is saved, so DKG is requested or has started.
    DkgUnderway,
    /// Every guardian is running and the common invite is saved.
    DkgComplete,
    /// Every guardian is running; the MG seat-binding directory is being
    /// written to consensus metadata and read back.
    PublishingSeatBindings,
}

/// Stable consumer-facing state of one selected seat.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatPhase {
    /// Locator selected; a quote has not yet been durably recorded.
    Selected,
    /// An exact signed quote is durably recorded.
    QuoteReady,
    /// The MG has durably accepted the seat.
    Created,
    /// The MG has returned this seat's guardian code.
    GuardianCodeReady,
    /// Every seat of the full pool has its code, so DKG is requested or
    /// has started.
    DkgUnderway,
    /// DKG completed, and the invite is saved.
    Running,
}

/// Latest progress for one selected MG seat.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SeatProgress {
    /// Stable slot id. Ids order the federation's guardians.
    pub index: u16,
    /// Badge-vouched identity of the currently assigned MG. Presentation
    /// material only: the locator remains the protocol-owned dialing and
    /// verification binding.
    #[serde(rename = "fman_id")]
    pub mg_id: PublicKey,
    /// Protocol-owned locator retained for reconnect and signature
    /// verification.
    pub locator: Locator,
    /// Durable MG-minted seat id, once creation succeeds.
    pub seat_id: Option<SeatId>,
    /// DKG code returned for this seat, once known.
    pub guardian_code: Option<GuardianCode>,
    /// Stable consumer-facing phase.
    pub phase: SeatPhase,
}

impl SeatProgress {
    /// Stable two-word display name derived from the authenticated MG id.
    ///
    /// Names can collide and never substitute for [`Self::mg_id`] in
    /// identity, trust, or deduplication.
    #[must_use]
    pub fn mg_name(&self) -> MgName {
        MgName::from_mg_id(self.mg_id)
    }
}

/// The federation this FA governs, formed here or restored from a backup.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FederationSnapshot {
    /// Stable local handle.
    pub formation_id: FormationId,
    pub federation_invite: InviteCode,
    /// Display name, when known.
    pub federation_name: Option<FederationName>,
    pub seats: Vec<FederationSeat>,
}

/// One guardian seat of the federation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FederationSeat {
    /// Badge-vouched identity of the MG.
    #[serde(rename = "fman_id")]
    pub mg_id: PublicKey,
    pub seat_id: SeatId,
    pub locator: Locator,
}

/// A restored mnemonic waits for its backup check before FA operations may
/// run. The run loop retries a failed check.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct RecoveryCheck {
    pub last_error: Option<FaErrorCode>,
}

/// The internal parts of FA state, each with its own machine and storage.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct FaStatus {
    /// Present while a restored mnemonic waits for its backup check.
    pub(crate) recovery: Option<RecoveryCheck>,
    /// The running federation creation.
    pub(crate) creation: Option<CreationSnapshot>,
    /// The formed or restored federation.
    pub(crate) federation: Option<FederationSnapshot>,
}

impl FaStatus {
    /// The single state a consumer observes.
    pub(crate) fn state(&self) -> FaState {
        if let Some(recovery) = &self.recovery {
            return FaState::Restoring(recovery.clone());
        }
        if let Some(creation) = &self.creation {
            return FaState::Creating(Box::new(creation.clone()));
        }
        match &self.federation {
            Some(federation) => FaState::Active(Box::new(federation.clone())),
            None => FaState::Idle,
        }
    }
}

/// What the FA is doing now. Exactly one state holds at a time, and
/// [`FaClient::run`](crate::FaClient::run) moves the FA between them.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FaState {
    /// No federation exists and none is being created.
    #[default]
    Idle,
    /// A restored mnemonic waits for its backup check.
    Restoring(RecoveryCheck),
    /// A federation is being created.
    Creating(Box<CreationSnapshot>),
    /// The FA governs a formed or restored federation.
    Active(Box<FederationSnapshot>),
}

/// A federation that is being created.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CreationSnapshot {
    /// Stable local handle, kept by the formed federation.
    pub formation_id: FormationId,
    /// Federation display name.
    pub federation_name: FederationName,
    /// Number of guardians the federation will have.
    pub federation_size: FederationSize,
    /// Major/minor/vendor identity every seat is bought for, once a purchase
    /// started.
    pub fedimintd_dkg_version: Option<FedimintdDkgVersion>,
    /// The guardian slots, in id order. A slot keeps its seat from
    /// acceptance until the federation forms.
    pub slots: Vec<SeatProgress>,
    /// What the creation does now.
    pub stage: CreationStage,
    /// Most recent operation error category.
    pub last_error: Option<FaErrorCode>,
}

/// The stage of a federation creation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CreationStage {
    /// The user must approve a preview of this many more seats.
    NeedsSeats {
        /// Empty slots.
        count: u16,
        /// Why the last slot was emptied, if one was.
        reauthorization: Option<crate::SelectionReauthorizationReason>,
    },
    /// The picked slots without a seat are being bought.
    Buying,
    /// Every slot holds a seat. They run DKG and publish the federation
    /// metadata.
    Forming {
        /// Current formation phase.
        phase: FormationPhase,
        /// Join deliverable once every seat reports running.
        invite_code: Option<InviteCode>,
    },
}
