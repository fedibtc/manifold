//! Read-only FMan advertisement discovery and static admission.
//!
//! This module turns one bounded, untrusted relay enumeration into a
//! statically admitted candidate set: every candidate carries an
//! authenticated advertisement that is fresh, currently eligible for the
//! caller's intent, and dialable through a [`Locator`] built from its
//! advertised iroh endpoint and self-attested commitment-signing service
//! pubkey. Relay-backed PeerBadge verification deliberately does not run
//! here: it is the most expensive admission stage, so it runs lazily, in
//! selection order, inside `selection` — only candidates the ranked
//! round-robin walk actually reaches cost verifier round trips. A candidate
//! returned by discovery therefore carries *claims*, not trust conclusions;
//! the sealed [`crate::VerifiedCandidate`] type only exists on the far side
//! of the selection walk. Discovery performs no durable writes, takes no
//! driver lease, and mutates no formation state. The approved verification
//! order, freshness policy, eligibility policy, and locator construction are
//! recorded in `specs/ARCH-fi-client-discovery-selection.md`.

use std::collections::BTreeMap;
use std::time::Duration;

use fedi_decentralized_domain::HolderAuthorizationEnvelope;
use fedi_decentralized_nostr::fman::{
    AdvertisementDocument, AdvertisementPayload, ApiEndpoint, Availability,
    FMAN_ADVERTISEMENT_D_TAG, FMAN_ADVERTISEMENT_EVENT_KIND, IROH_API_ENDPOINT_TRANSPORT,
    IROH_API_ENDPOINT_URL_SCHEME, verify_advertisement_self_signature,
};
use fedi_decentralized_nostr::has_exact_d_tag;
use fedi_decentralized_nostr_clients::{FMAN_ADVERTISEMENTS_CANDIDATE_LIMIT, FiNostrClient};
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerificationError;
use fedi_decentralized_service_fleet_manager::{
    FederationSize, FedimintdDkgVersion, FmanName, Locator, Plan, Timestamp,
};
use fedi_iroh_rpc::iroh::{EndpointAddr, EndpointId};
use fedimint_core::runtime::Instant;
use nostr_sdk::{Event, EventId, Kind, PublicKey};
use rand::seq::SliceRandom as _;

use crate::{
    FederationConsensusReader, FedimintdVersionRange, FiClient, FiError, FiIdentity, FiPayments,
    FiResult, FleetManagerConnector,
};

/// Default absolute deadline for one complete discovery run.
///
/// One run performs the bounded relay enumeration plus cheap local admission
/// checks per advertisement. Advertisements not admitted before the
/// discovery deadline are reported as
/// [`AdvertisementRejection::DeadlineExpired`] rather than silently dropped.
pub const FMAN_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum accepted advertisement age, measured from `issued_at`.
///
/// Honest FMans republish every 30 minutes (`fman-nostr`'s
/// `REPUBLISH_INTERVAL`) and self-expire each ad one hour after issue, so
/// this cap never binds an honest publisher. What it bounds is replay:
/// `expires_at` is publisher-controlled, so a maliciously long-expiry signed
/// ad could otherwise keep resurfacing indefinitely. Two hours is 4x the
/// publish cadence — generous slack for relay lag without letting a
/// long-expiry ad outlive its issuer's silence for long. Revisit this value
/// together with `REPUBLISH_INTERVAL`; they are coupled.
pub const FMAN_ADVERTISEMENT_MAX_AGE: Duration = Duration::from_secs(2 * 60 * 60);

/// Maximum embedded holder authorizations examined per advertisement.
///
/// Each examined envelope costs one bounded relay-backed badge verification
/// during the selection walk, so an attacker-authored advertisement must not
/// be able to multiply relay work without bound. Envelopes beyond this
/// prefix are ignored.
pub const FMAN_ADVERTISEMENT_MAX_HOLDER_AUTHORIZATIONS: usize = 4;

