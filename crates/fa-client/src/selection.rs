//! Ranked MG seat selection with lazy PeerBadge verification.
//!
//! Selection reduces discovery's statically admitted candidate pool to a
//! verified seat set for one formation intent: bucket the pool by claimed
//! issuance key (the region proxy), rank each bucket by advertised setup
//! fee, then fill seats round-robin across buckets.
//! The expensive relay-backed PeerBadge verification runs *inside* this
//! walk, in selection order, so only candidates the walk actually reaches
//! cost verifier round trips; a candidate whose badge fails to verify, to
//! bind to the advertisement author, or to match its claimed issuer is
//! dropped with a typed rejection and the walk continues. Verified authors
//! that advertise a service key already owned by a selected seat are likewise
//! rejected, so one commitment-signing authority cannot occupy multiple
//! seats. When the caller holds an MG connector, each reached, verified,
//! non-duplicate candidate is additionally probed for live availability —
//! the same four-check predicate quoting applies — so a stale advertisement
//! costs its author the seat here instead of invalidating the sealed
//! approval at quote time. The heuristic and its rationale are recorded in
//! `specs/ARCH-fa-client-discovery-selection.md`
//! (resurrecting PR #72); the admission order it extends is
//! `specs/ARCH-fa-client-discovery-selection.md`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use fedi_decentralized_domain::{HolderAuthorizationEnvelope, TrustScoreBadgeV1};
use fedi_decentralized_nostr::mg::{ApiEndpoint, Availability};
use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_peer_badge_verifier::{
    PeerBadgeVerificationError, PeerBadgeVerifier, PeerBadgeVerifierProvenance,
};
use fedi_decentralized_service_manifold_guardian::{
    FederationSize, FedimintdDkgVersion, FedimintdVersion, GetAvailabilityRequest,
    GetAvailabilityResponse, Locator, MgName, Plan, Timestamp,
};
use fedimint_core::runtime::{Instant, sleep_until};
use futures::future::{Either, select};
use nostr_sdk::PublicKey;

use crate::discovery::{
    AdvertisementRejection, EligibleMgCandidate, MG_ADVERTISEMENT_MAX_HOLDER_AUTHORIZATIONS,
    MgCandidateRequirements, MgRegistryQuery, RejectedAdvertisement, discover_mg_candidates_with,
};
use crate::state::{
    FedimintdVersionRange, MAX_FEDERATION_SIZE, MAX_FEDERATION_SIZE_EXCLUSIVE, MIN_FEDERATION_SIZE,
};
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusReader,
    GuardianReplacementRequirements, ManifoldGuardianConnector, PlanPreference,
    SelectionReauthorizationReason,
};

/// How long one freshly fetched advertisement selection may authorize the
/// start of a Pay-and-create operation and each pre-effect resume. Once an
/// admission is effect-authorized, this deadline no longer applies. This
/// freshness allowance does not size the consumer's retry budget.
pub const MG_SELECTION_PREVIEW_VALIDITY: Duration = Duration::from_secs(5 * 60);

/// Per-candidate budget for one live availability probe inside the walk.
///
/// Bounded separately from the walk's absolute deadline so a single
/// unresponsive MG degrades to a typed per-candidate rejection and its
/// bucket continues, instead of silently consuming the remaining preview
/// budget and failing the whole preview.
pub const MG_SELECTION_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Verified facts projected from one authentic PeerBadge envelope.
///
/// This is the selection-facing projection of the shared verifier's
/// `VerifiedPeerBadge`: the issuer, holder, and authorized subject
/// identities plus the typed trust-score badge. Fields are sealed: only the
/// selection walk constructs one, so a consumer can never fabricate a
/// verified trust conclusion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedBadgeFacts {
    /// Trusted issuer identity that signed the backing credential authority.
    pub(crate) issuer: PublicKey,

    /// Holder identity bound into the credential and authorization.
    pub(crate) holder: PublicKey,

    /// Service identity the holder authorized to present the badge.
    pub(crate) subject: PublicKey,

    /// Typed `fedi-trust-score-v1.0` claims.
    pub(crate) badge: TrustScoreBadgeV1,
}

impl VerifiedBadgeFacts {
    /// Trusted issuer identity that signed the backing credential authority.
    #[must_use]
    pub fn issuer(&self) -> PublicKey {
        self.issuer
    }

    /// Holder identity bound into the credential and authorization.
    #[must_use]
    pub fn holder(&self) -> PublicKey {
        self.holder
    }

    /// Service identity the holder authorized to present the badge.
    #[must_use]
    pub fn subject(&self) -> PublicKey {
        self.subject
    }

    /// Typed `fedi-trust-score-v1.0` claims.
    #[must_use]
    pub fn badge(&self) -> &TrustScoreBadgeV1 {
        &self.badge
    }
}

/// One fully verified, currently eligible MG advertisement.
///
/// Fields are sealed: only the selection walk constructs one, so a consumer
/// can never fabricate a verified candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedCandidate {
    /// Authenticated MG identity: the advertisement event author, equal to
    /// the signed payload's `fman_id_pubkey` and to the verified badge
    /// subject.
    pub(crate) mg_id: PublicKey,

    /// Ad-declared pre-formation RPC endpoints; non-trust dialing hints.
    pub(crate) api_endpoints: Vec<ApiEndpoint>,

    /// Dialing locator built during static admission.
    pub(crate) locator: Locator,

    /// Advertised one-time numeric price of the ad's `InfiniteBestEffort`
    /// plan, in millisatoshis.
    pub(crate) advertised_price_msats: u64,

    /// Advertised service compatibility; non-trust hints re-checked live
    /// during a probing selection walk and again at quote time.
    pub(crate) availability: Availability,

    /// Unix seconds at which the MG issued the advertisement.
    pub(crate) issued_at: Timestamp,

    /// Unix seconds after which the advertisement expires.
    pub(crate) expires_at: Timestamp,

    /// Facts from the first embedded envelope that verified, bound to the
    /// event author, and matched the claimed issuer.
    pub(crate) badge: VerifiedBadgeFacts,
}

impl VerifiedCandidate {
    /// Authenticated MG identity: the advertisement event author, equal to
    /// the signed payload's `fman_id_pubkey` and to the verified badge
    /// subject.
    #[must_use]
    pub fn mg_id(&self) -> PublicKey {
        self.mg_id
    }

