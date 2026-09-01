//! Tests for the ranked round-robin selection walk and its preview API.
//!
//! The bucketing key, in-bucket ordering, round-robin fill, lazy badge
//! verification order, bounded envelope prefix, and deadline semantics
//! these tests pin are recorded in
//! `crates/fi-client/specs/ARCH-fi-client-discovery-selection.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use fedi_decentralized_domain::{HolderAuthorizationEnvelope, parse_trust_score_badge_v1};
use fedi_decentralized_peer_badge_verifier::{
    PeerBadgeVerificationError, PeerBadgeVerifierProvenance,
};
use fedi_decentralized_service_fleet_manager::{
    FleetManagerError, FmResult, GetAvailabilityRequest, GetAvailabilityResponse, GetQuoteRequest,
    GetQuoteResponse, SignedResponse,
};
use fedimint_core::runtime::Instant;

use super::discovery::{
    AD_PRICE_MSATS, NOW, ad_event, envelope, envelope_with_issuer, fman_keys, generous_deadline,
    holder_keys, issuer_keys, payload, registry, requirements, self_authorized_payload,
    service_pubkey,
};
use super::*;
use crate::discovery::discover_fman_candidates_with;
use crate::selection::{
    LiveAvailabilityProber, LiveProbeOutcome, SelectionAvailabilityProber, SelectionBadgeVerifier,
    preview_fman_replacements_with, preview_fman_selection_until, preview_fman_selection_with,
    select_fman_seats,
};

/// Ad-only walk for tests that pin pre-probe selection semantics: no
/// transport capability, every probe is skipped.
struct AdOnlySelection;

impl SelectionAvailabilityProber for AdOnlySelection {
    async fn probe(&self, _locator: &crate::Locator) -> LiveProbeOutcome {
        LiveProbeOutcome::Skipped
    }
}
use crate::{
    FmanReplacementPreview, FmanSelectionRequest, GuardianReplacementId,
    GuardianReplacementRequirements, GuardianReplacementSeat, SeatProvenance,
};

/// Deterministic stand-in for the shared verifier: verification succeeds
/// with facts projected from the envelope unless the envelope's subject is
/// in the injected rejection set.
#[derive(Clone, Default)]
struct StubBadgeVerifier {
    reject_subjects: Arc<Mutex<HashSet<PublicKey>>>,
    calls: Arc<AtomicUsize>,
    attempted_subjects: Arc<Mutex<Vec<PublicKey>>>,
    delay: Option<Duration>,
}

impl StubBadgeVerifier {
    fn rejecting(subject: PublicKey) -> Self {
        let stub = Self::default();
        stub.reject_subjects
            .lock()
            .expect("test lock")
            .insert(subject);
        stub
    }

    fn slowly_rejecting(subjects: impl IntoIterator<Item = PublicKey>, delay: Duration) -> Self {
        let mut stub = Self::default();
        stub.reject_subjects
            .lock()
            .expect("test lock")
            .extend(subjects);
        stub.delay = Some(delay);
        stub
    }
}

impl SelectionBadgeVerifier for StubBadgeVerifier {
    async fn verify_badge(
        &self,
        envelope: &HolderAuthorizationEnvelope,
    ) -> Result<crate::VerifiedBadgeFacts, PeerBadgeVerificationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let subject = envelope.holder_authorization.authorization.subject_pubkey.0;
        self.attempted_subjects
            .lock()
            .expect("test lock")
            .push(subject);
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        if self
            .reject_subjects
            .lock()
            .expect("test lock")
            .contains(&subject)
        {
            return Err(PeerBadgeVerificationError::CredentialRevoked);
        }
        Ok(crate::VerifiedBadgeFacts {
            issuer: envelope.signed_credential.credential.issuer_id_pubkey.0,
            holder: envelope
                .holder_authorization
                .authorization
                .holder_id_pubkey
                .0,
            subject,
            badge: parse_trust_score_badge_v1(&envelope.signed_credential.credential)
                .expect("test badge parses"),
        })
    }
}

/// Verifier whose only in-flight work is cancelled when its future is dropped.
struct PendingBadgeVerifier {
    started: Arc<Notify>,
    cancelled: Arc<AtomicBool>,
}

impl SelectionBadgeVerifier for PendingBadgeVerifier {
    async fn verify_badge(
        &self,
        _envelope: &HolderAuthorizationEnvelope,
    ) -> Result<crate::VerifiedBadgeFacts, PeerBadgeVerificationError> {
        struct CancellationGuard(Arc<AtomicBool>);

        impl Drop for CancellationGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let _guard = CancellationGuard(Arc::clone(&self.cancelled));
        self.started.notify_one();
        pending().await
    }
}

/// Fully compatible live availability for the shared test request shape.
fn compatible_availability() -> GetAvailabilityResponse {
    GetAvailabilityResponse {
        accepting_seats: true,
        fedimintd_version: FEDIMINTD_VERSION_0_1.parse().expect("test version parses"),
        federation_sizes: vec![FederationSize(MIN_FEDERATION_SIZE)],
        plans: vec![
            fedi_decentralized_service_fleet_manager::Plan::InfiniteBestEffort {
                price_msats: AD_PRICE_MSATS,
            },
        ],
        additional_info: Vec::new(),
    }
}

/// Configured live outcome for one stubbed service key.
#[derive(Clone)]
enum StubProbeOutcome {
    Available(GetAvailabilityResponse),
    Unreachable,
    Hang,
}

/// Deterministic stand-in for the live availability probe: outcomes come
/// from a per-service-key table, unlisted keys answer with a fully
/// compatible live response, and every dialed key is recorded in walk
/// order.
#[derive(Clone, Default)]
struct StubProber {
    outcomes: Arc<Mutex<BTreeMap<secp256k1::XOnlyPublicKey, StubProbeOutcome>>>,
    dialed: Arc<Mutex<Vec<secp256k1::XOnlyPublicKey>>>,
}

impl StubProber {
    fn with_outcome(service_pubkey: secp256k1::XOnlyPublicKey, outcome: StubProbeOutcome) -> Self {
        let stub = Self::default();
        stub.outcomes
            .lock()
            .expect("test lock")
            .insert(service_pubkey, outcome);
        stub
    }

    fn dialed(&self) -> Vec<secp256k1::XOnlyPublicKey> {
        self.dialed.lock().expect("test lock").clone()
    }
}

impl SelectionAvailabilityProber for StubProber {
    async fn probe(&self, locator: &crate::Locator) -> LiveProbeOutcome {
        self.dialed
            .lock()
            .expect("test lock")
            .push(locator.service_pubkey);
        let outcome = self
            .outcomes
            .lock()
            .expect("test lock")
            .get(&locator.service_pubkey)
            .cloned();
        match outcome {
            None => LiveProbeOutcome::Available(compatible_availability()),
            Some(StubProbeOutcome::Available(availability)) => {
                LiveProbeOutcome::Available(availability)
            }
            Some(StubProbeOutcome::Unreachable) => {
                LiveProbeOutcome::Unreachable("stub FMan is unreachable".to_owned())
            }
            Some(StubProbeOutcome::Hang) => pending().await,
        }
    }
}

async fn eligible(events: Vec<Event>) -> Vec<crate::EligibleFmanCandidate> {
    let discovery =
        discover_fman_candidates_with(&registry(events), &requirements(), generous_deadline(), NOW)
            .await
            .expect("discovery completes");
    assert!(discovery.rejected.is_empty(), "{:?}", discovery.rejected);
    discovery.candidates
}

async fn select(
    events: Vec<Event>,
    verifier: &StubBadgeVerifier,
    seats: u16,
) -> (Vec<crate::SelectedFmanSeat>, Vec<RejectedAdvertisement>) {
    let candidates = eligible(events).await;
    let mut rejected = Vec::new();
    let selected = select_fman_seats(
        verifier,
        &AdOnlySelection,
        &preview_request(MIN_FEDERATION_SIZE),
        &fedimintd_version().dkg_version(),
        candidates,
        FederationSize(seats),
        BTreeMap::new(),
        generous_deadline(),
        &mut rejected,
    )
    .await;
    (selected, rejected)
}

/// Probe walk variant of [`select`] with a configurable prober.
async fn select_probed(
    events: Vec<Event>,
    verifier: &StubBadgeVerifier,
    prober: &StubProber,
    seats: u16,
) -> (Vec<crate::SelectedFmanSeat>, Vec<RejectedAdvertisement>) {
    let candidates = eligible(events).await;
    let mut rejected = Vec::new();
    let selected = select_fman_seats(
        verifier,
        prober,
        &preview_request(MIN_FEDERATION_SIZE),
        &fedimintd_version().dkg_version(),
        candidates,
        FederationSize(seats),
        BTreeMap::new(),
        generous_deadline(),
        &mut rejected,
    )
    .await;
    (selected, rejected)
}