/// Caller requirements one discovered FMan must currently satisfy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FmanCandidateRequirements {
    /// Federation size the FI intends to request.
    pub federation_size: FederationSize,

    /// FI-approved Fedimint release range.
    pub fedimintd_versions: FedimintdVersionRange,
}

/// Options for one discovery run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FmanDiscoveryOptions {
    timeout: Duration,
}

impl FmanDiscoveryOptions {
    /// Override the default absolute discovery deadline.
    ///
    /// The timeout is clamped into the shared native/WASM runtime timer
    /// domain — one millisecond through `i32::MAX` milliseconds — mirroring
    /// the formation timing discipline, so an out-of-range value cannot
    /// reach the runtime timers or overflow the monotonic deadline. The
    /// enumeration is fail-closed, so a timeout too small for the relay
    /// round trip does not return an empty listing: the run fails with a
    /// typed [`FiError::Registry`](crate::FiError) error, while a deadline
    /// that expires only during per-advertisement admission degrades to typed
    /// [`AdvertisementRejection::DeadlineExpired`] rejections on the standalone
    /// discovery surface. The public selection-preview surfaces instead accept
    /// only a result observed strictly before their absolute deadline and return
    /// [`FiError::SelectionPreviewTimeout`](crate::FiError) at or after it.
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout: timeout.clamp(
                crate::formation::MIN_RUNTIME_TIMER_DURATION,
                crate::formation::MAX_RUNTIME_TIMER_DURATION,
            ),
        }
    }

    /// Return the clamped absolute timeout.
    pub(crate) fn timeout(self) -> Duration {
        self.timeout
    }
}

impl Default for FmanDiscoveryOptions {
    fn default() -> Self {
        Self {
            timeout: FMAN_DISCOVERY_TIMEOUT,
        }
    }
}

/// Read-only FMan registry query surface.
///
/// This purpose-specific client owns only the registry transport needed for
/// static discovery. It has no trust verifier, FI identity, durable database,
/// payment capability, FMan connector, consensus reader, driver lease, or
/// formation state. Call [`FmanRegistryQuery::with_verifier`] to obtain the
/// stronger read-only capability required for verified selection preview.
#[derive(Clone)]
pub struct FmanRegistryQuery<N> {
    pub(crate) registry: N,
}

impl<N> FmanRegistryQuery<N> {
    /// Construct a static-discovery surface from its sole required capability.
    #[must_use]
    pub fn new(registry: N) -> Self {
        Self { registry }
    }
}

/// One statically admitted, fresh, currently eligible FMan advertisement.
///
/// The advertisement's event signature, document proof, and payload-author
/// identity rule have been verified, and its freshness and eligibility
/// checked — but its embedded PeerBadge envelopes have **not** been
/// verified. Every trust-bearing field is a publisher claim until the
/// selection walk verifies a badge and produces a sealed
/// [`crate::VerifiedCandidate`]. Fields are sealed: only the discovery
/// pipeline constructs one, so a consumer can never smuggle an unadmitted
/// advertisement into selection.
#[derive(Clone, Debug, PartialEq)]
pub struct EligibleFmanCandidate {
    /// Authenticated FMan identity: the advertisement event author, equal to
    /// the signed payload's `fman_id_pubkey`.
    pub(crate) fman_id: PublicKey,

    /// Ad-declared pre-formation RPC endpoints; non-trust dialing hints.
    pub(crate) api_endpoints: Vec<ApiEndpoint>,

    /// Dialing locator built from the ad's first parseable iroh endpoint and
    /// its self-attested commitment-signing `service_pubkey`.
    pub(crate) locator: Locator,

    /// Advertised one-time price of the ad's `InfiniteBestEffort` plan, in
    /// millisatoshis.
    pub(crate) advertised_price_msats: u64,

    /// DKG compatibility identity derived from the advertised version.
    pub(crate) fedimintd_dkg_version: FedimintdDkgVersion,

    /// What the ad says the FMan will serve; non-trust hints re-checked
    /// live during a probing selection walk and again at quote time.
    pub(crate) availability: Availability,