    /// Stable two-word display name derived from the authenticated MG id.
    ///
    /// Names can collide and never substitute for [`Self::mg_id`] in
    /// identity, trust, or deduplication.
    #[must_use]
    pub fn mg_name(&self) -> MgName {
        MgName::from_mg_id(self.mg_id)
    }

    /// Ad-declared pre-formation RPC endpoints; non-trust dialing hints.
    #[must_use]
    pub fn api_endpoints(&self) -> &[ApiEndpoint] {
        &self.api_endpoints
    }

    /// Dialing locator for this MG, preserved from static admission.
    #[must_use]
    pub fn locator(&self) -> &Locator {
        &self.locator
    }

    /// Advertised one-time price of the ad's `InfiniteBestEffort` plan, in
    /// millisatoshis.
    #[must_use]
    pub fn advertised_price_msats(&self) -> u64 {
        self.advertised_price_msats
    }

    /// Advertised service compatibility; non-trust hints re-checked live
    /// during a probing selection walk and again at quote time.
    #[must_use]
    pub fn availability(&self) -> &Availability {
        &self.availability
    }

    /// Unix seconds at which the MG issued the advertisement.
    #[must_use]
    pub fn issued_at(&self) -> Timestamp {
        self.issued_at
    }

    /// Unix seconds after which the advertisement expires.
    #[must_use]
    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    /// Facts from the first embedded envelope that verified, bound to the
    /// event author, and matched the claimed issuer.
    #[must_use]
    pub fn badge(&self) -> &VerifiedBadgeFacts {
        &self.badge
    }
}

/// Attestation provenance of one selected seat.
///
/// Every MVP seat must be Fedi-attested. The pinned/BYO seat shape is reserved
/// but its intake is deferred (`specs/ARCH-fa-client-discovery-selection.md`), so the
/// current walk only produces Fedi-attested seats; the enum is non-exhaustive
/// so a separately governed future provenance can arrive without a breaking
/// change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SeatProvenance {
    /// Seat verified through a badge from a trusted issuer root.
    FediAttested,
}

impl SeatProvenance {
    /// Stable machine code for consumer serialization and branching.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::FediAttested => "fedi_attested",
        }
    }
}

/// One verified seat produced by the selection walk.
///
/// Fields are sealed: only the selection walk constructs one.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectedMgSeat {
    pub(crate) candidate: VerifiedCandidate,
    pub(crate) provenance: SeatProvenance,
}

/// Stable authenticated identity and current dialing material sealed into an
/// approved selection.
///
/// The Nostr event author is the identity vouched for by PeerBadge.  The
/// locator's service key is only the currently advertised commitment key and
/// can rotate, so replacement identity exclusion uses `fman_id` rather than
/// deriving identity from `locator`. Service-key uniqueness is checked
/// independently to prevent two distinct authors from occupying seats as one
/// commitment-signing authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ApprovedMgSeat {
    pub(crate) mg_id: PublicKey,
    pub(crate) holder: PublicKey,
    pub(crate) locator: Locator,
}

impl From<SelectedMgSeat> for ApprovedMgSeat {
    fn from(seat: SelectedMgSeat) -> Self {
        Self {
            mg_id: seat.candidate.mg_id,
            holder: seat.candidate.badge.holder,
            locator: seat.candidate.locator,
        }
    }
}

impl SelectedMgSeat {
    /// The fully verified candidate seated by the walk.
    #[must_use]
    pub fn candidate(&self) -> &VerifiedCandidate {
        &self.candidate
    }

    /// The advertised `InfiniteBestEffort` price in millisatoshis.
    ///
    /// An informational estimate from the advertisement; the exact signed
    /// quote obtained at formation time is the commercial term.
    #[must_use]
    pub fn advertised_price_msats(&self) -> u64 {
        self.candidate.advertised_price_msats()
    }

    /// Attestation provenance of this seat.
    #[must_use]
    pub fn provenance(&self) -> SeatProvenance {
        self.provenance
    }
}

/// Validated inputs for one read-only selection preview.
///
/// Construction enforces the product federation-size range and the
/// currently supported plan family, mirroring `FormationIntent` discipline,
/// so invalid inputs cannot reach registry or verifier work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MgSelectionRequest {
    federation_size: FederationSize,
    fedimintd_versions: FedimintdVersionRange,
    plan: PlanPreference,
}

/// Read-only verified-selection capability.
///
/// This purpose-specific client owns exactly the registry transport and
/// concrete PeerBadge verifier required for lazy verified selection. It has no
/// FA identity, durable database, payment capability, consensus reader,
/// driver lease, or formation state. Without an MG connector its preview
/// seats candidates on advertised claims alone; [`Self::with_mg_connector`]
/// adds the transport capability that lets the walk probe each reached
/// candidate's live availability before seating it.
#[derive(Clone)]
pub struct MgSelectionQuery<N, F = crate::UnavailableManifoldGuardianConnector> {
    registry: N,
    peer_badge_verifier: PeerBadgeVerifier,
    /// `None` previews on advertised claims alone; `Some` probes live
    /// availability during the walk.
    mg_connector: Option<F>,
}

impl<N> MgRegistryQuery<N> {
    /// Add the trust-verification capability required for selection preview.
    #[must_use]
    pub fn with_verifier(self, peer_badge_verifier: PeerBadgeVerifier) -> MgSelectionQuery<N> {
        MgSelectionQuery {
            registry: self.registry,
            peer_badge_verifier,
            mg_connector: None,
        }
    }
}

impl<N, F> MgSelectionQuery<N, F> {
    /// Add the MG transport capability that upgrades the preview from
    /// advertised claims to live-probed availability.
    ///
    /// The connector carries no identity or durable state: probing stays a
    /// read-only query, but the walk contacts each reached candidate over
    /// this transport before seating it.
    #[must_use]
    pub fn with_mg_connector<F2: ManifoldGuardianConnector>(
        self,
        mg_connector: F2,
    ) -> MgSelectionQuery<N, F2> {
        MgSelectionQuery {
            registry: self.registry,
            peer_badge_verifier: self.peer_badge_verifier,
            mg_connector: Some(mg_connector),
        }
    }
}