#[tokio::test]
async fn replacement_preview_excludes_every_persisted_sibling_locator() {
    let excluded_fman = fman_keys(1);
    let fresh_fman = fman_keys(2);
    let mut excluded_payload = self_authorized_payload(&excluded_fman);
    excluded_payload.service_pubkey = excluded_fman.public_key().to_string();
    let mut fresh_payload = self_authorized_payload(&fresh_fman);
    fresh_payload.service_pubkey = fresh_fman.public_key().to_string();
    let events = vec![
        ad_event(&excluded_fman, excluded_payload),
        ad_event(&fresh_fman, fresh_payload),
    ];
    let previous_locator = locator(0);
    let requirements = GuardianReplacementRequirements {
        replacement_id: GuardianReplacementId::from_digest([17; 32]),
        seats: vec![GuardianReplacementSeat {
            index: 3,
            previous_fman_id: None,
            previous_quote_id: QuoteId([41; 32]),
            previous_locator,
        }],
    };
    let request = FmanSelectionRequest::new(
        FederationSize(MIN_FEDERATION_SIZE),
        fedimintd_version_range(),
        PlanPreference::InfiniteBestEffort,
    )
    .unwrap();
    let excluded_service_key = excluded_fman
        .public_key()
        .to_string()
        .parse()
        .expect("Nostr x-only key is a service key");
    let preview = preview_fman_replacements_with(
        &registry(events),
        &StubBadgeVerifier::default(),
        &AdOnlySelection,
        test_peer_badge_verifier().provenance(),
        &request,
        &fedimintd_version().dkg_version(),
        requirements.clone(),
        BTreeSet::from([excluded_service_key]),
        BTreeMap::new(),
        generous_deadline(),
        NOW,
        || NOW,
    )
    .await
    .unwrap();

    assert_eq!(preview.requirements(), &requirements);
    assert_eq!(preview.seats().len(), 1);
    assert_eq!(preview.valid_until(), Timestamp(NOW + 120));
    assert_eq!(
        preview.seats()[0]
            .candidate()
            .locator()
            .service_pubkey
            .to_string(),
        fresh_fman.public_key().to_string(),
        "preview must not offer an existing or terminal sibling again",
    );
}

#[tokio::test]
async fn replacement_preview_skips_a_retained_service_key_and_continues_the_bucket() {
    let colliding_fman = fman_keys(1);
    let fallback_fman = fman_keys(2);
    let issuer = issuer_keys(1);
    let retained_service_pubkey = manager_key(0).x_only_public_key().0;
    let fallback_service_pubkey = manager_key(1).x_only_public_key().0;
    let events = vec![
        issuer_ad_with_service_pubkey(&colliding_fman, &issuer, 0, 1, retained_service_pubkey),
        issuer_ad_with_service_pubkey(&fallback_fman, &issuer, 1, 1, fallback_service_pubkey),
    ];
    let requirements = GuardianReplacementRequirements {
        replacement_id: serde_json::from_value(serde_json::json!("42".repeat(32)))
            .expect("canonical replacement digest parses"),
        seats: vec![GuardianReplacementSeat {
            index: 3,
            previous_fman_id: None,
            previous_quote_id: QuoteId([42; 32]),
            previous_locator: locator(3),
        }],
    };
    let request = FmanSelectionRequest::new(
        FederationSize(MIN_FEDERATION_SIZE),
        fedimintd_version_range(),
        PlanPreference::InfiniteBestEffort,
    )
    .unwrap();
    let retained_fman = test_fman_id(0);
    let verifier = StubBadgeVerifier::default();

    let preview = preview_fman_replacements_with(
        &registry(events),
        &verifier,
        &AdOnlySelection,
        test_peer_badge_verifier().provenance(),
        &request,
        &fedimintd_version().dkg_version(),
        requirements,
        BTreeSet::new(),
        BTreeMap::from([(retained_service_pubkey, retained_fman)]),
        generous_deadline(),
        NOW,
        || NOW,
    )
    .await
    .expect("the walk continues after the retained-authority collision");

    assert_eq!(preview.seats().len(), 1);
    assert_eq!(
        preview.seats()[0].candidate().fman_id(),
        fallback_fman.public_key(),
    );
    assert!(preview.seats()[0].candidate().locator().service_pubkey == fallback_service_pubkey);
    assert_eq!(
        verifier.calls.load(Ordering::SeqCst),
        2,
        "the colliding candidate is verified, rejected, and the bucket continues",
    );
}

async fn replacement_preview_for_public_approval(
    requirements: GuardianReplacementRequirements,
    excluded: BTreeSet<PublicKey>,
    advertised_price_msats: u64,
    completed_at: u64,
) -> FmanReplacementPreview {
    replacement_preview_for_version(
        requirements,
        excluded,
        advertised_price_msats,
        FEDIMINTD_VERSION_0_1,
        completed_at,
    )
    .await
}

async fn replacement_preview_for_version(
    requirements: GuardianReplacementRequirements,
    excluded: BTreeSet<PublicKey>,
    advertised_price_msats: u64,
    version: &str,
    completed_at: u64,
) -> FmanReplacementPreview {
    let fman = fman_keys(19);
    let issuer = issuer_keys(1);
    let service_pubkey = manager_key(usize::from(MAX_FEDERATION_SIZE))
        .x_only_public_key()
        .0;
    let event = issuer_ad_for_version_and_service_key_at(
        &fman,
        &issuer,
        advertised_price_msats,
        version,
        service_pubkey,
        NOW,
    );
    let registry = registry(vec![event]);
    let request = FmanSelectionRequest::new(
        FederationSize(MIN_FEDERATION_SIZE),
        FedimintdVersionRange::new(
            "0.11.1".parse().expect("range minimum parses"),
            "0.11.3".parse().expect("range maximum parses"),
        )
        .expect("replacement range is ordered"),
        PlanPreference::InfiniteBestEffort,
    )
    .expect("test replacement request is valid");

    preview_fman_replacements_with(
        &registry,
        &StubBadgeVerifier::default(),
        &AdOnlySelection,
        test_peer_badge_verifier().provenance(),
        &request,
        &fedimintd_version().dkg_version(),
        requirements,
        excluded,
        BTreeMap::new(),
        generous_deadline(),
        NOW,
        || completed_at,
    )
    .await
    .expect("replacement preview succeeds")
}

#[tokio::test]
async fn replacement_preview_accepts_patch_skew_in_the_selected_dkg_identity() {
    let requirements = GuardianReplacementRequirements {
        replacement_id: GuardianReplacementId::from_digest([99; 32]),
        seats: vec![GuardianReplacementSeat {
            index: 3,
            previous_fman_id: None,
            previous_quote_id: QuoteId([42; 32]),
            previous_locator: locator(0),
        }],
    };
    let preview = replacement_preview_for_version(
        requirements,
        BTreeSet::new(),
        AD_PRICE_MSATS,
        "0.11.2+fedi",
        NOW,
    )
    .await;

    assert_eq!(preview.seats().len(), 1);
}

#[tokio::test]
async fn replacement_preview_public_approval_seals_cap_and_expires_before_effects() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::paid()
    };
    let client = open_client(database, payments, fman_state.clone(), config).await;
    let initial_cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);
    let error = client
        .pay_and_create(
            intent(),
            selection_approval(initial_cap),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, FiError::SeatRefused { .. }), "{error:?}");
    let status = client.status();
    let initial_formation = formation(&status);
    let FormationActionRequired::ReplaceGuardians(requirements) = initial_formation
        .action_required
        .clone()
        .expect("the refused selected seat requires replacement")
    else {
        panic!("selected refusal exposed the wrong recovery action")
    };
    let replaced_index = requirements.seats[0].index;
    let excluded = initial_formation
        .seats
        .iter()
        .map(|seat| {
            PublicKey::from_slice(&seat.locator.service_pubkey.serialize())
                .expect("formation service key is a valid Nostr public key")
        })
        .collect::<BTreeSet<_>>();

    let below_estimate = replacement_preview_for_public_approval(
        requirements.clone(),
        excluded.clone(),
        PAYMENT_AMOUNT_MSATS,
        test_now_secs(),
    )
    .await;
    assert_eq!(
        below_estimate.total_advertised_msats(),
        PAYMENT_AMOUNT_MSATS
    );
    let error = below_estimate
        .approve(PAYMENT_AMOUNT_MSATS - 1)
        .unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::AdvertisementEstimateExceedsLimit
        )
    ));

    let expired_completed_at =
        test_now_secs().saturating_sub(crate::FMAN_SELECTION_PREVIEW_VALIDITY.as_secs() + 1);
    let expired = replacement_preview_for_public_approval(
        requirements.clone(),
        excluded.clone(),
        PAYMENT_AMOUNT_MSATS,
        expired_completed_at,
    )
    .await
    .approve(PAYMENT_AMOUNT_MSATS)
    .expect("the displayed estimate fits its cap before expiry is consumed");
    assert_eq!(expired.requirements(), &requirements);
    assert_eq!(expired.max_total_msats(), PAYMENT_AMOUNT_MSATS);
    assert_eq!(
        expired.valid_until(),
        Timestamp(expired_completed_at + crate::FMAN_SELECTION_PREVIEW_VALIDITY.as_secs())
    );
    let payment_calls_before_expired = payment_state.create_calls.load(Ordering::SeqCst);
    let quote_calls_before_expired = fman_state.quote_calls.load(Ordering::SeqCst);
    let create_calls_before_expired = fman_state.create_calls.load(Ordering::SeqCst);
    let error = client
        .apply_fman_replacements(expired, options())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(SelectionReauthorizationReason::PreviewExpired)
    ));
    let status_after_expiry = client.status();
    assert_eq!(
        formation(&status_after_expiry).action_required,
        Some(FormationActionRequired::ReplaceGuardians(
            requirements.clone()
        ))
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        payment_calls_before_expired
    );
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        quote_calls_before_expired
    );
    assert_eq!(
        fman_state.create_calls.load(Ordering::SeqCst),
        create_calls_before_expired
    );

    let preview_completed_at = test_now_secs();
    let preview = replacement_preview_for_public_approval(
        requirements.clone(),
        excluded,
        PAYMENT_AMOUNT_MSATS,
        preview_completed_at,
    )
    .await;
    assert_eq!(
        preview.valid_until(),
        Timestamp(preview_completed_at + crate::FMAN_SELECTION_PREVIEW_VALIDITY.as_secs())
    );
    let replacement_locator = preview.seats()[0].candidate().locator().clone();
    let approval = preview
        .approve(PAYMENT_AMOUNT_MSATS)
        .expect("the displayed estimate fits the renewed cap");
    assert_eq!(approval.requirements(), &requirements);
    assert_eq!(approval.max_total_msats(), PAYMENT_AMOUNT_MSATS);
    assert_eq!(
        approval.valid_until(),
        Timestamp(preview_completed_at + crate::FMAN_SELECTION_PREVIEW_VALIDITY.as_secs())
    );
    client
        .apply_fman_replacements(approval, options())
        .await
        .unwrap();

    let formed_status = client.status();
    let formed = formation(&formed_status);
    assert_eq!(formed.phase, FormationPhase::Formed);
    assert_eq!(
        formed.seats[usize::from(replaced_index)].locator,
        replacement_locator
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        payment_calls_before_expired + 1,
        "the sealed public replacement approval funds only its exact row"
    );
}