    /// Unix seconds at which the FMan issued the advertisement.
    pub(crate) issued_at: Timestamp,

    /// Unix seconds after which the advertisement expires.
    pub(crate) expires_at: Timestamp,

    /// Issuer identity claimed by the first embedded envelope's credential.
    ///
    /// Untrusted publisher-controlled content, read locally without verifier
    /// round trips. Selection uses it only to bucket candidates; the
    /// selection walk then requires the verified badge issuer to equal this
    /// claim, so a false claim costs the candidate its seat rather than
    /// misplacing a verified one.
    pub(crate) claimed_issuer: PublicKey,

    /// Embedded envelopes, verified lazily during the selection walk.
    pub(crate) holder_authorizations: Vec<HolderAuthorizationEnvelope>,
}

impl EligibleFmanCandidate {
    /// Authenticated FMan identity: the advertisement event author, equal to
    /// the signed payload's `fman_id_pubkey`.
    #[must_use]
    pub fn fman_id(&self) -> PublicKey {
        self.fman_id
    }

    /// Stable two-word display name derived from the authenticated FMan id.
    ///
    /// Names can collide and never substitute for [`Self::fman_id`] in
    /// identity, trust, or deduplication.
    #[must_use]
    pub fn fman_name(&self) -> FmanName {
        FmanName::from_fman_id(self.fman_id)
    }

    /// Ad-declared pre-formation RPC endpoints; non-trust dialing hints.
    ///
    /// The dialing-ready projection of these is [`Self::locator`]; the raw
    /// list remains available for diagnostics and future multi-endpoint
    /// selection.
    #[must_use]
    pub fn api_endpoints(&self) -> &[ApiEndpoint] {
        &self.api_endpoints
    }

    /// Dialing locator for this FMan: the ad's first parseable iroh endpoint
    /// paired with the commitment-signing `service_pubkey` the signed payload
    /// names, exactly what [`crate::FleetManagerConnector::connect`] takes.
    ///
    /// The service key is self-attested by the badge-vouched FMan identity —
    /// the signed payload binds it to the authenticated event author — so
    /// signed responses from the dialed endpoint verify against the key that
    /// identity asserted, not against an independently vouched key
    /// (`specs/ARCH-fi-client-discovery-selection.md`, *Discovery*).
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

    /// What the ad says the FMan will serve; non-trust hints re-checked
    /// live during a probing selection walk and again at quote time.
    #[must_use]
    pub fn availability(&self) -> &Availability {
        &self.availability
    }

    /// Unix seconds at which the FMan issued the advertisement.
    #[must_use]
    pub fn issued_at(&self) -> Timestamp {
        self.issued_at
    }

    /// Unix seconds after which the advertisement expires.
    #[must_use]
    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    /// Issuer identity claimed by the first embedded envelope's credential.
    ///
    /// Untrusted until the selection walk verifies a badge from this issuer.
    #[must_use]
    pub fn claimed_issuer(&self) -> PublicKey {
        self.claimed_issuer
    }
}

/// Typed reason one fetched advertisement event was not admitted or seated.
#[derive(Debug)]
#[non_exhaustive]
pub enum AdvertisementRejection {
    /// The event kind or `d` tag is not the FMan advertisement role.
    WrongEventRole,

    /// The Nostr event id or signature is invalid.
    InvalidEventSignature,

    /// The event content is not a parsable advertisement document.
    UnparsableDocument,

    /// The document's own payload proof failed verification.
    InvalidAdvertisementProof,

    /// The signed payload `fman_id_pubkey` does not equal the event author.
    AuthorMismatch,

    /// A newer valid advertisement from the same author replaced this event.
    Superseded,

    /// The advertisement embeds no holder authorization.
    MissingHolderAuthorization,

    /// Every examined envelope failed shared PeerBadge verification.
    BadgeRejected(PeerBadgeVerificationError),

    /// A verified badge authorizes a subject other than the event author.
    SubjectMismatch,

    /// A verified badge names an issuer other than the advertisement's
    /// claimed bucketing issuer.
    ClaimedIssuerMismatch,

