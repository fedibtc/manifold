//! Public FI intent and progress state.

use fedi_decentralized_service_fleet_manager::{
    FederationId, FederationName, FederationSize, FedimintdDkgVersion, FedimintdVersion,
    FedimintdVersionCore, FmanName, GuardianCode, InviteCode, Locator, Plan, QuoteId, SeatId,
};
use nostr_sdk::PublicKey;

use crate::{FiError, FiErrorCode, FiResult};

/// Smallest product-supported federation size.
pub const MIN_FEDERATION_SIZE: u16 = 7;
/// Largest product-supported federation size.
pub const MAX_FEDERATION_SIZE: u16 = 20;
/// Exclusive upper bound for a product-supported federation size.
pub const MAX_FEDERATION_SIZE_EXCLUSIVE: u16 = MAX_FEDERATION_SIZE + 1;
/// Largest guardian fee the FI proposes, in parts per million (21%): the
/// pinned Fedi payer's own ceiling, not a separate product cap.
pub const MAX_GUARDIAN_FEE_PPM: u32 = 210_000;

/// FI-approved half-open range of three-number Fedimint releases.
///
/// Prerelease and build metadata are intentionally outside these bounds. This
/// policy controls which exact releases the FI accepts; DKG compatibility is
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
    type Error = FiError;

    fn try_from(value: UncheckedFedimintdVersionRange) -> FiResult<Self> {
        Self::from_cores(value.minimum, value.maximum_exclusive)
    }
}

impl FedimintdVersionRange {
    /// Construct `[minimum, maximum_exclusive)` from two Fedimint versions.
    ///
    /// Any prerelease suffixes on the bounds are ignored.
    pub fn new(minimum: FedimintdVersion, maximum_exclusive: FedimintdVersion) -> FiResult<Self> {
        Self::from_cores(minimum.core(), maximum_exclusive.core())
    }

    /// Construct a range directly from three-number releases.
    pub fn from_cores(
        minimum: FedimintdVersionCore,
        maximum_exclusive: FedimintdVersionCore,
    ) -> FiResult<Self> {
        let range = Self {
            minimum,
            maximum_exclusive,
        };
        range.validate()?;
        Ok(range)
    }

    fn validate(&self) -> FiResult<()> {
        if self.minimum >= self.maximum_exclusive {
            return Err(FiError::InvalidIntent(
                "fedimintd version range must have a lower minimum than maximum".to_owned(),
            ));
        }
        Ok(())
    }