fn priced_payload(
    fman: &Keys,
    envelopes: Vec<HolderAuthorizationEnvelope>,
    price_msats: u64,
    _slots: u32,
) -> fedi_decentralized_nostr::fman::AdvertisementPayload {
    let mut payload = payload(fman, envelopes);
    payload.plans = vec![Plan::InfiniteBestEffort { price_msats }];
    payload
}

fn issuer_ad(fman: &Keys, issuer: &Keys, price_msats: u64, slots: u32) -> Event {
    ad_event(
        fman,
        priced_payload(
            fman,
            vec![envelope_with_issuer(
                &holder_keys(),
                fman.public_key(),
                issuer,
            )],
            price_msats,
            slots,
        ),
    )
}

pub(super) fn issuer_ad_for_version(
    fman: &Keys,
    issuer: &Keys,
    price_msats: u64,
    version: &str,
) -> Event {
    issuer_ad_for_version_at(fman, issuer, price_msats, version, NOW)
}

pub(super) fn issuer_ad_for_version_at(
    fman: &Keys,
    issuer: &Keys,
    price_msats: u64,
    version: &str,
    now: u64,
) -> Event {
    issuer_ad_for_version_and_service_key_at(
        fman,
        issuer,
        price_msats,
        version,
        service_pubkey(fman),
        now,
    )
}

pub(super) fn issuer_ad_for_version_and_service_key_at(
    fman: &Keys,
    issuer: &Keys,
    price_msats: u64,
    version: &str,
    service_pubkey: secp256k1::XOnlyPublicKey,
    now: u64,
) -> Event {
    let mut payload = priced_payload(
        fman,
        vec![envelope_with_issuer(
            &holder_keys(),
            fman.public_key(),
            issuer,
        )],
        price_msats,
        1,
    );
    payload.issued_at = now.saturating_sub(1);
    payload.expires_at = now + 3_600;
    payload.availability.fedimintd_version = version.parse().expect("test version parses");
    payload.service_pubkey = service_pubkey.to_string();
    ad_event(fman, payload)
}

fn issuer_ad_with_service_pubkey(
    fman: &Keys,
    issuer: &Keys,
    price_msats: u64,
    slots: u32,
    service_pubkey: secp256k1::XOnlyPublicKey,
) -> Event {
    let mut payload = priced_payload(
        fman,
        vec![envelope_with_issuer(
            &holder_keys(),
            fman.public_key(),
            issuer,
        )],
        price_msats,
        slots,
    );
    payload.service_pubkey = service_pubkey.to_string();
    ad_event(fman, payload)
}

async fn verified_zero_price_selection_approval() -> FmanSelectionApproval {
    let issuer = issuer_keys(1);
    let events = (0..usize::from(MIN_FEDERATION_SIZE))
        .map(|index| {
            issuer_ad_with_service_pubkey(
                &super::fman_keys(index),
                &issuer,
                0,
                1,
                manager_key(index).x_only_public_key().0,
            )
        })
        .collect::<Vec<_>>();
    let preview = preview_fman_selection_with(
        &registry(events),
        &StubBadgeVerifier::default(),
        &AdOnlySelection,
        test_peer_badge_verifier().provenance(),
        &preview_request(MIN_FEDERATION_SIZE),
        generous_deadline(),
        NOW,
        test_now_secs,
    )
    .await
    .expect("the product selection is fully verified");
    assert_eq!(preview.total_advertised_msats(), 0);
    preview
        .approve(0)
        .expect("a zero cap seals an all-zero bootstrap estimate")
}

#[tokio::test]
async fn selected_verified_zero_price_bootstrap_needs_no_publisher_or_payer() {
    let approval = verified_zero_price_selection_approval().await;
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client_that_cannot_pay(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;

    client
        .create_without_payer(intent(), approval, options())
        .await
        .expect("the first verified federation forms from all-zero quotes");

    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.readiness_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
    );
    assert!(
        fman_state
            .quote_records
            .lock()
            .expect("test quote records")
            .iter()
            .all(|record| record.payment_federation_id.is_none()),
    );
}

#[tokio::test]
async fn selected_without_payer_rejects_a_priced_live_offer_before_effects() {
    let approval = verified_zero_price_selection_approval().await;
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client_that_cannot_pay(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;

    let error = client
        .create_without_payer(intent(), approval, options())
        .await
        .expect_err("a priced live offer requires a payer retry");

    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::PaymentFederationRequired
        )
    ));
    assert_eq!(client.status(), FiStatus::Idle);
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.readiness_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.quote_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn walk_seats_verified_candidates_and_projects_badge_facts() {
    let fman = fman_keys(1);
    let verifier = StubBadgeVerifier::default();
    let (seats, rejected) = select(
        vec![ad_event(&fman, self_authorized_payload(&fman))],
        &verifier,
        1,
    )
    .await;

    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(seats.len(), 1);
    let seat = &seats[0];
    assert_eq!(seat.candidate().fman_id(), fman.public_key());
    assert_eq!(
        seat.candidate().fman_name(),
        fedi_decentralized_service_fleet_manager::FmanName::from_fman_id(fman.public_key()),
    );
    assert_eq!(seat.candidate().advertised_price_msats(), AD_PRICE_MSATS);
    assert_eq!(seat.advertised_price_msats(), AD_PRICE_MSATS);
    assert_eq!(seat.provenance(), SeatProvenance::FediAttested);
    assert_eq!(seat.candidate().badge().subject(), fman.public_key());
    assert_eq!(
        seat.candidate().badge().holder(),
        holder_keys().public_key()
    );
    assert_eq!(
        seat.candidate().badge().issuer(),
        issuer_keys(0).public_key()
    );
    assert_eq!(
        seat.candidate().badge().badge().trust_level,
        test_peer_badge_minimum_trust_level()
    );
}

#[tokio::test]
async fn badge_subject_binding_mismatch_is_rejected() {
    // A valid badge authorizing another operator's service key, presented on
    // this FMan's otherwise-valid advertisement, must fail the author binding.
    let fman = fman_keys(1);
    let other_subject = fman_keys(9).public_key();
    let event = ad_event(
        &fman,
        payload(&fman, vec![envelope(&holder_keys(), other_subject)]),
    );

    let verifier = StubBadgeVerifier::default();
    let (seats, rejected) = select(vec![event], &verifier, 1).await;
    assert!(seats.is_empty());
    assert_eq!(rejected.len(), 1);
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::SubjectMismatch
    ));
}

#[tokio::test]
async fn verifier_rejection_is_propagated() {
    let fman = fman_keys(1);
    let event = ad_event(&fman, self_authorized_payload(&fman));

    let verifier = StubBadgeVerifier::rejecting(fman.public_key());
    let (seats, rejected) = select(vec![event], &verifier, 1).await;
    assert!(seats.is_empty());
    assert_eq!(rejected.len(), 1);
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::BadgeRejected(PeerBadgeVerificationError::CredentialRevoked)
    ));
}