    /// This verified FMan author advertises a commitment-signing service key
    /// already owned by an earlier selected FMan.
    DuplicateServicePubkey {
        /// Earlier verified FMan that first occupied the service-key identity
        /// in deterministic selection order.
        selected_fman: PublicKey,
    },

    /// The run deadline expired before this advertisement was processed.
    DeadlineExpired,

    /// The live availability probe produced no usable response: connecting,
    /// calling, or the Fleet Manager's own error, or the per-probe budget
    /// elapsed first.
    ProbeFailed {
        /// Diagnostic description from the sanitized-by-contract local
        /// connector error types, or a fixed marker for a Fleet
        /// Manager-returned error; remote error text is never embedded.
        message: String,
    },

    /// The live availability response is not accepting new seats.
    LiveNotAcceptingSeats,

    /// The live availability response does not offer the requested
    /// federation size.
    LiveUnsupportedFederationSize,

    /// The live response's version is outside the FI range or selected DKG
    /// identity.
    LiveUnsupportedFedimintdVersion,

    /// The live availability response offers no plan matching the requested
    /// plan preference.
    LiveNoRequestedPlan,

    /// `expires_at` has passed.
    Expired,

    /// `issued_at` lies in the consumer's future.
    IssuedInFuture,

    /// `issued_at` is older than [`FMAN_ADVERTISEMENT_MAX_AGE`].
    Stale,

    /// The advertised sizes do not include the requested federation size.
    UnsupportedFederationSize,

    /// The typed advertised version is outside the FI range or is not Fedi.
    UnsupportedFedimintdVersion,

    /// The advertisement offers no `InfiniteBestEffort` plan.
    NoInfiniteBestEffortPlan,

    /// The advertised `service_pubkey` is not a parseable x-only secp256k1
    /// key.
    MalformedServicePubkey,

    /// No advertised endpoint is a parseable `iroh://<endpoint-id>` URL, so
    /// the FMan cannot be dialed.
    NoDialableEndpoint,
}

impl AdvertisementRejection {
    /// Stable machine code for consumer serialization and branching.
    ///
    /// This intentionally omits nested diagnostic details. Consumers that
    /// need human-readable context may format the rejection separately, while
    /// serialized contracts remain independent of Rust `Debug` output and
    /// upstream error representation.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::WrongEventRole => "wrong_event_role",
            Self::InvalidEventSignature => "invalid_event_signature",
            Self::UnparsableDocument => "unparsable_document",
            Self::InvalidAdvertisementProof => "invalid_advertisement_proof",
            Self::AuthorMismatch => "author_mismatch",
            Self::Superseded => "superseded",
            Self::MissingHolderAuthorization => "missing_holder_authorization",
            Self::BadgeRejected(_) => "badge_rejected",
            Self::SubjectMismatch => "subject_mismatch",
            Self::ClaimedIssuerMismatch => "claimed_issuer_mismatch",
            Self::DuplicateServicePubkey { .. } => "duplicate_service_pubkey",
            Self::DeadlineExpired => "deadline_expired",
            Self::ProbeFailed { .. } => "probe_failed",
            Self::LiveNotAcceptingSeats => "live_not_accepting_seats",
            Self::LiveUnsupportedFederationSize => "live_unsupported_federation_size",
            Self::LiveUnsupportedFedimintdVersion => "live_unsupported_fedimintd_version",
            Self::LiveNoRequestedPlan => "live_no_requested_plan",
            Self::Expired => "expired",
            Self::IssuedInFuture => "issued_in_future",
            Self::Stale => "stale",
            Self::UnsupportedFederationSize => "unsupported_federation_size",
            Self::UnsupportedFedimintdVersion => "unsupported_fedimintd_version",
            Self::NoInfiniteBestEffortPlan => "no_infinite_best_effort_plan",
            Self::MalformedServicePubkey => "malformed_service_pubkey",
            Self::NoDialableEndpoint => "no_dialable_endpoint",
        }
    }
}