impl MgSelectionRequest {
    /// Construct a selection request with an FA-controlled compatibility range.
    ///
    /// # Errors
    ///
    /// Returns [`FaError::InvalidIntent`] for a federation size outside the
    /// product range 7..20 or a plan other than `InfiniteBestEffort` —
    /// free-plan capacity is not yet surfaced by discovery
    /// (`specs/ARCH-fa-client-discovery-selection.md`, *Discovery*).
    pub fn new(
        federation_size: FederationSize,
        fedimintd_versions: FedimintdVersionRange,
        plan: PlanPreference,
    ) -> FaResult<Self> {
        if !(MIN_FEDERATION_SIZE..MAX_FEDERATION_SIZE_EXCLUSIVE).contains(&federation_size.0) {
            return Err(FaError::InvalidIntent(format!(
                "federation size must be between {MIN_FEDERATION_SIZE} and \
                 {MAX_FEDERATION_SIZE}, inclusive"
            )));
        }
        if plan != PlanPreference::InfiniteBestEffort {
            return Err(FaError::InvalidIntent(
                "selection currently supports only the InfiniteBestEffort plan; free capacity \
                 is not yet surfaced by discovery"
                    .to_owned(),
            ));
        }
        Ok(Self {
            federation_size,
            fedimintd_versions,
            plan,
        })
    }

    /// Requested guardian count.
    #[must_use]
    pub fn federation_size(&self) -> FederationSize {
        self.federation_size
    }

    /// FA-approved Fedimint release range.
    #[must_use]
    pub fn fedimintd_versions(&self) -> &FedimintdVersionRange {
        &self.fedimintd_versions
    }

    /// Requested plan family.
    #[must_use]
    pub fn plan(&self) -> PlanPreference {
        self.plan
    }
}

/// Outcome of one read-only selection preview.
///
/// No durable state exists and no seat is reserved. The exact verified set is
/// nevertheless sealed into approval: any later unavailability requires a
/// fresh preview and renewed authorization; Pay-and-create never silently
/// substitutes a different guardian.
#[derive(Debug)]
pub struct MgSelectionPreview {
    /// Complete request whose verified selection this preview represents.
    request: MgSelectionRequest,

    /// Fedimint major/minor/vendor identity shared by every selected MG.
    fedimintd_dkg_version: FedimintdDkgVersion,

    /// Immutable trust configuration used to verify every selected seat.
    verifier_provenance: PeerBadgeVerifierProvenance,

    /// Verified seats in selection (round-robin) order; exactly the
    /// requested federation size.
    seats: Vec<SelectedMgSeat>,

    /// Checked sum of every seat's advertised price, in millisatoshis.
    ///
    /// An estimate from advertisements, not a quoted commercial term.
    total_advertised_msats: u64,

    /// Total advertisements the bounded enumeration observed.
    seen: usize,

    /// Statically admitted, currently eligible candidates before the walk.
    eligible: usize,

    /// Static non-admissions plus typed failures encountered while producing
    /// the chosen cohort. Unneeded candidates and failures from cohorts the
    /// preview did not choose do not appear here.
    rejected: Vec<RejectedAdvertisement>,

    /// Wall-clock deadline used only to reject a stale approval before any
    /// durable, network, or wallet effect.
    valid_until: Timestamp,
}

impl MgSelectionPreview {
    /// Fedimint major/minor/vendor identity shared by every selected MG.
    #[must_use]
    pub fn fedimintd_dkg_version(&self) -> &FedimintdDkgVersion {
        &self.fedimintd_dkg_version
    }

    /// Verified seats in deterministic selection order.
    #[must_use]
    pub fn seats(&self) -> &[SelectedMgSeat] {
        &self.seats
    }

    /// Number of verified seats the walk selected.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.seats.len()
    }

    /// Checked aggregate advertised estimate in millisatoshis.
    #[must_use]
    pub fn total_advertised_msats(&self) -> u64 {
        self.total_advertised_msats
    }

    /// Advertisements observed by the bounded enumeration.
    #[must_use]
    pub fn seen(&self) -> usize {
        self.seen
    }

    /// Statically eligible candidates before the verified walk.
    #[must_use]
    pub fn eligible(&self) -> usize {
        self.eligible
    }

    /// Typed non-admissions and chosen-cohort verification failures.
    #[must_use]
    pub fn rejected(&self) -> &[RejectedAdvertisement] {
        &self.rejected
    }

    /// Unix timestamp at which this uncached preview becomes stale.
    #[must_use]
    pub fn valid_until(&self) -> Timestamp {
        self.valid_until
    }

    /// Bind the displayed verified set to the user's maximum setup spend.
    ///
    /// This is commercial approval, not the irreversible wallet boundary.
    /// The returned sealed value can start Pay-and-create for five minutes;
    /// the wallet-output boundary is durably recorded only immediately before
    /// `FaPayments::create_seat_payment` is polled. A zero limit is valid only
    /// for an all-zero advertisement estimate and can be consumed only by the
    /// no-payer bootstrap entry.
    pub fn approve(self, max_total_msats: u64) -> FaResult<MgSelectionApproval> {
        if max_total_msats < self.total_advertised_msats {
            return Err(FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::AdvertisementEstimateExceedsLimit,
                failure: None,
            });
        }
        Ok(MgSelectionApproval {
            request: self.request,
            fedimintd_dkg_version: self.fedimintd_dkg_version,
            verifier_provenance: self.verifier_provenance,
            seats: self.seats.into_iter().map(ApprovedMgSeat::from).collect(),
            advertised_total_msats: self.total_advertised_msats,
            max_total_msats,
            valid_until: self.valid_until,
        })
    }
}

/// Sealed, short-lived approval of one verified advertisement selection.
///
/// Consumers can retain and return this capability but cannot construct or
/// alter its verified seats. It is intentionally not serializable: a bridge
/// keeps it only for the active five-minute screen flow and refetches when the
/// user backs out and re-enters.
#[derive(Clone, Debug)]
pub struct MgSelectionApproval {
    pub(crate) request: MgSelectionRequest,
    pub(crate) fedimintd_dkg_version: FedimintdDkgVersion,
    pub(crate) verifier_provenance: PeerBadgeVerifierProvenance,
    pub(crate) seats: Vec<ApprovedMgSeat>,
    pub(crate) advertised_total_msats: u64,
    pub(crate) max_total_msats: u64,
    pub(crate) valid_until: Timestamp,
}