#[tokio::test(start_paused = true)]
async fn preview_deadline_cancels_an_in_flight_verification() {
    let events = (1..=MIN_FEDERATION_SIZE)
        .map(|index| {
            let fman = fman_keys(u8::try_from(index).expect("small test index"));
            ad_event(&fman, self_authorized_payload(&fman))
        })
        .collect::<Vec<_>>();
    let verification_started = Arc::new(Notify::new());
    let cancelled = Arc::new(AtomicBool::new(false));
    let verifier = PendingBadgeVerifier {
        started: Arc::clone(&verification_started),
        cancelled: Arc::clone(&cancelled),
    };
    let started = verification_started.notified();
    let began = Instant::now();
    let deadline = began + Duration::from_secs(1);

    let task = tokio::spawn(async move {
        preview_fman_selection_until(
            &registry(events),
            &verifier,
            &AdOnlySelection,
            PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
            &preview_request(MIN_FEDERATION_SIZE),
            deadline,
            NOW,
            || NOW,
        )
        .await
    });

    started.await;
    assert!(Instant::now() < deadline);
    assert!(!cancelled.load(Ordering::SeqCst));
    tokio::time::advance(Duration::from_secs(1)).await;

    let error = task
        .await
        .expect("selection task does not panic")
        .expect_err("the preview deadline expires");
    assert!(matches!(error, FiError::SelectionPreviewTimeout), "{error}");
    assert_eq!(Instant::now().duration_since(began), Duration::from_secs(1));
    assert!(
        cancelled.load(Ordering::SeqCst),
        "timing out the preview drops the in-flight verifier future"
    );
}

#[tokio::test(start_paused = true)]
async fn already_expired_preview_returns_timeout_before_inner_shortfall() {
    let verifier = StubBadgeVerifier::default();

    let error = preview_fman_selection_until(
        &registry(Vec::new()),
        &verifier,
        &AdOnlySelection,
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        &preview_request(MIN_FEDERATION_SIZE),
        Instant::now(),
        NOW,
        || NOW,
    )
    .await
    .expect_err("the absolute deadline already elapsed");

    assert!(matches!(error, FiError::SelectionPreviewTimeout), "{error}");
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn completion_at_deadline_is_timeout_even_before_timer_wake() {
    let deadline = Instant::now();
    let inner_shortfall = Err(FiError::InsufficientFmanSeats {
        requested: MIN_FEDERATION_SIZE,
        selected: 0,
        seen: 0,
        eligible: 0,
    });

    let error = crate::selection::preview_result_before_deadline(inner_shortfall, deadline)
        .expect_err("completion at the deadline is a timeout");

    assert!(matches!(error, FiError::SelectionPreviewTimeout), "{error}");
}

#[tokio::test(start_paused = true)]
async fn registry_only_public_preview_uses_its_configured_deadline() {
    let query = FmanRegistryQuery::new(super::discovery::SlowRegistry {
        registry: registry(Vec::new()),
        delay: Duration::from_secs(10),
    })
    .with_verifier(test_peer_badge_verifier());
    let began = Instant::now();

    let error = query
        .preview_fman_selection(
            &preview_request(MIN_FEDERATION_SIZE),
            FmanDiscoveryOptions::with_timeout(Duration::from_secs(1)),
        )
        .await
        .expect_err("the public registry-only preview times out");

    assert!(matches!(error, FiError::SelectionPreviewTimeout), "{error}");
    assert_eq!(Instant::now().duration_since(began), Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn full_client_public_preview_uses_its_configured_deadline() {
    let registry = TestRegistry {
        advertisement_delay: Duration::from_secs(10),
        ..TestRegistry::default()
    };
    let (payments, _) = TestPayments::new();
    let client = open_client_with_registry(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::given_away(),
        registry,
    )
    .await;
    let began = Instant::now();

    let error = client
        .preview_fman_selection(
            &preview_request(MIN_FEDERATION_SIZE),
            FmanDiscoveryOptions::with_timeout(Duration::from_secs(1)),
        )
        .await
        .expect_err("the full public preview times out");

    assert!(matches!(error, FiError::SelectionPreviewTimeout), "{error}");
    assert_eq!(Instant::now().duration_since(began), Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn preview_completing_before_the_deadline_preserves_selection() {
    let events = (1..=MIN_FEDERATION_SIZE)
        .map(|index| {
            let fman = fman_keys(u8::try_from(index).expect("small test index"));
            ad_event(&fman, self_authorized_payload(&fman))
        })
        .collect::<Vec<_>>();
    let verifier = StubBadgeVerifier {
        delay: Some(Duration::from_millis(100)),
        ..StubBadgeVerifier::default()
    };
    let began = Instant::now();
    let deadline = began + Duration::from_secs(1);

    let preview = preview_fman_selection_until(
        &registry(events),
        &verifier,
        &AdOnlySelection,
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        &preview_request(MIN_FEDERATION_SIZE),
        deadline,
        NOW,
        || NOW,
    )
    .await
    .expect("selection completes within its deadline");

    assert_eq!(preview.seats().len(), usize::from(MIN_FEDERATION_SIZE));
    assert_eq!(
        Instant::now().duration_since(began),
        Duration::from_millis(700)
    );
    assert!(Instant::now() < deadline);
}

#[tokio::test]
async fn a_badge_verifying_under_a_different_issuer_cannot_seat_the_candidate() {
    // The bucketing issuer is the first envelope's untrusted claim. Here the
    // first envelope fails the subject binding, and the second verifies and
    // binds — but under a different issuer than the claim. Seating it would
    // place a verified candidate in a bucket it does not belong to, so the
    // candidate is dropped with the decisive typed issuer mismatch.
    let fman = fman_keys(1);
    let claim = envelope_with_issuer(&holder_keys(), fman_keys(9).public_key(), &issuer_keys(1));
    let other_issuer = envelope_with_issuer(&holder_keys(), fman.public_key(), &issuer_keys(2));
    let event = ad_event(&fman, payload(&fman, vec![claim, other_issuer]));

    let verifier = StubBadgeVerifier::default();
    let (seats, rejected) = select(vec![event], &verifier, 1).await;
    assert!(seats.is_empty());
    assert_eq!(rejected.len(), 1);
    assert!(
        matches!(
            rejected[0].reason,
            AdvertisementRejection::ClaimedIssuerMismatch
        ),
        "the verified-but-mismatched badge is the decisive reason: {:?}",
        rejected[0].reason
    );
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn envelope_examination_stops_at_the_bounded_prefix() {
    // Five embedded envelopes, all failing: only the first
    // FMAN_ADVERTISEMENT_MAX_HOLDER_AUTHORIZATIONS cost verifier work.
    let fman = fman_keys(1);
    let other_subject = fman_keys(9).public_key();
    let envelopes = (0..5)
        .map(|_| envelope(&holder_keys(), other_subject))
        .collect::<Vec<_>>();
    let event = ad_event(&fman, payload(&fman, envelopes));

    let verifier = StubBadgeVerifier::default();
    let (seats, rejected) = select(vec![event], &verifier, 1).await;
    assert!(seats.is_empty());
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::SubjectMismatch
    ));
    assert_eq!(
        verifier.calls.load(Ordering::SeqCst),
        FMAN_ADVERTISEMENT_MAX_HOLDER_AUTHORIZATIONS,
        "examination stops at the bounded envelope prefix",
    );
}

#[tokio::test]
async fn later_envelope_in_the_prefix_can_seat_the_candidate() {
    // The first envelope fails the author binding; the second verifies,
    // binds, and matches the claimed issuer, so the candidate seats.
    let fman = fman_keys(1);
    let other_subject = fman_keys(9).public_key();
    let event = ad_event(
        &fman,
        payload(
            &fman,
            vec![
                envelope(&holder_keys(), other_subject),
                envelope(&holder_keys(), fman.public_key()),
            ],
        ),
    );

    let verifier = StubBadgeVerifier::default();
    let (seats, rejected) = select(vec![event], &verifier, 1).await;
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(seats.len(), 1);
    assert_eq!(seats[0].candidate().badge().subject(), fman.public_key());
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn mixed_envelope_failures_report_the_first_in_examination_order() {
    // The reported reason follows examination order, not any preference
    // between failure kinds: whichever envelope fails first names the reason.
    let fman = fman_keys(1);
    let mismatched_subject = fman_keys(9).public_key();
    let rejected_subject = fman_keys(8).public_key();
    let mismatch = envelope(&holder_keys(), mismatched_subject);
    let rejected_envelope = envelope(&holder_keys(), rejected_subject);

    let verifier = StubBadgeVerifier::rejecting(rejected_subject);
    let (_, rejected) = select(
        vec![ad_event(
            &fman,
            payload(&fman, vec![mismatch.clone(), rejected_envelope.clone()]),
        )],
        &verifier,
        1,
    )
    .await;
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::SubjectMismatch
    ));

    let verifier = StubBadgeVerifier::rejecting(rejected_subject);
    let (_, rejected) = select(
        vec![ad_event(
            &fman,
            payload(&fman, vec![rejected_envelope, mismatch]),
        )],
        &verifier,
        1,
    )
    .await;
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::BadgeRejected(PeerBadgeVerificationError::CredentialRevoked)
    ));
}

#[tokio::test]
async fn preview_estimate_overflow_is_a_typed_selection_error() {
    // Every seat fills, but the aggregate advertised estimate overflows:
    // that is a selection outcome, not a registry failure.
    let issuer = issuer_keys(1);
    let events = (1..=MIN_FEDERATION_SIZE)
        .map(|index| {
            issuer_ad(
                &fman_keys(u8::try_from(index).expect("small test index")),
                &issuer,
                u64::MAX,
                3,
            )
        })
        .collect::<Vec<_>>();

    let error = preview_fman_selection_with(
        &registry(events),
        &StubBadgeVerifier::default(),
        &AdOnlySelection,
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        &preview_request(MIN_FEDERATION_SIZE),
        generous_deadline(),
        NOW,
        || NOW,
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, FiError::SelectionEstimateOverflow),
        "{error}"
    );
    assert_eq!(error.code(), FiErrorCode::Selection);
}