/// One rejected advertisement and its typed reason.
#[derive(Debug)]
pub struct RejectedAdvertisement {
    /// Claimed event author. Authenticated only for reasons that follow
    /// event-signature verification; earlier reasons report the unverified
    /// claimed author for diagnostics.
    pub author: PublicKey,

    /// Why the advertisement was not admitted.
    pub reason: AdvertisementRejection,
}

/// Outcome of one bounded discovery run.
#[derive(Debug, Default)]
pub struct FmanDiscovery {
    /// Statically admitted, fresh, currently eligible candidates, in a fresh
    /// random order. Their embedded badges are unverified claims until the
    /// selection walk examines them.
    pub candidates: Vec<EligibleFmanCandidate>,

    /// Advertisements observed but not admitted, with typed reasons.
    pub rejected: Vec<RejectedAdvertisement>,
}

/// One authenticated, fresh and dialable advertisement projected only as a
/// pinned diagnostic locator, without requiring PeerBadge material.
///
/// This type carries no trust conclusion and cannot enter verified selection.
#[derive(Clone, Debug, PartialEq)]
pub struct InsecureUntrustedPinnedFman {
    pub fman_id: PublicKey,
    pub locator: Locator,
}

/// Test-only diagnostic locator discovery with the ordinary typed rejections.
#[derive(Debug, Default)]
pub struct InsecureUntrustedPinnedFmanDiscovery {
    pub candidates: Vec<InsecureUntrustedPinnedFman>,
    pub rejected: Vec<RejectedAdvertisement>,
}

impl FmanDiscovery {
    /// Total number of bounded relay candidates this run observed.
    ///
    /// The transport counts one signed event once however many relays serve
    /// it, so identical cross-relay copies do not inflate this. Distinct
    /// events per author still can: an author's older event contributes one
    /// [`AdvertisementRejection::Superseded`] rejection.
    #[must_use]
    pub fn seen(&self) -> usize {
        self.candidates.len() + self.rejected.len()
    }
}

impl<I, P, N, F, C> FiClient<I, P, N, F, C>
where
    I: FiIdentity,
    P: FiPayments,
    N: FiNostrClient,
    F: FleetManagerConnector,
    C: FederationConsensusReader,
{
    /// Discover, statically admit, and filter current FMan advertisements.
    ///
    /// This is a read-only query: it writes no durable state, takes no driver
    /// lease, and publishes no status. Fetched events are untrusted relay
    /// data; a returned [`EligibleFmanCandidate`] has passed, in order, event
    /// role and signature authentication, advertisement document proof and
    /// author binding, per-author newest-`created_at` replacement, the
    /// freshness policy, and the caller's eligibility requirements — all
    /// cheap local checks. The expensive relay-backed PeerBadge verification
    /// deliberately does not run here: it runs lazily, in selection order,
    /// inside [`FiClient::preview_fman_selection`], so only candidates the
    /// ranked walk actually reaches cost verifier round trips
    /// (`specs/ARCH-fi-client-discovery-selection.md`). Everything runs under one
    /// absolute deadline; advertisements the deadline cut off are reported
    /// as typed rejections so consumers can render "N seen, M eligible"
    /// honestly.
    ///
    /// # Errors
    ///
    /// Returns [`FiError::Registry`] when the bounded relay enumeration fails
    /// or the deadline expires before the enumeration completes.
    pub async fn discover_fman_candidates(
        &self,
        requirements: &FmanCandidateRequirements,
        options: FmanDiscoveryOptions,
    ) -> FiResult<FmanDiscovery> {
        let now = fedimint_core::time::duration_since_epoch().as_secs();
        // `with_timeout` clamps into the runtime timer domain (at most
        // `i32::MAX` milliseconds), so the checked deadline sum cannot fail.
        let deadline = Instant::now()
            .checked_add(options.timeout())
            .expect("clamped discovery timeout fits the monotonic deadline domain");
        discover_fman_candidates_with(&self.inner.ports.registry, requirements, deadline, now).await
    }
}