impl MgSelectionApproval {
    /// Approved maximum aggregate setup spend.
    #[must_use]
    pub fn max_total_msats(&self) -> u64 {
        self.max_total_msats
    }

    /// Advertisement estimate shown when approval was obtained.
    #[must_use]
    pub fn advertised_total_msats(&self) -> u64 {
        self.advertised_total_msats
    }

    /// Unix timestamp at which Pay-and-create must reject this approval.
    #[must_use]
    pub fn valid_until(&self) -> Timestamp {
        self.valid_until
    }

    pub(crate) fn into_seats_at(self, now: Timestamp) -> FaResult<Vec<ApprovedMgSeat>> {
        if self.valid_until <= now {
            return Err(FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::PreviewExpired,
                failure: None,
            });
        }
        Ok(self.seats)
    }
}

/// Read-only verified selection for only the rows proven safe to replace.
#[derive(Debug)]
pub struct MgReplacementPreview {
    requirements: GuardianReplacementRequirements,
    verifier_provenance: PeerBadgeVerifierProvenance,
    seats: Vec<SelectedMgSeat>,
    total_advertised_msats: u64,
    valid_until: Timestamp,
}

impl MgReplacementPreview {
    /// Exact durable rows this preview replaces.
    #[must_use]
    pub fn requirements(&self) -> &GuardianReplacementRequirements {
        &self.requirements
    }

    /// Fresh verified replacements in stable row order.
    #[must_use]
    pub fn seats(&self) -> &[SelectedMgSeat] {
        &self.seats
    }

    /// Aggregate advertisement estimate for the replacement subset.
    #[must_use]
    pub fn total_advertised_msats(&self) -> u64 {
        self.total_advertised_msats
    }

    /// Unix timestamp at which this uncached replacement preview becomes stale.
    #[must_use]
    pub fn valid_until(&self) -> Timestamp {
        self.valid_until
    }

    /// Seal this exact subset to renewed user authorization.
    pub fn approve(self, max_total_msats: u64) -> FaResult<MgReplacementApproval> {
        if max_total_msats == 0 || max_total_msats < self.total_advertised_msats {
            return Err(FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::AdvertisementEstimateExceedsLimit,
                failure: None,
            });
        }
        Ok(MgReplacementApproval {
            requirements: self.requirements,
            verifier_provenance: self.verifier_provenance,
            seats: self.seats.into_iter().map(ApprovedMgSeat::from).collect(),
            max_total_msats,
            valid_until: self.valid_until,
        })
    }
}

/// Sealed renewed approval for a proven-safe replacement subset.
#[derive(Clone, Debug)]
pub struct MgReplacementApproval {
    pub(crate) requirements: GuardianReplacementRequirements,
    pub(crate) verifier_provenance: PeerBadgeVerifierProvenance,
    pub(crate) seats: Vec<ApprovedMgSeat>,
    pub(crate) max_total_msats: u64,
    pub(crate) valid_until: Timestamp,
}

impl MgReplacementApproval {
    /// Exact replacement action this approval satisfies.
    #[must_use]
    pub fn requirements(&self) -> &GuardianReplacementRequirements {
        &self.requirements
    }

    /// Approved maximum aggregate spend for the replacement subset.
    #[must_use]
    pub fn max_total_msats(&self) -> u64 {
        self.max_total_msats
    }

    /// Unix timestamp at which applying this replacement approval must fail.
    #[must_use]
    pub fn valid_until(&self) -> Timestamp {
        self.valid_until
    }

    pub(crate) fn into_seats_at(self, now: Timestamp) -> FaResult<Vec<ApprovedMgSeat>> {
        if self.valid_until <= now {
            return Err(FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::PreviewExpired,
                failure: None,
            });
        }
        Ok(self.seats)
    }
}

/// Selection-facing PeerBadge verification port.
///
/// The shared concrete verifier performs bounded relay I/O on every call and
/// exposes no test seam of its own, so the walk depends on this narrow
/// trait and tests substitute deterministic outcomes.
pub(crate) trait SelectionBadgeVerifier {
    /// Verify one envelope and project the selection-facing badge facts.
    async fn verify_badge(
        &self,
        envelope: &HolderAuthorizationEnvelope,
    ) -> Result<VerifiedBadgeFacts, PeerBadgeVerificationError>;
}

impl SelectionBadgeVerifier for PeerBadgeVerifier {
    async fn verify_badge(
        &self,
        envelope: &HolderAuthorizationEnvelope,
    ) -> Result<VerifiedBadgeFacts, PeerBadgeVerificationError> {
        self.verify(envelope)
            .await
            .map(|verified| VerifiedBadgeFacts {
                issuer: verified.issuer().0,
                holder: verified.holder().0,
                subject: verified.subject().0,
                badge: verified.badge().clone(),
            })
    }
}

/// Outcome of one live availability probe against a reached candidate.
pub(crate) enum LiveProbeOutcome {
    /// The caller holds no MG transport capability; seat the candidate on
    /// its advertised claims, exactly the pre-probe preview behavior.
    Skipped,

    /// A live availability response to check against the request.
    Available(GetAvailabilityResponse),

    /// The probe produced no usable response. The description comes from the
    /// sanitized-by-contract local connector error types or is a fixed
    /// marker for a Manifold Guardian-returned error; the MG's own error text
    /// is never embedded.
    Unreachable(String),
}

/// Selection-facing live availability port.
///
/// Like [`SelectionBadgeVerifier`], this narrow seam keeps the walk testable:
/// the concrete implementation dials the consumer's MG connector, and tests
/// substitute deterministic outcomes.
pub(crate) trait SelectionAvailabilityProber {
    /// Probe one reached candidate's live availability.
    async fn probe(&self, locator: &Locator) -> LiveProbeOutcome;
}

/// Live probe over the consumer's MG connector, or the ad-only walk when
/// the caller holds no transport capability.
///
/// Holding the absence inside the prober keeps "no connector" representable
/// exactly once: every entry point makes one unconditional
/// `preview_*` call, and only this type decides whether a candidate is
/// dialed or seated on advertised claims.
pub(crate) struct LiveAvailabilityProber<'a, F> {
    pub(crate) connector: Option<&'a F>,
}