#[tokio::test]
async fn buckets_fill_round_robin_across_claimed_issuers() {
    // Two issuers with two candidates each; three seats must take two
    // regions' cheapest before any region's second candidate.
    let issuer_a = issuer_keys(1);
    let issuer_b = issuer_keys(2);
    let a_cheap = fman_keys(1);
    let a_pricey = fman_keys(2);
    let b_cheap = fman_keys(3);
    let b_pricey = fman_keys(4);
    let events = vec![
        issuer_ad(&a_pricey, &issuer_a, 2_000, 3),
        issuer_ad(&a_cheap, &issuer_a, 1_000, 3),
        issuer_ad(&b_pricey, &issuer_b, 4_000, 3),
        issuer_ad(&b_cheap, &issuer_b, 3_000, 3),
    ];

    let verifier = StubBadgeVerifier::default();
    let (seats, rejected) = select(events, &verifier, 3).await;
    assert!(rejected.is_empty(), "{rejected:?}");

    let selected = seats
        .iter()
        .map(|seat| seat.candidate().fman_id())
        .collect::<Vec<_>>();
    let mut expected_first_round = [a_cheap.public_key(), b_cheap.public_key()];
    // Buckets iterate in issuer-key order; the cheapest of each bucket comes
    // first, then the round-robin returns for one bucket's second candidate.
    if issuer_b.public_key() < issuer_a.public_key() {
        expected_first_round.reverse();
    }
    assert_eq!(&selected[..2], &expected_first_round);
    let third = selected[2];
    let expected_third = if issuer_b.public_key() < issuer_a.public_key() {
        b_pricey.public_key()
    } else {
        a_pricey.public_key()
    };
    assert_eq!(third, expected_third);
    assert_eq!(
        verifier.calls.load(Ordering::SeqCst),
        3,
        "only walked candidates cost verifier round trips",
    );
}

#[tokio::test]
async fn distinct_verified_authors_sharing_a_service_key_cannot_both_seat() {
    let issuer = issuer_keys(1);
    let first = fman_keys(1);
    let duplicate = fman_keys(2);
    let shared_service_pubkey = service_pubkey(&first);
    let events = vec![
        issuer_ad_with_service_pubkey(&first, &issuer, 1_000, 3, shared_service_pubkey),
        issuer_ad_with_service_pubkey(&duplicate, &issuer, 2_000, 3, shared_service_pubkey),
    ];

    let verifier = StubBadgeVerifier::default();
    let (seats, rejected) = select(events, &verifier, 2).await;

    assert_eq!(seats.len(), 1);
    assert_eq!(seats[0].candidate().fman_id(), first.public_key());
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].author, duplicate.public_key());
    assert!(matches!(
        &rejected[0].reason,
        AdvertisementRejection::DuplicateServicePubkey { selected_fman }
            if *selected_fman == first.public_key()
    ));
    assert_eq!(
        rejected[0].reason.code(),
        "duplicate_service_pubkey",
        "the collision is a stable diagnostic, not a generic badge failure",
    );
    assert_eq!(
        verifier.calls.load(Ordering::SeqCst),
        2,
        "the duplicate author must verify before its service-key claim can be rejected",
    );
}

#[tokio::test]
async fn duplicate_service_key_keeps_the_colliding_buckets_turn() {
    let mut issuers = [issuer_keys(1), issuer_keys(2)];
    issuers.sort_by_key(Keys::public_key);
    let first = fman_keys(1);
    let first_backup = fman_keys(2);
    let duplicate = fman_keys(3);
    let duplicate_backup = fman_keys(4);
    let shared_service_pubkey = service_pubkey(&first);
    let events = vec![
        issuer_ad_with_service_pubkey(&first, &issuers[0], 1_000, 3, shared_service_pubkey),
        issuer_ad(&first_backup, &issuers[0], 2_000, 3),
        issuer_ad_with_service_pubkey(&duplicate, &issuers[1], 1_000, 3, shared_service_pubkey),
        issuer_ad(&duplicate_backup, &issuers[1], 2_000, 3),
    ];

    let verifier = StubBadgeVerifier::default();
    let (seats, rejected) = select(events, &verifier, 3).await;

    assert_eq!(rejected.len(), 1);
    assert!(matches!(
        &rejected[0].reason,
        AdvertisementRejection::DuplicateServicePubkey { selected_fman }
            if *selected_fman == first.public_key()
    ));
    assert_eq!(
        seats
            .iter()
            .map(|seat| seat.candidate().fman_id())
            .collect::<Vec<_>>(),
        vec![
            first.public_key(),
            duplicate_backup.public_key(),
            first_backup.public_key(),
        ],
        "the colliding bucket seats its next distinct service key before the walk advances",
    );
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn multi_round_walk_takes_one_seat_per_bucket_per_pass() {
    // The discovery-selection architecture worked example: for buckets [A..E], [F..J],
    // and [K..O], ten seats select A,F,K,B,G,L,C,H,M,D — one seat per
    // bucket per pass, repeating passes, never a bucket-contiguous prefix.
    let mut issuers = [issuer_keys(1), issuer_keys(2), issuer_keys(3)];
    issuers.sort_by_key(Keys::public_key);
    let mut events = Vec::new();
    let mut buckets: Vec<Vec<PublicKey>> = Vec::new();
    for (bucket, issuer) in issuers.iter().enumerate() {
        let mut members = Vec::new();
        for position in 0..5 {
            let fman = fman_keys(u8::try_from(bucket * 5 + position + 1).expect("small index"));
            // Distinct in-bucket prices pin the in-bucket order.
            events.push(issuer_ad(
                &fman,
                issuer,
                u64::try_from((position + 1) * 1_000).expect("small price"),
                3,
            ));
            members.push(fman.public_key());
        }
        buckets.push(members);
    }

    let verifier = StubBadgeVerifier::default();
    let (seats, rejected) = select(events, &verifier, 10).await;
    assert!(rejected.is_empty(), "{rejected:?}");
    let selected = seats
        .iter()
        .map(|seat| seat.candidate().fman_id())
        .collect::<Vec<_>>();
    let expected = vec![
        buckets[0][0],
        buckets[1][0],
        buckets[2][0],
        buckets[0][1],
        buckets[1][1],
        buckets[2][1],
        buckets[0][2],
        buckets[1][2],
        buckets[2][2],
        buckets[0][3],
    ];
    assert_eq!(
        selected, expected,
        "the walk interleaves one seat per bucket per pass",
    );
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 10);
}

#[tokio::test]
async fn in_bucket_order_is_price_first() {
    let issuer = issuer_keys(1);
    let cheap_second = fman_keys(1);
    let cheap_first = fman_keys(2);
    let pricey = fman_keys(3);
    let events = vec![
        issuer_ad(&pricey, &issuer, 1_000_000, 9),
        issuer_ad(&cheap_second, &issuer, 600_000, 1),
        issuer_ad(&cheap_first, &issuer, 500_000, 8),
    ];

    let verifier = StubBadgeVerifier::default();
    let (seats, rejected) = select(events, &verifier, 3).await;
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(
        seats
            .iter()
            .map(|seat| seat.candidate().fman_id())
            .collect::<Vec<_>>(),
        vec![
            cheap_first.public_key(),
            cheap_second.public_key(),
            pricey.public_key()
        ],
        "price ranks candidates within an issuer bucket",
    );
    assert_eq!(seats[0].advertised_price_msats(), 500_000);
    assert_eq!(seats[2].advertised_price_msats(), 1_000_000);
}

#[tokio::test]
async fn a_failing_candidate_keeps_its_buckets_turn() {
    // Bucket A's cheapest fails verification; A's next candidate takes the
    // seat before the walk moves to bucket B, preserving region spread.
    let issuer_a = issuer_keys(1);
    let issuer_b = issuer_keys(2);
    let (first_issuer, second_issuer) = if issuer_a.public_key() < issuer_b.public_key() {
        (issuer_a, issuer_b)
    } else {
        (issuer_b, issuer_a)
    };
    let failing = fman_keys(1);
    let backup = fman_keys(2);
    let other_region = fman_keys(3);
    let events = vec![
        issuer_ad(&failing, &first_issuer, 1_000, 3),
        issuer_ad(&backup, &first_issuer, 2_000, 3),
        issuer_ad(&other_region, &second_issuer, 1_500, 3),
    ];

    let verifier = StubBadgeVerifier::rejecting(failing.public_key());
    let (seats, rejected) = select(events, &verifier, 2).await;
    assert_eq!(rejected.len(), 1);
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::BadgeRejected(_)
    ));
    assert_eq!(
        seats
            .iter()
            .map(|seat| seat.candidate().fman_id())
            .collect::<Vec<_>>(),
        vec![backup.public_key(), other_region.public_key()],
        "the failing candidate's bucket still fills its diversity slot first",
    );
}