impl<N> FmanRegistryQuery<N>
where
    N: FiNostrClient,
{
    /// Discover statically admitted FMan advertisements without FI state.
    ///
    /// # Errors
    ///
    /// Returns [`FiError::Registry`] when the bounded relay enumeration fails
    /// or its deadline expires before enumeration completes.
    pub async fn discover_fman_candidates(
        &self,
        requirements: &FmanCandidateRequirements,
        options: FmanDiscoveryOptions,
    ) -> FiResult<FmanDiscovery> {
        let now = fedimint_core::time::duration_since_epoch().as_secs();
        let deadline = Instant::now()
            .checked_add(options.timeout())
            .expect("clamped discovery timeout fits the monotonic deadline domain");
        discover_fman_candidates_with(&self.registry, requirements, deadline, now).await
    }

    /// Discover locators for the existing pinned diagnostic formation path.
    ///
    /// Event signatures, advertisement proofs, author binding, freshness,
    /// intent compatibility, capacity and dialing material are still checked.
    /// HolderAuthorization presence and PeerBadge verification are deliberately
    /// omitted. The result cannot be converted into a verified selection and
    /// must never be used by a production consumer.
    pub async fn insecure_discover_untrusted_pinned_fmans(
        &self,
        requirements: &FmanCandidateRequirements,
        options: FmanDiscoveryOptions,
    ) -> FiResult<InsecureUntrustedPinnedFmanDiscovery> {
        let now = fedimint_core::time::duration_since_epoch().as_secs();
        let deadline = Instant::now()
            .checked_add(options.timeout())
            .expect("clamped discovery timeout fits the monotonic deadline domain");
        let events = self
            .registry
            .fetch_fman_advertisements(deadline.saturating_duration_since(Instant::now()))
            .await
            .map_err(|error| FiError::Registry(error.to_string()))?;
        let mut rejected = Vec::new();
        let newest =
            statically_admit_newest_per_author(events.into_iter(), deadline, &mut rejected);
        let mut candidates = Vec::new();
        for (author, admitted) in newest {
            match admit_insecure_untrusted_pinned_fman(
                requirements,
                author,
                admitted.document,
                now,
                deadline,
            ) {
                Ok(candidate) => candidates.push(candidate),
                Err(reason) => rejected.push(RejectedAdvertisement { author, reason }),
            }
        }
        Ok(InsecureUntrustedPinnedFmanDiscovery {
            candidates,
            rejected,
        })
    }
}

/// Candidates come back in a fresh random order on every run: see the shuffle
/// at the end of this function for why load spreading has to live here.
pub(crate) async fn discover_fman_candidates_with(
    registry: &impl FiNostrClient,
    requirements: &FmanCandidateRequirements,
    deadline: Instant,
    now: u64,
) -> FiResult<FmanDiscovery> {
    let events = registry
        .fetch_fman_advertisements(deadline.saturating_duration_since(Instant::now()))
        .await
        .map_err(|error| FiError::Registry(error.to_string()))?;

    let mut discovery = FmanDiscovery::default();
    // The transport bound belongs to the client; re-applying it here keeps a
    // misbehaving injected registry from expanding the admission workload.
    let admitted = statically_admit_newest_per_author(
        events
            .into_iter()
            .take(usize::from(FMAN_ADVERTISEMENTS_CANDIDATE_LIMIT)),
        deadline,
        &mut discovery.rejected,
    );

    for (author, admitted) in admitted {
        match admit_eligible_advertisement(requirements, author, admitted.document, now, deadline) {
            Ok(candidate) => discovery.candidates.push(candidate),
            Err(reason) => discovery
                .rejected
                .push(RejectedAdvertisement { author, reason }),
        }
    }
    // Advertisements no longer carry a capacity count, so no consumer can
    // spread its picks by "least used". Any deterministic order — and the
    // relay's is effectively by pubkey — would send every FI that shares a
    // price tier to the same FMans. Shuffling here makes an unbiased draw the
    // default for every consumer, including one that just takes the first N.
    discovery.candidates.shuffle(&mut rand::thread_rng());
    Ok(discovery)
}