impl<F: ManifoldGuardianConnector> SelectionAvailabilityProber for LiveAvailabilityProber<'_, F> {
    async fn probe(&self, locator: &Locator) -> LiveProbeOutcome {
        let Some(connector) = self.connector else {
            return LiveProbeOutcome::Skipped;
        };
        let client = match connector.connect(locator).await {
            Ok(client) => client,
            Err(error) => return LiveProbeOutcome::Unreachable(error.to_string()),
        };
        match connector
            .get_availability(&client, GetAvailabilityRequest)
            .await
        {
            Ok(Ok(availability)) => LiveProbeOutcome::Available(availability),
            // The inner error is MG-authored wire content. A fixed marker
            // keeps remote free text out of the consumer-visible rejection;
            // the typed refusal itself adds nothing a probe caller may act on.
            Ok(Err(_)) => LiveProbeOutcome::Unreachable(
                "Manifold Guardian returned an error to the availability probe".to_owned(),
            ),
            Err(error) => LiveProbeOutcome::Unreachable(error.to_string()),
        }
    }
}

/// Why one live availability response cannot serve one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AvailabilityMismatch {
    /// The MG is not accepting new seats.
    NotAcceptingSeats,
    /// The requested federation size is not offered.
    FederationSize,
    /// The live version is outside the selected DKG identity or FA range.
    FedimintdVersion,
    /// No offered plan matches the requested plan preference.
    Plan,
}

impl From<AvailabilityMismatch> for AdvertisementRejection {
    fn from(mismatch: AvailabilityMismatch) -> Self {
        match mismatch {
            AvailabilityMismatch::NotAcceptingSeats => Self::LiveNotAcceptingSeats,
            AvailabilityMismatch::FederationSize => Self::LiveUnsupportedFederationSize,
            AvailabilityMismatch::FedimintdVersion => Self::LiveUnsupportedFedimintdVersion,
            AvailabilityMismatch::Plan => Self::LiveNoRequestedPlan,
        }
    }
}

/// Exact live build and plan chosen for one compatible MG.
pub(crate) struct MatchedAvailability<'a> {
    pub(crate) fedimintd_version: &'a FedimintdVersion,
    pub(crate) plan: &'a Plan,
}

/// The one definition of "this live availability serves the request".
///
/// Shared by the selection walk's probe and formation's quote-time gate so
/// the two stages can never disagree about what a compatible MG is. On
/// success returns the MG's exact build plus the offered plan matching
/// the preference. Quoting uses both; selection discards the build and plan.
/// Deliberately price-blind: the
/// signed quote remains the only commercial term, so a live price drift
/// still surfaces at quote time, not here.
pub(crate) fn match_requested_availability<'a>(
    availability: &'a GetAvailabilityResponse,
    federation_size: FederationSize,
    fedimintd_versions: &FedimintdVersionRange,
    fedimintd_dkg_version: &FedimintdDkgVersion,
    plan: PlanPreference,
) -> Result<MatchedAvailability<'a>, AvailabilityMismatch> {
    if !availability.accepting_seats {
        return Err(AvailabilityMismatch::NotAcceptingSeats);
    }
    if !availability.federation_sizes.contains(&federation_size) {
        return Err(AvailabilityMismatch::FederationSize);
    }
    let fedimintd_version = &availability.fedimintd_version;
    if fedimintd_version.dkg_version() != *fedimintd_dkg_version
        || !fedimintd_versions.contains(fedimintd_version)
    {
        return Err(AvailabilityMismatch::FedimintdVersion);
    }
    let plan = availability
        .plans
        .iter()
        .find(|offered| plan.matches(offered))
        .ok_or(AvailabilityMismatch::Plan)?;
    Ok(MatchedAvailability {
        fedimintd_version,
        plan,
    })
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Preview the verified seat set one formation intent would select.
    ///
    /// This is a read-only query: it writes no durable state, takes no
    /// driver lease, reserves no seat, and publishes no status. It runs the
    /// bounded enumeration, static admission, and the ranked round-robin
    /// selection walk with lazy badge verification, and returns the selected
    /// seats, the aggregate advertised estimate, and an honest
    /// seen/eligible/selected rejection summary. The result is
    /// informational until approved: the pool is not reserved. Once approved,
    /// Pay-and-create uses exactly this set or returns typed reauthorization.
    ///
    /// The preview shares [`MgDiscoveryOptions`](crate::MgDiscoveryOptions)
    /// with [`FaClient::discover_mg_candidates`], including its
    /// clamped-timeout semantics: one absolute deadline derived from the
    /// clamped timeout covers enumeration, admission, and the walk, including
    /// an in-flight badge verification or live availability probe. The
    /// preview must complete strictly
    /// before that deadline; expiry wins simultaneous readiness and cancels the
    /// preview. It deliberately does not take formation run options — no lease
    /// or driver timing applies to a read-only query.
    ///
    /// The walk probes each reached, verified, non-duplicate candidate's live
    /// availability over the consumer's MG connector before seating it, so
    /// a stale advertisement is rejected here instead of surviving into a
    /// sealed approval that quote time must then invalidate.
    ///
    /// # Errors
    ///
    /// Returns [`FaError::Registry`] when the bounded relay enumeration
    /// fails, [`FaError::SelectionPreviewTimeout`] when the absolute deadline
    /// elapses, [`FaError::InsufficientMgSeats`] when the candidate pool is
    /// exhausted before the walk verified-fills the requested seat count, and
    /// [`FaError::SelectionEstimateOverflow`] when every seat fills but the
    /// aggregate advertised estimate is unrepresentable.
    pub async fn preview_mg_selection(
        &self,
        request: &MgSelectionRequest,
        options: crate::MgDiscoveryOptions,
    ) -> FaResult<MgSelectionPreview> {
        let now = fedimint_core::time::duration_since_epoch().as_secs();
        // `with_timeout` clamps into the runtime timer domain (at most
        // `i32::MAX` milliseconds), so the checked deadline sum cannot fail.
        let deadline = Instant::now()
            .checked_add(options.timeout())
            .expect("clamped discovery timeout fits the monotonic deadline domain");
        preview_mg_selection_until(
            &self.inner.ports.registry,
            &self.inner.peer_badge_verifier,
            &LiveAvailabilityProber {
                connector: Some(&self.inner.ports.mg_connector),
            },
            self.inner.peer_badge_verifier.provenance(),
            request,
            deadline,
            now,
            || fedimint_core::time::duration_since_epoch().as_secs(),
        )
        .await
    }
}