    /// Range containing exactly one patch release.
    pub fn one_core(core: FedimintdVersionCore) -> FiResult<Self> {
        let maximum_exclusive = FedimintdVersionCore {
            major: core.major,
            minor: core.minor,
            patch: core.patch.checked_add(1).ok_or_else(|| {
                FiError::InvalidIntent("fedimintd release patch cannot be ranged".to_owned())
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

    /// Whether one exact FMan build lies inside this release range.
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
/// One variant, because FMans offer one plan: free seats left the plan
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
    /// Whether an FMan-offered plan satisfies this preference. The one
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

/// Consumer-owned federation formation request.
///
/// A missing name asks `fi-client` to generate a human-friendly default. The
/// resolved name is persisted before any stateful formation effect and is exposed
/// in [`FormationSnapshot::intent`]. Deserialization rejects unknown object
/// fields and values that violate the name, size, or spending-cap
/// invariants. Construction
/// returns [`FiError::InvalidIntent`] unless the optional name contains 1..=128
/// UTF-8 bytes, includes non-whitespace, and has no control characters; the size
/// is 7..20; and the optional spending cap is greater than zero.
///
/// The intent deliberately carries no guardian-fee rate: formation installs the
/// initial compiled fee policy, while post-formation [`propose_guardian_fees`]
/// changes its rate explicitly. Because the schema is strict, a payload still carrying
/// the retired `guardian_fee_ppm` field is rejected as an unknown field — the
/// intended pre-launch behavior for this namespace.
///
/// [`propose_guardian_fees`]: crate::FiClient::propose_guardian_fees
///
/// The serialized form stays a strict schema: unknown fields are rejected.
/// `max_total_msats` evolved as a new optional field with a default — an
/// older serialized intent without the field decodes to no cap, and a
/// capless intent serializes without the field, so independently serialized
/// public intent values remain interoperable. This does not migrate durable
/// FI store records. Those are separate: schema 11 requires explicit creation mode,
/// commercial-history, and wallet-output tombstones; every older pre-launch
/// record is rejected fail-closed with reset guidance.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "UncheckedFormationIntent")]
pub struct FormationIntent {
    /// Optional federation display name.
    federation_name: Option<FederationName>,
    /// Requested guardian count.
    federation_size: FederationSize,
    /// Requested FMan plan family.
    plan: PlanPreference,
    /// FI-approved Fedimint release range.
    fedimintd_versions: FedimintdVersionRange,
    /// Optional aggregate spending cap in millisatoshis.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_total_msats: Option<u64>,
}

impl FormationIntent {
    /// Construct an intent with an FI-controlled compatibility range.
    pub fn new(
        federation_name: Option<FederationName>,
        federation_size: FederationSize,
        plan: PlanPreference,
        fedimintd_versions: FedimintdVersionRange,
    ) -> FiResult<Self> {
        fedimintd_versions.validate()?;
        if let Some(name) = &federation_name {
            validate_federation_name(name)?;
        }
        if !(MIN_FEDERATION_SIZE..MAX_FEDERATION_SIZE_EXCLUSIVE).contains(&federation_size.0) {
            return Err(FiError::InvalidIntent(format!(
                "federation size must be between {MIN_FEDERATION_SIZE} and \
                 {MAX_FEDERATION_SIZE}, inclusive"
            )));
        }
        Ok(Self {
            federation_name,
            federation_size,
            plan,
            fedimintd_versions,
            max_total_msats: None,
        })
    }

    /// Set an aggregate spending cap in millisatoshis.
    ///
    /// When present, the engine self-authorizes only the initial aggregate
    /// paid quote set when its checked total is within the cap and no prior
    /// aggregate authorization was recorded. Generic quote changes after that
    /// authorization re-park for explicit review. A proven-safe guardian
    /// replacement has a separate fresh, verified and sealed approval; its
    /// exact replacement subset self-authorizes when the new total is within
    /// that renewed cap and otherwise exposes a new payment action. Returns
    /// [`FiError::InvalidIntent`] for a zero cap: a cap that can never admit
    /// a paid quote set is a consumer bug, not a preference.
    pub fn with_max_total_msats(mut self, max_total_msats: u64) -> FiResult<Self> {
        if max_total_msats == 0 {
            return Err(FiError::InvalidIntent(
                "spending cap must be greater than zero millisatoshis".to_owned(),
            ));
        }
        self.max_total_msats = Some(max_total_msats);
        Ok(self)
    }

    /// Return the optional aggregate spending cap in millisatoshis.
    pub fn max_total_msats(&self) -> Option<u64> {
        self.max_total_msats
    }

    /// Return the optional consumer-supplied federation name.
    pub fn federation_name(&self) -> Option<&FederationName> {
        self.federation_name.as_ref()
    }

    /// Return the requested federation size.
    pub fn federation_size(&self) -> FederationSize {
        self.federation_size
    }

    /// Return the requested plan family.
    pub fn plan(&self) -> PlanPreference {
        self.plan
    }

    /// Return the FI-approved Fedimint release range.
    pub fn fedimintd_versions(&self) -> &FedimintdVersionRange {
        &self.fedimintd_versions
    }

    pub(crate) fn resolve_for_dkg(
        self,
        default_name: FederationName,
        dkg: FedimintdDkgVersion,
    ) -> FiResult<ResolvedFormationIntent> {
        if !dkg.is_fedi() || !self.fedimintd_versions.overlaps_dkg(&dkg) {
            return Err(FiError::InvalidIntent(
                "selected Fedimint DKG identity is outside the formation intent".to_owned(),
            ));
        }
        let federation_name = self.federation_name.unwrap_or(default_name);
        validate_federation_name(&federation_name)?;
        Ok(ResolvedFormationIntent {
            federation_name,
            federation_size: self.federation_size,
            plan: self.plan,
            fedimintd_versions: self.fedimintd_versions,
            fedimintd_dkg_version: dkg,
            max_total_msats: self.max_total_msats,
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedFormationIntent {
    federation_name: Option<FederationName>,
    federation_size: FederationSize,
    plan: PlanPreference,
    fedimintd_versions: FedimintdVersionRange,
    /// Deliberate serde evolution inside the strict schema: absent means no
    /// cap, so pre-cap serialized intents keep decoding.
    #[serde(default)]
    max_total_msats: Option<u64>,
}

impl TryFrom<UncheckedFormationIntent> for FormationIntent {
    type Error = FiError;

    fn try_from(value: UncheckedFormationIntent) -> Result<Self, Self::Error> {
        let intent = Self::new(
            value.federation_name,
            value.federation_size,
            value.plan,
            value.fedimintd_versions,
        )?;
        match value.max_total_msats {
            Some(max_total_msats) => intent.with_max_total_msats(max_total_msats),
            None => Ok(intent),
        }
    }
}

fn validate_federation_name(name: &FederationName) -> FiResult<()> {
    if name.0.is_empty()
        || name.0.len() > 128
        || name.0.trim().is_empty()
        || name.0.chars().any(char::is_control)
    {
        return Err(FiError::InvalidIntent(
            "federation name must contain 1..=128 UTF-8 bytes, include a non-whitespace \
             character, and contain no Unicode control characters"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Formation intent after `fi-client` has resolved every default.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ResolvedFormationIntent {
    /// Persisted federation display name.
    pub federation_name: FederationName,
    /// Requested guardian count.
    pub federation_size: FederationSize,
    /// Requested FMan plan family.
    pub plan: PlanPreference,
    /// FI-approved Fedimint release range.
    pub fedimintd_versions: FedimintdVersionRange,
    /// Major/minor/vendor identity shared by every FMan in this DKG.
    pub fedimintd_dkg_version: FedimintdDkgVersion,
    /// Optional aggregate spending cap in millisatoshis, persisted with the
    /// intent so it survives resume. The serde default supports standalone
    /// public intent values; pre-tombstone stored formations are rejected by
    /// the FI storage schema rather than migrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_msats: Option<u64>,
}

/// Consumer-observed aggregate formation phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormationPhase {
    /// Validating availability and obtaining exact signed quotes.
    Preparing,
    /// Every quote is durable; explicit aggregate payment authorization is
    /// required before any paid quote may be funded.
    AwaitingPaymentReadiness,
    /// Creating or recovering independently paid/free seats.
    AcquiringSeats,
    /// Every seat exists and guardian DKG inputs are being prepared.
    PreparingDkg,
    /// DKG has started.
    DkgUnderway,
    /// Every guardian is running; the FMan seat-binding directory is being
    /// written to consensus metadata and read back.
    PublishingSeatBindings,
    /// Federation is running and joinable.
    Formed,
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
    /// The FMan has durably accepted the seat.
    Created,
    /// The FMan has returned this seat's guardian code.
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
    /// Badge-vouched identity of the FMan the quote pays, absent for a
    /// pinned FMan. Presentation material; the payment binds to the quote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fman_id: Option<PublicKey>,
    /// Exact quote the authorization applies to.
    pub quote_id: QuoteId,
    /// Federation whose wallet will fund the quote.
    pub payment_federation_id: FederationId,
    /// Exact face value requested by the quote.
    pub amount_msats: u64,
}

impl SeatPaymentRequirement {
    /// Stable two-word display name derived from the authenticated FMan id,
    /// when one exists.
    ///
    /// Names can collide and never substitute for [`Self::fman_id`] in
    /// identity, trust, or deduplication.
    #[must_use]
    pub fn fman_name(&self) -> Option<FmanName> {
        self.fman_id.map(FmanName::from_fman_id)
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
/// reservation by reinterpreting an unrelated string. `fi-client` derives it
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
    /// Badge-vouched identity of the outgoing FMan, absent for a pinned FMan.
    /// Presentation material; the replacement binds to the quote and locator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_fman_id: Option<PublicKey>,
    /// Exact terminal quote whose payment was released or refunded.
    pub previous_quote_id: QuoteId,
    /// Original locator retained for audit and exclusion from fresh selection.
    pub previous_locator: Locator,
}

impl GuardianReplacementSeat {
    /// Stable two-word display name derived from the outgoing FMan's
    /// authenticated id, when one exists.
    ///
    /// Names can collide and never substitute for
    /// [`Self::previous_fman_id`] in identity, trust, or deduplication.
    #[must_use]
    pub fn previous_fman_name(&self) -> Option<FmanName> {
        self.previous_fman_id.map(FmanName::from_fman_id)
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

/// Explicit consumer action required before formation can continue.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormationActionRequired {
    /// Authorize all exact paid quotes as one decision.
    AuthorizePayments(PaymentRequirements),
    /// Select and approve fresh verified guardians for only proven-safe rows.
    ReplaceGuardians(GuardianReplacementRequirements),
}

/// Latest progress for one selected FMan seat.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SeatProgress {
    /// Stable selected-seat index.
    pub index: u16,
    /// Badge-vouched identity of the currently assigned FMan, absent for a
    /// pinned FMan. Presentation material only: the locator remains the
    /// protocol-owned dialing and verification binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fman_id: Option<PublicKey>,
    /// Protocol-owned locator retained for reconnect and signature
    /// verification.
    pub locator: Locator,
    /// Durable FMan-minted seat id, once creation succeeds.
    pub seat_id: Option<SeatId>,
    /// DKG code returned for this seat, once known.
    pub guardian_code: Option<GuardianCode>,
    /// Stable consumer-facing phase.
    pub phase: SeatPhase,
    /// Current freshness.
    pub freshness: FormationFreshness,
}

impl SeatProgress {
    /// Stable two-word display name derived from the authenticated FMan id,
    /// when one exists.
    ///
    /// Names can collide and never substitute for [`Self::fman_id`] in
    /// identity, trust, or deduplication.
    #[must_use]
    pub fn fman_name(&self) -> Option<FmanName> {
        self.fman_id.map(FmanName::from_fman_id)
    }
}

/// Latest aggregate formation snapshot.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FormationSnapshot {
    /// Active record id.
    pub formation_id: FormationId,
    /// Resolved durable intent.
    pub intent: ResolvedFormationIntent,
    /// Current aggregate phase.
    pub phase: FormationPhase,
    /// Per-FMan progress.
    pub seats: Vec<SeatProgress>,
    /// Aggregate freshness.
    pub freshness: FormationFreshness,
    /// Explicit aggregate consumer action, if any.
    pub action_required: Option<FormationActionRequired>,
    /// Whether the durable wallet-output-generation boundary has been armed.
    /// Once true, payer switching and value-destructive abandon are forbidden;
    /// resume must recover the exact quote operations.
    #[serde(default)]
    pub payment_outputs_started: bool,
    /// Join deliverable once every seat reports running.
    pub invite_code: Option<InviteCode>,
    /// Most recent operation error category.
    pub last_error: Option<FiErrorCode>,
}

/// Complete FI state. Invalid “active formation without id/intent” shapes are
/// unrepresentable.
///
/// This consumer-facing snapshot is returned infrequently, so preserving the
/// direct, ergonomic `FormationSnapshot` payload is preferable to boxing it.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FiStatus {
    /// No durable formation exists.
    #[default]
    Idle,
    /// One active or completed formation exists.
    Formation(FormationSnapshot),
    /// Authenticated backup facts awaiting authoritative reconciliation.
    Restored(RestoredFormationSnapshot),
}

/// Lean recovery facts imported from an authenticated FI backup.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct RestoredFormationSnapshot {
    pub snapshot_generation: u64,
    /// Stable local handle derived when the authenticated backup is imported.
    pub formation_id: FormationId,
    pub federation_invite: InviteCode,
    /// Fresh display metadata, when federation consensus published it.
    pub federation_name: Option<FederationName>,
    pub seats: Vec<RestoredSeat>,
    pub phase: FormationPhase,
    pub freshness: FormationFreshness,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct RestoredSeat {
    pub fman_identity: PublicKey,
    pub seat_id: SeatId,
    pub locator: Locator,
}