#[tokio::test]
async fn unwalked_candidates_cost_no_verifier_work() {
    let issuer = issuer_keys(1);
    let events = (1..=4)
        .map(|index| {
            issuer_ad(
                &fman_keys(u8::try_from(index).expect("small test index")),
                &issuer,
                1_000,
                3,
            )
        })
        .collect::<Vec<_>>();

    let verifier = StubBadgeVerifier::default();
    let (seats, rejected) = select(events, &verifier, 2).await;
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(seats.len(), 2);
    assert_eq!(
        verifier.calls.load(Ordering::SeqCst),
        2,
        "verification is lazy: candidates beyond the filled seats cost nothing",
    );
}

/// Connector whose availability call yields a configurable result after a
/// successful connect, for probing the concrete prober's error-arm mapping.
struct AvailabilityArmConnector {
    result: fn() -> Result<FmResult<GetAvailabilityResponse>, crate::FleetManagerCallError>,
}

impl crate::FleetManagerConnector for AvailabilityArmConnector {
    type Client = crate::unavailable::UnavailableFleetManagerClient;

    async fn connect(
        &self,
        _locator: &crate::Locator,
    ) -> Result<Self::Client, crate::FleetManagerConnectorError> {
        Ok(crate::unavailable::UnavailableFleetManagerClient)
    }

    async fn get_availability(
        &self,
        _client: &Self::Client,
        _request: GetAvailabilityRequest,
    ) -> Result<FmResult<GetAvailabilityResponse>, crate::FleetManagerCallError> {
        (self.result)()
    }

    async fn get_quote(
        &self,
        _client: &Self::Client,
        _request: GetQuoteRequest,
    ) -> Result<FmResult<SignedResponse<GetQuoteResponse>>, crate::FleetManagerCallError> {
        Err(crate::FleetManagerCallError::new("unused in probe tests"))
    }
}

#[tokio::test]
async fn concrete_prober_maps_every_arm_without_remote_text() {
    let target = locator(0);

    // No connector: the probe is skipped, the ad-only walk.
    let skipped =
        LiveAvailabilityProber::<crate::UnavailableFleetManagerConnector> { connector: None };
    assert!(matches!(
        skipped.probe(&target).await,
        LiveProbeOutcome::Skipped
    ));

    // Local connect failure: the sanitized connector error is reported.
    let unreachable = LiveAvailabilityProber {
        connector: Some(&crate::UnavailableFleetManagerConnector),
    };
    assert!(matches!(
        unreachable.probe(&target).await,
        LiveProbeOutcome::Unreachable(message)
            if message == "FMan transport capability unavailable"
    ));

    // Local call failure after connect: the sanitized call error is reported.
    let call_failure = AvailabilityArmConnector {
        result: || Err(crate::FleetManagerCallError::new("local stream loss")),
    };
    let prober = LiveAvailabilityProber {
        connector: Some(&call_failure),
    };
    assert!(matches!(
        prober.probe(&target).await,
        LiveProbeOutcome::Unreachable(message) if message == "local stream loss"
    ));

    // FMan-returned wire error: a fixed marker, never the remote error text.
    let wire_error = AvailabilityArmConnector {
        result: || {
            Ok(Err(FleetManagerError::Other(
                "remote-authored text".to_owned(),
            )))
        },
    };
    let prober = LiveAvailabilityProber {
        connector: Some(&wire_error),
    };
    assert!(matches!(
        prober.probe(&target).await,
        LiveProbeOutcome::Unreachable(message)
            if message == "Fleet Manager returned an error to the availability probe"
    ));

    // Live response: passed through for the predicate.
    let available = AvailabilityArmConnector {
        result: || Ok(Ok(compatible_availability())),
    };
    let prober = LiveAvailabilityProber {
        connector: Some(&available),
    };
    assert!(matches!(
        prober.probe(&target).await,
        LiveProbeOutcome::Available(availability) if availability == compatible_availability()
    ));
}

#[tokio::test]
async fn unverified_candidate_is_never_dialed() {
    let mut issuers = [issuer_keys(1), issuer_keys(2)];
    issuers.sort_by_key(Keys::public_key);
    let unverifiable = fman_keys(1);
    let backup = fman_keys(2);
    let events = vec![
        issuer_ad(&unverifiable, &issuers[0], 1_000, 3),
        issuer_ad(&backup, &issuers[0], 2_000, 3),
    ];

    let verifier = StubBadgeVerifier::rejecting(unverifiable.public_key());
    let prober = StubProber::default();
    let (seats, rejected) = select_probed(events, &verifier, &prober, 1).await;

    assert_eq!(seats.len(), 1);
    assert_eq!(rejected.len(), 1);
    assert_eq!(
        prober.dialed(),
        vec![service_pubkey(&backup)],
        "a candidate that fails badge verification costs no dial",
    );
}

#[tokio::test]
async fn live_unavailable_candidate_keeps_its_buckets_turn() {
    // Bucket A's cheapest advertises seats but its live response is closed;
    // A's next candidate takes the seat before the walk moves to bucket B.
    let mut issuers = [issuer_keys(1), issuer_keys(2)];
    issuers.sort_by_key(Keys::public_key);
    let stale = fman_keys(1);
    let backup = fman_keys(2);
    let other_region = fman_keys(3);
    let events = vec![
        issuer_ad(&stale, &issuers[0], 1_000, 3),
        issuer_ad(&backup, &issuers[0], 2_000, 3),
        issuer_ad(&other_region, &issuers[1], 1_500, 3),
    ];

    let prober = StubProber::with_outcome(
        service_pubkey(&stale),
        StubProbeOutcome::Available(GetAvailabilityResponse {
            accepting_seats: false,
            ..compatible_availability()
        }),
    );
    let (seats, rejected) = select_probed(events, &StubBadgeVerifier::default(), &prober, 2).await;

    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].author, stale.public_key());
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::LiveNotAcceptingSeats
    ));
    assert_eq!(
        seats
            .iter()
            .map(|seat| seat.candidate().fman_id())
            .collect::<Vec<_>>(),
        vec![backup.public_key(), other_region.public_key()],
        "the stale candidate's bucket seats its live backup before the walk advances",
    );
}

#[tokio::test]
async fn unreachable_candidate_keeps_its_buckets_turn() {
    let mut issuers = [issuer_keys(1), issuer_keys(2)];
    issuers.sort_by_key(Keys::public_key);
    let dead = fman_keys(1);
    let backup = fman_keys(2);
    let other_region = fman_keys(3);
    let events = vec![
        issuer_ad(&dead, &issuers[0], 1_000, 3),
        issuer_ad(&backup, &issuers[0], 2_000, 3),
        issuer_ad(&other_region, &issuers[1], 1_500, 3),
    ];

    let prober = StubProber::with_outcome(service_pubkey(&dead), StubProbeOutcome::Unreachable);
    let (seats, rejected) = select_probed(events, &StubBadgeVerifier::default(), &prober, 2).await;

    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].author, dead.public_key());
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::ProbeFailed { .. }
    ));
    assert_eq!(
        seats
            .iter()
            .map(|seat| seat.candidate().fman_id())
            .collect::<Vec<_>>(),
        vec![backup.public_key(), other_region.public_key()],
    );
}

#[tokio::test]
async fn live_mismatches_reject_with_their_typed_reasons() {
    let incompatible = [
        (
            GetAvailabilityResponse {
                federation_sizes: vec![FederationSize(MIN_FEDERATION_SIZE + 1)],
                ..compatible_availability()
            },
            "live_unsupported_federation_size",
        ),
        (
            GetAvailabilityResponse {
                fedimintd_version: "9.9.9+fedi".parse().expect("test version parses"),
                ..compatible_availability()
            },
            "live_unsupported_fedimintd_version",
        ),
        (
            GetAvailabilityResponse {
                fedimintd_version: "0.11.1+acme".parse().expect("test version parses"),
                ..compatible_availability()
            },
            "live_unsupported_fedimintd_version",
        ),
        (
            GetAvailabilityResponse {
                plans: Vec::new(),
                ..compatible_availability()
            },
            "live_no_requested_plan",
        ),
    ];
    for (availability, expected_code) in incompatible {
        let fman = fman_keys(1);
        let events = vec![ad_event(&fman, self_authorized_payload(&fman))];
        let prober = StubProber::with_outcome(
            service_pubkey(&fman),
            StubProbeOutcome::Available(availability),
        );

        let (seats, rejected) =
            select_probed(events, &StubBadgeVerifier::default(), &prober, 1).await;

        assert!(seats.is_empty());
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].reason.code(), expected_code);
    }
}