impl<N, F> MgSelectionQuery<N, F>
where
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
{
    /// Preview verified MG selection without FA identity or durable state.
    ///
    /// With an MG connector added through [`Self::with_mg_connector`],
    /// the walk probes each reached candidate's live availability exactly
    /// like [`FaClient::preview_mg_selection`]; without one the preview
    /// seats candidates on advertised claims alone.
    ///
    /// # Errors
    ///
    /// Returns the same typed timeout, registry, shortfall, and selection errors as
    /// [`FaClient::preview_mg_selection`].
    pub async fn preview_mg_selection(
        &self,
        request: &MgSelectionRequest,
        options: crate::MgDiscoveryOptions,
    ) -> FaResult<MgSelectionPreview> {
        let now = fedimint_core::time::duration_since_epoch().as_secs();
        let deadline = Instant::now()
            .checked_add(options.timeout())
            .expect("clamped discovery timeout fits the monotonic deadline domain");
        preview_mg_selection_until(
            &self.registry,
            &self.peer_badge_verifier,
            &LiveAvailabilityProber {
                connector: self.mg_connector.as_ref(),
            },
            self.peer_badge_verifier.provenance(),
            request,
            deadline,
            now,
            || fedimint_core::time::duration_since_epoch().as_secs(),
        )
        .await
    }
}

/// Run the complete selection preview under one absolute runtime deadline.
///
/// Accepts an inner result only when observed strictly before the deadline.
/// Returns [`FaError::SelectionPreviewTimeout`] when the operation starts,
/// remains pending, or completes at or after the deadline.
pub(crate) async fn preview_mg_selection_until(
    registry: &impl FaNostrClient,
    verifier: &impl SelectionBadgeVerifier,
    prober: &impl SelectionAvailabilityProber,
    verifier_provenance: PeerBadgeVerifierProvenance,
    request: &MgSelectionRequest,
    deadline: Instant,
    now: u64,
    completed_at: impl FnOnce() -> u64,
) -> FaResult<MgSelectionPreview> {
    if !(Instant::now() < deadline) {
        return Err(FaError::SelectionPreviewTimeout);
    }
    let deadline_timer = sleep_until(deadline);
    let preview = preview_mg_selection_with(
        registry,
        verifier,
        prober,
        verifier_provenance,
        request,
        deadline,
        now,
        completed_at,
    );
    futures::pin_mut!(deadline_timer, preview);
    match select(deadline_timer, preview).await {
        // Polling the absolute timer first makes deadline expiry win when both
        // futures become ready on the same executor turn.
        Either::Left(((), _preview)) => Err(FaError::SelectionPreviewTimeout),
        Either::Right((result, _deadline_timer)) => {
            preview_result_before_deadline(result, deadline)
        }
    }
}

/// Return an inner preview result only when it completed strictly before the
/// absolute deadline.
pub(crate) fn preview_result_before_deadline(
    result: FaResult<MgSelectionPreview>,
    deadline: Instant,
) -> FaResult<MgSelectionPreview> {
    if Instant::now() < deadline {
        result
    } else {
        Err(FaError::SelectionPreviewTimeout)
    }
}