struct AdmittedAdvertisement {
    created_at: u64,
    event_id: EventId,
    document: AdvertisementDocument,
}

/// Statically authenticate every event and keep one newest document per author.
///
/// Static admission covers the event role, the event signature, document
/// parsing, the document's own proof, and the payload-author identity rule.
/// Replacement order is newest `created_at` first with the NIP-01 lowest
/// event id breaking ties; superseded valid events are reported as typed
/// rejections.
fn statically_admit_newest_per_author(
    events: impl Iterator<Item = Event>,
    deadline: Instant,
    rejected: &mut Vec<RejectedAdvertisement>,
) -> BTreeMap<PublicKey, AdmittedAdvertisement> {
    let mut admitted: BTreeMap<PublicKey, AdmittedAdvertisement> = BTreeMap::new();
    for event in events {
        let author = event.pubkey;
        if Instant::now() >= deadline {
            rejected.push(RejectedAdvertisement {
                author,
                reason: AdvertisementRejection::DeadlineExpired,
            });
            continue;
        }
        match statically_admit(&event) {
            Ok(document) => {
                let candidate = AdmittedAdvertisement {
                    created_at: event.created_at.as_secs(),
                    event_id: event.id,
                    document,
                };
                match admitted.entry(author) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(candidate);
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        let candidate_is_newer = candidate.created_at > entry.get().created_at
                            || (candidate.created_at == entry.get().created_at
                                && candidate.event_id.as_bytes() < entry.get().event_id.as_bytes());
                        if candidate_is_newer {
                            entry.insert(candidate);
                        }
                        rejected.push(RejectedAdvertisement {
                            author,
                            reason: AdvertisementRejection::Superseded,
                        });
                    }
                }
            }
            Err(reason) => rejected.push(RejectedAdvertisement { author, reason }),
        }
    }
    admitted
}

fn statically_admit(event: &Event) -> Result<AdvertisementDocument, AdvertisementRejection> {
    if event.kind != Kind::Custom(FMAN_ADVERTISEMENT_EVENT_KIND)
        || !has_exact_d_tag(event, FMAN_ADVERTISEMENT_D_TAG)
    {
        return Err(AdvertisementRejection::WrongEventRole);
    }
    event
        .verify()
        .map_err(|_| AdvertisementRejection::InvalidEventSignature)?;
    let document = serde_json::from_str::<AdvertisementDocument>(&event.content)
        .map_err(|_| AdvertisementRejection::UnparsableDocument)?;
    verify_advertisement_self_signature(&document)
        .map_err(|_| AdvertisementRejection::InvalidAdvertisementProof)?;
    if document.payload.fman_id_pubkey != event.pubkey.to_string() {
        return Err(AdvertisementRejection::AuthorMismatch);
    }
    Ok(document)
}

fn admit_eligible_advertisement(
    requirements: &FmanCandidateRequirements,
    author: PublicKey,
    document: AdvertisementDocument,
    now: u64,
    deadline: Instant,
) -> Result<EligibleFmanCandidate, AdvertisementRejection> {
    let (payload, advertised_price_msats, locator, fedimintd_dkg_version) =
        admit_eligible_payload(requirements, document, now, deadline)?;
    // An advertisement with no envelope can never verify during selection,
    // and the claimed issuer used for bucketing comes from the first
    // envelope, so the empty case is rejected here where it is free.
    let claimed_issuer = payload
        .holder_authorizations
        .first()
        .map(|envelope| envelope.signed_credential.credential.issuer_id_pubkey.0)
        .ok_or(AdvertisementRejection::MissingHolderAuthorization)?;

    Ok(EligibleFmanCandidate {
        fman_id: author,
        api_endpoints: payload.api_endpoints,
        locator,
        advertised_price_msats,
        fedimintd_dkg_version,
        availability: payload.availability,
        issued_at: Timestamp(payload.issued_at),
        expires_at: Timestamp(payload.expires_at),
        claimed_issuer,
        holder_authorizations: payload.holder_authorizations,
    })
}

