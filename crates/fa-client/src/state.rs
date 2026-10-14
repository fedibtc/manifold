//! Public FA intent and progress state.

use fedi_decentralized_service_manifold_guardian::{
    FederationId, FederationName, FederationSize, FedimintdDkgVersion, FedimintdVersion,
    FedimintdVersionCore, GuardianCode, InviteCode, Locator, MgName, Plan, QuoteId, SeatId,
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

/// Reject a federation name that `form_federation` would refuse.
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

/// Seat purchase request after `fa-client` has resolved every default.
///
/// Stored purchase rows from before the seat pool carried the federation
/// name here too; decoding ignores it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ResolvedSeatPurchase {
    /// Number of seats, and the federation size every quote is priced for.
    pub federation_size: FederationSize,
    /// Requested MG plan family.
    pub plan: PlanPreference,
    /// FA-approved Fedimint release range.
    pub fedimintd_versions: FedimintdVersionRange,
    /// Major/minor/vendor identity shared by every bought seat.
    pub fedimintd_dkg_version: FedimintdDkgVersion,
    /// Optional aggregate spending cap in millisatoshis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_msats: Option<u64>,
}

/// Consumer-observed phase of one seat purchase.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PurchasePhase {
    /// Validating availability and obtaining exact signed quotes.
    Preparing,
    /// Every quote is durable; explicit aggregate payment authorization is
    /// required before any paid quote may be funded.
    AwaitingPaymentReadiness,
    /// Creating or recovering independently paid/free seats.
    AcquiringSeats,
}

/// Consumer-observed phase of one formation, from pool seats to a formed
/// federation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormationPhase {
    /// Guardian codes are being collected from the seats.
    PreparingDkg,
    /// DKG has started.
    DkgUnderway,
    /// Every guardian is running and the common invite is saved.
    DkgComplete,
    /// Every guardian is running; the MG seat-binding directory is being
    /// written to consensus metadata and read back.
    PublishingSeatBindings,
}

impl FormationPhase {
    pub(crate) fn dkg_complete(self) -> bool {
        matches!(self, Self::DkgComplete | Self::PublishingSeatBindings)
    }
}

/// Whether displayed state has been reconciled with its authoritative service.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormationFreshness {
    /// State was reconciled during the current run.
    Fresh,
    /// State came from durable storage and still needs reconciliation.
    Unsynced,
}

/// Stable consumer-facing state of one selected seat.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatPhase {
    /// Locator selected; a quote has not yet been durably recorded.
    Selected,
    /// A paid/refunded or wallet-proven-unstarted row awaits a fresh verified
    /// guardian approval. Its original locator and quote identity remain
    /// durable until the replacement is atomically applied.
    ReplacementRequired,
    /// An exact signed quote is durably recorded.
    QuoteReady,
    /// Payment recovery/funding or `CreateSeat` is in progress.
    Acquiring,
    /// The MG has durably accepted the seat.
    Created,
    /// The MG has returned this seat's guardian code.
    GuardianCodeReady,
    /// DKG has been requested for this seat.
    DkgUnderway,
    /// The seat reports a running federation.
    Running,
}

/// One exact paid quote included in an aggregate authorization prompt.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SeatPaymentRequirement {
    /// Stable selected-seat index.
    pub index: u16,
    /// Badge-vouched identity of the MG the quote pays. Presentation
    /// material; the payment binds to the quote.
    #[serde(rename = "fman_id")]
    pub mg_id: PublicKey,
    /// Exact quote the authorization applies to.
    pub quote_id: QuoteId,
    /// Federation whose wallet will fund the quote.
    pub payment_federation_id: FederationId,
    /// Exact face value requested by the quote.
    pub amount_msats: u64,
}

impl SeatPaymentRequirement {
    /// Stable two-word display name derived from the authenticated MG id.
    ///
    /// Names can collide and never substitute for [`Self::mg_id`] in
    /// identity, trust, or deduplication.
    #[must_use]
    pub fn mg_name(&self) -> MgName {
        MgName::from_mg_id(self.mg_id)
    }
}