#[tokio::test]
async fn duplicate_service_key_candidate_is_never_dialed() {
    let mut issuers = [issuer_keys(1), issuer_keys(2)];
    issuers.sort_by_key(Keys::public_key);
    let first = fman_keys(1);
    let first_backup = fman_keys(2);
    let duplicate = fman_keys(3);
    let duplicate_backup = fman_keys(4);
    let shared_service_pubkey = service_pubkey(&first);
    let events = vec![
        issuer_ad_with_service_pubkey(&first, &issuers[0], 1_000, 3, shared_service_pubkey),
        issuer_ad(&first_backup, &issuers[0], 2_000, 3),
        issuer_ad_with_service_pubkey(&duplicate, &issuers[1], 1_000, 3, shared_service_pubkey),
        issuer_ad(&duplicate_backup, &issuers[1], 2_000, 3),
    ];

    let prober = StubProber::default();
    let (seats, rejected) = select_probed(events, &StubBadgeVerifier::default(), &prober, 3).await;

    assert_eq!(seats.len(), 3);
    assert_eq!(rejected.len(), 1);
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::DuplicateServicePubkey { .. }
    ));
    // The duplicate is rejected before the probe, so exactly the three
    // seated candidates were dialed.
    assert_eq!(prober.dialed().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn hung_probe_exhausts_its_budget_and_the_walk_continues() {
    let issuer = issuer_keys(1);
    let hung = fman_keys(1);
    let backup = fman_keys(2);
    let events = vec![
        issuer_ad(&hung, &issuer, 1_000, 3),
        issuer_ad(&backup, &issuer, 2_000, 3),
    ];

    let prober = StubProber::with_outcome(service_pubkey(&hung), StubProbeOutcome::Hang);
    let began = Instant::now();
    let (seats, rejected) = select_probed(events, &StubBadgeVerifier::default(), &prober, 1).await;

    assert_eq!(rejected.len(), 1);
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::ProbeFailed { .. }
    ));
    assert_eq!(
        seats
            .iter()
            .map(|seat| seat.candidate().fman_id())
            .collect::<Vec<_>>(),
        vec![backup.public_key()],
        "a hung probe costs only its per-candidate budget, not the preview",
    );
    assert_eq!(
        Instant::now().duration_since(began),
        crate::FMAN_SELECTION_PROBE_TIMEOUT,
    );
}

#[tokio::test(start_paused = true)]
async fn hung_probe_at_the_walk_deadline_is_deadline_expired() {
    let issuer = issuer_keys(1);
    let hung = fman_keys(1);
    let unexamined = fman_keys(2);
    let events = vec![
        issuer_ad(&hung, &issuer, 1_000, 3),
        issuer_ad(&unexamined, &issuer, 2_000, 3),
    ];
    let candidates = eligible(events).await;
    // A walk deadline shorter than the per-probe budget: the deadline cut
    // the probe, so the typed reason is expiry and the walk stops.
    let deadline = Instant::now() + Duration::from_secs(2);

    let prober = StubProber::with_outcome(service_pubkey(&hung), StubProbeOutcome::Hang);
    let mut rejected = Vec::new();
    let seats = select_fman_seats(
        &StubBadgeVerifier::default(),
        &prober,
        &preview_request(MIN_FEDERATION_SIZE),
        &fedimintd_version().dkg_version(),
        candidates,
        FederationSize(1),
        BTreeMap::new(),
        deadline,
        &mut rejected,
    )
    .await;

    assert!(seats.is_empty());
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].author, hung.public_key());
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::DeadlineExpired
    ));
    assert_eq!(prober.dialed().len(), 1, "the walk stops at expiry");
}

#[tokio::test]
async fn probing_preview_backfills_stale_ads_from_the_pool() {
    let issuer = issuer_keys(1);
    let stale = fman_keys(101);
    let mut events = vec![issuer_ad(&stale, &issuer, 1_000, 3)];
    events.extend((1..=MIN_FEDERATION_SIZE).map(|index| {
        let fman = fman_keys(u8::try_from(index).expect("small test index"));
        issuer_ad(&fman, &issuer, 2_000, 3)
    }));

    let prober = StubProber::with_outcome(
        service_pubkey(&stale),
        StubProbeOutcome::Available(GetAvailabilityResponse {
            accepting_seats: false,
            ..compatible_availability()
        }),
    );
    let preview = preview_fman_selection_with(
        &registry(events),
        &StubBadgeVerifier::default(),
        &prober,
        test_peer_badge_verifier().provenance(),
        &preview_request(MIN_FEDERATION_SIZE),
        generous_deadline(),
        NOW,
        || NOW,
    )
    .await
    .expect("the pool backfills the stale candidate");

    assert_eq!(preview.selected(), usize::from(MIN_FEDERATION_SIZE));
    assert_eq!(
        preview.total_advertised_msats(),
        u64::from(MIN_FEDERATION_SIZE) * 2_000
    );
    assert!(preview.rejected().iter().any(|rejection| {
        rejection.author == stale.public_key()
            && matches!(
                rejection.reason,
                AdvertisementRejection::LiveNotAcceptingSeats
            )
    }));
}