fn admit_insecure_untrusted_pinned_fman(
    requirements: &FmanCandidateRequirements,
    author: PublicKey,
    document: AdvertisementDocument,
    now: u64,
    deadline: Instant,
) -> Result<InsecureUntrustedPinnedFman, AdvertisementRejection> {
    let (payload, _, locator, _) = admit_eligible_payload(requirements, document, now, deadline)?;
    debug_assert_eq!(payload.fman_id_pubkey, author.to_string());
    Ok(InsecureUntrustedPinnedFman {
        fman_id: author,
        locator,
    })
}

/// Apply every non-credential eligibility check shared by verified discovery
/// and the explicitly untrusted pinned diagnostic projection.
fn admit_eligible_payload(
    requirements: &FmanCandidateRequirements,
    document: AdvertisementDocument,
    now: u64,
    deadline: Instant,
) -> Result<(AdvertisementPayload, u64, Locator, FedimintdDkgVersion), AdvertisementRejection> {
    if Instant::now() >= deadline {
        return Err(AdvertisementRejection::DeadlineExpired);
    }
    let payload = document.payload;
    if payload.expires_at <= now {
        return Err(AdvertisementRejection::Expired);
    }
    if payload.issued_at > now {
        return Err(AdvertisementRejection::IssuedInFuture);
    }
    if now - payload.issued_at > FMAN_ADVERTISEMENT_MAX_AGE.as_secs() {
        return Err(AdvertisementRejection::Stale);
    }
    let availability = &payload.availability;
    if !availability
        .federation_sizes
        .contains(&requirements.federation_size.0)
    {
        return Err(AdvertisementRejection::UnsupportedFederationSize);
    }
    let version = &availability.fedimintd_version;
    if !requirements.fedimintd_versions.contains(&version) {
        return Err(AdvertisementRejection::UnsupportedFedimintdVersion);
    }
    let fedimintd_dkg_version = version.dkg_version();
    if !fedimintd_dkg_version.is_fedi() {
        return Err(AdvertisementRejection::UnsupportedFedimintdVersion);
    }
    let advertised_price_msats = payload
        .plans
        .iter()
        .find_map(|plan| match plan {
            Plan::InfiniteBestEffort { price_msats } => Some(*price_msats),
            _ => None,
        })
        .ok_or(AdvertisementRejection::NoInfiniteBestEffortPlan)?;
    let locator = dialing_locator(&payload)?;
    Ok((
        payload,
        advertised_price_msats,
        locator,
        fedimintd_dkg_version,
    ))
}

/// Build the dialing [`Locator`] an eligible advertisement implies.
///
/// The locator pairs the first advertised endpoint whose URL is a parseable
/// `iroh://<endpoint-id>` (extra URL components after the endpoint id are
/// ignored as dialing hints this consumer does not use) with the payload's
/// commitment-signing `service_pubkey`. A malformed service key or the absence
/// of a parseable iroh endpoint makes the advertisement ineligible rather than
/// surfacing a candidate that cannot be dialed with verifiable responses.
fn dialing_locator(payload: &AdvertisementPayload) -> Result<Locator, AdvertisementRejection> {
    let service_pubkey = payload
        .service_pubkey
        .parse::<secp256k1::XOnlyPublicKey>()
        .map_err(|_| AdvertisementRejection::MalformedServicePubkey)?;
    let endpoint_id = payload
        .api_endpoints
        .iter()
        .filter(|endpoint| endpoint.transport == IROH_API_ENDPOINT_TRANSPORT)
        .find_map(|endpoint| {
            endpoint
                .url
                .strip_prefix(IROH_API_ENDPOINT_URL_SCHEME)?
                .split(['/', '?', '#'])
                .next()?
                .parse::<EndpointId>()
                .ok()
        })
        .ok_or(AdvertisementRejection::NoDialableEndpoint)?;
    Ok(Locator::new(EndpointAddr::new(endpoint_id), service_pubkey))
}