/// Opaque binding to the complete quote set displayed to the consumer.
///
/// The consumer must return this value when authorizing payment. A quote
/// replacement changes the binding and invalidates the complete authorization,
/// preventing a delayed UI command from authorizing a different set.
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize)]
pub struct PaymentAuthorizationId(String);

impl PaymentAuthorizationId {
    /// Parse the opaque binding displayed by a prior status response.
    pub fn try_from_opaque(value: String) -> Result<Self, String> {
        validate_canonical_digest(&value, "payment binding")?;
        Ok(Self(value))
    }

    /// Stable opaque representation for an authorization RPC.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_digest(digest: [u8; 32]) -> Self {
        Self(encode_canonical_digest(digest))
    }
}

impl<'de> serde::Deserialize<'de> for PaymentAuthorizationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::try_from_opaque(value).map_err(serde::de::Error::custom)
    }
}

/// Deterministic wallet reservation for one exact aggregate of paid quotes.
///
/// The string is deliberately private: consumers can compare, log, and pass
/// this semantic identifier back to their wallet adapter, but cannot forge a
/// reservation by reinterpreting an unrelated string. `fa-client` derives it
/// from the formation identity and current exact requirements.
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize)]
pub struct PaymentReservationId(String);

impl PaymentReservationId {
    /// Stable opaque representation suitable for a wallet journal key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_digest(digest: [u8; 32]) -> Self {
        Self(encode_canonical_digest(digest))
    }
}

impl<'de> serde::Deserialize<'de> for PaymentReservationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        validate_canonical_digest(&value, "payment binding").map_err(serde::de::Error::custom)?;
        Ok(Self(value))
    }
}

fn encode_canonical_digest(bytes: [u8; 32]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String is infallible");
    }
    encoded
}

fn validate_canonical_digest(value: &str, description: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "{description} must be a 32-byte lowercase hexadecimal digest"
        ));
    }
    Ok(())
}

/// Aggregate payment authorization required from the consumer.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PaymentRequirements {
    /// Binding to this exact, complete set of quote requirements.
    pub authorization_id: PaymentAuthorizationId,
    /// Checked sum of every paid seat requirement.
    pub total_msats: u64,
    /// The intent's aggregate spending cap, when one exists. A parked
    /// action therefore carries both numbers, so a consumer can render an
    /// over-cap total against the configured cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_msats: Option<u64>,
    /// Per-seat, quote-bound requirements.
    pub seats: Vec<SeatPaymentRequirement>,
}

/// Opaque binding to the exact rows that are safe and required to replace.
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize)]
pub struct GuardianReplacementId(String);

impl GuardianReplacementId {
    /// Stable opaque representation for an RPC action token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_digest(digest: [u8; 32]) -> Self {
        Self(encode_canonical_digest(digest))
    }
}

impl<'de> serde::Deserialize<'de> for GuardianReplacementId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        validate_canonical_digest(&value, "guardian replacement binding")
            .map_err(serde::de::Error::custom)?;
        Ok(Self(value))
    }
}

/// One proven-safe seat row included in a replacement action.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GuardianReplacementSeat {
    /// Stable formation seat index.
    pub index: u16,
    /// Badge-vouched identity of the outgoing MG. Presentation material; the
    /// replacement binds to the quote and locator.
    #[serde(rename = "previous_fman_id")]
    pub previous_mg_id: PublicKey,
    /// Exact terminal quote whose payment was released or refunded.
    pub previous_quote_id: QuoteId,
    /// Original locator retained for audit and exclusion from fresh selection.
    pub previous_locator: Locator,
}

impl GuardianReplacementSeat {
    /// Stable two-word display name derived from the outgoing MG's
    /// authenticated id.
    ///
    /// Names can collide and never substitute for
    /// [`Self::previous_mg_id`] in identity, trust, or deduplication.
    #[must_use]
    pub fn previous_mg_name(&self) -> MgName {
        MgName::from_mg_id(self.previous_mg_id)
    }
}