pub(crate) async fn preview_mg_selection_with(
    registry: &impl FaNostrClient,
    verifier: &impl SelectionBadgeVerifier,
    prober: &impl SelectionAvailabilityProber,
    verifier_provenance: PeerBadgeVerifierProvenance,
    request: &MgSelectionRequest,
    deadline: Instant,
    now: u64,
    completed_at: impl FnOnce() -> u64,
) -> FaResult<MgSelectionPreview> {
    let requirements = MgCandidateRequirements {
        federation_size: request.federation_size,
        fedimintd_versions: request.fedimintd_versions.clone(),
    };
    let discovery = discover_mg_candidates_with(registry, &requirements, deadline, now).await?;
    let seen = discovery.seen();
    let eligible = discovery.candidates.len();
    let mut cohorts = BTreeMap::<FedimintdDkgVersion, Vec<EligibleMgCandidate>>::new();
    for candidate in discovery.candidates {
        cohorts
            .entry(candidate.fedimintd_dkg_version.clone())
            .or_default()
            .push(candidate);
    }
    let mut best = None;
    let mut largest_partial = 0;
    let mut complete_overflowed = false;
    for (dkg_version, candidates) in cohorts.into_iter().rev() {
        let mut cohort_rejected = Vec::new();
        let seats = select_mg_seats(
            verifier,
            prober,
            request,
            &dkg_version,
            candidates,
            request.federation_size,
            BTreeMap::new(),
            BTreeSet::new(),
            deadline,
            &mut cohort_rejected,
        )
        .await;
        largest_partial = largest_partial.max(seats.len());
        if seats.len() < usize::from(request.federation_size.0) {
            continue;
        }
        let Some(total) = seats.iter().try_fold(0u64, |total, seat| {
            total.checked_add(seat.advertised_price_msats())
        }) else {
            complete_overflowed = true;
            continue;
        };
        let replace = best.as_ref().is_none_or(|(best_total, best_dkg, _, _)| {
            total < *best_total || (total == *best_total && dkg_version > *best_dkg)
        });
        if replace {
            best = Some((total, dkg_version, seats, cohort_rejected));
        }
    }

    let Some((total_advertised_msats, fedimintd_dkg_version, seats, cohort_rejected)) = best else {
        if complete_overflowed {
            return Err(FaError::SelectionEstimateOverflow);
        }
        return Err(FaError::InsufficientMgSeats {
            requested: request.federation_size.0,
            selected: u16::try_from(largest_partial).expect("selected seats fit the requested u16"),
            seen,
            eligible,
        });
    };
    let mut rejected = discovery.rejected;
    rejected.extend(cohort_rejected);
    let valid_until = completed_at()
        .checked_add(MG_SELECTION_PREVIEW_VALIDITY.as_secs())
        .ok_or_else(|| FaError::Selection("selection preview expiry overflows".to_owned()))?;
    Ok(MgSelectionPreview {
        request: request.clone(),
        fedimintd_dkg_version,
        verifier_provenance,
        seats,
        total_advertised_msats,
        seen,
        eligible,
        rejected,
        valid_until: Timestamp(valid_until),
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn preview_mg_replacements_with(
    registry: &impl FaNostrClient,
    verifier: &impl SelectionBadgeVerifier,
    prober: &impl SelectionAvailabilityProber,
    verifier_provenance: PeerBadgeVerifierProvenance,
    request: &MgSelectionRequest,
    fedimintd_dkg_version: &FedimintdDkgVersion,
    requirements: GuardianReplacementRequirements,
    excluded: BTreeSet<PublicKey>,
    retained_service_pubkeys: BTreeMap<secp256k1::XOnlyPublicKey, PublicKey>,
    retained_holders: BTreeSet<PublicKey>,
    deadline: Instant,
    now: u64,
    completed_at: impl FnOnce() -> u64,
) -> FaResult<MgReplacementPreview> {
    let discovery_requirements = MgCandidateRequirements {
        federation_size: request.federation_size,
        fedimintd_versions: request.fedimintd_versions.clone(),
    };
    let mut discovery =
        discover_mg_candidates_with(registry, &discovery_requirements, deadline, now).await?;
    discovery
        .candidates
        .retain(|candidate| !excluded.contains(&candidate.mg_id));
    let seen = discovery.seen();
    let eligible = discovery.candidates.len();
    let requested = u16::try_from(requirements.seats.len())
        .map_err(|_| FaError::Selection("too many replacement rows".to_owned()))?;
    let seats = select_mg_seats(
        verifier,
        prober,
        request,
        fedimintd_dkg_version,
        discovery.candidates,
        FederationSize(requested),
        retained_service_pubkeys,
        retained_holders,
        deadline,
        &mut discovery.rejected,
    )
    .await;
    if seats.len() != usize::from(requested) {
        return Err(FaError::InsufficientMgSeats {
            requested,
            selected: u16::try_from(seats.len()).expect("selected replacements fit u16"),
            seen,
            eligible,
        });
    }
    let total_advertised_msats = seats.iter().try_fold(0u64, |total, seat| {
        total.checked_add(seat.advertised_price_msats())
    });
    let total_advertised_msats = total_advertised_msats.ok_or_else(|| {
        FaError::Selection("replacement advertisement estimate overflows".to_owned())
    })?;
    let valid_until = completed_at()
        .checked_add(MG_SELECTION_PREVIEW_VALIDITY.as_secs())
        .ok_or_else(|| FaError::Selection("replacement preview expiry overflows".to_owned()))?;
    Ok(MgReplacementPreview {
        requirements,
        verifier_provenance,
        seats,
        total_advertised_msats,
        valid_until: Timestamp(valid_until),
    })
}

/// Fill seats round-robin across claimed-issuer buckets, verifying lazily.
///
/// Buckets iterate in issuer-key order; within a bucket candidates rank by
/// advertised price (cheapest first), then available slots (least-used
/// first), then MG id. A bucket keeps its turn until it seats one
/// verified candidate or runs out, so one unverifiable candidate cannot
/// hand its bucket's diversity slot to another region. After badge
/// verification and the duplicate-service-key check, a reached candidate is
/// probed for live availability; a candidate whose probe fails or whose
/// live response cannot serve the request is dropped with a typed rejection
/// and its bucket continues, so a stale advertisement costs its author the
/// seat instead of poisoning the sealed approval. The walk stops when
/// the requested seat count is verified-filled, the pool is exhausted, or
/// the deadline expires; the caller turns a shortfall into the typed
/// partial-failure error. `selected_service_pubkeys` contains signing
/// authorities already occupied by retained seats during replacement; initial
/// selection supplies an empty map. `seats_to_fill` is the number of seats
/// to fill, which during replacement is smaller than the request's
/// federation size.
/// `selected_holders` likewise reserves every known retained badge holder.
pub(crate) async fn select_mg_seats(
    verifier: &impl SelectionBadgeVerifier,
    prober: &impl SelectionAvailabilityProber,
    request: &MgSelectionRequest,
    fedimintd_dkg_version: &FedimintdDkgVersion,
    candidates: Vec<EligibleMgCandidate>,
    seats_to_fill: FederationSize,
    mut selected_service_pubkeys: BTreeMap<secp256k1::XOnlyPublicKey, PublicKey>,
    mut selected_holders: BTreeSet<PublicKey>,
    deadline: Instant,
    rejected: &mut Vec<RejectedAdvertisement>,
) -> Vec<SelectedMgSeat> {
    let mut buckets: BTreeMap<PublicKey, Vec<EligibleMgCandidate>> = BTreeMap::new();
    for candidate in candidates {
        buckets
            .entry(candidate.claimed_issuer)
            .or_default()
            .push(candidate);
    }
    let mut queues = buckets
        .into_values()
        .map(|mut bucket| {
            bucket.sort_by(|first, second| {
                first
                    .advertised_price_msats
                    .cmp(&second.advertised_price_msats)
            });
            VecDeque::from(bucket)
        })
        .collect::<Vec<_>>();

    let needed = usize::from(seats_to_fill.0);
    let mut seats = Vec::new();
    'walk: while seats.len() < needed && queues.iter().any(|queue| !queue.is_empty()) {
        for queue in &mut queues {
            while let Some(candidate) = queue.pop_front() {
                match verify_bound_badge(verifier, &candidate, deadline).await {
                    Ok(badge) => {
                        if let Some(selected_mg) = selected_service_pubkeys
                            .get(&candidate.locator.service_pubkey)
                            .copied()
                        {
                            rejected.push(RejectedAdvertisement {
                                author: candidate.mg_id,
                                reason: AdvertisementRejection::DuplicateServicePubkey {
                                    selected_mg,
                                },
                            });
                            // The duplicate was reached and verified, but it
                            // does not fill this bucket's turn. Keep walking
                            // the same bucket so another region does not gain
                            // a diversity slot from the collision.
                            continue;
                        }
                        if selected_holders.contains(&badge.holder) {
                            rejected.push(RejectedAdvertisement {
                                author: candidate.mg_id,
                                reason: AdvertisementRejection::DuplicateBadgeHolder,
                            });
                            continue;
                        }
                        // Probe after the duplicate check so a candidate that
                        // can never seat is not dialed at all.
                        if let Err(reason) = probe_reached_candidate(
                            prober,
                            &candidate,
                            request,
                            fedimintd_dkg_version,
                            deadline,
                        )
                        .await
                        {
                            let expired = matches!(reason, AdvertisementRejection::DeadlineExpired);
                            rejected.push(RejectedAdvertisement {
                                author: candidate.mg_id,
                                reason,
                            });
                            if expired {
                                break 'walk;
                            }
                            // A live-unavailable candidate does not fill this
                            // bucket's turn; its bucket continues.
                            continue;
                        }
                        selected_service_pubkeys
                            .insert(candidate.locator.service_pubkey, candidate.mg_id);
                        selected_holders.insert(badge.holder);
                        seats.push(SelectedMgSeat {
                            candidate: VerifiedCandidate {
                                mg_id: candidate.mg_id,
                                api_endpoints: candidate.api_endpoints,
                                locator: candidate.locator,
                                advertised_price_msats: candidate.advertised_price_msats,
                                availability: candidate.availability,
                                issued_at: candidate.issued_at,
                                expires_at: candidate.expires_at,
                                badge,
                            },
                            provenance: SeatProvenance::FediAttested,
                        });
                        break;
                    }
                    Err(reason) => {
                        // Only the candidate the deadline actually cut off is
                        // a typed rejection; unexamined candidates were never
                        // reached and stay out of the summary.
                        let expired = matches!(reason, AdvertisementRejection::DeadlineExpired);
                        rejected.push(RejectedAdvertisement {
                            author: candidate.mg_id,
                            reason,
                        });
                        if expired {
                            break 'walk;
                        }
                    }
                }
            }
            if seats.len() == needed {
                break 'walk;
            }
        }
    }
    seats
}

/// Verify embedded envelopes until one authentic badge seats the candidate.
///
/// The subject binding is the security point of registry selection: a valid
/// badge stolen from another operator's advertisement names that operator's
/// service key as its subject and must fail here. The claimed-issuer
/// equality keeps the untrusted bucketing key honest: a candidate that
/// claimed one region's issuer but verifies under another is dropped rather
/// than seated in the wrong bucket. When no examined envelope seats the
/// candidate, the reported reason is the first failure in examination
/// order, except that a verified-but-issuer-mismatched badge is decisive
/// and overrides earlier failures.
async fn verify_bound_badge(
    verifier: &impl SelectionBadgeVerifier,
    candidate: &EligibleMgCandidate,
    deadline: Instant,
) -> Result<VerifiedBadgeFacts, AdvertisementRejection> {
    let mut first_failure = None;
    for envelope in candidate
        .holder_authorizations
        .iter()
        .take(MG_ADVERTISEMENT_MAX_HOLDER_AUTHORIZATIONS)
    {
        if Instant::now() >= deadline {
            return Err(AdvertisementRejection::DeadlineExpired);
        }
        match verifier.verify_badge(envelope).await {
            Ok(facts) => {
                if facts.subject != candidate.mg_id {
                    first_failure.get_or_insert(AdvertisementRejection::SubjectMismatch);
                } else if facts.issuer != candidate.claimed_issuer {
                    // A verified, author-bound badge under a different issuer
                    // than the bucketing claim is the decisive reason this
                    // candidate cannot seat; it overrides earlier envelopes'
                    // failures instead of being masked by them.
                    first_failure = Some(AdvertisementRejection::ClaimedIssuerMismatch);
                } else {
                    return Ok(facts);
                }
            }
            Err(error) => {
                first_failure.get_or_insert(AdvertisementRejection::BadgeRejected(error));
            }
        }
    }
    // Discovery eligibility rejects an advertisement with no embedded
    // envelope, so the examined prefix is non-empty here.
    Err(first_failure.expect("non-empty envelope prefix records a failure"))
}

/// Probe one verified, non-duplicate candidate's live availability under a
/// per-candidate budget capped by the walk deadline.
///
/// Only a probe the walk deadline actually cut off is `DeadlineExpired` (the
/// caller stops the walk); a probe that merely exhausted its own budget is a
/// per-candidate `ProbeFailed` rejection and the walk continues. The live
/// response is checked with the same predicate formation applies at quote
/// time, so selection cannot seat a candidate quoting would reject.
async fn probe_reached_candidate(
    prober: &impl SelectionAvailabilityProber,
    candidate: &EligibleMgCandidate,
    request: &MgSelectionRequest,
    fedimintd_dkg_version: &FedimintdDkgVersion,
    deadline: Instant,
) -> Result<(), AdvertisementRejection> {
    if Instant::now() >= deadline {
        return Err(AdvertisementRejection::DeadlineExpired);
    }
    let budget = Instant::now()
        .checked_add(MG_SELECTION_PROBE_TIMEOUT)
        .map_or(deadline, |cutoff| cutoff.min(deadline));
    let timer = sleep_until(budget);
    let probe = prober.probe(&candidate.locator);
    futures::pin_mut!(timer, probe);
    let outcome = match select(timer, probe).await {
        // Polling the budget timer first makes expiry win simultaneous
        // readiness, mirroring the preview's absolute-deadline discipline.
        Either::Left(((), _probe)) => {
            return Err(if Instant::now() >= deadline {
                AdvertisementRejection::DeadlineExpired
            } else {
                AdvertisementRejection::ProbeFailed {
                    message: "live availability probe timed out".to_owned(),
                }
            });
        }
        Either::Right((outcome, _timer)) => outcome,
    };
    match outcome {
        LiveProbeOutcome::Skipped => Ok(()),
        LiveProbeOutcome::Unreachable(message) => {
            Err(AdvertisementRejection::ProbeFailed { message })
        }
        LiveProbeOutcome::Available(availability) => match_requested_availability(
            &availability,
            request.federation_size,
            &request.fedimintd_versions,
            fedimintd_dkg_version,
            request.plan,
        )
        .map(|_availability| ())
        .map_err(AdvertisementRejection::from),
    }
}