#[tokio::test]
async fn deadline_expiry_stops_the_walk_with_a_typed_rejection() {
    let fman = fman_keys(1);
    let candidates = eligible(vec![ad_event(&fman, self_authorized_payload(&fman))]).await;

    let verifier = StubBadgeVerifier::default();
    let mut rejected = Vec::new();
    let seats = select_fman_seats(
        &verifier,
        &AdOnlySelection,
        &preview_request(MIN_FEDERATION_SIZE),
        &fedimintd_version().dkg_version(),
        candidates,
        FederationSize(1),
        BTreeMap::new(),
        // Already-expired deadline: the walk must not verify anything.
        Instant::now(),
        &mut rejected,
    )
    .await;

    assert!(seats.is_empty());
    assert_eq!(rejected.len(), 1);
    assert!(matches!(
        rejected[0].reason,
        AdvertisementRejection::DeadlineExpired
    ));
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn cheap_slow_spam_times_out_before_honest_badge_verification() {
    // One publisher-controlled claimed-issuer bucket ranks cheap spam before
    // every honest candidate. The fake verifier consumes one second per spam
    // candidate (within the concrete verifier's ten-second bound).
    let issuer = issuer_keys(1);
    let spam = (1..=7).map(fman_keys).collect::<Vec<_>>();
    let honest = (8..=14).map(fman_keys).collect::<Vec<_>>();
    let honest_events = honest
        .iter()
        .map(|fman| issuer_ad(fman, &issuer, 1_000, 3))
        .collect::<Vec<_>>();

    // The retained honest advertisements are enough and badge-valid under the
    // same selection port when the attacker does not consume the deadline.
    let honest_preview = preview_fman_selection_with(
        &registry(honest_events.clone()),
        &StubBadgeVerifier::default(),
        &AdOnlySelection,
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        &preview_request(MIN_FEDERATION_SIZE),
        Instant::now() + Duration::from_secs(10),
        NOW,
        || NOW,
    )
    .await
    .expect("honest candidates fill the requested selection");
    assert_eq!(honest_preview.selected(), usize::from(MIN_FEDERATION_SIZE));

    let mut events = spam
        .iter()
        .map(|fman| issuer_ad(fman, &issuer, 0, 3))
        .collect::<Vec<_>>();
    events.extend(honest_events);
    let retained = eligible(events.clone()).await;
    assert_eq!(retained.len(), spam.len() + honest.len());
    let retained_authors = retained
        .into_iter()
        .map(|candidate| candidate.fman_id())
        .collect::<HashSet<_>>();
    assert!(
        spam.iter()
            .chain(&honest)
            .all(|fman| retained_authors.contains(&fman.public_key())),
        "combined discovery retains every spam and honest candidate"
    );
    let verifier = StubBadgeVerifier::slowly_rejecting(
        spam.iter().map(Keys::public_key),
        Duration::from_secs(1),
    );
    let error = preview_fman_selection_until(
        &registry(events),
        &verifier,
        &AdOnlySelection,
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        &preview_request(MIN_FEDERATION_SIZE),
        Instant::now() + Duration::from_secs(7),
        NOW,
        || NOW,
    )
    .await
    .expect_err("the slow cheapest spam consumes the shared deadline");

    assert!(matches!(error, FiError::SelectionPreviewTimeout), "{error}");
    assert_eq!(
        verifier.calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "the deadline prevents verification of any honest candidate"
    );
    let attempted_subjects = verifier.attempted_subjects.lock().expect("test lock");
    assert!(
        attempted_subjects
            .iter()
            .all(|subject| spam.iter().any(|fman| fman.public_key() == *subject)),
        "every reached verification belongs to a spam candidate"
    );
}

fn preview_request(size: u16) -> FmanSelectionRequest {
    FmanSelectionRequest::new(
        FederationSize(size),
        fedimintd_version_range(),
        PlanPreference::InfiniteBestEffort,
    )
    .expect("test request is valid")
}

fn multi_release_preview_request(size: u16) -> FmanSelectionRequest {
    FmanSelectionRequest::new(
        FederationSize(size),
        FedimintdVersionRange::new(
            "0.11.1".parse().expect("range minimum parses"),
            "0.13.0".parse().expect("range maximum parses"),
        )
        .expect("test range is ordered"),
        PlanPreference::InfiniteBestEffort,
    )
    .expect("test request is valid")
}

fn cohort_ads(start: u8, count: u8, price: u64, version: &str) -> Vec<Event> {
    let issuer = issuer_keys(1);
    (start..start + count)
        .map(|index| issuer_ad_for_version(&fman_keys(index), &issuer, price, version))
        .collect()
}

async fn preview_cohorts(events: Vec<Event>) -> FiResult<FmanSelectionPreview> {
    preview_fman_selection_with(
        &registry(events),
        &StubBadgeVerifier::default(),
        &AdOnlySelection,
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        &multi_release_preview_request(MIN_FEDERATION_SIZE),
        generous_deadline(),
        NOW,
        || NOW,
    )
    .await
}

#[tokio::test]
async fn preview_accepts_patch_skew_within_the_fedi_minor_line() {
    let preview = preview_cohorts(cohort_ads(
        1,
        u8::try_from(MIN_FEDERATION_SIZE).expect("small test size"),
        1_000,
        "0.11.2-rc.1+fedi",
    ))
    .await
    .expect("Fedi patch skew stays in the same DKG cohort");

    assert_eq!(preview.fedimintd_dkg_version().to_string(), "0.11+fedi");
}

#[tokio::test]
async fn preview_never_mixes_minor_lines_to_fill_a_federation() {
    let mut events = cohort_ads(1, 4, 1_000, "0.11.1+fedi");
    events.extend(cohort_ads(5, 3, 1_000, "0.12.0+fedi"));
    let error = preview_cohorts(events)
        .await
        .expect_err("partial cohorts cannot be combined");

    assert!(matches!(
        error,
        FiError::InsufficientFmanSeats {
            requested: 7,
            selected: 4,
            ..
        }
    ));
}

#[tokio::test]
async fn preview_chooses_cheapest_complete_cohort_then_newer_on_a_tie() {
    let count = u8::try_from(MIN_FEDERATION_SIZE).expect("small test size");
    for (new_price, expected_dkg) in [(2_000, "0.11+fedi"), (1_000, "0.12+fedi")] {
        let mut events = cohort_ads(1, count, 1_000, "0.11.1+fedi");
        events.extend(cohort_ads(8, count, new_price, "0.12.0+fedi"));
        let preview = preview_cohorts(events)
            .await
            .expect("both cohorts can fill the federation");
        assert_eq!(preview.fedimintd_dkg_version().to_string(), expected_dkg);
        assert_eq!(
            preview.total_advertised_msats(),
            u64::from(MIN_FEDERATION_SIZE) * 1_000
        );
    }
}

#[tokio::test]
async fn preview_returns_selected_seats_estimate_and_summary() {
    let issuer_a = issuer_keys(1);
    let issuer_b = issuer_keys(2);
    let mut events = (1..=4)
        .map(|index| issuer_ad(&fman_keys(index), &issuer_a, 1_000, 3))
        .collect::<Vec<_>>();
    events.extend((5..=8).map(|index| issuer_ad(&fman_keys(index), &issuer_b, 2_000, 3)));
    // One ineligible advertisement contributes to `seen` but not `eligible`.
    let free_only = fman_keys(9);
    let mut free_payload = self_authorized_payload(&free_only);
    free_payload.plans = vec![Plan::SubscriptionBased {
        initial_price_msats: 1_000,
        renewal_price_msats: 1_000,
        period: "every-30-days".to_owned(),
    }];
    events.push(ad_event(&free_only, free_payload));

    let verifier = StubBadgeVerifier::default();
    let preview = preview_fman_selection_with(
        &registry(events),
        &verifier,
        &AdOnlySelection,
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        &preview_request(MIN_FEDERATION_SIZE),
        generous_deadline(),
        NOW,
        || NOW,
    )
    .await
    .expect("preview succeeds");

    assert_eq!(preview.seats().len(), usize::from(MIN_FEDERATION_SIZE));
    assert_eq!(preview.selected(), usize::from(MIN_FEDERATION_SIZE));
    assert_eq!(preview.seen(), 9);
    assert_eq!(preview.eligible(), 8);
    // The round-robin walk takes four seats from the first bucket in
    // issuer-key order and three from the other.
    let expected_total = if issuer_a.public_key() < issuer_b.public_key() {
        4 * 1_000 + 3 * 2_000
    } else {
        4 * 2_000 + 3 * 1_000
    };
    assert_eq!(preview.total_advertised_msats(), expected_total);
    assert_eq!(preview.valid_until(), Timestamp(NOW + 120));
    assert_eq!(preview.rejected().len(), 1, "{:?}", preview.rejected());
    assert!(matches!(
        preview.rejected()[0].reason,
        AdvertisementRejection::NoInfiniteBestEffortPlan
    ));
    assert_eq!(
        verifier.calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "preview verification is lazy",
    );
    let approval = preview
        .approve(expected_total)
        .expect("displayed estimate is an admissible cap");
    assert_eq!(approval.advertised_total_msats(), expected_total);
    assert_eq!(approval.max_total_msats(), expected_total);
    assert_eq!(approval.valid_until(), Timestamp(NOW + 120));
}

#[tokio::test]
async fn preview_validity_starts_when_the_verified_walk_completes() {
    let issuer = issuer_keys(1);
    let events = (1..=MIN_FEDERATION_SIZE)
        .map(|index| {
            issuer_ad(
                &fman_keys(u8::try_from(index).expect("small test index")),
                &issuer,
                1_000,
                3,
            )
        })
        .collect::<Vec<_>>();
    let completed_at = NOW + 90;

    let preview = preview_fman_selection_with(
        &registry(events),
        &StubBadgeVerifier::default(),
        &AdOnlySelection,
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        &preview_request(MIN_FEDERATION_SIZE),
        generous_deadline(),
        NOW,
        || completed_at,
    )
    .await
    .expect("preview succeeds after a slow verified walk");

    assert_eq!(
        preview.valid_until(),
        Timestamp(completed_at + FMAN_SELECTION_PREVIEW_VALIDITY.as_secs())
    );
}

#[tokio::test]
async fn preview_shortfall_is_a_typed_partial_failure() {
    let issuer = issuer_keys(1);
    let events = (1..=3)
        .map(|index| issuer_ad(&fman_keys(index), &issuer, 1_000, 3))
        .collect::<Vec<_>>();

    let error = preview_fman_selection_with(
        &registry(events.clone()),
        &StubBadgeVerifier::default(),
        &AdOnlySelection,
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        &preview_request(MIN_FEDERATION_SIZE),
        generous_deadline(),
        NOW,
        || NOW,
    )
    .await
    .unwrap_err();

    assert!(
        matches!(
            error,
            FiError::InsufficientFmanSeats {
                requested: 7,
                selected: 3,
                seen: 3,
                eligible: 3,
            }
        ),
        "{error}"
    );
    assert_eq!(error.code(), FiErrorCode::Selection);

    let verifier = StubBadgeVerifier::slowly_rejecting(
        (1..=3).map(|index| fman_keys(index).public_key()),
        Duration::ZERO,
    );
    let error = preview_fman_selection_with(
        &registry(events),
        &verifier,
        &AdOnlySelection,
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        &preview_request(MIN_FEDERATION_SIZE),
        generous_deadline(),
        NOW,
        || NOW,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        FiError::InsufficientFmanSeats { selected: 0, .. }
    ));
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn preview_relay_failure_is_a_typed_registry_error() {
    let registry = TestRegistry::default();
    registry.fail.store(true, Ordering::SeqCst);
    let error = preview_fman_selection_with(
        &registry,
        &StubBadgeVerifier::default(),
        &AdOnlySelection,
        PeerBadgeVerifierProvenance::ExplicitTestConfiguration,
        &preview_request(MIN_FEDERATION_SIZE),
        generous_deadline(),
        NOW,
        || NOW,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, FiError::Registry(_)), "{error}");
}

#[test]
fn selection_request_enforces_product_size_and_plan() {
    let versions = fedimintd_version_range();
    assert!(
        FmanSelectionRequest::new(
            FederationSize(MIN_FEDERATION_SIZE),
            versions.clone(),
            PlanPreference::InfiniteBestEffort,
        )
        .is_ok()
    );
    assert!(
        FmanSelectionRequest::new(
            FederationSize(MAX_FEDERATION_SIZE_EXCLUSIVE - 1),
            versions.clone(),
            PlanPreference::InfiniteBestEffort,
        )
        .is_ok()
    );
    assert!(matches!(
        FmanSelectionRequest::new(
            FederationSize(MIN_FEDERATION_SIZE - 1),
            versions.clone(),
            PlanPreference::InfiniteBestEffort,
        ),
        Err(FiError::InvalidIntent(_))
    ));
    assert!(matches!(
        FmanSelectionRequest::new(
            FederationSize(MAX_FEDERATION_SIZE_EXCLUSIVE),
            versions.clone(),
            PlanPreference::InfiniteBestEffort,
        ),
        Err(FiError::InvalidIntent(_))
    ));
}

#[tokio::test]
async fn walk_fills_the_inclusive_custom_size_ceiling() {
    let events = (1..=MAX_FEDERATION_SIZE_EXCLUSIVE - 1)
        .map(|index| {
            let fman = fman_keys(u8::try_from(index).expect("test index fits u8"));
            ad_event(&fman, self_authorized_payload(&fman))
        })
        .collect();
    let verifier = StubBadgeVerifier::default();

    let (seats, rejected) = select(events, &verifier, MAX_FEDERATION_SIZE_EXCLUSIVE - 1).await;

    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(seats.len(), usize::from(MAX_FEDERATION_SIZE_EXCLUSIVE - 1));
    assert_eq!(
        verifier.calls.load(Ordering::SeqCst),
        usize::from(MAX_FEDERATION_SIZE_EXCLUSIVE - 1)
    );
}