/// Complete post-output subset that requires renewed guardian approval.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GuardianReplacementRequirements {
    /// Binding to the exact durable replacement rows.
    pub replacement_id: GuardianReplacementId,
    /// Rows safe to replace; accepted, paid, prepared, and ambiguous siblings
    /// are never included.
    pub seats: Vec<GuardianReplacementSeat>,
}

/// Explicit consumer action required before a purchase can continue.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PurchaseActionRequired {
    /// Authorize all exact paid quotes as one decision.
    AuthorizePayments(PaymentRequirements),
    /// Select and approve fresh verified guardians for only proven-safe rows.
    ReplaceGuardians(GuardianReplacementRequirements),
}

/// Latest progress for one selected MG seat.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SeatProgress {
    /// Stable selected-seat index.
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
    /// Current freshness.
    pub freshness: FormationFreshness,
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

/// Latest snapshot of the active seat purchase.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SeatPurchaseSnapshot {
    /// Active purchase id.
    pub purchase_id: FormationId,
    /// Resolved durable request.
    pub request: ResolvedSeatPurchase,
    /// Current aggregate phase.
    pub phase: PurchasePhase,
    /// Per-MG progress.
    pub seats: Vec<SeatProgress>,
    /// Aggregate freshness.
    pub freshness: FormationFreshness,
    /// Explicit aggregate consumer action, if any.
    pub action_required: Option<PurchaseActionRequired>,
    /// Whether the durable wallet-output-generation boundary has been armed.
    /// Once true, payer switching and value-destructive abandon are forbidden;
    /// resume must recover the exact quote operations.
    pub payment_outputs_started: bool,
    /// Most recent operation error category.
    pub last_error: Option<FaErrorCode>,
}

/// One bought seat that hosts no guardian yet.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PoolSeat {
    /// MG-minted seat id.
    pub seat_id: SeatId,
    /// Badge-vouched identity of the MG that hosts the seat.
    #[serde(rename = "fman_id")]
    pub mg_id: PublicKey,
    /// Protocol-owned locator for the MG.
    pub locator: Locator,
    /// Exact Fedimint build the seat was bought for.
    pub fedimintd_version: FedimintdVersion,
    /// Federation size the seat was priced for.
    pub federation_size: FederationSize,
}

impl PoolSeat {
    /// Stable two-word display name derived from the authenticated MG id.
    #[must_use]
    pub fn mg_name(&self) -> MgName {
        MgName::from_mg_id(self.mg_id)
    }
}

/// Latest snapshot of a formation that has not formed yet.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FormationSnapshot {
    /// Active record id.
    pub formation_id: FormationId,
    /// Federation display name.
    pub federation_name: FederationName,
    /// Major/minor/vendor identity shared by every guardian.
    pub fedimintd_dkg_version: FedimintdDkgVersion,
    /// Current aggregate phase.
    pub phase: FormationPhase,
    /// Per-MG progress.
    pub seats: Vec<SeatProgress>,
    /// Aggregate freshness.
    pub freshness: FormationFreshness,
    /// Join deliverable once every seat reports running.
    pub invite_code: Option<InviteCode>,
    /// Most recent operation error category.
    pub last_error: Option<FaErrorCode>,
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
    /// Whether this run verified the federation against consensus.
    pub freshness: FormationFreshness,
    /// False only for an imported backup that has not been reconciled
    /// against consensus yet.
    pub reconciled: bool,
    /// Error from the last failed recheck.
    pub last_error: Option<FaErrorCode>,
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
/// run. A failed read is retryable; the worker remains active.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct RecoveryCheck {
    pub last_error: Option<FaErrorCode>,
}

/// Complete FA state. Each part has its own machine and storage.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FaStatus {
    /// Present while a restored mnemonic waits for its backup check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryCheck>,
    /// The active seat purchase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase: Option<SeatPurchaseSnapshot>,
    /// Bought seats that host no guardian yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pool: Vec<PoolSeat>,
    /// The active formation, until it forms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formation: Option<FormationSnapshot>,
    /// The formed or restored federation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub federation: Option<FederationSnapshot>,
}
