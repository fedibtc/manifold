mod callback;
mod discovery;
mod maintenance;
mod recovery_policy;
mod selection;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::pending;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bitcoin::secp256k1::{PublicKey as BitcoinPublicKey, SecretKey as BitcoinSecretKey};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_nostr::setup_payment_federations::{
    SETUP_PAYMENT_FEDERATIONS_D_TAG, SETUP_PAYMENT_FEDERATIONS_EVENT_KIND,
};
use fedi_decentralized_nostr_clients::{FiNostrClient, NostrClientError, NostrClientResult};
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_service_fleet_manager::*;
use fedi_decentralized_service_liquidity_manager as liquidity_api;
use fedi_iroh_rpc::iroh::{EndpointAddr, SecretKey as IrohSecretKey};
use fedimint_core::PeerId;
use fedimint_core::db::{IRawDatabaseExt as _, mem_impl::MemDatabase};
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::util::SafeUrl;
use nostr_sdk::{Event, EventBuilder, Keys, Kind, PublicKey, Tag};
use secp256k1::{Keypair, SECP256K1, SecretKey};
use stability_pool_common::{Account, AccountType};
use tokio::sync::Notify;

use crate::db::{FiRecovery, QuoteAuthorization};
use crate::liquidity::TestLiquidityBadgeVerifier;

use super::*;

const PAYMENT_AMOUNT_MSATS: u64 = 100;
const PAYMENT_INVITE: &str = "fed11qgqpu8rhwden5te0vejkg6tdd9h8gepwd4cxcumxv4jzuen0duhsqqfqh6nl7sgk72caxfx8khtfnn8y436q3nhyrkev3qp8ugdhdllnh86qmp42pm";

fn guardian_fee_account(byte: u8) -> Account {
    Account::single(
        BitcoinPublicKey::from_secret_key(
            bitcoin::secp256k1::SECP256K1,
            &BitcoinSecretKey::from_slice(&[byte; 32]).expect("fixed test scalar is valid"),
        ),
        AccountType::BtcDepositor,
    )
}

#[derive(Clone)]
struct TestFiFeeAccountProvider {
    account: Option<Account>,
    requested_federations: Arc<Mutex<Vec<FedimintFederationId>>>,
}

impl TestFiFeeAccountProvider {
    fn new(account: Account) -> Self {
        Self {
            account: Some(account),
            requested_federations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn unavailable() -> Self {
        Self {
            account: None,
            requested_federations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn requested_federations(&self) -> Vec<FedimintFederationId> {
        self.requested_federations
            .lock()
            .expect("test lock")
            .clone()
    }
}

impl Default for TestFiFeeAccountProvider {
    fn default() -> Self {
        Self::new(guardian_fee_account(30))
    }
}

impl FiFeeAccountProvider for TestFiFeeAccountProvider {
    fn formed_federation_fee_account(
        &self,
        federation_id: &FedimintFederationId,
    ) -> Result<Account, FiFeeAccountError> {
        self.requested_federations
            .lock()
            .expect("test lock")
            .push(*federation_id);
        self.account
            .clone()
            .ok_or_else(|| FiFeeAccountError::new("test FI fee account unavailable"))
    }
}

fn test_peer_badge_verifier() -> PeerBadgeVerifier {
    peer_badge_verifier(ManifoldEnvironment::Development)
}

fn test_peer_badge_minimum_trust_level() -> u64 {
    ManifoldEnvironment::Development
        .profile()
        .expect("test environment profile resolves")
        .minimum_peer_badge_trust_level()
}

fn peer_badge_verifier(environment: ManifoldEnvironment) -> PeerBadgeVerifier {
    PeerBadgeVerifier::try_from_profile(
        &environment
            .profile()
            .expect("test environment profile resolves"),
    )
    .expect("test PeerBadge verifier")
}

#[derive(Clone, Copy)]
struct TestIdentity;

impl TestIdentity {
    fn keypair() -> Keypair {
        Keypair::from_secret_key(
            SECP256K1,
            &SecretKey::from_byte_array(&[7; 32]).expect("valid test secret"),
        )
    }

    fn fi_id() -> FiId {
        FiId(Self::keypair().x_only_public_key().0)
    }
}

impl FiIdentity for TestIdentity {
    fn public_key(&self) -> Result<FiId, String> {
        Ok(Self::fi_id())
    }

    fn sign_digest(&self, digest: [u8; 32]) -> Result<FiSignature, String> {
        Ok(FiSignature(
            SECP256K1.sign_schnorr_no_aux_rand(&digest, &Self::keypair()),
        ))
    }
}

#[derive(Clone, Copy)]
struct OtherIdentity;

impl OtherIdentity {
    fn keypair() -> Keypair {
        Keypair::from_secret_key(
            SECP256K1,
            &SecretKey::from_byte_array(&[8; 32]).expect("valid test secret"),
        )
    }
}

impl FiIdentity for OtherIdentity {
    fn public_key(&self) -> Result<FiId, String> {
        Ok(FiId(Self::keypair().x_only_public_key().0))
    }

    fn sign_digest(&self, digest: [u8; 32]) -> Result<FiSignature, String> {
        Ok(FiSignature(
            SECP256K1.sign_schnorr_no_aux_rand(&digest, &Self::keypair()),
        ))
    }
}

#[derive(Clone, Default)]
struct TestRegistry {
    candidates: Arc<Mutex<Vec<Event>>>,
    advertisements: Arc<Mutex<Vec<Event>>>,
    fail: Arc<AtomicBool>,
    block_fetch: Arc<AtomicBool>,
    advertisement_delay: Duration,
    fetch_started: Arc<Notify>,
    fetch_continue: Arc<Notify>,
}

impl FiNostrClient for TestRegistry {
    async fn fetch_fman_advertisement(
        &self,
        _fman_pubkey: PublicKey,
        _timeout: Duration,
    ) -> NostrClientResult<Event> {
        Err(NostrClientError::MissingEvent { context: "test" })
    }

    async fn fetch_setup_payment_federations(
        &self,
        _publisher: PublicKey,
        _timeout: Duration,
    ) -> NostrClientResult<Vec<Event>> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(NostrClientError::MissingEvent { context: "test" });
        }
        if self.block_fetch.load(Ordering::SeqCst) {
            self.fetch_started.notify_one();
            self.fetch_continue.notified().await;
        }
        Ok(self.candidates.lock().expect("test lock").clone())
    }

    async fn fetch_fman_advertisements(&self, _timeout: Duration) -> NostrClientResult<Vec<Event>> {
        if self.fail.load(Ordering::SeqCst) {
            // The registry client's fail-closed enumeration reports a stalled
            // or truncated relay answer as a typed incomplete query.
            return Err(NostrClientError::IncompleteQuery {
                reason: "test relay stalled before EOSE",
            });
        }
        if !self.advertisement_delay.is_zero() {
            tokio::time::sleep(self.advertisement_delay).await;
        }
        Ok(self.advertisements.lock().expect("test lock").clone())
    }

    async fn fetch_liquidity_provider_advertisements(
        &self,
        _timeout: Duration,
    ) -> NostrClientResult<Vec<Event>> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(NostrClientError::IncompleteQuery {
                reason: "test relay stalled before EOSE",
            });
        }
        Ok(self.advertisements.lock().expect("test lock").clone())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestEffect {
    FmanConnected(usize),
    PaymentOutputPolled,
    SeatCheckpointed,
}

#[derive(Default)]
struct PaymentState {
    payable_calls: AtomicUsize,
    readiness_calls: AtomicUsize,
    reservation_recover_calls: AtomicUsize,
    whole_release_calls: AtomicUsize,
    recover_calls: AtomicUsize,
    create_calls: AtomicUsize,
    refund_calls: AtomicUsize,
    funded_quotes: Mutex<HashSet<QuoteId>>,
    created_quotes: Mutex<Vec<QuoteId>>,
    settled_quotes: Mutex<HashSet<QuoteId>>,
    refund_contexts: Mutex<Vec<QuoteId>>,
    reject_recovery: AtomicBool,
    rejected_quotes: Mutex<HashSet<QuoteId>>,
    released_quotes: Mutex<HashSet<QuoteId>>,
    /// Exact terminal releases that fail once before normal proof handling.
    failed_terminal_release_quotes: Mutex<HashSet<QuoteId>>,
    failed_recovery_quotes: Mutex<HashSet<QuoteId>>,
    block_recovery: AtomicBool,
    barrier_recovery: AtomicBool,
    recovery_started: Notify,
    recovery_continue: Notify,
    release_recovery: AtomicBool,
    recovery_release: Notify,
    /// One-based funding call whose committed wallet result is lost.
    hang_funding_on_call: AtomicUsize,
    funding_started: Notify,
    hang_first_refund: AtomicBool,
    refund_started: Notify,
    pay_none: AtomicBool,
    insufficient_funds: AtomicBool,
    fail_readiness_on_call: AtomicUsize,
    /// One-based reserve call that persists its journal before losing the
    /// result. The returned generic error deliberately carries no cleanup
    /// proof, so FI recovery must retain and reconstruct the same id.
    lose_reservation_result_on_call: AtomicUsize,
    fail_whole_release: AtomicBool,
    reverse_payable: AtomicBool,
    /// Model a payer whose complete spendable balance is one ecash note.
    /// Funding removes that note until consensus returns its change, so a
    /// concurrently polled sibling observes no spendable input even when the
    /// aggregate value is sufficient.
    single_note_wallet: AtomicBool,
    single_note_balance_msats: AtomicU64,
    single_note_fee_msats: AtomicU64,
    /// Settle under a generation the quote did not name, as a wallet whose
    /// dispatch disagrees with the terms it was handed would.
    settle_wrong_generation: AtomicBool,
    effect_log: Mutex<Option<Arc<Mutex<Vec<TestEffect>>>>>,
    reservations: Mutex<HashMap<String, TestReservation>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestReservation {
    quote_ids: Vec<QuoteId>,
    started: HashSet<QuoteId>,
    terminal: HashSet<QuoteId>,
    released: HashSet<QuoteId>,
}

#[derive(Clone)]
struct TestPayments(Arc<PaymentState>);

impl TestPayments {
    fn new() -> (Self, Arc<PaymentState>) {
        let state = Arc::new(PaymentState::default());
        (Self(state.clone()), state)
    }

    fn prepared(&self, quote_id: QuoteId) -> PreparedSeatPayment<QuoteId> {
        PreparedSeatPayment {
            payment_signatures: vec![LockedBlindedSignature(vec![1, 2, 3])],
            // The test FMan quotes mintv1 terms.
            settled_under: if self.0.settle_wrong_generation.load(Ordering::SeqCst) {
                MintGeneration::MintV2
            } else {
                MintGeneration::MintV1
            },
            refund_context: quote_id,
        }
    }
}

impl FiPayments for TestPayments {
    type RefundContext = QuoteId;
    type PaymentReservation = crate::PaymentReservationId;
    type TerminalReleaseProof = QuoteId;

    async fn payable_federations(
        &self,
        admitted: &[FederationId],
    ) -> Result<Vec<FederationId>, FiPaymentError> {
        self.0.payable_calls.fetch_add(1, Ordering::SeqCst);
        if self.0.pay_none.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        let mut payable = admitted.to_vec();
        if self.0.reverse_payable.load(Ordering::SeqCst) {
            payable.reverse();
        }
        Ok(payable)
    }

    async fn recover_payment_reservation(
        &self,
        reservation_id: &crate::PaymentReservationId,
        preflight: &crate::ExactPaymentPreflight<'_>,
    ) -> Result<crate::PaymentReservationRecovery<Self::PaymentReservation>, FiPaymentError> {
        self.0
            .reservation_recover_calls
            .fetch_add(1, Ordering::SeqCst);
        let quote_ids = preflight
            .seats()
            .iter()
            .map(crate::ExactSeatPaymentPreflight::quote_id)
            .collect::<Vec<_>>();
        let reservations = self.0.reservations.lock().expect("test lock");
        match reservations.get(reservation_id.as_str()) {
            Some(existing) if existing.quote_ids != quote_ids => Err(FiPaymentError::new(
                "reservation id belongs to a different exact plan",
            )),
            Some(_) => Ok(crate::PaymentReservationRecovery::Existing(
                reservation_id.clone(),
            )),
            None => Ok(crate::PaymentReservationRecovery::Absent),
        }
    }

    async fn reserve_payment_requirements(
        &self,
        reservation_id: &crate::PaymentReservationId,
        preflight: &crate::ExactPaymentPreflight<'_>,
    ) -> Result<Self::PaymentReservation, FiPaymentError> {
        assert!(!preflight.seats().is_empty());
        assert!(
            preflight
                .seats()
                .iter()
                .all(|seat| seat.quote().terms.payment.is_some())
        );
        let quote_ids = preflight
            .seats()
            .iter()
            .map(crate::ExactSeatPaymentPreflight::quote_id)
            .collect::<Vec<_>>();
        if self.0.single_note_wallet.load(Ordering::SeqCst) {
            let fee_msats = self.0.single_note_fee_msats.load(Ordering::SeqCst);
            let required_msats = preflight.seats().iter().try_fold(0u64, |total, seat| {
                total
                    .checked_add(seat.requirement().amount_msats)
                    .and_then(|total| total.checked_add(fee_msats))
            });
            if required_msats.is_none_or(|required| {
                self.0.single_note_balance_msats.load(Ordering::SeqCst) < required
            }) {
                return Err(FiPaymentError::insufficient_funds_without_reservation(
                    "single ecash note cannot cover the aggregate plus fees",
                ));
            }
        }
        let mut reservations = self.0.reservations.lock().expect("test lock");
        match reservations.get(reservation_id.as_str()) {
            Some(existing) if existing.quote_ids != quote_ids => {
                return Err(FiPaymentError::new(
                    "reservation id belongs to a different exact plan",
                ));
            }
            Some(_) => return Ok(reservation_id.clone()),
            None => {
                let call = self.0.readiness_calls.fetch_add(1, Ordering::SeqCst) + 1;
                if self.0.insufficient_funds.load(Ordering::SeqCst)
                    || self.0.fail_readiness_on_call.load(Ordering::SeqCst) == call
                {
                    return Err(FiPaymentError::insufficient_funds_without_reservation(
                        "insufficient funds including fees",
                    ));
                }
                reservations.insert(
                    reservation_id.as_str().to_owned(),
                    TestReservation {
                        quote_ids,
                        started: HashSet::new(),
                        terminal: HashSet::new(),
                        released: HashSet::new(),
                    },
                );
                if self
                    .0
                    .lose_reservation_result_on_call
                    .compare_exchange(call, 0, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    return Err(FiPaymentError::new(
                        "payment reservation result was lost after journaling",
                    ));
                }
            }
        }
        Ok(reservation_id.clone())
    }

    async fn release_payment_reservation(
        &self,
        reservation: Self::PaymentReservation,
    ) -> Result<(), FiPaymentError> {
        self.0.whole_release_calls.fetch_add(1, Ordering::SeqCst);
        if self.0.fail_whole_release.load(Ordering::SeqCst) {
            return Err(FiPaymentError::new("injected reservation release failure"));
        }
        let mut reservations = self.0.reservations.lock().expect("test lock");
        let stored = reservations
            .get(reservation.as_str())
            .ok_or_else(|| FiPaymentError::new("reservation is missing"))?;
        if !stored.started.is_empty() {
            return Err(FiPaymentError::new("reservation has started outputs"));
        }
        reservations.remove(reservation.as_str());
        Ok(())
    }

    async fn release_seat_payment_reservation(
        &self,
        proof: Self::TerminalReleaseProof,
    ) -> Result<(), FiPaymentError> {
        if self
            .0
            .failed_terminal_release_quotes
            .lock()
            .expect("test lock")
            .remove(&proof)
        {
            return Err(FiPaymentError::new(
                "injected exact terminal reservation release failure",
            ));
        }
        let terminal = self
            .0
            .rejected_quotes
            .lock()
            .expect("test lock")
            .contains(&proof)
            || self
                .0
                .settled_quotes
                .lock()
                .expect("test lock")
                .contains(&proof)
            || self.0.reject_recovery.load(Ordering::SeqCst);
        if !terminal {
            return Err(FiPaymentError::new(
                "test wallet refused a nonterminal release proof",
            ));
        }
        let mut reservations = self.0.reservations.lock().expect("test lock");
        let mut matched = false;
        for reservation in reservations.values_mut() {
            if reservation.quote_ids.contains(&proof) {
                matched = true;
                if !reservation.terminal.contains(&proof) {
                    return Err(FiPaymentError::new(
                        "test wallet release proof is not journaled terminal",
                    ));
                }
                reservation.released.insert(proof);
            }
        }
        if !matched {
            return Err(FiPaymentError::new(
                "test wallet release proof names no reservation",
            ));
        }
        drop(reservations);
        self.0
            .released_quotes
            .lock()
            .expect("test lock")
            .insert(proof);
        Ok(())
    }

    async fn prepare_quote_refund(
        &self,
        _federation_id: &FederationId,
        _plan: &Plan,
    ) -> Result<RefundIssuance, FiPaymentError> {
        Ok(RefundIssuance::MintV1 {
            refund_nonce: [9; 32],
            issuance: vec![LockedIssuanceRequest {
                amount_msats: PAYMENT_AMOUNT_MSATS,
                blind_nonce: vec![4, 5, 6],
            }],
        })
    }

    async fn recover_seat_payment(
        &self,
        _reservation_id: &crate::PaymentReservationId,
        quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<SeatPaymentRecovery<Self::RefundContext, Self::TerminalReleaseProof>, FiPaymentError>
    {
        self.0.recover_calls.fetch_add(1, Ordering::SeqCst);
        let quote_id = quote.quote_id();
        if self.0.barrier_recovery.load(Ordering::SeqCst) {
            self.0.recovery_started.notify_one();
            while !self.0.release_recovery.load(Ordering::SeqCst) {
                let released = self.0.recovery_release.notified();
                if self.0.release_recovery.load(Ordering::SeqCst) {
                    break;
                }
                released.await;
            }
        }
        if self.0.block_recovery.load(Ordering::SeqCst) {
            self.0.recovery_started.notify_one();
            self.0.recovery_continue.notified().await;
        }
        if self
            .0
            .failed_recovery_quotes
            .lock()
            .expect("test lock")
            .contains(&quote_id)
        {
            return Err(FiPaymentError::new("injected recovery failure"));
        }
        if self.0.reject_recovery.load(Ordering::SeqCst)
            || self
                .0
                .rejected_quotes
                .lock()
                .expect("test lock")
                .contains(&quote_id)
        {
            for reservation in self.0.reservations.lock().expect("test lock").values_mut() {
                if reservation.quote_ids.contains(&quote_id) {
                    reservation.terminal.insert(quote_id);
                }
            }
            return Ok(SeatPaymentRecovery::Rejected(quote_id));
        }
        if self
            .0
            .funded_quotes
            .lock()
            .expect("test lock")
            .contains(&quote_id)
        {
            Ok(SeatPaymentRecovery::Prepared(self.prepared(quote_id)))
        } else {
            Ok(SeatPaymentRecovery::NotStarted)
        }
    }

    async fn create_seat_payment(
        &self,
        reservation: &Self::PaymentReservation,
        quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<PreparedSeatPayment<Self::RefundContext>, FiPaymentError> {
        if let Some(log) = self.0.effect_log.lock().expect("test lock").as_ref() {
            log.lock()
                .expect("test effect log")
                .push(TestEffect::PaymentOutputPolled);
        }
        let call = self.0.create_calls.fetch_add(1, Ordering::SeqCst);
        let quote_id = quote.quote_id();
        if self.0.single_note_wallet.load(Ordering::SeqCst) {
            let note_msats = self.0.single_note_balance_msats.swap(0, Ordering::SeqCst);
            if note_msats == 0 {
                return Err(FiPaymentError::new(
                    "the payer's only ecash note is reserved by another payment",
                ));
            }
            let Plan::InfiniteBestEffort { price_msats } = quote.terms.request.plan else {
                panic!("single-note test payer supports one-time prices");
            };
            let payment_msats = price_msats;
            let debit_msats = payment_msats
                .checked_add(self.0.single_note_fee_msats.load(Ordering::SeqCst))
                .expect("test payment debit fits u64");
            assert!(note_msats >= debit_msats, "test note covers the payment");
            // Make the note unavailable to sibling wallet futures until this
            // accepted transaction's change becomes spendable.
            tokio::task::yield_now().await;
            self.0
                .single_note_balance_msats
                .store(note_msats - debit_msats, Ordering::SeqCst);
        }
        {
            let mut reservations = self.0.reservations.lock().expect("test lock");
            let journal = reservations
                .get_mut(reservation.as_str())
                .ok_or_else(|| FiPaymentError::new("payment reservation is missing"))?;
            if !journal.quote_ids.contains(&quote_id) || journal.released.contains(&quote_id) {
                return Err(FiPaymentError::new(
                    "quote is not startable under this reservation",
                ));
            }
            journal.started.insert(quote_id);
        }
        let inserted = self
            .0
            .funded_quotes
            .lock()
            .expect("test lock")
            .insert(quote_id);
        if !inserted {
            return Err(FiPaymentError::new(
                "test wallet was asked to fund the same quote twice",
            ));
        }
        self.0
            .created_quotes
            .lock()
            .expect("test lock")
            .push(quote_id);
        if self
            .0
            .hang_funding_on_call
            .compare_exchange(call + 1, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.0.funding_started.notify_one();
            return pending().await;
        }
        Ok(self.prepared(quote_id))
    }

    async fn settle_seat_refund(
        &self,
        context: Self::RefundContext,
        _refund: RefundTransaction,
    ) -> Result<crate::SettledSeatRefund<Self::TerminalReleaseProof>, FiPaymentError> {
        self.0.refund_calls.fetch_add(1, Ordering::SeqCst);
        let funded = self
            .0
            .funded_quotes
            .lock()
            .expect("test lock")
            .contains(&context);
        let already_settled = self
            .0
            .settled_quotes
            .lock()
            .expect("test lock")
            .contains(&context);
        if !funded && !already_settled {
            return Err(FiPaymentError::new("refund context did not match"));
        }
        self.0
            .settled_quotes
            .lock()
            .expect("test lock")
            .insert(context);
        for reservation in self.0.reservations.lock().expect("test lock").values_mut() {
            if reservation.quote_ids.contains(&context) {
                reservation.terminal.insert(context);
            }
        }
        self.0
            .refund_contexts
            .lock()
            .expect("test lock")
            .push(context);
        if self.0.hang_first_refund.swap(false, Ordering::SeqCst) {
            self.0.refund_started.notify_one();
            return pending().await;
        }
        Ok(crate::SettledSeatRefund {
            amount_msats: PAYMENT_AMOUNT_MSATS,
            release_proof: context,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CreateBehavior {
    Accept,
    HangFirst,
    RefuseFirstQuote,
}

#[derive(Clone, Copy)]
struct FmanConfig {
    /// What this FMan offers its seats at. Zero is a give-away: it quotes no
    /// payment terms and expects no payment federation.
    price_msats: u64,
    accepting_seats: bool,
    federation_size: FederationSize,
    hang_availability: bool,
    create_behavior: CreateBehavior,
    reject_quote: bool,
    capacity_exhausted_quote: bool,
}

impl FmanConfig {
    /// The FMan offers one priced plan and the FI arranges a payment for it.
    fn paid() -> Self {
        Self {
            price_msats: PAYMENT_AMOUNT_MSATS,
            accepting_seats: true,
            federation_size: FederationSize(MIN_FEDERATION_SIZE),
            hang_availability: false,
            create_behavior: CreateBehavior::Accept,
            reject_quote: false,
            capacity_exhausted_quote: false,
        }
    }

    /// The bootstrap offer: seats at zero, for an FI with no way to pay.
    fn given_away() -> Self {
        Self {
            price_msats: 0,
            ..Self::paid()
        }
    }
}

#[derive(Clone)]
struct QuoteRecord {
    quote_id: QuoteId,
    payment_federation_id: Option<FederationId>,
    fedimintd_version: FedimintdVersion,
}

#[derive(Clone)]
struct CreateRecord {
    signed_quote: SignedResponse<GetQuoteResponse>,
    quote_id: QuoteId,
    seat_id: SeatId,
}

#[derive(Default)]
struct FmanState {
    connect_calls: AtomicUsize,
    connect_attempts: Mutex<HashMap<usize, usize>>,
    fail_connect_on_attempt: Mutex<Option<(usize, usize)>>,
    hang_connect_on_attempt: Mutex<Option<(usize, usize)>>,
    effect_log: Mutex<Option<Arc<Mutex<Vec<TestEffect>>>>>,
    connect_failures_remaining: AtomicUsize,
    /// One-based connection call to fail once; zero disables the hook.
    connect_failure_on_call: AtomicUsize,
    offline_indices: Mutex<HashSet<usize>>,
    availability_calls: AtomicUsize,
    availability_transport_failures_remaining: AtomicUsize,
    fedimintd_version_overrides: Mutex<HashMap<usize, FedimintdVersion>>,
    quote_calls: AtomicUsize,
    quote_transport_failures_remaining: AtomicUsize,
    create_calls: AtomicUsize,
    status_calls: AtomicUsize,
    dkg_code_calls: AtomicUsize,
    restart_calls: AtomicUsize,
    report_dkg_already_started: AtomicBool,
    start_callbacks: Mutex<Vec<Option<DkgCompletionCallback>>>,
    invite_calls: AtomicUsize,
    quote_records: Mutex<Vec<QuoteRecord>>,
    /// Per-FMan price overrides for mixed paid/free replacement-wave tests.
    price_overrides_msats: Mutex<HashMap<usize, u64>>,
    changed_quote_index: Mutex<Option<usize>>,
    blocked_quote_index: Mutex<Option<usize>>,
    quote_blocked: Notify,
    release_quotes: AtomicBool,
    quote_release: Notify,
    create_records: Mutex<Vec<CreateRecord>>,
    allocated_quotes: Mutex<HashSet<QuoteId>>,
    hung_quote: Mutex<Option<QuoteId>>,
    refused_quote: Mutex<Option<QuoteId>>,
    failed_create_quotes: Mutex<HashSet<QuoteId>>,
    failed_create_indices: Mutex<HashSet<usize>>,
    refused_create_indices: Mutex<HashSet<usize>>,
    create_started: Notify,
    block_accepts: AtomicBool,
    release_accepts: AtomicBool,
    create_release: Notify,
    disagreeing_invite: AtomicBool,
    /// Seat index -> the exact (key, value) that seat accepted.
    meta_submissions: Mutex<HashMap<usize, (String, String)>>,
    /// Seat index -> the exact guardian-fee policy that seat accepted.
    fee_submissions: Mutex<HashMap<usize, (u64, String)>>,
    /// Per-seat Guardian Verification Fee account overrides for divergence
    /// tests.
    formation_guardian_verification_fee_accounts: Mutex<HashMap<usize, Account>>,
    /// Every LNv2 gateway URL accepted by a fake guardian.
    gateway_registrations: Mutex<Vec<(usize, String)>>,
    /// Every signed merge base observed by a fake, in submission-wave order.
    meta_request_bases: Mutex<Vec<(usize, MetaConsensusBase)>>,
    /// Positive test-only submission count that completes one metadata wave.
    meta_request_wave_size: AtomicUsize,
    /// Wakes a test after a configured metadata submission wave is in flight.
    meta_request_wave_complete: Notify,
    /// The raw consensus object each fake independently compares a request to.
    meta_consensus_raw: Mutex<Option<Vec<u8>>>,
    /// The meta module's monotone consensus revision paired with
    /// `meta_consensus_raw`. The test reader bumps it on every adoption whose
    /// bytes differ from the live object, as `change_consensus` would; tests
    /// may also bump it directly to model an adoption elsewhere — including a
    /// revert that reproduces old bytes under a fresh occurrence.
    meta_consensus_revision: AtomicU64,
    /// Opt into the stateful base contract for tests that exercise rebasing.
    enforce_meta_bases: AtomicBool,
    /// Seat indices that report a stale merge base instead of submitting.
    stale_meta_indices: Mutex<HashSet<usize>>,
    /// Seat indices whose metadata RPC never returns within the request bound.
    hang_meta_indices: Mutex<HashSet<usize>>,
    /// Seat indices whose metadata RPC waits until the test releases it.
    block_meta_indices: Mutex<HashSet<usize>>,
    meta_call_blocked: Notify,
    release_meta_calls: AtomicBool,
    meta_call_release: Notify,
    /// Deterministic terminal metadata refusal returned by selected seats.
    meta_terminal_errors: Mutex<HashMap<usize, FleetManagerError>>,
    /// Wakes cancellation tests after a fake accepts a metadata proposal.
    meta_submission_changed: Notify,
    attestation_calls: AtomicUsize,
    /// Replacement FMan index -> original federation peer id it operates.
    attested_peer_overrides: Mutex<HashMap<usize, usize>>,
    /// Seat 0 attests a peer id outside the federation's config.
    attest_foreign_peer: AtomicBool,
    /// Seat 0 signs an account different from its earlier signed acceptance.
    attest_wrong_fee_account: AtomicBool,
    /// One entry per public trust-material request, preserving FMan and peers.
    trust_material_requests: Mutex<Vec<(usize, Vec<fedi_decentralized_domain::PeerId>)>>,
    /// Corrupt the first FMan trust-material response signature.
    corrupt_trust_material_signature: AtomicBool,
}

#[derive(Clone)]
struct TestConnector {
    state: Arc<FmanState>,
    config: FmanConfig,
}

impl FleetManagerConnector for TestConnector {
    type Client = TestFman;

    async fn connect(&self, locator: &Locator) -> Result<Self::Client, FleetManagerConnectorError> {
        let call = self.state.connect_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self
            .state
            .connect_failure_on_call
            .compare_exchange(call, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Err(FleetManagerConnectorError::new(
                "injected one-shot connection failure",
            ));
        }
        if self
            .state
            .connect_failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(FleetManagerConnectorError::new(
                "injected connection failure",
            ));
        }
        let index = (0..=usize::from(MAX_FEDERATION_SIZE))
            .find(|index| manager_key(*index).x_only_public_key().0 == locator.service_pubkey)
            .ok_or_else(|| FleetManagerConnectorError::new("unknown test locator"))?;
        let attempt = {
            let mut attempts = self.state.connect_attempts.lock().expect("test lock");
            let attempt = attempts.entry(index).or_default();
            *attempt += 1;
            *attempt
        };
        if *self
            .state
            .fail_connect_on_attempt
            .lock()
            .expect("test lock")
            == Some((index, attempt))
        {
            return Err(FleetManagerConnectorError::new(
                "injected pre-value connection failure",
            ));
        }
        if *self
            .state
            .hang_connect_on_attempt
            .lock()
            .expect("test lock")
            == Some((index, attempt))
        {
            return pending().await;
        }
        if let Some(log) = self.state.effect_log.lock().expect("test lock").as_ref() {
            log.lock()
                .expect("test effect log")
                .push(TestEffect::FmanConnected(index));
        }
        if self
            .state
            .offline_indices
            .lock()
            .expect("test lock")
            .contains(&index)
        {
            return Err(FleetManagerConnectorError::new(
                "test Fleet Manager is offline",
            ));
        }
        Ok(TestFman {
            index,
            manager_key: manager_key(index),
            state: self.state.clone(),
            config: self.config,
        })
    }

    async fn get_availability(
        &self,
        client: &Self::Client,
        request: GetAvailabilityRequest,
    ) -> Result<FmResult<GetAvailabilityResponse>, FleetManagerCallError> {
        if self
            .state
            .availability_transport_failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            self.state.availability_calls.fetch_add(1, Ordering::SeqCst);
            return Err(FleetManagerCallError::new(
                "injected availability stream loss",
            ));
        }
        Ok(client.get_availability(request).await)
    }

    async fn get_quote(
        &self,
        client: &Self::Client,
        request: GetQuoteRequest,
    ) -> Result<FmResult<SignedResponse<GetQuoteResponse>>, FleetManagerCallError> {
        if self
            .state
            .quote_transport_failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            self.state.quote_calls.fetch_add(1, Ordering::SeqCst);
            return Err(FleetManagerCallError::new("injected quote stream loss"));
        }
        Ok(client.get_quote(request).await)
    }
}

struct TestFman {
    index: usize,
    manager_key: Keypair,
    state: Arc<FmanState>,
    config: FmanConfig,
}

impl TestFman {
    fn price_msats(&self) -> u64 {
        self.state
            .price_overrides_msats
            .lock()
            .expect("test lock")
            .get(&self.index)
            .copied()
            .unwrap_or(self.config.price_msats)
    }

    fn refused_response(
        &self,
        quote_id: QuoteId,
        paid: bool,
    ) -> FmResult<SignedResponse<CreateSeatResponse>> {
        SignedResponse::create(
            &CreateSeatResponse {
                quote_id,
                outcome: CreateSeatOutcome::Refused {
                    reason: RefusalReason::OfferChanged,
                    refund_transaction: paid.then(|| RefundTransaction(vec![self.index as u8, 42])),
                },
            },
            &self.manager_key,
        )
        .map_err(Into::into)
    }
}

impl FleetManagerService for TestFman {
    async fn get_availability(
        &self,
        _request: GetAvailabilityRequest,
    ) -> FmResult<GetAvailabilityResponse> {
        self.state.availability_calls.fetch_add(1, Ordering::SeqCst);
        if self.config.hang_availability {
            return pending().await;
        }
        let plans = vec![Plan::InfiniteBestEffort {
            price_msats: self.price_msats(),
        }];
        let fedimintd_version = self
            .state
            .fedimintd_version_overrides
            .lock()
            .expect("test lock")
            .get(&self.index)
            .cloned()
            .unwrap_or_else(fedimintd_version);
        Ok(GetAvailabilityResponse {
            accepting_seats: self.config.accepting_seats,
            fedimintd_version,
            federation_sizes: vec![self.config.federation_size],
            plans,
            additional_info: Vec::new(),
        })
    }

    async fn get_quote(
        &self,
        request: GetQuoteRequest,
    ) -> FmResult<SignedResponse<GetQuoteResponse>> {
        let call = self.state.quote_calls.fetch_add(1, Ordering::SeqCst);
        if *self.state.blocked_quote_index.lock().expect("test lock") == Some(self.index) {
            self.state.quote_blocked.notify_one();
            while !self.state.release_quotes.load(Ordering::SeqCst) {
                let released = self.state.quote_release.notified();
                if self.state.release_quotes.load(Ordering::SeqCst) {
                    break;
                }
                released.await;
            }
        }
        let payment_federation_id = request.payment_federation_id.clone();
        let fedimintd_version = request.fedimintd_version.clone();
        if self.config.reject_quote {
            return Err(FleetManagerError::Other(
                "test daemon rejects the selected payment federation".to_owned(),
            ));
        }
        if self.config.capacity_exhausted_quote {
            return Err(FleetManagerError::CapacityExhausted);
        }
        let mut quote_nonce = [0; 32];
        quote_nonce[..8].copy_from_slice(&(call as u64).to_be_bytes());
        quote_nonce[8..16].copy_from_slice(&(self.index as u64).to_be_bytes());
        let configured_price_msats = self.price_msats();
        let (price_msats, payment) = if configured_price_msats == 0 {
            assert!(
                request.payment_federation_id.is_none(),
                "a give-away quote is not asked for against a payment federation"
            );
            (0, None)
        } else {
            let price_msats =
                if *self.state.changed_quote_index.lock().expect("test lock") == Some(self.index) {
                    configured_price_msats + 1
                } else {
                    configured_price_msats
                };
            let federation_id = request
                .payment_federation_id
                .clone()
                .expect("paid test request");
            (
                price_msats,
                Some(PaymentTerms::MintV1 {
                    federation_id,
                    issuance: vec![LockedIssuanceRequest {
                        amount_msats: price_msats,
                        blind_nonce: vec![self.index as u8, call as u8, 42],
                    }],
                }),
            )
        };
        let signed_quote = SignedResponse::create(
            &GetQuoteResponse {
                terms: QuoteTerms {
                    quote_nonce,
                    offer_epoch: OfferEpoch::from_bytes([0; 32]),
                    request,
                    price_msats,
                    payment,
                },
            },
            &self.manager_key,
        )?;
        let quote_id = signed_quote
            .verify(&self.manager_key.x_only_public_key().0)?
            .quote_id();
        self.state
            .quote_records
            .lock()
            .expect("test lock")
            .push(QuoteRecord {
                quote_id,
                payment_federation_id,
                fedimintd_version,
            });
        Ok(signed_quote)
    }

    async fn create_seat(
        &self,
        request: SignedRequest<CreateSeatRequest>,
    ) -> FmResult<SignedResponse<CreateSeatResponse>> {
        let request = request.verify(Timestamp(test_now_secs()))?.into_inner();
        let quote = request
            .quote
            .verify(&self.manager_key.x_only_public_key().0)?;
        let quote_id = quote.quote_id();
        let seat_id = SeatId::from(quote_id);
        self.state.create_calls.fetch_add(1, Ordering::SeqCst);
        self.state
            .allocated_quotes
            .lock()
            .expect("test lock")
            .insert(quote_id);
        self.state
            .create_records
            .lock()
            .expect("test lock")
            .push(CreateRecord {
                signed_quote: request.quote.clone(),
                quote_id,
                seat_id: seat_id.clone(),
            });
        if self
            .state
            .failed_create_quotes
            .lock()
            .expect("test lock")
            .contains(&quote_id)
        {
            return Err(FleetManagerError::Other(
                "injected CreateSeat failure".to_owned(),
            ));
        }
        if self
            .state
            .failed_create_indices
            .lock()
            .expect("test lock")
            .contains(&self.index)
        {
            return Err(FleetManagerError::Other(
                "injected indexed CreateSeat failure".to_owned(),
            ));
        }
        if self
            .state
            .refused_create_indices
            .lock()
            .expect("test lock")
            .contains(&self.index)
        {
            return self.refused_response(quote_id, quote.terms.payment.is_some());
        }

        match self.config.create_behavior {
            CreateBehavior::HangFirst => {
                let should_hang = {
                    let mut hung = self.state.hung_quote.lock().expect("test lock");
                    if hung.is_none() {
                        *hung = Some(quote_id);
                        true
                    } else {
                        false
                    }
                };
                if should_hang {
                    self.state.create_started.notify_one();
                    return pending().await;
                }
            }
            CreateBehavior::RefuseFirstQuote => {
                let mut refused = self.state.refused_quote.lock().expect("test lock");
                let should_refuse = match *refused {
                    Some(refused_quote) => refused_quote == quote_id,
                    None => {
                        *refused = Some(quote_id);
                        true
                    }
                };
                if should_refuse {
                    return self.refused_response(quote_id, quote.terms.payment.is_some());
                }
            }
            CreateBehavior::Accept => {}
        }
        if self.state.block_accepts.load(Ordering::SeqCst) {
            while !self.state.release_accepts.load(Ordering::SeqCst) {
                let released = self.state.create_release.notified();
                if self.state.release_accepts.load(Ordering::SeqCst) {
                    break;
                }
                released.await;
            }
        }

        SignedResponse::create(
            &CreateSeatResponse {
                quote_id,
                outcome: CreateSeatOutcome::Accepted {
                    seat_id,
                    guardian_fee_account: guardian_fee_account(
                        32 + u8::try_from(self.index).expect("test index fits in u8"),
                    )
                    .try_into()
                    .expect("test guardian fee account is valid"),
                },
            },
            &self.manager_key,
        )
        .map_err(Into::into)
    }

    async fn get_dkg_code(
        &self,
        _request: SignedRequest<GetDkgCodeRequest>,
    ) -> FmResult<GetDkgCodeResponse> {
        self.state.dkg_code_calls.fetch_add(1, Ordering::SeqCst);
        Ok(GetDkgCodeResponse {
            guardian_code: GuardianCode(format!("guardiancode{}", self.index)),
        })
    }

    async fn start_dkg(
        &self,
        request: SignedRequest<StartDkgRequest>,
    ) -> FmResult<StartDkgResponse> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock follows Unix epoch")
            .as_secs();
        let request = request
            .verify(Timestamp(now))
            .expect("test FI request verifies")
            .into_inner();
        self.state
            .start_callbacks
            .lock()
            .expect("test lock")
            .push(request.completion_callback);
        if self.state.report_dkg_already_started.load(Ordering::SeqCst) {
            Err(FleetManagerError::WrongState {
                status: ServiceStatus::DkgInProcess,
            })
        } else {
            Ok(StartDkgResponse)
        }
    }

    async fn restart_dkg(
        &self,
        _request: SignedRequest<RestartDkgRequest>,
    ) -> FmResult<RestartDkgResponse> {
        self.state.restart_calls.fetch_add(1, Ordering::SeqCst);
        Ok(RestartDkgResponse {
            status: ServiceStatus::DkgInProcess,
        })
    }

    async fn get_status(
        &self,
        _request: SignedRequest<GetStatusRequest>,
    ) -> FmResult<GetStatusResponse> {
        self.state.status_calls.fetch_add(1, Ordering::SeqCst);
        Ok(GetStatusResponse {
            status: ServiceStatus::Running,
            detail: StatusDetail::None,
            seat_health: Some(SeatHealth::Healthy),
        })
    }

    async fn get_invite_code(
        &self,
        _request: SignedRequest<GetInviteCodeRequest>,
    ) -> FmResult<GetInviteCodeResponse> {
        self.state.invite_calls.fetch_add(1, Ordering::SeqCst);
        Ok(GetInviteCodeResponse {
            invite_code: if self.state.disagreeing_invite.load(Ordering::SeqCst)
                && self.index + 1 == usize::from(MIN_FEDERATION_SIZE)
            {
                test_invite_for_federation(self.index, 1)
            } else {
                test_invite(self.index)
            },
        })
    }

    async fn get_peer_attestation(
        &self,
        _request: SignedRequest<GetPeerAttestationRequest>,
    ) -> FmResult<GetPeerAttestationResponse> {
        self.state.attestation_calls.fetch_add(1, Ordering::SeqCst);
        let foreign = self.index == 0 && self.state.attest_foreign_peer.load(Ordering::SeqCst);
        let wrong_fee_account =
            self.index == 0 && self.state.attest_wrong_fee_account.load(Ordering::SeqCst);
        let mut attestation = if foreign {
            foreign_peer_attestation()
        } else if let Some(peer) = self
            .state
            .attested_peer_overrides
            .lock()
            .expect("test lock")
            .get(&self.index)
            .copied()
        {
            test_attestation_for_peer(self.index, peer)
        } else if self.index < usize::from(MIN_FEDERATION_SIZE) {
            test_attestation(self.index)
        } else {
            test_attestation_for_peer(self.index, 0)
        };
        if wrong_fee_account {
            attestation.attestation.guardian_fee_account = guardian_fee_account(60);
            let message = nostr_sdk::secp256k1::Message::from_digest(
                attestation
                    .attestation
                    .digest()
                    .expect("statement canonicalizes"),
            );
            attestation.proof.signature = fman_keys(self.index).sign_schnorr(&message);
        }
        Ok(GetPeerAttestationResponse {
            fman_peer_attestation: attestation,
            seat_endpoint_proof: fedi_decentralized_domain::SeatEndpointProof {
                signature: vec![0; 64],
            },
        })
    }

    async fn get_federation_trust_material(
        &self,
        request: GetFederationTrustMaterialRequest,
    ) -> FmResult<GetFederationTrustMaterialResponse> {
        self.state
            .trust_material_requests
            .lock()
            .expect("test lock")
            .push((self.index, request.peer_ids.clone()));
        let now = test_now_secs();
        let keys = fman_keys(self.index);
        let material = fedi_decentralized_domain::FmanFederationTrustMaterial {
            fman_pubkey: fedi_decentralized_domain::Pubkey(keys.public_key().to_string()),
            federation_id: request.federation_id,
            federation_config_hash: request.federation_config_hash,
            issued_at: Timestamp(now),
            expires_at: Timestamp(
                now + crate::liquidity::FI_LIQUIDITY_TRUST_MATERIAL_VALIDITY.as_secs(),
            ),
            public_api_urls: vec![fedi_decentralized_domain::Url(format!(
                "iroh://{}",
                locator(self.index).endpoint_addr.id
            ))],
            peer_attestations: vec![test_attestation(self.index)],
            holder_authorizations: vec![discovery::envelope(
                &discovery::holder_keys(),
                keys.public_key(),
            )],
        };
        let digest = material
            .digest()
            .expect("test trust material canonicalizes");
        let signing_keys = if self
            .state
            .corrupt_trust_material_signature
            .swap(false, Ordering::SeqCst)
        {
            fman_keys(self.index + 1)
        } else {
            keys
        };
        Ok(GetFederationTrustMaterialResponse {
            version: fedi_decentralized_domain::ProtocolV1,
            proof: fedi_decentralized_domain::SchnorrSignatureProof {
                signature: signing_keys
                    .sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(digest)),
            },
            material,
        })
    }

    async fn set_meta_field(
        &self,
        request: SignedRequest<SetMetaFieldRequest>,
    ) -> FmResult<SetMetaFieldResponse> {
        let request = request.verify(Timestamp(test_now_secs()))?.into_inner();
        let request_count = {
            let mut bases = self.state.meta_request_bases.lock().expect("test lock");
            bases.push((self.index, request.expected_base));
            bases.len()
        };
        if request_count == self.state.meta_request_wave_size.load(Ordering::SeqCst) {
            self.state.meta_request_wave_complete.notify_waiters();
        }
        if self
            .state
            .hang_meta_indices
            .lock()
            .expect("test lock")
            .contains(&self.index)
        {
            return pending().await;
        }
        if self
            .state
            .block_meta_indices
            .lock()
            .expect("test lock")
            .contains(&self.index)
        {
            self.state.meta_call_blocked.notify_one();
            while !self.state.release_meta_calls.load(Ordering::SeqCst) {
                let released = self.state.meta_call_release.notified();
                if self.state.release_meta_calls.load(Ordering::SeqCst) {
                    break;
                }
                released.await;
            }
        }
        if let Some(error) = self
            .state
            .meta_terminal_errors
            .lock()
            .expect("test lock")
            .get(&self.index)
            .cloned()
        {
            return Err(error);
        }
        if self
            .state
            .stale_meta_indices
            .lock()
            .expect("test lock")
            .contains(&self.index)
        {
            return Err(FleetManagerError::MetaConsensusChanged);
        }
        if self.state.enforce_meta_bases.load(Ordering::SeqCst) {
            let actual_base = MetaConsensusBase::from_consensus(
                self.state
                    .meta_consensus_raw
                    .lock()
                    .expect("test lock")
                    .as_deref()
                    .map(|value| {
                        (
                            self.state.meta_consensus_revision.load(Ordering::SeqCst),
                            value,
                        )
                    }),
            );
            if request.expected_base != actual_base {
                return Err(FleetManagerError::MetaConsensusChanged);
            }
        }
        self.state
            .meta_submissions
            .lock()
            .expect("test lock")
            .insert(self.index, (request.key.0, request.value.0));
        self.state.meta_submission_changed.notify_one();
        Ok(SetMetaFieldResponse)
    }

    async fn propose_formation_meta(
        &self,
        request: SignedRequest<ProposeFormationMetaRequest>,
    ) -> FmResult<ProposeFormationMetaResponse> {
        let request = request.verify(Timestamp(test_now_secs()))?.into_inner();
        self.state
            .meta_request_bases
            .lock()
            .expect("test lock")
            .push((self.index, request.expected_base));
        if self
            .state
            .hang_meta_indices
            .lock()
            .expect("test lock")
            .contains(&self.index)
        {
            return pending().await;
        }
        if self
            .state
            .block_meta_indices
            .lock()
            .expect("test lock")
            .contains(&self.index)
        {
            self.state.meta_call_blocked.notify_one();
            while !self.state.release_meta_calls.load(Ordering::SeqCst) {
                self.state.meta_call_release.notified().await;
            }
        }
        if let Some(error) = self
            .state
            .meta_terminal_errors
            .lock()
            .expect("test lock")
            .get(&self.index)
            .cloned()
        {
            return Err(error);
        }
        if self
            .state
            .stale_meta_indices
            .lock()
            .expect("test lock")
            .contains(&self.index)
        {
            return Err(FleetManagerError::MetaConsensusChanged);
        }
        if self.state.enforce_meta_bases.load(Ordering::SeqCst) {
            let actual_base = MetaConsensusBase::from_consensus(
                self.state
                    .meta_consensus_raw
                    .lock()
                    .expect("test lock")
                    .as_deref()
                    .map(|value| {
                        (
                            self.state.meta_consensus_revision.load(Ordering::SeqCst),
                            value,
                        )
                    }),
            );
            if request.expected_base != actual_base {
                return Err(FleetManagerError::MetaConsensusChanged);
            }
        }
        let guardian_verification_fee_account = self
            .state
            .formation_guardian_verification_fee_accounts
            .lock()
            .expect("test lock")
            .get(&self.index)
            .cloned()
            .unwrap_or_else(|| guardian_fee_account(31));
        let guardian_verification_fee_account =
            fedi_decentralized_service_fleet_manager::GuardianFeeAccount::try_from(
                guardian_verification_fee_account,
            )
            .unwrap();
        if request.guardian_verification_fee_account != guardian_verification_fee_account {
            return Err(FleetManagerError::GuardianVerificationFeeAccountMismatch);
        }
        let bindings = fedi_decentralized_domain::FmanSeatBindings::new(
            request
                .seat_bindings
                .iter()
                .map(|binding| binding.attestation.clone()),
        )
        .map_err(|error| FleetManagerError::Other(error.to_string()))?;
        let canonical_bindings = bindings
            .canonical_string()
            .map_err(|error| FleetManagerError::Other(error.to_string()))?;
        let mut recipients = bindings
            .seat_bindings()
            .iter()
            .map(|binding| {
                GuardianFeeRecipient::new(
                    fedi_decentralized_service_fleet_manager::GuardianFeeAccount::try_from(
                        binding.attestation.guardian_fee_account.clone(),
                    )
                    .expect("test attestation account is valid"),
                    GUARDIAN_GUARDIAN_FEE_WEIGHT,
                )
            })
            .collect::<Vec<_>>();
        recipients.push(GuardianFeeRecipient::new(
            request.fi_fee_account,
            FI_GUARDIAN_FEE_WEIGHT,
        ));
        recipients.push(GuardianFeeRecipient::new(
            guardian_verification_fee_account.clone(),
            GUARDIAN_VERIFICATION_FEE_WEIGHT,
        ));
        recipients.sort_by_key(|recipient| recipient.account.as_account().id());
        let recipients = canonical_guardian_fee_recipient_list(&recipients)
            .map_err(|error| FleetManagerError::Other(error.to_string()))?;
        self.state
            .meta_submissions
            .lock()
            .expect("test lock")
            .insert(
                self.index,
                (
                    fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY.to_owned(),
                    canonical_bindings,
                ),
            );
        self.state
            .fee_submissions
            .lock()
            .expect("test lock")
            .insert(self.index, (request.send_ppm, recipients));
        self.state.meta_submission_changed.notify_one();
        Ok(ProposeFormationMetaResponse)
    }

    async fn register_gateway(
        &self,
        request: SignedRequest<RegisterGatewayRequest>,
    ) -> FmResult<RegisterGatewayResponse> {
        let request = request.verify(Timestamp(test_now_secs()))?.into_inner();
        self.state
            .gateway_registrations
            .lock()
            .expect("test lock")
            .push((self.index, request.gateway_api.to_string()));
        Ok(RegisterGatewayResponse { was_added: true })
    }

    async fn get_fedimint_stats(
        &self,
        _request: SignedRequest<GetFedimintStatsRequest>,
    ) -> FmResult<GetFedimintStatsResponse> {
        Err(unsupported())
    }
}

fn unsupported() -> FleetManagerError {
    FleetManagerError::Other("test operation unavailable".to_owned())
}

/// The final config the test federation converges on.
///
/// Shared with the FMan fake's attestations, so the peer set `fi-client`
/// derives from this config is exactly the one the attestations bind to.
fn test_federation_config() -> fedimint_core::config::ClientConfig {
    let mut config =
        fedi_decentralized_domain::test_support::test_config(usize::from(MIN_FEDERATION_SIZE));
    for (peer, endpoint) in &mut config.global.api_endpoints {
        let byte = u8::try_from(peer.to_usize() + 1).expect("small test peer");
        let api_pk = iroh_base_035::SecretKey::from_bytes(&[byte; 32]).public();
        endpoint.url = fedimint_core::util::SafeUrl::parse(&format!("iroh://{api_pk}"))
            .expect("test Iroh endpoint parses");
    }
    config
}

fn test_federation_seats() -> fedi_decentralized_domain::FederationSeats {
    fedi_decentralized_domain::federation_seats(&test_federation_config())
        .expect("fixture config derives seats")
}

/// The FMan operating keys for one seat.
///
/// Signing goes through nostr's `secp256k1`, not this crate's: they are
/// different major versions, and `FmanPeerAttestation::verify` uses nostr's.
fn fman_keys(index: usize) -> Keys {
    // Production derives the Nostr/attestation identity and the RPC
    // commitment key from distinct HKDF labels. Keep the fixture domains
    // distinct so identity/locator conflation cannot pass tests.
    let byte = u8::try_from(index + 80).expect("small test index");
    Keys::new(nostr_sdk::SecretKey::from_slice(&[byte; 32]).expect("valid test secret"))
}

/// A genuinely signed attestation binding seat `index` to its FMan.
fn test_attestation(index: usize) -> fedi_decentralized_domain::FmanPeerAttestation {
    test_attestation_for_peer(index, index)
}

/// A fresh replacement FMan identity binding the peer row it takes over.
fn test_attestation_for_peer(
    operator_index: usize,
    peer_index: usize,
) -> fedi_decentralized_domain::FmanPeerAttestation {
    let seats = test_federation_seats();
    let seat = &seats.seats()[peer_index];
    let keys = fman_keys(operator_index);
    let attestation = fedi_decentralized_domain::FmanPeerAttestationStatement {
        fman_pubkey: fedi_decentralized_domain::Pubkey(keys.public_key().to_string()),
        federation_id: seats.federation_id().clone(),
        federation_config_hash: seats.federation_config_hash().clone(),
        peer_id: seat.peer_id.clone(),
        guardian_identity: seat.guardian_identity.clone(),
        guardian_fee_account: guardian_fee_account(
            u8::try_from(operator_index + 32).expect("test operator account fits"),
        ),
        issued_at: Timestamp(1_700_000_000),
    };
    let message = nostr_sdk::secp256k1::Message::from_digest(
        attestation.digest().expect("statement canonicalizes"),
    );

    fedi_decentralized_domain::FmanPeerAttestation {
        version: fedi_decentralized_domain::ProtocolV1,
        attestation,
        proof: fedi_decentralized_domain::SchnorrSignatureProof {
            signature: keys.sign_schnorr(&message),
        },
    }
}

/// A validly signed attestation for a peer id outside the federation.
///
/// Structurally fine — distinct, canonical peer id — so it survives
/// `FmanSeatBindings::new` and only fails when checked against the config.
fn foreign_peer_attestation() -> fedi_decentralized_domain::FmanPeerAttestation {
    let seats = test_federation_seats();
    let keys = fman_keys(0);
    let attestation = fedi_decentralized_domain::FmanPeerAttestationStatement {
        fman_pubkey: fedi_decentralized_domain::Pubkey(keys.public_key().to_string()),
        federation_id: seats.federation_id().clone(),
        federation_config_hash: seats.federation_config_hash().clone(),
        peer_id: fedi_decentralized_domain::PeerId("9".to_owned()),
        guardian_identity: seats.seats()[0].guardian_identity.clone(),
        guardian_fee_account: guardian_fee_account(32),
        issued_at: Timestamp(1_700_000_000),
    };
    let message = nostr_sdk::secp256k1::Message::from_digest(
        attestation.digest().expect("statement canonicalizes"),
    );

    fedi_decentralized_domain::FmanPeerAttestation {
        version: fedi_decentralized_domain::ProtocolV1,
        attestation,
        proof: fedi_decentralized_domain::SchnorrSignatureProof {
            signature: keys.sign_schnorr(&message),
        },
    }
}

/// Consensus reader backed by what the FMan fakes actually accepted.
///
/// It reports a directory only once a consensus threshold of seats hold
/// byte-identical bytes, mirroring the real rule that a meta value becomes
/// consensus only when threshold guardians submit the same value. A fake that
/// echoed back whatever was written would make the readback vacuous — the
/// exact failure the port's contract warns about.
#[derive(Clone)]
struct TestConsensusReader {
    state: Arc<FmanState>,
    config: fedimint_core::config::ClientConfig,
    failures: Arc<AtomicUsize>,
    forced_value: Arc<Mutex<Option<String>>>,
    advance_meta_after_next_read: Arc<Mutex<Option<Vec<u8>>>>,
    reads_before_meta_advance: Arc<AtomicUsize>,
    revision_bump_after_next_read: Arc<AtomicU64>,
}

impl TestConsensusReader {
    fn new(state: Arc<FmanState>) -> Self {
        Self {
            state,
            config: test_federation_config(),
            failures: Arc::new(AtomicUsize::new(0)),
            forced_value: Arc::new(Mutex::new(None)),
            advance_meta_after_next_read: Arc::new(Mutex::new(None)),
            reads_before_meta_advance: Arc::new(AtomicUsize::new(0)),
            revision_bump_after_next_read: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Report `value` as consensus regardless of what any seat accepted.
    fn force_value(&self, value: &str) {
        *self.forced_value.lock().expect("test lock") = Some(value.to_owned());
    }

    /// Fail the next `count` reads, as an unreachable federation would.
    fn fail_next(&self, count: usize) {
        self.failures.store(count, Ordering::SeqCst);
    }

    /// Return `initial` once, while atomically advancing what every fake
    /// guardian observes to `next` before the first submission wave arrives.
    fn change_base_after_next_read(&self, initial: Vec<u8>, next: Vec<u8>) {
        self.change_base_after_reads(initial, next, 1);
    }

    /// Return `initial` for `reads` reads, then advance what subsequent reads
    /// and every fake guardian observe to `next`.
    fn change_base_after_reads(&self, initial: Vec<u8>, next: Vec<u8>, reads: usize) {
        assert!(reads > 0);
        *self.state.meta_consensus_raw.lock().expect("test lock") = Some(initial);
        *self.advance_meta_after_next_read.lock().expect("test lock") = Some(next);
        self.reads_before_meta_advance
            .store(reads, Ordering::SeqCst);
        self.state.enforce_meta_bases.store(true, Ordering::SeqCst);
    }

    /// Return the current occurrence once, while advancing the revision every
    /// fake guardian observes by `bump` with the bytes unchanged — the state a
    /// concurrent do/undo pair adopted elsewhere leaves behind.
    fn bump_revision_after_next_read(&self, initial: Vec<u8>, bump: u64) {
        *self.state.meta_consensus_raw.lock().expect("test lock") = Some(initial);
        self.revision_bump_after_next_read
            .store(bump, Ordering::SeqCst);
        self.state.enforce_meta_bases.store(true, Ordering::SeqCst);
    }

    /// Adopt `next` as live consensus, bumping the revision exactly when the
    /// bytes change, as one real `change_consensus` promotion would (a value
    /// equal to current consensus is ignored upstream, not re-promoted).
    fn adopt(&self, next: Vec<u8>) {
        let mut raw = self.state.meta_consensus_raw.lock().expect("test lock");
        if raw.as_deref() == Some(next.as_slice()) {
            return;
        }
        if raw.is_some() {
            self.state
                .meta_consensus_revision
                .fetch_add(1, Ordering::SeqCst);
        }
        *raw = Some(next);
    }

    fn agreed_value(&self) -> Option<String> {
        if let Some(forced) = self.forced_value.lock().expect("test lock").clone() {
            return Some(forced);
        }
        let submissions = self.state.meta_submissions.lock().expect("test lock");
        let threshold = test_federation_seats().consensus_threshold() as usize;
        let mut counts: HashMap<(String, String), usize> = HashMap::new();
        for entry in submissions.values() {
            *counts.entry(entry.clone()).or_default() += 1;
        }
        counts
            .into_iter()
            .find(|((key, _), count)| {
                *count >= threshold
                    && key == fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY
            })
            .map(|((_, value), _)| value)
    }

    fn agreed_maintenance_field(&self) -> Option<(String, String)> {
        let submissions = self.state.meta_submissions.lock().expect("test lock");
        let threshold = test_federation_seats().consensus_threshold() as usize;
        let mut counts: HashMap<(String, String), usize> = HashMap::new();
        for entry in submissions.values() {
            if entry.0 != fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY {
                *counts.entry(entry.clone()).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .find(|(_, count)| *count >= threshold)
            .map(|(field, _)| field)
    }

    fn agreed_fee_policy(&self) -> Option<(u64, String)> {
        let submissions = self.state.fee_submissions.lock().expect("test lock");
        let threshold = test_federation_seats().consensus_threshold() as usize;
        let mut counts: HashMap<(u64, String), usize> = HashMap::new();
        for entry in submissions.values() {
            *counts.entry(entry.clone()).or_default() += 1;
        }
        counts
            .into_iter()
            .find(|(_, count)| *count >= threshold)
            .map(|(policy, _)| policy)
    }
}

impl FederationConsensusReader for TestConsensusReader {
    async fn read_consensus(
        &self,
        _invite_code: &InviteCode,
    ) -> Result<FederationConsensusSnapshot, FederationConsensusError> {
        if self
            .failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(FederationConsensusError::new("test federation unreachable"));
        }
        let current = self
            .state
            .meta_consensus_raw
            .lock()
            .expect("test lock")
            .clone();
        let forced = self.forced_value.lock().expect("test lock").clone();
        let has_forced = forced.is_some();
        let mut meta_value = current.clone();
        if current.is_none()
            && let Some(value) = forced
        {
            let value = serde_json::to_vec(&serde_json::json!({
                fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY: value,
            }))
            .expect("test meta object serializes");
            self.adopt(value.clone());
            meta_value = Some(value);
        }
        if !has_forced
            && let Ok(fields) = current
                .as_deref()
                .map(serde_json::from_slice::<BTreeMap<String, serde_json::Value>>)
                .transpose()
        {
            let mut fields = fields.unwrap_or_default();
            let mut changed = false;
            if !fields.contains_key(fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY)
                && let (Some(value), Some((send_ppm, recipients))) =
                    (self.agreed_value(), self.agreed_fee_policy())
            {
                fields.insert(
                    fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY.to_owned(),
                    serde_json::Value::String(value),
                );
                fields.insert(
                    "fedi:guardian_fee_send_ppm".to_owned(),
                    serde_json::Value::String(send_ppm.to_string()),
                );
                fields.insert(
                    "fedi:guardian_fee_remittance_account".to_owned(),
                    serde_json::Value::String(recipients),
                );
                changed = true;
            }
            if changed {
                let value = serde_json::to_vec(&fields).expect("test metadata serializes");
                self.adopt(value.clone());
                meta_value = Some(value);
            }
        }
        if let Some((key, value)) = self.agreed_maintenance_field() {
            let mut fields = meta_value
                .as_deref()
                .map(serde_json::from_slice::<BTreeMap<String, serde_json::Value>>)
                .transpose()
                .expect("test metadata is valid JSON")
                .unwrap_or_default();
            fields.insert(key, serde_json::Value::String(value));
            let next = serde_json::to_vec(&fields).expect("test metadata serializes");
            self.adopt(next.clone());
            meta_value = Some(next);
        }
        // The returned occurrence pairs the bytes above with the revision as
        // of this read; the deferred advances below only affect what later
        // reads and enforcing guardians observe.
        let meta_revision = meta_value
            .is_some()
            .then(|| self.state.meta_consensus_revision.load(Ordering::SeqCst));
        if self.reads_before_meta_advance.fetch_update(
            Ordering::SeqCst,
            Ordering::SeqCst,
            |reads| reads.checked_sub(1),
        ) == Ok(1)
        {
            if let Some(next) = self
                .advance_meta_after_next_read
                .lock()
                .expect("test lock")
                .take()
            {
                self.adopt(next);
            }
        }
        let bump = self.revision_bump_after_next_read.swap(0, Ordering::SeqCst);
        if bump > 0 {
            self.state
                .meta_consensus_revision
                .fetch_add(bump, Ordering::SeqCst);
        }

        Ok(FederationConsensusSnapshot {
            config: self.config.clone(),
            meta_value,
            meta_revision,
            network: fedi_decentralized_domain::BitcoinNetwork::Regtest,
        })
    }

    async fn read_lnv2_gateways(
        &self,
        _invite_code: &InviteCode,
    ) -> Result<Vec<GatewayApiUrl>, FederationConsensusError> {
        self.state
            .gateway_registrations
            .lock()
            .expect("test lock")
            .iter()
            .map(|(_, url)| {
                GatewayApiUrl::try_from(url.as_str())
                    .map_err(|error| FederationConsensusError::new(error.to_string()))
            })
            .collect::<Result<BTreeSet<_>, _>>()
            .map(|urls| urls.into_iter().collect())
    }
}

fn fedimintd_version() -> FedimintdVersion {
    FEDIMINTD_VERSION_0_1
        .parse()
        .expect("release version is valid")
}

fn fedimintd_version_range() -> FedimintdVersionRange {
    FedimintdVersionRange::one_core(fedimintd_version().core())
        .expect("test release can form a range")
}

fn version_range(version: &str) -> FedimintdVersionRange {
    FedimintdVersionRange::one_core(
        version
            .parse::<FedimintdVersion>()
            .expect("test version parses")
            .core(),
    )
    .expect("test release can form a range")
}

fn set_fman_version(state: &FmanState, index: usize, version: &str) {
    state
        .fedimintd_version_overrides
        .lock()
        .expect("test lock")
        .insert(index, version.parse().expect("test version parses"));
}

fn test_invite(index: usize) -> InviteCode {
    test_invite_for_federation(index, 0)
}

fn test_invite_for_federation(index: usize, federation: u8) -> InviteCode {
    InviteCode(
        FedimintInviteCode::new(
            SafeUrl::parse(&format!("https://guardian-{index}.example/")).expect("valid test URL"),
            PeerId::from(u16::try_from(index).expect("test index fits peer id")),
            format!("{federation:064x}")
                .parse()
                .expect("valid test federation id"),
            None,
        )
        .to_string(),
    )
}

fn payment_federation_id() -> FederationId {
    let invite = PAYMENT_INVITE
        .parse::<fedimint_core::invite_code::InviteCode>()
        .expect("valid test payment invite");
    FederationId(invite.federation_id().to_string())
}

fn setup_payment_keys() -> Keys {
    Keys::parse("0000000000000000000000000000000000000000000000000000000000000001")
        .expect("fixed test setup-payment key is valid")
}

fn setup_payment_event(created_at: u64, invites: &[&str]) -> Event {
    setup_payment_raw_event(
        created_at,
        serde_json::json!({
            "version": 1,
            "fman_version": "0.1.0",
            "federations": invites,
            "telemetry_registration_url":
                "https://push.fedi.example/v1/telemetry/registrations",
        })
        .to_string(),
    )
}

fn setup_payment_event_with_min_fee_ppm(
    created_at: u64,
    invites: &[&str],
    min_fee_ppm: u64,
) -> Event {
    setup_payment_raw_event(
        created_at,
        serde_json::json!({
            "version": 1,
            "fman_version": "0.1.0",
            "federations": invites,
            "telemetry_registration_url":
                "https://push.fedi.example/v1/telemetry/registrations",
            "min_fee_ppm": min_fee_ppm,
        })
        .to_string(),
    )
}

fn setup_payment_raw_event(created_at: u64, content: impl Into<String>) -> Event {
    EventBuilder::new(Kind::Custom(SETUP_PAYMENT_FEDERATIONS_EVENT_KIND), content)
        .tag(Tag::identifier(SETUP_PAYMENT_FEDERATIONS_D_TAG))
        .custom_created_at(nostr_sdk::Timestamp::from_secs(created_at))
        .sign_with_keys(&setup_payment_keys())
        .expect("test setup-payment event signs")
}

fn second_payment_invite() -> String {
    FedimintInviteCode::new(
        SafeUrl::parse("https://second-payment.example/").expect("test URL is valid"),
        PeerId::from(0),
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .parse()
            .expect("test federation ID is valid"),
        None,
    )
    .to_string()
}

fn manager_key(index: usize) -> Keypair {
    let byte = u8::try_from(index + 20).expect("small test index");
    Keypair::from_secret_key(
        SECP256K1,
        &SecretKey::from_byte_array(&[byte; 32]).expect("valid test secret"),
    )
}

fn locator(index: usize) -> Locator {
    let byte = u8::try_from(index + 40).expect("small test index");
    Locator::new(
        EndpointAddr::new(IrohSecretKey::from_bytes(&[byte; 32]).public()),
        manager_key(index).x_only_public_key().0,
    )
}

fn locators() -> Vec<Locator> {
    (0..usize::from(MIN_FEDERATION_SIZE)).map(locator).collect()
}

fn selection_approval(max_total_msats: u64) -> FmanSelectionApproval {
    FmanSelectionApproval {
        request: FmanSelectionRequest::new(
            FederationSize(MIN_FEDERATION_SIZE),
            fedimintd_version_range(),
            PlanPreference::InfiniteBestEffort,
        )
        .expect("valid test selection request"),
        fedimintd_dkg_version: fedimintd_version().dkg_version(),
        verifier_provenance: test_peer_badge_verifier().provenance(),
        seats: locators()
            .into_iter()
            .enumerate()
            .map(|(index, locator)| crate::selection::ApprovedFmanSeat {
                fman_id: test_fman_id(index),
                locator,
            })
            .collect(),
        advertised_total_msats: 1,
        max_total_msats,
        valid_until: Timestamp(test_now_secs() + 120),
    }
}

fn compatible_selection_approval(max_total_msats: u64) -> FmanSelectionApproval {
    let mut approval = selection_approval(max_total_msats);
    approval.request = FmanSelectionRequest::new(
        FederationSize(MIN_FEDERATION_SIZE),
        FedimintdVersionRange::new(
            "0.11.1".parse().expect("range minimum parses"),
            "0.11.3".parse().expect("range maximum parses"),
        )
        .expect("test range is ordered"),
        PlanPreference::InfiniteBestEffort,
    )
    .expect("valid test selection request");
    approval
}

fn intent() -> FormationIntent {
    FormationIntent::new(
        Some(FederationName("Test Federation".to_owned())),
        FederationSize(MIN_FEDERATION_SIZE),
        PlanPreference::InfiniteBestEffort,
        fedimintd_version_range(),
    )
    .unwrap()
}

#[test]
fn serialized_intent_rejects_unknown_fields() {
    let error = serde_json::from_value::<FormationIntent>(serde_json::json!({
        "federation_name": null,
        "federation_size": 7,
        "plan": "infinite_best_effort",
        "fedimintd_versions": {"minimum":{"major":0,"minor":11,"patch":1},"maximum_exclusive":{"major":0,"minor":11,"patch":2}},
        "unknown_field": 100,
    }))
    .unwrap_err();

    assert!(error.to_string().contains("unknown field `unknown_field`"));
}

#[test]
fn serialized_intent_rejects_the_retired_formation_time_fee_field() {
    // The rate is compiled at formation rather than consumer intent: a payload still carrying
    // the retired formation-time rate is refused by the strict schema rather
    // than silently ignored.
    let error = serde_json::from_value::<FormationIntent>(serde_json::json!({
        "federation_name": null,
        "federation_size": 7,
        "guardian_fee_ppm": 0,
        "plan": "infinite_best_effort",
        "fedimintd_versions": {"minimum":{"major":0,"minor":11,"patch":1},"maximum_exclusive":{"major":0,"minor":11,"patch":2}},
    }))
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("unknown field `guardian_fee_ppm`")
    );
}

#[test]
fn serialized_intent_rejects_invalid_product_values() {
    let valid = intent();
    assert_eq!(
        serde_json::from_value::<FormationIntent>(serde_json::to_value(&valid).unwrap()).unwrap(),
        valid
    );

    for invalid_fields in [
        serde_json::json!({"federation_name": null, "federation_size": 6}),
        serde_json::json!({"federation_name": "   ", "federation_size": 7}),
    ] {
        let mut value = invalid_fields;
        let object = value.as_object_mut().unwrap();
        object.insert(
            "plan".to_owned(),
            serde_json::json!("friends_with_benefits"),
        );
        object.insert(
            "fedimintd_versions".to_owned(),
            serde_json::json!({"minimum":{"major":0,"minor":11,"patch":1},"maximum_exclusive":{"major":0,"minor":11,"patch":2}}),
        );
        assert!(serde_json::from_value::<FormationIntent>(value).is_err());
    }
}

#[test]
fn exact_payment_preflight_rejects_duplicate_semantic_quote_ids() {
    let resolved = resolved_intent();
    let signed = signed_quote(0, &resolved, [33; 32]);
    let manager = manager_key(0).x_only_public_key().0;
    let first = signed.verify(&manager).unwrap();
    let second = signed.verify(&manager).unwrap();
    let quote_id = first.quote_id();
    let payer = payment_federation_id();
    let requirements = PaymentRequirements {
        authorization_id: PaymentAuthorizationId::from_digest([44; 32]),
        total_msats: PAYMENT_AMOUNT_MSATS * 2,
        max_total_msats: Some(1_000),
        seats: vec![
            SeatPaymentRequirement {
                index: 0,
                fman_id: None,
                quote_id,
                payment_federation_id: payer.clone(),
                amount_msats: PAYMENT_AMOUNT_MSATS,
            },
            SeatPaymentRequirement {
                index: 1,
                fman_id: None,
                quote_id,
                payment_federation_id: payer,
                amount_msats: PAYMENT_AMOUNT_MSATS,
            },
        ],
    };
    let quotes = vec![first, second];

    let error = match crate::ExactPaymentPreflight::new(&requirements, &quotes) {
        Err(error) => error,
        Ok(_) => panic!("duplicate semantic quote ids were accepted"),
    };
    assert!(matches!(
        error,
        FiError::Storage(message) if message.contains("duplicate semantic quote ids")
    ));
}

#[test]
fn opaque_payment_bindings_reject_noncanonical_deserialization() {
    let authorization = PaymentAuthorizationId::from_digest([1; 32]);
    let reservation = PaymentReservationId::from_digest([2; 32]);
    let replacement = GuardianReplacementId::from_digest([3; 32]);
    assert_eq!(
        serde_json::from_value::<PaymentAuthorizationId>(
            serde_json::to_value(&authorization).unwrap()
        )
        .unwrap(),
        authorization,
    );
    assert_eq!(
        serde_json::from_value::<PaymentReservationId>(serde_json::to_value(&reservation).unwrap())
            .unwrap(),
        reservation,
    );
    assert_eq!(
        serde_json::from_value::<GuardianReplacementId>(
            serde_json::to_value(&replacement).unwrap()
        )
        .unwrap(),
        replacement,
    );
    for invalid in ["arbitrary".to_owned(), "AA".repeat(32), "0".repeat(63)] {
        assert!(
            serde_json::from_value::<PaymentAuthorizationId>(serde_json::json!(&invalid)).is_err()
        );
        assert!(
            serde_json::from_value::<PaymentReservationId>(serde_json::json!(&invalid)).is_err()
        );
        assert!(
            serde_json::from_value::<GuardianReplacementId>(serde_json::json!(&invalid)).is_err()
        );
    }
}

#[test]
fn formation_intent_accepts_product_size_boundaries_only() {
    for size in [MIN_FEDERATION_SIZE, MAX_FEDERATION_SIZE_EXCLUSIVE - 1] {
        assert!(
            FormationIntent::new(
                None,
                FederationSize(size),
                PlanPreference::InfiniteBestEffort,
                fedimintd_version_range(),
            )
            .is_ok()
        );
    }
    for size in [MIN_FEDERATION_SIZE - 1, MAX_FEDERATION_SIZE_EXCLUSIVE] {
        assert!(
            FormationIntent::new(
                None,
                FederationSize(size),
                PlanPreference::InfiniteBestEffort,
                fedimintd_version_range(),
            )
            .is_err()
        );
    }
}

fn resolved_intent() -> ResolvedFormationIntent {
    intent()
        .resolve_for_dkg(
            FederationName("Fallback Name".to_owned()),
            fedimintd_version().dkg_version(),
        )
        .expect("valid test intent")
}

fn resolved_intent_with_size(federation_size: FederationSize) -> ResolvedFormationIntent {
    let mut intent = resolved_intent();
    intent.federation_size = federation_size;
    intent
}

fn seat_progress(index: u16) -> SeatProgress {
    SeatProgress {
        index,
        fman_id: None,
        locator: locator(usize::from(index)),
        seat_id: None,
        guardian_code: None,
        phase: SeatPhase::Selected,
        freshness: FormationFreshness::Fresh,
    }
}

fn test_fman_id(index: usize) -> nostr_sdk::PublicKey {
    fman_keys(index).public_key()
}

fn selected_initial_seat(index: u16, valid_until: u64) -> crate::db::InitialSeat {
    crate::db::InitialSeat::new(
        usize::from(index),
        locator(usize::from(index)),
        crate::db::FmanAdmission::fresh_peer_badge(
            test_fman_id(usize::from(index)),
            test_peer_badge_verifier().provenance().into(),
            Timestamp(valid_until),
        ),
    )
}

fn signed_quote(
    manager_index: usize,
    intent: &ResolvedFormationIntent,
    quote_nonce: [u8; 32],
) -> SignedResponse<GetQuoteResponse> {
    signed_quote_at_price(manager_index, intent, quote_nonce, PAYMENT_AMOUNT_MSATS)
}

fn signed_quote_at_price(
    manager_index: usize,
    intent: &ResolvedFormationIntent,
    quote_nonce: [u8; 32],
    payment_amount_msats: u64,
) -> SignedResponse<GetQuoteResponse> {
    let (plan, price_msats, payment_federation_id, payment) = {
        let federation_id = payment_federation_id();
        (
            Plan::InfiniteBestEffort {
                price_msats: payment_amount_msats,
            },
            payment_amount_msats,
            Some(federation_id.clone()),
            Some(PaymentTerms::MintV1 {
                federation_id,
                issuance: vec![LockedIssuanceRequest {
                    amount_msats: payment_amount_msats,
                    blind_nonce: vec![manager_index as u8, 31, 32],
                }],
            }),
        )
    };
    SignedResponse::create(
        &GetQuoteResponse {
            terms: QuoteTerms {
                quote_nonce,
                offer_epoch: OfferEpoch::from_bytes([0; 32]),
                request: GetQuoteRequest {
                    fi_id: TestIdentity::fi_id(),
                    fedimintd_version: fedimintd_version(),
                    federation_size: intent.federation_size,
                    plan,
                    payment_federation_id,
                    refund_issuance: payment.as_ref().map(|_| RefundIssuance::MintV1 {
                        refund_nonce: [9; 32],
                        issuance: vec![LockedIssuanceRequest {
                            amount_msats: payment_amount_msats,
                            blind_nonce: vec![4, 5, 6],
                        }],
                    }),
                },
                price_msats,
                payment,
            },
        },
        &manager_key(manager_index),
    )
    .expect("valid signed test quote")
}

fn signed_free_quote(
    manager_index: usize,
    intent: &ResolvedFormationIntent,
    quote_nonce: [u8; 32],
) -> SignedResponse<GetQuoteResponse> {
    SignedResponse::create(
        &GetQuoteResponse {
            terms: QuoteTerms {
                quote_nonce,
                offer_epoch: OfferEpoch::from_bytes([0; 32]),
                request: GetQuoteRequest {
                    fi_id: TestIdentity::fi_id(),
                    fedimintd_version: fedimintd_version(),
                    federation_size: intent.federation_size,
                    plan: Plan::InfiniteBestEffort { price_msats: 0 },
                    payment_federation_id: None,
                    refund_issuance: None,
                },
                price_msats: 0,
                payment: None,
            },
        },
        &manager_key(manager_index),
    )
    .expect("valid signed free test quote")
}

fn test_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock after Unix epoch")
        .as_secs()
}

fn stored_liquidity_operation(marker: u64) -> crate::liquidity::StoredLiquidityOperation {
    use fedi_decentralized_domain::{
        BitcoinNetwork, FederationId as DomainFederationId, FederationName, HashBytes,
        InviteCode as DomainInviteCode,
    };
    use fedi_decentralized_service_liquidity_manager::{
        FederationLiquidityDetails, LiquidityAmountBounds, Pubkey, Sats,
        Timestamp as FlipTimestamp, Url, request_liquidity_details_hash,
    };

    let commitment =
        fedi_decentralized_service_liquidity_manager::RequestLiquidityDetailsCommitmentV1 {
            version:
                fedi_decentralized_service_liquidity_manager::PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
            requester_pubkey: Pubkey(format!("requester-{marker}")),
            provider_pubkey: Pubkey("provider".to_owned()),
            network: BitcoinNetwork::Regtest,
            amounts: LiquidityAmountBounds {
                gateway_min_amount: Sats(marker),
                gateway_max_amount: None,
                stability_min_amount: Sats(0),
                stability_max_amount: None,
            },
            federation_details: FederationLiquidityDetails {
                invite_code: DomainInviteCode(format!("invite-{marker}")),
                federation_id: DomainFederationId(format!("federation-{marker}")),
                federation_name: FederationName(format!("Federation {marker}")),
                federation_config_hash: HashBytes(vec![marker as u8; 32]),
                fleet_seat_hints: Vec::new(),
                revocation_locations: Vec::new(),
            },
            expires_at: FlipTimestamp(test_now_secs() + 3_600),
        };
    let details_payload_hash = request_liquidity_details_hash(&commitment).expect("hash intent");
    crate::liquidity::StoredLiquidityOperation {
        schema_version: 3,
        operation_id: LiquidityOperationId(hex::encode(details_payload_hash.0)),
        formation_id: FormationId(format!("formation-{marker}")),
        commitment,
        endpoint_hint: Url("iroh://provider".to_owned()),
        details_payload_hash,
        response: None,
        status: None,
        verified_gateway_api: None,
    }
}

#[derive(Clone, Copy)]
struct TestLiquidityVerifier;

impl TestLiquidityBadgeVerifier for TestLiquidityVerifier {
    async fn verify_subject_for_test(
        &self,
        envelope: &fedi_decentralized_domain::HolderAuthorizationEnvelope,
    ) -> Result<PublicKey, fedi_decentralized_peer_badge_verifier::PeerBadgeVerificationError> {
        Ok(envelope.holder_authorization.authorization.subject_pubkey.0)
    }
}

fn liquidity_provider_keys() -> Keys {
    Keys::parse("0000000000000000000000000000000000000000000000000000000000000032")
        .expect("fixed provider key")
}

fn other_liquidity_provider_keys() -> Keys {
    Keys::parse("0000000000000000000000000000000000000000000000000000000000000033")
        .expect("fixed alternate provider key")
}

fn liquidity_provider_endpoint() -> EndpointAddr {
    EndpointAddr::new(IrohSecretKey::from_bytes(&[77; 32]).public())
}

fn sign_liquidity_payload<T: serde::Serialize>(
    domain: liquidity_api::PublicRpcPayloadDomain,
    payload: T,
    keys: &Keys,
) -> liquidity_api::Signed<T> {
    let hash = liquidity_api::public_rpc_payload_hash(domain, &payload)
        .expect("test provider payload hashes");
    liquidity_api::Signed {
        payload,
        proof: liquidity_api::PayloadProof {
            signature: liquidity_api::Signature(
                keys.sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(hash.0))
                    .serialize()
                    .to_vec(),
            ),
        },
    }
}

fn liquidity_provider_event() -> Event {
    use fedi_decentralized_nostr::flip::{
        FLIP_PROVIDER_ADVERTISEMENT_D_TAG, FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
        FLIP_PROVIDER_ADVERTISEMENT_HASHTAG,
    };

    let keys = liquidity_provider_keys();
    let now = test_now_secs();
    let endpoint = liquidity_provider_endpoint();
    let alpn =
        String::from_utf8_lossy(liquidity_api::PUBLIC_LIQUIDITY_API_ALPN).replace('/', "%2F");
    let payload = liquidity_api::LiquidityProviderAdvertisement {
        version: liquidity_api::PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
        provider_pubkey: liquidity_api::Pubkey(keys.public_key().to_string()),
        issued_at: liquidity_api::Timestamp(now - 60),
        expires_at: liquidity_api::Timestamp(now + 3_600),
        supported_sources: vec![
            liquidity_api::SourceType::Gateway,
            liquidity_api::SourceType::StabilityPool,
        ],
        holder_authorizations: vec![discovery::envelope(
            &discovery::holder_keys(),
            keys.public_key(),
        )],
        policy: liquidity_api::ProviderPolicy {
            accepted_attester_policies: Vec::new(),
            supported_networks: vec![fedi_decentralized_domain::BitcoinNetwork::Regtest],
        },
        display: None,
        api_endpoints: vec![liquidity_api::Url(format!(
            "iroh://{}?alpn={alpn}",
            endpoint.id
        ))],
        api_versions: vec![liquidity_api::PUBLIC_LIQUIDITY_PROTOCOL_VERSION],
        relay_hints: Vec::new(),
    };
    let hash = liquidity_api::advertisement_hash(&payload).expect("advertisement hashes");
    let signed = liquidity_api::Signed {
        payload,
        proof: liquidity_api::PayloadProof {
            signature: liquidity_api::Signature(
                keys.sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(hash.0))
                    .serialize()
                    .to_vec(),
            ),
        },
    };
    EventBuilder::new(
        Kind::Custom(FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND),
        serde_json::to_string(&signed).expect("advertisement serializes"),
    )
    .tag(Tag::identifier(FLIP_PROVIDER_ADVERTISEMENT_D_TAG))
    .tag(Tag::hashtag(FLIP_PROVIDER_ADVERTISEMENT_HASHTAG))
    .custom_created_at(nostr_sdk::Timestamp::from_secs(now - 30))
    .sign_with_keys(&keys)
    .expect("advertisement event signs")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LiquidityResponseFault {
    InvalidSignature,
    WrongHash,
    WrongItemSet,
    BelowMinimum,
    AboveMaximum,
    Rejected,
}

struct TestLiquidityProviderState {
    store: db::FiStore,
    calls: Mutex<Vec<&'static str>>,
    requests: Mutex<Vec<liquidity_api::Signed<liquidity_api::RequestLiquidityRequest>>>,
    allocations: Mutex<HashMap<liquidity_api::Sha256Digest, liquidity_api::AllocationStatus>>,
    lose_first_ack: AtomicBool,
    fail_first_before_allocation: AtomicBool,
    fail_next_connect: AtomicBool,
    response_fault: Mutex<Option<LiquidityResponseFault>>,
    status_error: Mutex<Option<liquidity_api::ServiceErrorCode>>,
}

impl TestLiquidityProviderState {
    fn new(store: db::FiStore) -> Arc<Self> {
        Arc::new(Self {
            store,
            calls: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
            allocations: Mutex::new(HashMap::new()),
            lose_first_ack: AtomicBool::new(false),
            fail_first_before_allocation: AtomicBool::new(false),
            fail_next_connect: AtomicBool::new(false),
            response_fault: Mutex::new(None),
            status_error: Mutex::new(None),
        })
    }
}

#[derive(Clone)]
struct TestLiquidityProvider(Arc<TestLiquidityProviderState>);

fn allocation_items(
    amounts: &liquidity_api::LiquidityAmountBounds,
) -> Vec<liquidity_api::AllocationItemStatus> {
    let now = liquidity_api::Timestamp(test_now_secs());
    let mut items = Vec::new();
    if amounts.gateway_min_amount.0 > 0 {
        items.push(liquidity_api::AllocationItemStatus {
            target: liquidity_api::AllocationItemTarget::Gateway {
                item_id: liquidity_api::ItemId("gateway-item".to_owned()),
                gateway_id: liquidity_api::GatewayId("gateway".to_owned()),
                gateway_name: liquidity_api::GatewayName("Fedi Gateway".to_owned()),
                amount: amounts.gateway_min_amount,
            },
            status: liquidity_api::ItemAllocationStatus::Pending,
            fulfilled_amount: None,
            completion_evidence: None,
            failure: None,
            updated_at: now,
        });
    }
    if amounts.stability_min_amount.0 > 0 {
        items.push(liquidity_api::AllocationItemStatus {
            target: liquidity_api::AllocationItemTarget::StabilityPool {
                item_id: liquidity_api::ItemId("stability-item".to_owned()),
                amount: amounts.stability_min_amount,
            },
            status: liquidity_api::ItemAllocationStatus::Pending,
            fulfilled_amount: None,
            completion_evidence: None,
            failure: None,
            updated_at: now,
        });
    }
    items
}

fn complete_gateway_allocation(
    allocation: &mut liquidity_api::AllocationStatus,
    gateway_api: GatewayApiUrl,
) {
    let item = allocation
        .item_statuses
        .iter_mut()
        .find(|item| {
            matches!(
                &item.target,
                liquidity_api::AllocationItemTarget::Gateway { .. }
            )
        })
        .expect("gateway item");
    let (gateway_id, amount) = match &item.target {
        liquidity_api::AllocationItemTarget::Gateway {
            gateway_id, amount, ..
        } => (gateway_id.clone(), *amount),
        _ => unreachable!("selected a gateway item"),
    };
    item.status = liquidity_api::ItemAllocationStatus::Completed;
    item.fulfilled_amount = Some(amount);
    item.completion_evidence = Some(liquidity_api::CompletionEvidence::Gateway(
        liquidity_api::GatewayCompletionEvidence {
            gateway_id,
            gateway_api,
            fulfilled_amount: amount,
            observed_gateway_balance: amount,
            observed_at: liquidity_api::Timestamp(test_now_secs()),
            withdrawal_txid: Some("txid".to_owned()),
            wallet_operation_id: None,
        },
    ));
}

impl liquidity_api::PublicLiquidityApi for TestLiquidityProvider {
    async fn get_provider_info(
        &self,
        _request: liquidity_api::Signed<liquidity_api::GetProviderInfoRequest>,
    ) -> liquidity_api::ServiceResult<liquidity_api::Signed<liquidity_api::GetProviderInfoResponse>>
    {
        Err(liquidity_api::ServiceError::with_code(
            liquidity_api::ServiceErrorCode::NotFound,
            "test provider info unavailable",
        ))
    }

    async fn request_liquidity(
        &self,
        request: liquidity_api::Signed<liquidity_api::RequestLiquidityRequest>,
    ) -> liquidity_api::ServiceResult<liquidity_api::Signed<liquidity_api::RequestLiquidityResponse>>
    {
        self.0.calls.lock().expect("test lock").push("request");
        let operation_id =
            LiquidityOperationId(hex::encode(request.payload.details_payload_hash.0));
        self.0
            .store
            .load_liquidity_operation(&operation_id)
            .await
            .expect("FI persisted the exact operation before provider mutation");
        self.0
            .requests
            .lock()
            .expect("test lock")
            .push(request.clone());
        if self
            .0
            .fail_first_before_allocation
            .swap(false, Ordering::SeqCst)
        {
            return Err(liquidity_api::ServiceError::with_code(
                liquidity_api::ServiceErrorCode::Unavailable,
                "injected pre-accept failure",
            ));
        }

        let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());
        let fault = self.0.response_fault.lock().expect("test lock").take();
        if fault == Some(LiquidityResponseFault::Rejected) {
            // A signed rejection is terminal: no allocation is created.
            let payload = liquidity_api::RequestLiquidityResponse {
                version: liquidity_api::PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
                details_payload_hash: request.payload.details_payload_hash,
                provider_pubkey: provider,
                issued_at: liquidity_api::Timestamp(test_now_secs()),
                outcome: liquidity_api::RequestLiquidityOutcome::Rejected(
                    liquidity_api::PublicRejection {
                        code: liquidity_api::PublicRejectionCode::InsufficientCapacity,
                        reason: Some("injected rejection".to_owned()),
                    },
                ),
            };
            return Ok(sign_liquidity_payload(
                liquidity_api::PublicRpcPayloadDomain::RequestLiquidityResponse,
                payload,
                &liquidity_provider_keys(),
            ));
        }
        let mut status = liquidity_api::AllocationStatus {
            details_payload_hash: request.payload.details_payload_hash,
            provider_pubkey: provider.clone(),
            item_statuses: allocation_items(&request.payload.amounts),
        };
        if fault == Some(LiquidityResponseFault::WrongItemSet) {
            status.item_statuses = allocation_items(&liquidity_api::LiquidityAmountBounds {
                gateway_min_amount: liquidity_api::Sats(0),
                gateway_max_amount: None,
                stability_min_amount: liquidity_api::Sats(1),
                stability_max_amount: None,
            });
        }
        if matches!(
            fault,
            Some(LiquidityResponseFault::BelowMinimum | LiquidityResponseFault::AboveMaximum)
        ) {
            let liquidity_api::AllocationItemTarget::Gateway { amount, .. } =
                &mut status.item_statuses[0].target
            else {
                panic!("fault fixture requires a gateway item");
            };
            *amount = match fault {
                Some(LiquidityResponseFault::BelowMinimum) => {
                    liquidity_api::Sats(request.payload.amounts.gateway_min_amount.0 - 1)
                }
                Some(LiquidityResponseFault::AboveMaximum) => liquidity_api::Sats(
                    request
                        .payload
                        .amounts
                        .gateway_max_amount
                        .expect("above-maximum fixture supplies a maximum")
                        .0
                        + 1,
                ),
                _ => unreachable!(),
            };
        }
        self.0
            .allocations
            .lock()
            .expect("test lock")
            .insert(request.payload.details_payload_hash, status.clone());
        if self.0.lose_first_ack.swap(false, Ordering::SeqCst) {
            return Err(liquidity_api::ServiceError::with_code(
                liquidity_api::ServiceErrorCode::Unavailable,
                "injected lost acknowledgement",
            ));
        }

        let payload = liquidity_api::RequestLiquidityResponse {
            version: liquidity_api::PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
            details_payload_hash: if fault == Some(LiquidityResponseFault::WrongHash) {
                liquidity_api::Sha256Digest([99; 32])
            } else {
                request.payload.details_payload_hash
            },
            provider_pubkey: provider,
            issued_at: liquidity_api::Timestamp(test_now_secs()),
            outcome: liquidity_api::RequestLiquidityOutcome::Accepted(status),
        };
        let keys = if fault == Some(LiquidityResponseFault::InvalidSignature) {
            other_liquidity_provider_keys()
        } else {
            liquidity_provider_keys()
        };
        Ok(sign_liquidity_payload(
            liquidity_api::PublicRpcPayloadDomain::RequestLiquidityResponse,
            payload,
            &keys,
        ))
    }

    async fn get_allocation_status(
        &self,
        request: liquidity_api::Signed<liquidity_api::GetAllocationStatusRequest>,
    ) -> liquidity_api::ServiceResult<
        liquidity_api::Signed<liquidity_api::GetAllocationStatusResponse>,
    > {
        self.0.calls.lock().expect("test lock").push("status");
        if let Some(code) = self.0.status_error.lock().expect("test lock").take() {
            return Err(liquidity_api::ServiceError::with_code(
                code,
                "injected status failure",
            ));
        }
        let status = self
            .0
            .allocations
            .lock()
            .expect("test lock")
            .get(&request.payload.details_payload_hash)
            .cloned()
            .ok_or_else(|| {
                liquidity_api::ServiceError::with_code(
                    liquidity_api::ServiceErrorCode::NotFound,
                    "allocation not found",
                )
            })?;
        let payload = liquidity_api::GetAllocationStatusResponse {
            version: liquidity_api::PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
            provider_pubkey: liquidity_api::Pubkey(
                liquidity_provider_keys().public_key().to_string(),
            ),
            issued_at: liquidity_api::Timestamp(test_now_secs()),
            status,
        };
        Ok(sign_liquidity_payload(
            liquidity_api::PublicRpcPayloadDomain::GetAllocationStatusResponse,
            payload,
            &liquidity_provider_keys(),
        ))
    }
}

#[derive(Clone)]
struct TestLiquidityConnector(Arc<TestLiquidityProviderState>);

impl LiquidityProviderConnector for TestLiquidityConnector {
    type Client = TestLiquidityProvider;

    async fn connect(
        &self,
        endpoint: &EndpointAddr,
    ) -> Result<Self::Client, LiquidityProviderConnectorError> {
        if self.0.fail_next_connect.swap(false, Ordering::SeqCst) {
            return Err(LiquidityProviderConnectorError::new(
                "injected connect failure",
            ));
        }
        if endpoint != &liquidity_provider_endpoint() {
            return Err(LiquidityProviderConnectorError::new(
                "unexpected provider endpoint",
            ));
        }
        Ok(TestLiquidityProvider(self.0.clone()))
    }
}

async fn formed_client_for_liquidity() -> (
    TestClient,
    FormationId,
    Arc<FmanState>,
    TestLiquidityConnector,
) {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    registry
        .advertisements
        .lock()
        .expect("test lock")
        .push(liquidity_provider_event());
    let client = open_client_with_registry(
        database,
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
        registry,
    )
    .await;
    client
        .pay_and_create(
            intent(),
            selection_approval(1),
            payment_federation_id(),
            options(),
        )
        .await
        .expect("form selected test federation");
    let formation_id = formation(&client.status()).formation_id.clone();
    let provider = TestLiquidityProviderState::new(client.inner.store.clone());
    (
        client,
        formation_id,
        fman_state,
        TestLiquidityConnector(provider),
    )
}

fn options() -> FormationRunOptions {
    FormationRunOptions::new(crate::FormationRunOptionsConfig {
        poll_interval: Duration::from_millis(1),
        run_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_millis(200),
    })
    .unwrap()
}

fn long_request_options() -> FormationRunOptions {
    FormationRunOptions::new(crate::FormationRunOptionsConfig {
        poll_interval: Duration::from_millis(1),
        run_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(30),
    })
    .unwrap()
}

#[test]
fn formation_run_options_enforce_boundaries_and_lease_horizon() {
    let valid = Duration::from_millis(1);
    for (config, expected) in [
        (
            crate::FormationRunOptionsConfig {
                poll_interval: Duration::ZERO,
                run_timeout: valid,
                request_timeout: valid,
            },
            crate::InvalidFormationRunOptions::BelowMinimum {
                field: crate::FormationTimingField::PollInterval,
            },
        ),
        (
            crate::FormationRunOptionsConfig {
                poll_interval: valid,
                run_timeout: Duration::from_micros(1_999),
                request_timeout: valid,
            },
            crate::InvalidFormationRunOptions::NonIntegral {
                field: crate::FormationTimingField::RunTimeout,
            },
        ),
        (
            crate::FormationRunOptionsConfig {
                poll_interval: valid,
                run_timeout: valid,
                request_timeout: Duration::from_millis(i32::MAX as u64 + 1),
            },
            crate::InvalidFormationRunOptions::AboveMaximum {
                field: crate::FormationTimingField::RequestTimeout,
            },
        ),
    ] {
        assert_eq!(FormationRunOptions::new(config).unwrap_err(), expected);
    }
    for duration in [valid, Duration::from_millis(i32::MAX as u64)] {
        FormationRunOptions::new(crate::FormationRunOptionsConfig {
            poll_interval: duration,
            run_timeout: duration,
            request_timeout: duration,
        })
        .unwrap();
    }
    let bounded = FormationRunOptions::new(crate::FormationRunOptionsConfig {
        poll_interval: valid,
        run_timeout: valid,
        request_timeout: Duration::from_millis(i32::MAX as u64),
    })
    .unwrap();
    assert_eq!(bounded.lease_duration(), Duration::from_secs(60) + valid);
    assert_eq!(
        bounded.lease_renewal_duration(),
        Duration::from_secs(60) + valid
    );
}

type TestClient =
    FiClient<TestIdentity, TestPayments, TestRegistry, TestConnector, TestConsensusReader>;

trait TestSetupPaymentSelection {
    async fn select_setup_payment_federation_for_test(
        &self,
        request_timeout: Duration,
    ) -> FiResult<FederationId>;
}

impl TestSetupPaymentSelection for TestClient {
    async fn select_setup_payment_federation_for_test(
        &self,
        request_timeout: Duration,
    ) -> FiResult<FederationId> {
        let options = FormationRunOptions::new(crate::FormationRunOptionsConfig {
            poll_interval: Duration::from_millis(1),
            run_timeout: Duration::from_secs(2),
            request_timeout,
        })?;
        let (deadline, lease) =
            crate::formation::start_driver_run(&self.inner.store, options).await?;
        let run = crate::formation::DriverRun::new(options, deadline, &lease);
        let result = self.select_setup_payment_federation(run).await;
        crate::formation::finish_driver_run(
            result,
            self.inner.store.release_driver_lease(lease).await,
        )
    }
}

async fn open_client(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<FmanState>,
    config: FmanConfig,
) -> TestClient {
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    open_client_with_registry(database, payments, state, config, registry).await
}

/// An FI with no deployment-pinned setup-payment publisher: it has no
/// authenticated set of federations to fund from, so it can only take seats an
/// FMan gives away.
async fn open_client_that_cannot_pay(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<FmanState>,
    config: FmanConfig,
) -> TestClient {
    FiClient::open_inner(
        database,
        FiClientPorts {
            identity: TestIdentity,
            payments,
            registry: TestRegistry::default(),
            fman_connector: TestConnector {
                state: state.clone(),
                config,
            },
            consensus_reader: TestConsensusReader::new(state),
            fi_fee_account_provider: Arc::new(TestFiFeeAccountProvider::default()),
        },
        test_peer_badge_verifier(),
        None,
        Some(guardian_fee_account(31)),
    )
    .await
    .expect("open test FI client")
}

async fn open_client_with_reader(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<FmanState>,
    config: FmanConfig,
    consensus_reader: TestConsensusReader,
) -> TestClient {
    open_client_with_registry_and_reader(
        database,
        payments,
        state,
        config,
        TestRegistry::default(),
        consensus_reader,
    )
    .await
}

async fn open_client_with_registry(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<FmanState>,
    config: FmanConfig,
    registry: TestRegistry,
) -> TestClient {
    let consensus_reader = TestConsensusReader::new(state.clone());
    open_client_with_registry_and_reader(
        database,
        payments,
        state,
        config,
        registry,
        consensus_reader,
    )
    .await
}

async fn open_client_with_registry_and_reader(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<FmanState>,
    config: FmanConfig,
    registry: TestRegistry,
    consensus_reader: TestConsensusReader,
) -> TestClient {
    open_client_with_registry_reader_and_fee_account(
        database,
        payments,
        state,
        config,
        registry,
        consensus_reader,
        TestFiFeeAccountProvider::default(),
    )
    .await
}

async fn open_client_with_fee_account(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<FmanState>,
    config: FmanConfig,
    fi_fee_account_provider: TestFiFeeAccountProvider,
) -> TestClient {
    open_client_with_registry_reader_and_fee_account(
        database,
        payments,
        state.clone(),
        config,
        TestRegistry::default(),
        TestConsensusReader::new(state),
        fi_fee_account_provider,
    )
    .await
}

async fn open_client_with_registry_reader_and_fee_account(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<FmanState>,
    config: FmanConfig,
    registry: TestRegistry,
    consensus_reader: TestConsensusReader,
    fi_fee_account_provider: TestFiFeeAccountProvider,
) -> TestClient {
    FiClient::open_inner(
        database,
        FiClientPorts {
            identity: TestIdentity,
            payments,
            registry,
            fman_connector: TestConnector { state, config },
            consensus_reader,
            fi_fee_account_provider: Arc::new(fi_fee_account_provider),
        },
        test_peer_badge_verifier(),
        Some(setup_payment_keys().public_key()),
        Some(guardian_fee_account(31)),
    )
    .await
    .expect("open test FI client")
}

async fn open_client_with_verifier(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<FmanState>,
    config: FmanConfig,
    verifier: PeerBadgeVerifier,
) -> TestClient {
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    FiClient::open_inner(
        database,
        FiClientPorts {
            identity: TestIdentity,
            payments,
            registry,
            fman_connector: TestConnector {
                state: state.clone(),
                config,
            },
            consensus_reader: TestConsensusReader::new(state),
            fi_fee_account_provider: Arc::new(TestFiFeeAccountProvider::default()),
        },
        verifier,
        Some(setup_payment_keys().public_key()),
        Some(guardian_fee_account(31)),
    )
    .await
    .expect("open test FI client with explicit verifier")
}

async fn open_client_with_store(
    store: db::FiStore,
    payments: TestPayments,
    state: Arc<FmanState>,
    config: FmanConfig,
) -> TestClient {
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    open_client_with_store_and_registry(store, payments, state, config, registry).await
}

async fn open_client_with_store_and_registry(
    store: db::FiStore,
    payments: TestPayments,
    state: Arc<FmanState>,
    config: FmanConfig,
    registry: TestRegistry,
) -> TestClient {
    let status = store.load_status(TestIdentity::fi_id()).await.unwrap();
    let (progress, _) = tokio::sync::watch::channel(status);
    FiClient {
        inner: Arc::new(FiClientInner {
            store,
            ports: FiClientPorts {
                identity: TestIdentity,
                payments,
                registry,
                fman_connector: TestConnector {
                    state: state.clone(),
                    config,
                },
                consensus_reader: TestConsensusReader::new(state),
                fi_fee_account_provider: Arc::new(TestFiFeeAccountProvider::default()),
            },
            progress,
            run_guard: tokio::sync::Mutex::new(()),
            peer_badge_verifier: test_peer_badge_verifier(),
            setup_payment_publisher: Some(setup_payment_keys().public_key()),
            guardian_verification_fee_account: Some(guardian_fee_account(31)),
        }),
    }
}

fn formation(status: &FiStatus) -> &FormationSnapshot {
    match status {
        FiStatus::Formation(snapshot) => snapshot,
        FiStatus::Idle => panic!("expected active formation"),
    }
}

fn payment_requirements(status: &FiStatus) -> &PaymentRequirements {
    match formation(status).action_required.as_ref() {
        Some(FormationActionRequired::AuthorizePayments(requirements)) => requirements,
        Some(FormationActionRequired::ReplaceGuardians(_)) => {
            panic!("expected payment authorization, found guardian replacement")
        }
        None => panic!("expected aggregate payment authorization"),
    }
}

fn active_recovery(recovery: FiRecovery) -> crate::db::ActiveFormationRecovery {
    match recovery {
        FiRecovery::Formation(recovery) => *recovery,
        FiRecovery::Idle => panic!("expected active formation"),
    }
}

fn quote_ids(records: &[QuoteRecord]) -> HashSet<QuoteId> {
    records.iter().map(|record| record.quote_id).collect()
}

#[tokio::test]
async fn canonical_policy_order_overrides_wallet_output_order() {
    let second_invite = second_payment_invite();
    let mut expected_members = [
        (
            payment_federation_id(),
            InviteCode(PAYMENT_INVITE.to_owned()),
        ),
        (
            FederationId(
                second_invite
                    .parse::<FedimintInviteCode>()
                    .expect("second invite parses")
                    .federation_id()
                    .to_string(),
            ),
            InviteCode(second_invite.clone()),
        ),
    ];
    expected_members.sort_by(|left, right| left.0.cmp(&right.0));
    let expected_ids = expected_members
        .iter()
        .map(|(federation_id, _)| federation_id.clone())
        .collect::<Vec<_>>();
    let (payments, payment_state) = TestPayments::new();
    payment_state.reverse_payable.store(true, Ordering::SeqCst);
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(
            test_now_secs(),
            &[PAYMENT_INVITE, &second_invite],
        ));
    let client = open_client_with_registry(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
        registry,
    )
    .await;

    let admitted = client
        .admitted_setup_payment_federations(options())
        .await
        .unwrap();
    assert_eq!(
        admitted
            .iter()
            .map(|member| (member.federation_id(), member.invite_code()))
            .collect::<Vec<_>>(),
        expected_members
            .iter()
            .map(|(federation_id, invite_code)| (federation_id, invite_code))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        client
            .select_setup_payment_federation_for_test(options().request_timeout())
            .await
            .unwrap(),
        expected_ids[0]
    );
}

#[tokio::test]
async fn paid_selection_requires_a_nonempty_authenticated_common_set() {
    for candidates in [Vec::new(), vec![setup_payment_event(test_now_secs(), &[])]] {
        let database = MemDatabase::new().into_database();
        let (payments, payment_state) = TestPayments::new();
        let registry = TestRegistry::default();
        *registry.candidates.lock().expect("test lock") = candidates;
        let client = open_client_with_registry(
            database,
            payments,
            Arc::new(FmanState::default()),
            FmanConfig::paid(),
            registry,
        )
        .await;

        let error = client
            .select_setup_payment_federation_for_test(options().request_timeout())
            .await
            .expect_err("missing or empty policy stops paid selection");
        assert!(matches!(error, FiError::Payment(_)));
        assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn paid_selection_requires_a_wallet_member_of_authenticated_policy() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    payment_state.pay_none.store(true, Ordering::SeqCst);
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    let client = open_client_with_registry(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
        registry,
    )
    .await;

    let error = client
        .select_setup_payment_federation_for_test(options().request_timeout())
        .await
        .expect_err("wallet holdings only filter authenticated policy");
    assert!(matches!(error, FiError::Payment(_)));
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn admitted_payer_listing_includes_a_zero_balance_policy_member() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    payment_state.pay_none.store(true, Ordering::SeqCst);
    let client = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;

    let admitted = client
        .admitted_setup_payment_federations(options())
        .await
        .unwrap();
    assert_eq!(admitted.len(), 1);
    assert_eq!(admitted[0].federation_id(), &payment_federation_id());
    assert_eq!(
        admitted[0].invite_code(),
        &InviteCode(PAYMENT_INVITE.to_owned())
    );
    assert_eq!(
        payment_state.payable_calls.load(Ordering::SeqCst),
        0,
        "admitted policy listing must not filter on wallet balance"
    );
}

#[tokio::test]
async fn admitted_payer_listing_represents_an_authenticated_empty_stop_set() {
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[]));
    let (payments, payment_state) = TestPayments::new();
    let client = open_client_with_registry(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
        registry,
    )
    .await;

    assert!(
        client
            .admitted_setup_payment_federations(options())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn common_set_high_water_survives_restart_and_rejects_rollback() {
    let database = MemDatabase::new().into_database();
    let now = test_now_secs();
    let current = setup_payment_event(now, &[PAYMENT_INVITE]);
    let registry = TestRegistry::default();
    registry.candidates.lock().expect("test lock").push(current);
    let (payments, _) = TestPayments::new();
    let client = open_client_with_registry(
        database.clone(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
        registry,
    )
    .await;
    assert_eq!(
        client
            .select_setup_payment_federation_for_test(options().request_timeout())
            .await
            .expect("current authenticated set selects"),
        payment_federation_id()
    );
    drop(client);

    let rollback_registry = TestRegistry::default();
    rollback_registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(now - 1, &[]));
    let (payments, _) = TestPayments::new();
    let reopened = open_client_with_registry(
        database.clone(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
        rollback_registry.clone(),
    )
    .await;
    assert_eq!(
        reopened
            .select_setup_payment_federation_for_test(options().request_timeout())
            .await
            .expect("rollback cannot replace last-known-good policy"),
        payment_federation_id()
    );

    rollback_registry.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        reopened
            .select_setup_payment_federation_for_test(options().request_timeout())
            .await
            .expect("publisher or relay outage retains non-expiring policy"),
        payment_federation_id()
    );
}

#[tokio::test]
async fn malformed_newer_common_set_preserves_durable_last_known_good() {
    let database = MemDatabase::new().into_database();
    let now = test_now_secs();
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(now, &[PAYMENT_INVITE]));
    let (payments, _) = TestPayments::new();
    let client = open_client_with_registry(
        database.clone(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
        registry,
    )
    .await;
    client
        .select_setup_payment_federation_for_test(options().request_timeout())
        .await
        .expect("initial policy selects");
    drop(client);

    let malformed_registry = TestRegistry::default();
    malformed_registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_raw_event(now + 1, "not-json"));
    let (payments, _) = TestPayments::new();
    let reopened = open_client_with_registry(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
        malformed_registry,
    )
    .await;

    assert_eq!(
        reopened
            .select_setup_payment_federation_for_test(options().request_timeout())
            .await
            .expect("malformed update cannot replace last-known-good"),
        payment_federation_id()
    );
}

#[tokio::test]
async fn newer_empty_common_set_replaces_older_nonempty_set() {
    let database = MemDatabase::new().into_database();
    let now = test_now_secs();
    let registry = TestRegistry::default();
    *registry.candidates.lock().expect("test lock") = vec![
        setup_payment_event(now + 1, &[]),
        setup_payment_event(now, &[PAYMENT_INVITE]),
    ];
    let (payments, _) = TestPayments::new();
    let client = open_client_with_registry(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
        registry,
    )
    .await;

    assert!(matches!(
        client
            .select_setup_payment_federation_for_test(options().request_timeout())
            .await,
        Err(FiError::Payment(_))
    ));
}

#[tokio::test]
async fn daemon_rejection_of_selected_common_set_member_is_quote_failure() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let state = Arc::new(FmanState::default());
    let client = open_client(
        database,
        payments,
        state.clone(),
        FmanConfig {
            reject_quote: true,
            ..FmanConfig::paid()
        },
    )
    .await;

    let error = client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .expect_err("daemon-local policy rejection is actionable quote failure");
    assert!(matches!(error, FiError::FleetManager { .. }));
    assert_eq!(
        state.quote_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn authenticated_common_set_selection_is_sent_in_paid_quotes() {
    let (payments, _) = TestPayments::new();
    let state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        state.clone(),
        FmanConfig::paid(),
    )
    .await;

    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .expect("authenticated common-set member funds paid quote");
    let records = state.quote_records.lock().expect("test lock");
    assert_eq!(records.len(), usize::from(MIN_FEDERATION_SIZE));
    assert!(
        records.iter().all(|record| {
            record.payment_federation_id.as_ref() == Some(&payment_federation_id())
        })
    );
}

#[tokio::test]
async fn intent_and_locator_validation_precede_external_calls() {
    for size in [MIN_FEDERATION_SIZE - 1, MAX_FEDERATION_SIZE_EXCLUSIVE] {
        assert!(matches!(
            FormationIntent::new(
                None,
                FederationSize(size),
                PlanPreference::InfiniteBestEffort,
                fedimintd_version_range(),
            ),
            Err(FiError::InvalidIntent(_))
        ));
    }
    assert!(
        FormationIntent::new(
            None,
            FederationSize(MAX_FEDERATION_SIZE_EXCLUSIVE - 1),
            PlanPreference::InfiniteBestEffort,
            fedimintd_version_range(),
        )
        .is_ok()
    );

    for name in [
        String::new(),
        "   ".to_owned(),
        "has\ncontrol".to_owned(),
        "x".repeat(129),
    ] {
        assert!(matches!(
            FormationIntent::new(
                Some(FederationName(name)),
                FederationSize(MIN_FEDERATION_SIZE),
                PlanPreference::InfiniteBestEffort,
                fedimintd_version_range(),
            ),
            Err(FiError::InvalidIntent(_))
        ));
    }

    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments,
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let acquisition_started = Arc::new(Notify::new());
    let acquisition_proceed = Arc::new(Notify::new());
    client
        .inner
        .store
        .set_lease_acquisition_hook(acquisition_started.clone(), acquisition_proceed.clone());
    acquisition_proceed.notify_one();

    let too_few = locators().into_iter().take(6).collect();
    assert!(matches!(
        client
            .create_with_pinned_fmans(intent(), too_few, options())
            .await,
        Err(FiError::InvalidFleetManagers(_))
    ));
    assert_eq!(client.status(), FiStatus::Idle);
    assert!(matches!(
        client
            .inner
            .store
            .load_recovery(TestIdentity.public_key().unwrap())
            .await
            .unwrap(),
        FiRecovery::Idle
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(10), acquisition_started.notified())
            .await
            .is_err(),
        "pure validation must not attempt lease acquisition"
    );
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);

    let duplicate = vec![locator(0); usize::from(MIN_FEDERATION_SIZE)];
    assert!(matches!(
        client
            .create_with_pinned_fmans(intent(), duplicate, options())
            .await,
        Err(FiError::InvalidFleetManagers(_))
    ));
    assert_eq!(fman_state.availability_calls.load(Ordering::SeqCst), 0);
}

/// The deployment bootstrap: the first federation forms before any ecash to
/// pay for it exists, so an FI with no setup-payment publisher forms against
/// FMans offering their seats at zero, and never asks its wallet for anything.
#[tokio::test]
async fn an_fi_that_cannot_pay_forms_against_seats_offered_at_zero() {
    let (payments, payment_state) = TestPayments::new();
    let client = open_client_that_cannot_pay(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::given_away(),
    )
    .await;

    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    let status = client.status();
    let formed = formation(&status);
    assert_eq!(formed.phase, FormationPhase::Formed);
    assert_eq!(formed.invite_code, Some(test_invite(0)));
    assert!(
        formed
            .seats
            .iter()
            .all(|seat| seat.fman_id.is_none() && seat.fman_name().is_none()),
        "pinned rows carry no badge-vouched identity to derive a name from",
    );
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

/// The wallet states the protocol it settled under, and the FI compares it
/// against the quote's signed terms: funds locked under a generation the quote
/// did not name never reach a `CreateSeat`.
#[tokio::test]
async fn a_payment_settled_under_the_wrong_generation_is_never_presented() {
    let (payments, payment_state) = TestPayments::new();
    payment_state
        .settle_wrong_generation
        .store(true, Ordering::SeqCst);
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;

    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let authorization_id = payment_requirements(&client.status())
        .authorization_id
        .clone();
    let error = client
        .authorize_payments(authorization_id, options())
        .await
        .expect_err("a payment that disagrees with the quote is not presented");
    assert!(
        matches!(&error, FiError::FleetManager { message, .. }
            if message.contains("payment generation did not match")),
        "unexpected error: {error:?}"
    );
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);
}

/// The other half of the same rule: an FI that cannot pay is refused by an
/// FMan that charges, rather than forming a seat it has no way to settle.
#[tokio::test]
async fn an_fi_that_cannot_pay_is_refused_by_a_priced_fman() {
    let (payments, payment_state) = TestPayments::new();
    let client = open_client_that_cannot_pay(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;

    let error = client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .expect_err("a priced seat cannot be taken without a way to pay");
    assert!(
        matches!(&error, FiError::FleetManager { message, .. }
            if message.contains("no authenticated payment policy")),
        "unexpected error: {error:?}"
    );
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn default_name_final_status_and_formed_reconciliation_are_typed() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    let unnamed = FormationIntent::new(
        None,
        FederationSize(MIN_FEDERATION_SIZE),
        PlanPreference::InfiniteBestEffort,
        fedimintd_version_range(),
    )
    .unwrap();
    client
        .create_with_pinned_fmans(unnamed, locators(), options())
        .await
        .unwrap();

    let formed_status = client.status();
    let formed = formation(&formed_status);
    assert_eq!(formed.phase, FormationPhase::Formed);
    assert_eq!(formed.freshness, FormationFreshness::Fresh);
    assert!(formed.action_required.is_none());
    assert_eq!(formed.invite_code, Some(test_invite(0)));
    assert!(
        formed
            .seats
            .iter()
            .all(|seat| seat.phase == SeatPhase::Running)
    );
    let generated_name = formed.intent.federation_name.clone();
    assert_eq!(generated_name.0.split_whitespace().count(), 2);

    let json = serde_json::to_value(&formed_status).unwrap();
    assert_eq!(json["formation"]["phase"], "formed");
    assert_eq!(
        json["formation"]["intent"]["federation_name"],
        generated_name.0
    );
    assert_eq!(json["formation"]["seats"][0]["phase"], "running");
    assert_eq!(
        serde_json::from_value::<FiStatus>(json).unwrap(),
        formed_status
    );

    let quote_calls = fman_state.quote_calls.load(Ordering::SeqCst);
    let create_calls = fman_state.create_calls.load(Ordering::SeqCst);
    let status_calls = fman_state.status_calls.load(Ordering::SeqCst);
    let invite_calls = fman_state.invite_calls.load(Ordering::SeqCst);
    let reopened = open_client(
        database,
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    let reloaded_status = reopened.status();
    let reloaded = formation(&reloaded_status);
    assert_eq!(reloaded.phase, FormationPhase::Formed);
    assert_eq!(reloaded.freshness, FormationFreshness::Unsynced);
    assert_eq!(reloaded.intent.federation_name, generated_name);
    assert!(
        reloaded
            .seats
            .iter()
            .all(|seat| seat.freshness == FormationFreshness::Unsynced)
    );

    reopened.resume().await.unwrap();
    let reconciled_status = reopened.status();
    let reconciled = formation(&reconciled_status);
    assert_eq!(reconciled.phase, FormationPhase::Formed);
    assert_eq!(reconciled.freshness, FormationFreshness::Fresh);
    assert_eq!(reconciled.intent.federation_name, generated_name);
    assert_eq!(fman_state.quote_calls.load(Ordering::SeqCst), quote_calls);
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), create_calls);
    assert_eq!(
        fman_state.status_calls.load(Ordering::SeqCst),
        status_calls + usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(
        fman_state.invite_calls.load(Ordering::SeqCst),
        invite_calls + usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn an_already_started_reply_resumes_through_status_instead_of_stranding_formation() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    fman_state
        .report_dkg_already_started
        .store(true, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;

    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    assert_eq!(
        fman_state.start_callbacks.lock().expect("test lock").len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn inconsistent_formed_storage_is_rejected_before_status_publication() {
    use crate::db::FormedFact;

    for missing_fact in [
        FormedFact::SeatCount,
        FormedFact::InviteCode,
        FormedFact::SignedQuote,
        FormedFact::SeatId,
        FormedFact::GuardianFeeAccount,
        FormedFact::GuardianCode,
    ] {
        let database = MemDatabase::new().into_database();
        let (payments, _) = TestPayments::new();
        let fman_state = Arc::new(FmanState::default());
        let client = open_client(
            database.clone(),
            payments.clone(),
            fman_state.clone(),
            FmanConfig::given_away(),
        )
        .await;
        client
            .create_with_pinned_fmans(intent(), locators(), options())
            .await
            .unwrap();

        client
            .inner
            .store
            .remove_formed_fact_for_test(missing_fact)
            .await;

        let reopened = FiClient::open(
            database,
            TestIdentity,
            payments,
            TestRegistry::default(),
            TestConnector {
                state: fman_state.clone(),
                config: FmanConfig::given_away(),
            },
            test_peer_badge_verifier(),
            TestConsensusReader::new(fman_state),
            TestFiFeeAccountProvider::default(),
        )
        .await;
        assert!(
            matches!(reopened, Err(FiError::Storage(_))),
            "corrupted terminal fact {missing_fact:?} was accepted"
        );
    }
}

#[tokio::test]
async fn record_formed_rejects_incomplete_seats_without_changing_recovery_state() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state, FmanConfig::given_away()).await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let formation_id = formation(&client.status()).formation_id.clone();
    client
        .inner
        .store
        .rewind_formed_without_guardian_for_test()
        .await;

    assert!(matches!(
        client
            .inner
            .store
            .record_formed(&formation_id, test_invite(0))
            .await,
        Err(FiError::Storage(_))
    ));
    let status = client
        .inner
        .store
        .load_status(TestIdentity::fi_id())
        .await
        .unwrap();
    assert_eq!(formation(&status).phase, FormationPhase::PreparingDkg);
    assert!(formation(&status).invite_code.is_none());
}

#[tokio::test]
async fn formation_rejects_disagreeing_final_federation_identities() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    fman_state.disagreeing_invite.store(true, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;

    assert!(matches!(
        client
            .create_with_pinned_fmans(intent(), locators(), options())
            .await,
        Err(FiError::InvalidFleetManagers(message))
            if message.contains("different federation identities")
    ));
    assert_eq!(
        formation(&client.status()).phase,
        FormationPhase::PreparingDkg
    );
    assert!(formation(&client.status()).invite_code.is_none());
}

#[tokio::test]
async fn concurrent_driver_is_rejected() {
    let (payments, _) = TestPayments::new();
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    let _guard = client.inner.run_guard.lock().await;
    assert!(matches!(client.resume().await, Err(FiError::Busy)));
}

#[tokio::test]
async fn separately_opened_clients_share_one_database_driver_lease() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let first = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    first
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let second = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let authorization_id = payment_requirements(&first.status())
        .authorization_id
        .clone();
    payment_state.block_recovery.store(true, Ordering::SeqCst);
    let recovery_started = payment_state.recovery_started.notified();
    let operation = tokio::spawn(async move {
        first
            .authorize_payments(authorization_id, long_request_options())
            .await
    });
    recovery_started.await;

    assert!(matches!(second.resume().await, Err(FiError::Busy)));
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
}

fn test_driver_run<'a>(
    lease: &'a db::DriverLease,
    deadline: fedimint_core::runtime::Instant,
    request_timeout: Duration,
) -> crate::formation::DriverRun<'a> {
    crate::formation::DriverRun::new(
        FormationRunOptions::new(crate::FormationRunOptionsConfig {
            poll_interval: Duration::from_millis(1),
            run_timeout: Duration::from_secs(2),
            request_timeout,
        })
        .unwrap(),
        deadline,
        lease,
    )
}

#[tokio::test]
async fn stalled_driver_is_fenced_after_lease_takeover() {
    let database = MemDatabase::new().into_database();
    let now = Arc::new(AtomicU64::new(1_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
    };
    let store = db::FiStore::new_with_lease_clock(database, clock);
    let old = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
        .await
        .unwrap();

    // Model a suspended future: its monotonic deadline can be recreated by
    // buggy caller code, but its database lease expires while it is not running.
    now.store(1_006, Ordering::SeqCst);
    let _replacement = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
        .await
        .unwrap();
    let factories = AtomicUsize::new(0);
    let effects = AtomicUsize::new(0);
    let result = test_driver_run(
        &old,
        fedimint_core::runtime::Instant::now() + Duration::from_secs(30),
        Duration::from_secs(5),
    )
    .call("test external effect", || {
        factories.fetch_add(1, Ordering::SeqCst);
        Ok(async {
            effects.fetch_add(1, Ordering::SeqCst);
        })
    })
    .await;

    assert!(matches!(result, Err(FiError::Busy)));
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn takeover_after_factory_is_fenced_before_future_polling() {
    let database = MemDatabase::new().into_database();
    let now = Arc::new(AtomicU64::new(7_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
    };
    let store = db::FiStore::new_with_lease_clock(database, clock);
    let old = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
        .await
        .unwrap();
    let factory_started = Arc::new(Notify::new());
    let (takeover_done_tx, takeover_done_rx) = std::sync::mpsc::channel();
    let takeover = {
        let factory_started = factory_started.clone();
        let now = now.clone();
        let store = store.clone();
        tokio::spawn(async move {
            factory_started.notified().await;
            now.store(7_006, Ordering::SeqCst);
            let replacement = store
                .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
                .await
                .unwrap();
            takeover_done_tx.send(()).unwrap();
            replacement
        })
    };
    let factories = AtomicUsize::new(0);
    let effects = AtomicUsize::new(0);
    let run = test_driver_run(
        &old,
        fedimint_core::runtime::Instant::now() + Duration::from_secs(30),
        Duration::from_secs(5),
    );

    let result = run
        .call("test construction-to-poll fence", || {
            factories.fetch_add(1, Ordering::SeqCst);
            factory_started.notify_one();
            takeover_done_rx.recv().unwrap();
            Ok(async {
                effects.fetch_add(1, Ordering::SeqCst);
            })
        })
        .await;

    assert!(matches!(result, Err(FiError::Busy)));
    assert_eq!(factories.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    takeover.await.unwrap().renew().await.unwrap();
}

#[tokio::test]
async fn forward_clock_jump_fences_old_driver_before_external_effect() {
    let database = MemDatabase::new().into_database();
    let now = Arc::new(AtomicU64::new(2_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
    };
    let store = db::FiStore::new_with_lease_clock(database, clock);
    let old = store
        .acquire_driver_lease(Duration::from_secs(600), Duration::from_secs(90))
        .await
        .unwrap();

    // Cross the current 2,090 expiry without crossing the 2,600 maximum.
    now.store(2_200, Ordering::SeqCst);
    let factories = AtomicUsize::new(0);
    let effects = AtomicUsize::new(0);
    let result = test_driver_run(
        &old,
        fedimint_core::runtime::Instant::now() + Duration::from_secs(600),
        Duration::from_secs(30),
    )
    .call("test value-moving effect", || {
        factories.fetch_add(1, Ordering::SeqCst);
        Ok(async {
            effects.fetch_add(1, Ordering::SeqCst);
        })
    })
    .await;

    assert!(matches!(result, Err(FiError::Busy)));
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);

    let replacement = store
        .acquire_driver_lease(Duration::from_secs(600), Duration::from_secs(90))
        .await
        .unwrap();
    store.release_driver_lease(old).await.unwrap();
    replacement.renew().await.unwrap();
}

#[tokio::test]
async fn takeover_during_lease_renewal_returns_busy_without_polling_effect() {
    let database = MemDatabase::new().into_database();
    let now = Arc::new(AtomicU64::new(4_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
    };
    let store = db::FiStore::new_with_lease_clock(database, clock);
    let old = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(4))
        .await
        .unwrap();
    now.store(4_003, Ordering::SeqCst);
    let renewal_read = Arc::new(Notify::new());
    let allow_commit = Arc::new(Notify::new());
    store.set_lease_renewal_commit_hook(renewal_read.clone(), allow_commit.clone());
    let factories = Arc::new(AtomicUsize::new(0));
    let effects = Arc::new(AtomicUsize::new(0));
    let operation = {
        let factories = factories.clone();
        let effects = effects.clone();
        tokio::spawn(async move {
            test_driver_run(
                &old,
                fedimint_core::runtime::Instant::now() + Duration::from_secs(30),
                Duration::from_secs(5),
            )
            .call("test conflicting takeover", || {
                factories.fetch_add(1, Ordering::SeqCst);
                Ok(async move {
                    effects.fetch_add(1, Ordering::SeqCst);
                })
            })
            .await
        })
    };
    renewal_read.notified().await;

    now.store(4_005, Ordering::SeqCst);
    let replacement = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(4))
        .await
        .unwrap();
    allow_commit.notify_one();

    assert!(matches!(operation.await.unwrap(), Err(FiError::Busy)));
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    tokio::task::yield_now().await;
    replacement.renew().await.unwrap();
}

#[tokio::test]
async fn takeover_during_lease_release_returns_busy_without_removing_replacement() {
    let database = MemDatabase::new().into_database();
    let now = Arc::new(AtomicU64::new(5_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
    };
    let store = db::FiStore::new_with_lease_clock(database, clock);
    let old = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(4))
        .await
        .unwrap();
    let release_read = Arc::new(Notify::new());
    let allow_release = Arc::new(Notify::new());
    store.set_lease_release_commit_hook(release_read.clone(), allow_release.clone());
    let release = {
        let store = store.clone();
        tokio::spawn(async move { store.release_driver_lease(old).await })
    };
    release_read.notified().await;

    now.store(5_005, Ordering::SeqCst);
    let replacement = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(4))
        .await
        .unwrap();
    allow_release.notify_one();

    assert!(matches!(release.await.unwrap(), Err(FiError::Busy)));
    replacement.renew().await.unwrap();
}

#[test]
fn lease_commit_errors_are_mapped_by_retryability() {
    assert!(matches!(
        db::map_lease_commit_error(fedimint_core::db::DatabaseError::SnapshotTooOld(Box::new(
            std::io::Error::other("test snapshot")
        ))),
        FiError::Busy
    ));
    assert!(matches!(
        db::map_lease_commit_error(fedimint_core::db::DatabaseError::TransactionConsumed),
        FiError::Storage(message) if message == "FI driver lease database commit failed"
    ));
}

#[test]
fn formation_autocommit_errors_are_mapped_by_failure_kind() {
    assert!(matches!(
        db::map_formation_tx_error(fedimint_core::db::AutocommitError::CommitFailed {
            attempts: 10,
            last_error: fedimint_core::db::DatabaseError::TransactionConsumed,
        }),
        FiError::Storage(message) if message == "updating FI formation transaction failed"
    ));
    assert!(matches!(
        db::map_formation_tx_error(fedimint_core::db::AutocommitError::CommitFailed {
            attempts: 10,
            last_error: fedimint_core::db::DatabaseError::WriteConflict,
        }),
        FiError::Storage(message) if message == "updating FI formation transaction failed"
    ));
    assert!(matches!(
        db::map_formation_tx_error(fedimint_core::db::AutocommitError::ClosureError {
            attempts: 1,
            error: FiError::Busy,
        }),
        FiError::Busy
    ));
}

#[tokio::test]
async fn setup_policy_takeover_between_registry_and_wallet_fences_wallet_call() {
    let database = MemDatabase::new().into_database();
    let now = Arc::new(AtomicU64::new(6_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
    };
    let store = db::FiStore::new_with_lease_clock(database, clock);
    let (payments, payment_state) = TestPayments::new();
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    registry.block_fetch.store(true, Ordering::SeqCst);
    let fetch_started = registry.fetch_started.clone();
    let fetch_continue = registry.fetch_continue.clone();
    let client = open_client_with_store_and_registry(
        store.clone(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
        registry,
    )
    .await;
    let old = store
        .acquire_driver_lease(Duration::from_secs(600), Duration::from_secs(90))
        .await
        .unwrap();
    let operation = tokio::spawn(async move {
        let run = crate::formation::DriverRun::new(
            options(),
            fedimint_core::runtime::Instant::now() + Duration::from_secs(600),
            &old,
        );
        client.select_setup_payment_federation(run).await
    });
    fetch_started.notified().await;

    now.store(6_100, Ordering::SeqCst);
    let replacement = store
        .acquire_driver_lease(Duration::from_secs(600), Duration::from_secs(90))
        .await
        .unwrap();
    let newer_event = setup_payment_event(test_now_secs() + 1, &[]);
    store
        .store_setup_payment_federations_event(newer_event.clone())
        .await
        .unwrap();
    fetch_continue.notify_one();

    assert!(matches!(operation.await.unwrap(), Err(FiError::Busy)));
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        store
            .load_setup_payment_federations_event()
            .await
            .expect("durable setup-payment high-water")
            .id,
        newer_event.id
    );
    replacement.renew().await.unwrap();
}

#[tokio::test]
async fn taken_over_client_cannot_continue_from_a_stalled_wallet_call() {
    let database = MemDatabase::new().into_database();
    let now = Arc::new(AtomicU64::new(10_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
    };
    let store = db::FiStore::new_with_lease_clock(database, clock);
    let (payments, payment_state) = TestPayments::new();
    let client = open_client_with_store(
        store.clone(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let authorization_id = payment_requirements(&client.status())
        .authorization_id
        .clone();
    payment_state.block_recovery.store(true, Ordering::SeqCst);
    let recovery_started = payment_state.recovery_started.notified();
    let operation =
        tokio::spawn(async move { client.authorize_payments(authorization_id, options()).await });
    recovery_started.await;

    now.store(10_100, Ordering::SeqCst);
    let replacement = store
        .acquire_driver_lease(
            options().lease_duration(),
            options().lease_renewal_duration(),
        )
        .await
        .unwrap();
    payment_state.block_recovery.store(false, Ordering::SeqCst);
    payment_state.recovery_continue.notify_waiters();

    assert!(matches!(operation.await.unwrap(), Err(FiError::Busy)));
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    tokio::task::yield_now().await;
    replacement.renew().await.unwrap();
}

#[tokio::test]
async fn lease_acquisition_delay_consumes_the_existing_run_deadline() {
    let options = FormationRunOptions::new(crate::FormationRunOptionsConfig {
        poll_interval: Duration::from_millis(1),
        run_timeout: Duration::from_millis(1),
        request_timeout: Duration::from_secs(1),
    })
    .unwrap();
    let store = db::FiStore::new(MemDatabase::new().into_database());
    let acquisition_started = Arc::new(Notify::new());
    let allow_acquisition = Arc::new(Notify::new());
    store.set_lease_acquisition_hook(acquisition_started.clone(), allow_acquisition.clone());
    let operation = {
        let store = store.clone();
        tokio::spawn(async move { crate::formation::start_driver_run(&store, options).await })
    };
    acquisition_started.notified().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    allow_acquisition.notify_one();
    let (deadline, lease) = operation.await.unwrap().unwrap();
    let factories = AtomicUsize::new(0);

    let result = test_driver_run(&lease, deadline, options.request_timeout())
        .call("test delayed acquisition", || {
            factories.fetch_add(1, Ordering::SeqCst);
            Ok(async {})
        })
        .await;

    assert!(matches!(result, Err(FiError::Timeout(_))));
    assert_eq!(factories.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn initialized_formation_reloads_as_unsynced_typed_status() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            FormationId("formation1".to_owned()),
            resolved_intent_with_size(FederationSize(1)),
            vec![seat_progress(0)],
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let status = reopened.status();
    let snapshot = formation(&status);
    assert_eq!(snapshot.formation_id, FormationId("formation1".to_owned()));
    assert_eq!(snapshot.phase, FormationPhase::Preparing);
    assert_eq!(snapshot.freshness, FormationFreshness::Unsynced);
    assert_eq!(snapshot.seats[0].freshness, FormationFreshness::Unsynced);
    assert_eq!(snapshot.seats[0].phase, SeatPhase::Selected);
}

#[tokio::test]
async fn persisted_formation_rejects_a_different_identity_before_observation() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            FormationId("ownedformation".to_owned()),
            resolved_intent_with_size(FederationSize(1)),
            vec![seat_progress(0)],
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();

    let reopened = FiClient::open(
        database,
        OtherIdentity,
        payments,
        TestRegistry::default(),
        TestConnector {
            state: fman_state.clone(),
            config: FmanConfig::paid(),
        },
        test_peer_badge_verifier(),
        TestConsensusReader::new(fman_state),
        TestFiFeeAccountProvider::default(),
    )
    .await;
    assert!(
        matches!(reopened, Err(FiError::Storage(message)) if message.contains("different identity"))
    );
}

#[tokio::test]
async fn pre_tombstone_schema_record_is_rejected_fail_closed() {
    // This pre-boundary record predates the distinct output-generation tombstone, so
    // its absence cannot prove that no value-moving wallet call began. The
    // record is rejected fail-closed with reset guidance.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            FormationId("prebranchformation".to_owned()),
            resolved_intent_with_size(FederationSize(1)),
            vec![seat_progress(0)],
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();
    client.inner.store.downgrade_schema_for_test(7).await;

    let reopened = FiClient::open(
        database,
        TestIdentity,
        payments,
        TestRegistry::default(),
        TestConnector {
            state: fman_state.clone(),
            config: FmanConfig::given_away(),
        },
        test_peer_badge_verifier(),
        TestConsensusReader::new(fman_state),
        TestFiFeeAccountProvider::default(),
    )
    .await;
    assert!(
        matches!(
            reopened,
            Err(FiError::Storage(message))
                if message.contains("unsupported FI storage schema version 7")
                    && message.contains("reset this unreleased FI namespace")
        ),
        "schema 7 must be rejected with reset guidance",
    );
}

#[tokio::test]
async fn current_storage_requires_selected_mode_and_output_tombstone_fields() {
    let (payments, _) = TestPayments::new();
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            FormationId("required-schema-fields".to_owned()),
            resolved_intent_with_size(FederationSize(1)),
            vec![seat_progress(0)],
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();

    for field in [
        "creation_mode",
        "payment_authorization_recorded",
        "payment_reservation_id",
        "payment_outputs_started",
    ] {
        assert!(
            client
                .inner
                .store
                .rejects_missing_recovery_field_for_test(field)
                .await,
            "schema 11 must reject a missing {field}"
        );
    }
}

#[tokio::test]
async fn accepting_one_seat_preserves_a_sibling_journaled_quote() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    let formation_id = FormationId("twoseatinterleaving".to_owned());
    let intent = resolved_intent_with_size(FederationSize(2));
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            formation_id.clone(),
            intent.clone(),
            vec![seat_progress(0), seat_progress(1)],
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();
    let first_quote = signed_quote(0, &intent, [21; 32]);
    let second_quote = signed_quote(1, &intent, [22; 32]);
    client
        .inner
        .store
        .store_quote(&formation_id, 0, first_quote.clone())
        .await
        .unwrap();
    client
        .inner
        .store
        .store_quote(&formation_id, 1, second_quote.clone())
        .await
        .unwrap();
    client
        .inner
        .store
        .record_seat_accepted(
            &formation_id,
            0,
            SeatId::from(QuoteId([0x01; 32])),
            guardian_fee_account(32),
        )
        .await
        .unwrap();

    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert_eq!(recovery.seats[0].signed_quote, Some(first_quote));
    assert_eq!(recovery.seats[1].signed_quote, Some(second_quote));
    assert_eq!(
        recovery.snapshot.seats[0].seat_id,
        Some(SeatId::from(QuoteId([0x01; 32])))
    );
    assert!(recovery.snapshot.seats[1].seat_id.is_none());
}

#[tokio::test]
async fn concurrent_two_seat_acceptance_retries_and_derives_completion_from_durable_siblings() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    let store = &client.inner.store;
    let formation_id = FormationId("concurrentseatacceptance".to_owned());
    let intent = resolved_intent_with_size(FederationSize(2));
    store
        .initialize(
            TestIdentity::fi_id(),
            formation_id.clone(),
            intent.clone(),
            vec![seat_progress(0), seat_progress(1)],
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();
    for index in 0..2 {
        store
            .store_quote(
                &formation_id,
                index,
                signed_quote(
                    usize::from(index),
                    &intent,
                    [40 + u8::try_from(index).unwrap(); 32],
                ),
            )
            .await
            .unwrap();
    }

    store.set_next_formation_commit_barrier(2);
    let (first, second) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(
            store.record_seat_accepted(
                &formation_id,
                0,
                SeatId::from(QuoteId([0x02; 32])),
                guardian_fee_account(32),
            ),
            store.record_seat_accepted(
                &formation_id,
                1,
                SeatId::from(QuoteId([0x03; 32])),
                guardian_fee_account(33),
            ),
        )
    })
    .await
    .expect("forced seat-acceptance conflict completed");
    first.unwrap();
    second.unwrap();

    let reloaded = store.load_status(TestIdentity::fi_id()).await.unwrap();
    let formation = formation(&reloaded);
    assert_eq!(formation.phase, FormationPhase::PreparingDkg);
    assert_eq!(
        formation.seats[0].seat_id,
        Some(SeatId::from(QuoteId([0x02; 32])))
    );
    assert_eq!(
        formation.seats[1].seat_id,
        Some(SeatId::from(QuoteId([0x03; 32])))
    );
}

#[tokio::test]
async fn concurrent_two_seat_clear_and_changed_term_store_retry_without_losing_sibling_facts() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    let store = &client.inner.store;
    let formation_id = FormationId("concurrentquotereplacement".to_owned());
    let intent = resolved_intent_with_size(FederationSize(2));
    store
        .initialize(
            TestIdentity::fi_id(),
            formation_id.clone(),
            intent.clone(),
            vec![seat_progress(0), seat_progress(1)],
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();
    let old_quotes = [
        signed_quote(0, &intent, [31; 32]),
        signed_quote(1, &intent, [32; 32]),
    ];
    store
        .store_quote(&formation_id, 0, old_quotes[0].clone())
        .await
        .unwrap();
    store
        .store_quote(&formation_id, 1, old_quotes[1].clone())
        .await
        .unwrap();
    let status = store.load_status(TestIdentity::fi_id()).await.unwrap();
    let requirements = payment_requirements(&status).clone();
    let authorizations = requirements
        .seats
        .iter()
        .map(|requirement| QuoteAuthorization {
            index: requirement.index,
            quote_id: requirement.quote_id,
        })
        .collect::<Vec<_>>();
    store
        .authorize_payments(
            &formation_id,
            &requirements.authorization_id,
            &authorizations,
        )
        .await
        .unwrap();

    store.set_next_formation_commit_barrier(2);
    let (first_clear, second_clear) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(
            store.clear_quote(&formation_id, 0, &old_quotes[0]),
            store.clear_quote(&formation_id, 1, &old_quotes[1]),
        )
    })
    .await
    .expect("forced quote-clear conflict completed");
    first_clear.unwrap();
    second_clear.unwrap();
    let cleared = active_recovery(store.load_recovery(TestIdentity::fi_id()).await.unwrap());
    assert!(cleared.seats.iter().all(|seat| seat.signed_quote.is_none()));
    assert!(
        cleared
            .seats
            .iter()
            .all(|seat| seat.replacement_for.is_none()),
        "a generic quote refresh must not manufacture replacement authority",
    );
    assert!(authorizations.iter().all(|authorization| {
        !cleared.quote_is_authorized(authorization.index, authorization.quote_id)
    }));

    let fresh_quotes = [
        signed_quote_at_price(0, &intent, [33; 32], 200),
        signed_quote_at_price(1, &intent, [34; 32], 300),
    ];
    store.set_next_formation_commit_barrier(2);
    let (first_store, second_store) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(
            store.store_quote(&formation_id, 0, fresh_quotes[0].clone()),
            store.store_quote(&formation_id, 1, fresh_quotes[1].clone()),
        )
    })
    .await
    .expect("forced quote-store conflict completed");
    first_store.unwrap();
    second_store.unwrap();
    let reloaded = active_recovery(store.load_recovery(TestIdentity::fi_id()).await.unwrap());
    assert_eq!(
        reloaded.seats[0].signed_quote,
        Some(fresh_quotes[0].clone())
    );
    assert_eq!(
        reloaded.seats[1].signed_quote,
        Some(fresh_quotes[1].clone())
    );
    let reloaded_status = FiStatus::Formation(reloaded.snapshot.clone());
    let refreshed = payment_requirements(&reloaded_status);
    assert_eq!(refreshed.seats.len(), 2);
    assert_eq!(refreshed.total_msats, 500);
    assert!(
        refreshed
            .seats
            .iter()
            .all(|requirement| !authorizations.contains(&QuoteAuthorization {
                index: requirement.index,
                quote_id: requirement.quote_id,
            }))
    );
}

#[tokio::test]
async fn replacement_authority_requires_selected_post_output_terminal_transition() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    let store = &client.inner.store;
    let formation_id = FormationId("terminal-replacement-gate".to_owned());
    let resolved = intent()
        .with_max_total_msats(1_000)
        .unwrap()
        .resolve_for_dkg(
            FederationName("Replacement Gate".to_owned()),
            fedimintd_version().dkg_version(),
        )
        .unwrap();
    store
        .initialize(
            TestIdentity::fi_id(),
            formation_id.clone(),
            resolved.clone(),
            (0..MIN_FEDERATION_SIZE)
                .map(|index| selected_initial_seat(index, test_now_secs() + 120))
                .collect(),
            crate::db::FormationCreationMode::Selected {
                payment_federation_id: Some(payment_federation_id()),
            },
            None,
        )
        .await
        .unwrap();
    let quote = signed_quote(0, &resolved, [71; 32]);
    for index in 0..MIN_FEDERATION_SIZE {
        store
            .store_quote(
                &formation_id,
                index,
                if index == 0 {
                    quote.clone()
                } else {
                    signed_quote(
                        usize::from(index),
                        &resolved,
                        [71 + u8::try_from(index).unwrap(); 32],
                    )
                },
            )
            .await
            .unwrap();
    }

    let error = store
        .mark_replacement_required(&formation_id, 0, &quote)
        .await
        .unwrap_err();
    assert!(matches!(error, FiError::Storage(message) if message.contains("post-output")));
    assert_eq!(
        active_recovery(store.load_recovery(TestIdentity::fi_id()).await.unwrap()).seats[0]
            .signed_quote,
        Some(quote.clone()),
        "a rejected transition must preserve the exact quote",
    );

    let requirements = active_recovery(store.load_recovery(TestIdentity::fi_id()).await.unwrap())
        .payment_requirements(TestIdentity::fi_id())
        .unwrap()
        .expect("complete exact paid aggregate");
    store
        .authorize_payments(
            &formation_id,
            &requirements.authorization_id,
            &requirements
                .seats
                .iter()
                .map(|requirement| QuoteAuthorization {
                    index: requirement.index,
                    quote_id: requirement.quote_id,
                })
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    let reservation_id = crate::db::payment_reservation_id(&formation_id, &requirements);
    store
        .record_payment_reservation(&formation_id, &reservation_id)
        .await
        .unwrap();
    let lease = store
        .acquire_driver_lease(
            options().lease_duration(),
            options().lease_renewal_duration(),
        )
        .await
        .unwrap();
    lease
        .arm_payment_outputs_started(
            &formation_id,
            test_peer_badge_verifier().provenance().into(),
        )
        .await
        .unwrap();
    store.release_driver_lease(lease).await.unwrap();
    store
        .mark_replacement_required(&formation_id, 0, &quote)
        .await
        .unwrap();

    let recovery = active_recovery(store.load_recovery(TestIdentity::fi_id()).await.unwrap());
    assert!(recovery.seats[0].signed_quote.is_none());
    assert_eq!(
        recovery.seats[0].replacement_for,
        Some(requirements.seats[0].quote_id)
    );
    assert!(
        store
            .store_quote(&formation_id, 0, quote)
            .await
            .unwrap_err()
            .to_string()
            .contains("requires replacement"),
        "replacement authority cannot be silently collapsed back into a quote refresh",
    );
}

#[tokio::test]
async fn recovery_rejects_mixed_quote_and_replacement_authority() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    let formation_id = FormationId("mixed-replacement-corruption".to_owned());
    let resolved = resolved_intent_with_size(FederationSize(1));
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            formation_id.clone(),
            resolved.clone(),
            vec![seat_progress(0)],
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();
    client
        .inner
        .store
        .store_quote(&formation_id, 0, signed_quote(0, &resolved, [72; 32]))
        .await
        .unwrap();
    client
        .inner
        .store
        .mix_quote_with_replacement_for_test(0)
        .await;

    let error = match client
        .inner
        .store
        .load_recovery(TestIdentity::fi_id())
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("mixed replacement authority was accepted"),
    };
    assert!(
        matches!(error, FiError::Storage(message) if message.contains("mixes a quote with replacement authority"))
    );
}

#[tokio::test]
async fn aggregate_authorization_blocks_spends_and_unpaid_quotes_refresh_on_reopen() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    let first_status = client.status();
    let requirements = payment_requirements(&first_status).clone();
    assert_eq!(
        formation(&first_status).phase,
        FormationPhase::AwaitingPaymentReadiness
    );
    assert_eq!(requirements.seats.len(), usize::from(MIN_FEDERATION_SIZE));
    assert_eq!(
        requirements.total_msats,
        PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE)
    );
    let quoted = quote_ids(&fman_state.quote_records.lock().expect("test lock"));
    let required = requirements
        .seats
        .iter()
        .map(|requirement| requirement.quote_id)
        .collect::<HashSet<_>>();
    assert_eq!(required, quoted);
    assert_eq!(payment_state.recover_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);

    let reopened = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    reopened.resume().await.unwrap();
    let reopened_status = reopened.status();
    let refreshed_requirements = payment_requirements(&reopened_status).clone();
    let refreshed = refreshed_requirements
        .seats
        .iter()
        .map(|requirement| requirement.quote_id)
        .collect::<HashSet<_>>();
    assert!(required.is_disjoint(&refreshed));
    assert_eq!(payment_state.recover_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);

    reopened
        .authorize_payments(refreshed_requirements.authorization_id.clone(), options())
        .await
        .unwrap();
    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
    assert_eq!(
        payment_state.recover_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(
        fman_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    let created = payment_state
        .created_quotes
        .lock()
        .expect("test lock")
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    assert!(created.is_disjoint(&required));
    assert_eq!(created, refreshed);
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        2 * usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn one_ecash_note_pays_every_seat_and_returns_change_sequentially() {
    const TRANSACTION_FEE_MSATS: u64 = 7;
    const RETURNED_CHANGE_MSATS: u64 = 53;

    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    payment_state
        .single_note_wallet
        .store(true, Ordering::SeqCst);
    payment_state
        .single_note_fee_msats
        .store(TRANSACTION_FEE_MSATS, Ordering::SeqCst);
    let payment_count = u64::from(MIN_FEDERATION_SIZE);
    let aggregate_payments = PAYMENT_AMOUNT_MSATS * payment_count;
    let aggregate_fees = TRANSACTION_FEE_MSATS * payment_count;
    let initial_note_msats = aggregate_payments + aggregate_fees + RETURNED_CHANGE_MSATS;
    payment_state
        .single_note_balance_msats
        .store(initial_note_msats, Ordering::SeqCst);

    let client = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let requirements = payment_requirements(&client.status()).clone();
    assert_eq!(requirements.total_msats, aggregate_payments);

    client
        .authorize_payments(requirements.authorization_id, options())
        .await
        .expect("one note is reused only after each accepted payment returns spendable change");

    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    assert_eq!(
        payment_state
            .single_note_balance_msats
            .load(Ordering::SeqCst),
        initial_note_msats - aggregate_payments - aggregate_fees,
        "final balance is the original note minus every setup payment and Fedimint fee",
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
    );
}

#[tokio::test]
async fn stale_payment_authorization_cannot_approve_a_replaced_quote_set() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let observed = payment_requirements(&client.status()).clone();
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    let old_quote = recovery.seats[0]
        .signed_quote
        .clone()
        .expect("journaled paid quote");
    client
        .inner
        .store
        .clear_quote(&recovery.snapshot.formation_id, 0, &old_quote)
        .await
        .unwrap();
    client
        .inner
        .store
        .store_quote(
            &recovery.snapshot.formation_id,
            0,
            signed_quote(0, &recovery.snapshot.intent, [99; 32]),
        )
        .await
        .unwrap();

    assert!(matches!(
        client
            .authorize_payments(observed.authorization_id, options())
            .await,
        Err(FiError::InvalidIntent(message)) if message.contains("changed")
    ));
    assert_eq!(payment_state.recover_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    let current = client
        .inner
        .store
        .load_status(TestIdentity::fi_id())
        .await
        .unwrap();
    assert_eq!(
        payment_requirements(&current).seats.len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn partial_seat_recovery_projects_payment_readiness_after_replacement() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    let formation_id = FormationId("partialpaymentreadiness".to_owned());
    let intent = resolved_intent_with_size(FederationSize(2));
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            formation_id.clone(),
            intent.clone(),
            vec![seat_progress(0), seat_progress(1)],
            crate::db::FormationCreationMode::Pinned,
            None,
        )
        .await
        .unwrap();
    let first_quote = signed_quote(0, &intent, [51; 32]);
    let second_quote = signed_quote(1, &intent, [52; 32]);
    client
        .inner
        .store
        .store_quote(&formation_id, 0, first_quote)
        .await
        .unwrap();
    client
        .inner
        .store
        .store_quote(&formation_id, 1, second_quote.clone())
        .await
        .unwrap();
    let status = client
        .inner
        .store
        .load_status(TestIdentity::fi_id())
        .await
        .unwrap();
    let requirements = payment_requirements(&status).clone();
    let authorizations = requirements
        .seats
        .iter()
        .map(|requirement| QuoteAuthorization {
            index: requirement.index,
            quote_id: requirement.quote_id,
        })
        .collect::<Vec<_>>();
    client
        .inner
        .store
        .authorize_payments(
            &formation_id,
            &requirements.authorization_id,
            &authorizations,
        )
        .await
        .unwrap();
    client
        .inner
        .store
        .record_seat_accepted(
            &formation_id,
            0,
            SeatId::from(QuoteId([0x04; 32])),
            guardian_fee_account(32),
        )
        .await
        .unwrap();
    client
        .inner
        .store
        .clear_quote(&formation_id, 1, &second_quote)
        .await
        .unwrap();
    client
        .inner
        .store
        .store_quote(&formation_id, 1, signed_quote(1, &intent, [53; 32]))
        .await
        .unwrap();

    let recovered = client
        .inner
        .store
        .load_status(TestIdentity::fi_id())
        .await
        .unwrap();
    assert_eq!(
        formation(&recovered).phase,
        FormationPhase::AwaitingPaymentReadiness
    );
    assert_eq!(payment_requirements(&recovered).seats.len(), 1);
}

#[tokio::test]
async fn authorized_reserved_recovery_replays_exact_quotes_before_funding() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let stored_quotes = fman_state
        .quote_records
        .lock()
        .expect("test lock")
        .iter()
        .map(|record| record.quote_id)
        .collect::<HashSet<_>>();

    payment_state.block_recovery.store(true, Ordering::SeqCst);
    let recovery_started = payment_state.recovery_started.notified();
    let authorization_id = payment_requirements(&client.status())
        .authorization_id
        .clone();
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .authorize_payments(authorization_id, long_request_options())
            .await
    });
    recovery_started.await;
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    payment_state.block_recovery.store(false, Ordering::SeqCst);

    let reopened = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    let recovered_status = reopened.status();
    assert_eq!(
        formation(&recovered_status).phase,
        FormationPhase::AcquiringSeats
    );
    assert!(formation(&recovered_status).action_required.is_none());
    reopened.resume().await.unwrap();
    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(
        payment_state
            .created_quotes
            .lock()
            .expect("test lock")
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(
        payment_state
            .created_quotes
            .lock()
            .expect("test lock")
            .iter()
            .copied()
            .collect::<HashSet<_>>(),
        stored_quotes,
        "a durable reservation pins the exact authorized quote set across reopen",
    );
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn a_closed_fman_stops_before_quote_or_value_moving_payment_work() {
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let mut config = FmanConfig::paid();
    config.accepting_seats = false;
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        config,
    )
    .await;

    assert!(matches!(
        client
            .create_with_pinned_fmans(intent(), locators(), options())
            .await,
        Err(FiError::FleetManager { .. })
    ));
    assert_eq!(
        fman_state.availability_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(fman_state.quote_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);
    // The payment federation is resolved by the first priced seat, and no
    // seat got that far, so the wallet was not consulted at all.
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.recover_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn advertised_size_capability_stops_formation_before_quotes() {
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let mut config = FmanConfig::paid();
    config.federation_size = FederationSize(10);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        config,
    )
    .await;

    let error = client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap_err();
    assert_eq!(error.code(), FiErrorCode::FleetManager);
    assert!(
        error
            .to_string()
            .contains("requested federation size is not offered")
    );
    assert_eq!(
        fman_state.availability_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(fman_state.quote_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn external_capability_calls_obey_request_timeout() {
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let mut config = FmanConfig::paid();
    config.hang_availability = true;
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        config,
    )
    .await;
    let timeout_options = FormationRunOptions::new(crate::FormationRunOptionsConfig {
        poll_interval: Duration::from_millis(1),
        run_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_millis(10),
    })
    .unwrap();

    assert!(matches!(
        client
            .create_with_pinned_fmans(intent(), locators(), timeout_options)
            .await,
        Err(FiError::Timeout(_))
    ));
    let status = client.status();
    assert_eq!(formation(&status).last_error, Some(FiErrorCode::Timeout));
    assert_eq!(fman_state.quote_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn terminal_payment_rejection_clears_quotes_and_requires_fresh_authorization() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let original_status = client.status();
    let original = payment_requirements(&original_status)
        .seats
        .iter()
        .map(|requirement| requirement.quote_id)
        .collect::<HashSet<_>>();

    payment_state.reject_recovery.store(true, Ordering::SeqCst);
    let authorization_id = payment_requirements(&original_status)
        .authorization_id
        .clone();
    assert!(matches!(
        client.authorize_payments(authorization_id, options()).await,
        Err(FiError::Payment(_))
    ));
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(
        recovery
            .seats
            .iter()
            .all(|seat| seat.signed_quote.is_none())
    );

    payment_state.reject_recovery.store(false, Ordering::SeqCst);
    client.resume().await.unwrap();
    let replacement_status = client.status();
    assert_eq!(
        formation(&replacement_status).phase,
        FormationPhase::AwaitingPaymentReadiness
    );
    let replacement = payment_requirements(&replacement_status)
        .seats
        .iter()
        .map(|requirement| requirement.quote_id)
        .collect::<HashSet<_>>();
    assert!(original.is_disjoint(&replacement));
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        2 * usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);

    client
        .authorize_payments(
            payment_requirements(&replacement_status)
                .authorization_id
                .clone(),
            options(),
        )
        .await
        .unwrap();
    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
}

#[tokio::test]
async fn lost_create_seat_response_replays_exact_quote_without_refunding_twice() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let mut config = FmanConfig::paid();
    config.create_behavior = CreateBehavior::HangFirst;
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    let create_started = fman_state.create_started.notified();
    let authorization_id = payment_requirements(&client.status())
        .authorization_id
        .clone();
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .authorize_payments(authorization_id, long_request_options())
            .await
    });
    create_started.await;
    let hung_quote = fman_state
        .hung_quote
        .lock()
        .expect("test lock")
        .expect("hung quote");
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let quote_calls_after_funding = fman_state.quote_calls.load(Ordering::SeqCst);
    assert_eq!(
        quote_calls_after_funding,
        usize::from(MIN_FEDERATION_SIZE),
        "the durable reservation removes the old same-terms refresh round"
    );

    let reopened = open_client(database, payments, fman_state.clone(), config).await;
    reopened.resume().await.unwrap();
    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        quote_calls_after_funding
    );
    let created_quotes = payment_state
        .created_quotes
        .lock()
        .expect("test lock")
        .clone();
    assert_eq!(created_quotes.len(), usize::from(MIN_FEDERATION_SIZE));
    assert_eq!(
        created_quotes.iter().copied().collect::<HashSet<_>>().len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
    let records = fman_state.create_records.lock().expect("test lock");
    let replayed = records
        .iter()
        .filter(|record| record.quote_id == hung_quote)
        .collect::<Vec<_>>();
    assert_eq!(replayed.len(), 2);
    assert_eq!(replayed[0].signed_quote, replayed[1].signed_quote);
}

#[tokio::test]
async fn repeated_refund_settlement_replays_exact_context_before_quote_replacement() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let mut config = FmanConfig::paid();
    config.create_behavior = CreateBehavior::RefuseFirstQuote;
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    payment_state
        .hang_first_refund
        .store(true, Ordering::SeqCst);
    let refund_started = payment_state.refund_started.notified();
    let authorization_id = payment_requirements(&client.status())
        .authorization_id
        .clone();
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .authorize_payments(authorization_id, long_request_options())
            .await
    });
    refund_started.await;
    let refused_quote = fman_state
        .refused_quote
        .lock()
        .expect("test lock")
        .expect("refused quote");
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());

    let reopened = open_client(database, payments, fman_state.clone(), config).await;
    let replay_error = reopened.resume().await.unwrap_err();
    assert!(
        matches!(replay_error, FiError::SeatRefused { .. }),
        "{replay_error:?}"
    );
    assert_eq!(payment_state.refund_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        payment_state
            .refund_contexts
            .lock()
            .expect("test lock")
            .as_slice(),
        &[refused_quote, refused_quote]
    );
    {
        let records = fman_state.create_records.lock().expect("test lock");
        let replayed = records
            .iter()
            .filter(|record| record.quote_id == refused_quote)
            .collect::<Vec<_>>();
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].signed_quote, replayed[1].signed_quote);
    }

    let payment_creates = payment_state.create_calls.load(Ordering::SeqCst);
    let seat_creates = fman_state.create_calls.load(Ordering::SeqCst);
    reopened.resume().await.unwrap();
    let status = reopened.status();
    let requirements = payment_requirements(&status);
    assert_eq!(requirements.seats.len(), 1);
    assert_ne!(requirements.seats[0].quote_id, refused_quote);
    assert_eq!(
        formation(&status).phase,
        FormationPhase::AwaitingPaymentReadiness
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        payment_creates
    );
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), seat_creates);
}

#[tokio::test]
async fn lost_free_create_seat_response_replays_exact_quote_and_allocation() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let mut config = FmanConfig::given_away();
    config.create_behavior = CreateBehavior::HangFirst;
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let create_started = fman_state.create_started.notified();
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .create_with_pinned_fmans(intent(), locators(), long_request_options())
            .await
    });
    create_started.await;
    let hung_quote = fman_state
        .hung_quote
        .lock()
        .expect("test lock")
        .expect("hung quote");
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let quote_calls_before_resume = fman_state.quote_calls.load(Ordering::SeqCst);

    let reopened = open_client(database, payments, fman_state.clone(), config).await;
    reopened.resume().await.unwrap();
    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        quote_calls_before_resume
    );
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    let records = fman_state.create_records.lock().expect("test lock");
    let replayed = records
        .iter()
        .filter(|record| record.quote_id == hung_quote)
        .collect::<Vec<_>>();
    assert_eq!(replayed.len(), 2);
    assert_eq!(replayed[0].signed_quote, replayed[1].signed_quote);
    assert_eq!(replayed[0].seat_id, replayed[1].seat_id);
    assert_eq!(
        fman_state.allocated_quotes.lock().expect("test lock").len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn formed_fi_proposes_and_confirms_the_fixed_4_1_1_policy() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let fi_account = guardian_fee_account(30);
    let fee_account_provider = TestFiFeeAccountProvider::new(fi_account.clone());
    let client = open_client_with_fee_account(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
        fee_account_provider.clone(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    client
        .propose_guardian_fees(GuardianFeePpm::MANIFOLD_DEFAULT, options())
        .await
        .unwrap();

    let formed_status = client.status();
    let formed_invite = formation(&formed_status)
        .invite_code
        .as_ref()
        .expect("formed FI persisted its invite");
    let formed_federation_id = formed_invite
        .0
        .parse::<FedimintInviteCode>()
        .expect("formed invite parses")
        .federation_id();
    assert_eq!(
        fee_account_provider.requested_federations(),
        vec![formed_federation_id],
        "fi-client chooses the provider lookup identity from durable formed state"
    );

    let submissions = fman_state.fee_submissions.lock().expect("test lock");
    assert_eq!(submissions.len(), usize::from(MIN_FEDERATION_SIZE));
    let distinct = submissions.values().cloned().collect::<HashSet<_>>();
    assert_eq!(
        distinct.len(),
        1,
        "every FMan receives identical policy bytes"
    );
    let (send_ppm, recipients) = distinct.into_iter().next().unwrap();
    assert_eq!(send_ppm, 5_000);
    let value: serde_json::Value = serde_json::from_str(&recipients).unwrap();
    let entries = value["recipients"].as_array().unwrap();
    assert_eq!(entries.len(), usize::from(MIN_FEDERATION_SIZE) + 2);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry["weight"].as_u64().unwrap())
            .sum::<u64>(),
        u64::from(MIN_FEDERATION_SIZE) + 5,
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry["weight"] == serde_json::json!(4))
            .count(),
        1,
    );
    let fi = entries
        .iter()
        .find(|entry| entry["account_id"] == fi_account.id().to_string())
        .expect("FI role account is present");
    assert_eq!(fi["weight"], 4);
    assert_eq!(fi["account"], serde_json::to_value(&fi_account).unwrap());
    let guardian_verification_fee_account = guardian_fee_account(31);
    let guardian_verification_fee = entries
        .iter()
        .find(|entry| entry["account_id"] == guardian_verification_fee_account.id().to_string())
        .expect("Guardian Verification Fee account is present");
    assert_eq!(guardian_verification_fee["weight"], 1);
}

#[tokio::test]
async fn formation_names_the_fman_with_a_divergent_guardian_verification_fee_account() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let divergent_index = 2;
    let divergent_account = guardian_fee_account(29);
    fman_state
        .formation_guardian_verification_fee_accounts
        .lock()
        .expect("test lock")
        .insert(divergent_index, divergent_account.clone());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;

    let error = client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        FiError::FleetManager { index: 2, message }
            if message == "Guardian Verification Fee account does not match this Fleet Manager's configuration"
    ));
    assert!(
        !fman_state
            .fee_submissions
            .lock()
            .expect("test lock")
            .contains_key(&divergent_index),
        "the divergent FMan must reject before submitting a vote"
    );
}

#[tokio::test]
async fn unavailable_fi_fee_account_fails_before_any_guardian_vote() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client_with_fee_account(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
        TestFiFeeAccountProvider::unavailable(),
    )
    .await;
    assert!(matches!(
        client
            .create_with_pinned_fmans(intent(), locators(), options())
            .await,
        Err(FiError::CapabilityUnavailable(Capability::FeeArrangement))
    ));
    assert!(
        fman_state
            .fee_submissions
            .lock()
            .expect("test lock")
            .is_empty(),
        "an unavailable consumer account must fail before any guardian vote"
    );
}

#[tokio::test]
async fn fi_and_guardian_accounts_must_be_distinct() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let colliding_with_seat_zero = guardian_fee_account(32);
    let client = open_client_with_fee_account(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
        TestFiFeeAccountProvider::new(colliding_with_seat_zero.clone()),
    )
    .await;
    assert!(matches!(
        client.create_with_pinned_fmans(intent(), locators(), options()).await,
        Err(FiError::InvalidFleetManagers(message))
            if message.contains("do not form a canonical recipient set")
    ));
    assert!(
        fman_state
            .fee_submissions
            .lock()
            .expect("test lock")
            .is_empty()
    );
}

#[tokio::test]
async fn guardian_verification_fee_and_fi_accounts_must_be_distinct() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let shared_account = guardian_fee_account(31);
    let client = open_client_with_fee_account(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
        TestFiFeeAccountProvider::new(shared_account.clone()),
    )
    .await;

    assert!(matches!(
        client.create_with_pinned_fmans(intent(), locators(), options()).await,
        Err(FiError::InvalidFleetManagers(message))
            if message.contains("do not form a canonical recipient set")
    ));
    assert!(
        fman_state
            .fee_submissions
            .lock()
            .expect("test lock")
            .is_empty()
    );
}

#[tokio::test]
async fn a_guardian_fee_below_the_admitted_minimum_never_reaches_a_guardian_vote() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let admitted = fman_state
        .fee_submissions
        .lock()
        .expect("test lock")
        .clone();

    // The publisher raises the floor above the 1,500-ppm default. It rides the
    // same durable setup-payment publication the payer set already comes from,
    // so the FI reads it without a fetch and without a new stored field.
    client
        .inner
        .store
        .store_setup_payment_federations_event(setup_payment_event_with_min_fee_ppm(
            test_now_secs() + 10,
            &[PAYMENT_INVITE],
            2_500,
        ))
        .await
        .unwrap();
    assert_eq!(client.min_guardian_fee_ppm().await, 2_500);

    // Every FMan would vote this down. Refusing it here is what turns a
    // tolerated per-guardian rejection — which the wave retries until the run
    // deadline — into the one answer that names the reason.
    for send_ppm in [0, 1, 2_499] {
        assert!(
            matches!(
                client
                    .propose_guardian_fees(
                        send_ppm.try_into().unwrap(),
                        options(),
                    )
                    .await,
                Err(FiError::InvalidIntent(message))
                    if message.contains("below the published minimum of 2500")
            ),
            "{send_ppm} ppm is below the admitted minimum"
        );
    }
    assert_eq!(
        *fman_state.fee_submissions.lock().expect("test lock"),
        admitted,
        "a refused rate must never reach a guardian vote"
    );

    // The floor itself is a rate an FI may propose.
    client
        .propose_guardian_fees(2_500.try_into().unwrap(), options())
        .await
        .unwrap();
    let raw = fman_state
        .meta_consensus_raw
        .lock()
        .expect("test lock")
        .clone()
        .expect("fee rate reached consensus");
    let fields: BTreeMap<String, serde_json::Value> = serde_json::from_slice(&raw).unwrap();
    assert_eq!(fields["fedi:guardian_fee_send_ppm"], "2500");
}

#[tokio::test]
async fn an_explicit_no_op_rate_write_must_satisfy_the_current_floor() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let admitted = fman_state
        .fee_submissions
        .lock()
        .expect("test lock")
        .clone();
    client
        .inner
        .store
        .store_setup_payment_federations_event(setup_payment_event_with_min_fee_ppm(
            test_now_secs() + 10,
            &[PAYMENT_INVITE],
            6_000,
        ))
        .await
        .unwrap();

    assert!(matches!(
        client
            .propose_guardian_fees(GuardianFeePpm::MANIFOLD_DEFAULT, options())
            .await,
        Err(FiError::InvalidIntent(message))
            if message.contains("below the published minimum of 6000")
    ));
    assert_eq!(
        *fman_state.fee_submissions.lock().expect("test lock"),
        admitted,
        "an explicit no-op rate proposal is still gated before guardian votes"
    );
}

#[tokio::test]
async fn the_default_guardian_fee_minimum_applies_before_any_publication_is_admitted() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let admitted = fman_state
        .fee_submissions
        .lock()
        .expect("test lock")
        .clone();

    // A given-away formation never needed a payer, so no publication carrying
    // a floor has been admitted. Falling back to zero here is exactly what
    // would let an FI propose 0 ppm, so the published default stands in.
    let default_minimum = fedi_decentralized_domain::DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM;
    assert_eq!(client.min_guardian_fee_ppm().await, default_minimum);
    let default_minimum =
        u32::try_from(default_minimum).expect("the published default fits the ppm domain");

    assert!(matches!(
        client
            .propose_guardian_fees(
                (default_minimum - 1).try_into().unwrap(),
                options(),
            )
            .await,
        Err(FiError::InvalidIntent(message))
            if message.contains("below the published minimum of 1500")
    ));
    assert_eq!(
        *fman_state.fee_submissions.lock().expect("test lock"),
        admitted
    );

    // Anything above the floor is untouched: the 5,000-ppm Manifold default
    // proposes exactly as it did before the floor existed.
    client
        .propose_guardian_fees(GuardianFeePpm::MANIFOLD_DEFAULT, options())
        .await
        .unwrap();
    let raw = fman_state
        .meta_consensus_raw
        .lock()
        .expect("test lock")
        .clone()
        .expect("fee rate reached consensus");
    let fields: BTreeMap<String, serde_json::Value> = serde_json::from_slice(&raw).unwrap();
    assert_eq!(fields["fedi:guardian_fee_send_ppm"], "5000");
}

#[tokio::test]
async fn guardian_fee_policy_rebases_after_a_stale_wave_and_preserves_other_fields() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let reader = TestConsensusReader::new(fman_state.clone());
    let client = open_client_with_reader(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
        reader.clone(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    fman_state
        .meta_submissions
        .lock()
        .expect("test lock")
        .clear();
    fman_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    let formed = fman_state
        .meta_consensus_raw
        .lock()
        .expect("test lock")
        .clone()
        .expect("formation metadata reached consensus");
    let mut initial_fields: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&formed).unwrap();
    initial_fields.insert("existing".to_owned(), serde_json::json!("old"));
    let initial = serde_json::to_vec(&initial_fields).unwrap();
    initial_fields.insert("existing".to_owned(), serde_json::json!("new"));
    let concurrent = serde_json::to_vec(&initial_fields).unwrap();
    reader.change_base_after_next_read(initial.clone(), concurrent.clone());
    let revision_before = fman_state.meta_consensus_revision.load(Ordering::SeqCst);
    client
        .propose_guardian_fees(2_500.try_into().unwrap(), options())
        .await
        .unwrap();

    let bases = fman_state.meta_request_bases.lock().expect("test lock");
    let seats = usize::from(MIN_FEDERATION_SIZE);
    assert_eq!(bases.len(), seats * 2);
    // The advance from `initial` to `concurrent` is one adoption: the fake
    // consensus revision moves exactly once with it.
    let initial_base = MetaConsensusBase::from_consensus(Some((revision_before, &initial)));
    let concurrent_base =
        MetaConsensusBase::from_consensus(Some((revision_before + 1, &concurrent)));
    assert!(bases[..seats].iter().all(|(_, base)| *base == initial_base));
    assert!(
        bases[seats..]
            .iter()
            .all(|(_, base)| *base == concurrent_base)
    );
    let final_raw = fman_state
        .meta_consensus_raw
        .lock()
        .expect("test lock")
        .clone()
        .expect("fee policy published metadata");
    let fields: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&final_raw).expect("updated metadata parses");
    assert_eq!(
        fields.get("existing"),
        Some(&serde_json::Value::String("new".to_owned()))
    );
    assert_eq!(
        fields.get("fedi:guardian_fee_send_ppm"),
        Some(&serde_json::Value::String("2500".to_owned()))
    );
    assert!(fields.contains_key("fedi:guardian_fee_remittance_account"));
}

#[tokio::test]
async fn generic_open_fails_closed_for_the_deployment_guardian_verification_fee_account() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = FiClient::open(
        MemDatabase::new().into_database(),
        TestIdentity,
        payments,
        TestRegistry::default(),
        TestConnector {
            state: fman_state,
            config: FmanConfig::given_away(),
        },
        test_peer_badge_verifier(),
        TestConsensusReader::new(Arc::new(FmanState::default())),
        UnavailableFiFeeAccountProvider,
    )
    .await
    .unwrap();

    assert!(matches!(
        client
            .propose_guardian_fees((MAX_GUARDIAN_FEE_PPM + 1).try_into().unwrap(), options(),)
            .await,
        Err(FiError::InvalidIntent(_))
    ));
    assert!(matches!(
        client
            .propose_guardian_fees(GuardianFeePpm::MANIFOLD_DEFAULT, options(),)
            .await,
        Err(FiError::NoActiveFormation)
    ));
}

#[tokio::test]
async fn selected_free_lost_create_seat_reopens_after_verifier_drift_with_exact_presentation() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let mut config = FmanConfig::given_away();
    config.create_behavior = CreateBehavior::HangFirst;
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let create_started = fman_state.create_started.notified();
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .pay_and_create(
                intent(),
                selection_approval(1),
                payment_federation_id(),
                long_request_options(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), create_started)
        .await
        .expect("selected free CreateSeat reaches the injected lost response");
    let hung_quote = fman_state
        .hung_quote
        .lock()
        .expect("test lock")
        .expect("hung quote");
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    let interrupted = recovery
        .seats
        .iter()
        .find(|seat| {
            seat.signed_quote.as_ref().is_some_and(|signed| {
                signed
                    .verify(&seat.progress.locator.service_pubkey)
                    .is_ok_and(|quote| quote.quote_id() == hung_quote)
            })
        })
        .expect("the interrupted selected seat remains durable");
    assert!(matches!(
        &interrupted.admission,
        crate::db::FmanAdmission::PeerBadge {
            state: crate::db::AdmissionState::EffectAuthorized {
                quote_id,
                effect: crate::db::AdmissionEffect::FreePresentation,
            },
            ..
        } if *quote_id == hung_quote
    ));
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let quote_calls_before_resume = fman_state.quote_calls.load(Ordering::SeqCst);
    drop(client);

    let reopened = open_client_with_verifier(
        database,
        payments,
        fman_state.clone(),
        config,
        peer_badge_verifier(ManifoldEnvironment::Staging),
    )
    .await;
    reopened.resume().await.unwrap();

    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        quote_calls_before_resume,
        "authorized recovery must not require a fresh preview or quote",
    );
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    let records = fman_state.create_records.lock().expect("test lock");
    let replayed = records
        .iter()
        .filter(|record| record.quote_id == hung_quote)
        .collect::<Vec<_>>();
    assert_eq!(replayed.len(), 2);
    assert_eq!(replayed[0].signed_quote, replayed[1].signed_quote);
    assert_eq!(replayed[0].seat_id, replayed[1].seat_id);
    assert_eq!(
        fman_state.allocated_quotes.lock().expect("test lock").len(),
        usize::from(MIN_FEDERATION_SIZE),
    );
}

#[tokio::test]
async fn selected_free_refusal_returns_idle_and_requires_a_fresh_preview() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::given_away()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;

    let error = client
        .pay_and_create(
            intent(),
            selection_approval(1),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::SelectedFmanUnavailable
        )
    ));
    assert_eq!(client.status(), FiStatus::Idle);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    let refused_quote = fman_state
        .refused_quote
        .lock()
        .expect("test lock")
        .expect("one signed free refusal was returned");
    let presentations_before_reopen = fman_state.create_calls.load(Ordering::SeqCst);
    assert_eq!(
        fman_state
            .create_records
            .lock()
            .expect("test lock")
            .iter()
            .filter(|record| record.quote_id == refused_quote)
            .count(),
        1,
        "the refused quote-bound admission was presented exactly once",
    );
    drop(client);

    let reopened = open_client(
        database,
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    assert_eq!(reopened.status(), FiStatus::Idle);
    assert!(matches!(
        reopened.resume().await,
        Err(FiError::NoActiveFormation)
    ));
    assert_eq!(
        fman_state.create_calls.load(Ordering::SeqCst),
        presentations_before_reopen,
        "reopen cannot replay or re-quote the consumed admission",
    );

    reopened
        .pay_and_create(
            intent(),
            selection_approval(1),
            payment_federation_id(),
            options(),
        )
        .await
        .expect("a newly approved selection can start a fresh formation");
    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
    assert_eq!(
        fman_state
            .create_records
            .lock()
            .expect("test lock")
            .iter()
            .filter(|record| record.quote_id == refused_quote)
            .count(),
        1,
        "fresh formation never presents the refused quote a second time",
    );
}

#[tokio::test]
async fn consensus_carrying_a_different_directory_never_completes_formation() {
    // The readback exists to prove the exact bytes reached consensus. A
    // federation reporting some other value is not "done" — it is not done
    // yet, and the run must keep waiting rather than declare success.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let reader = TestConsensusReader::new(fman_state.clone());
    reader.force_value("{\"seat_bindings\":[],\"version\":1}");
    let client = open_client_with_reader(
        database,
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
        reader,
    )
    .await;

    let result = client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await;

    assert!(
        matches!(&result, Err(FiError::Timeout(_))),
        "expected the run to keep waiting until its deadline, got {result:?}"
    );
    // It really did submit; only consensus disagreed.
    assert_eq!(
        fman_state.meta_submissions.lock().expect("test lock").len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn an_fman_claiming_a_foreign_peer_is_not_pinned_for_recovery() {
    // The FI must reject the invalid directory before persisting its target:
    // recovery replays target bytes exactly and cannot repair a foreign peer
    // once that target is durable.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    fman_state.attest_foreign_peer.store(true, Ordering::SeqCst);
    let client = open_client(
        database,
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;

    let result = client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await;

    assert!(
        matches!(&result, Err(FiError::InvalidFleetManagers(message))
            if message.contains("names peer 9") && message.contains("not a guardian seat")),
        "expected the directory to be rejected against the config, got {result:?}"
    );
    assert!(
        fman_state
            .meta_submissions
            .lock()
            .expect("test lock")
            .is_empty(),
        "an invalid directory must not reach a guardian vote"
    );

    fman_state
        .attest_foreign_peer
        .store(false, Ordering::SeqCst);
    client.resume().await.unwrap();
    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
}

#[tokio::test]
async fn an_attested_fee_account_must_match_the_signed_seat_acceptance() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    fman_state
        .attest_wrong_fee_account
        .store(true, Ordering::SeqCst);
    let client = open_client(database, payments, fman_state, FmanConfig::given_away()).await;

    let result = client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await;

    assert!(
        matches!(&result, Err(FiError::InvalidFleetManagers(message))
            if message.contains("differs from its signed seat acceptance")),
        "expected the account rebind to fail before publication, got {result:?}"
    );
}

#[tokio::test]
async fn resume_replays_the_persisted_directory_without_refetching_attestations() {
    // Reassembling on resume could produce different bytes, and consensus only
    // accepts a value threshold guardians submitted byte-identically — so a
    // reassembling resume would never converge.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    let after_formation = fman_state.attestation_calls.load(Ordering::SeqCst);
    assert_eq!(after_formation, usize::from(MIN_FEDERATION_SIZE));

    let reopened = open_client(
        database,
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    reopened.resume().await.unwrap();

    assert_eq!(
        fman_state.attestation_calls.load(Ordering::SeqCst),
        after_formation,
        "resume refetched attestations instead of replaying the persisted directory"
    );
}

#[tokio::test]
async fn formed_resume_accepts_a_later_rate_without_fee_account_capabilities() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    client
        .propose_guardian_fees(2_500.try_into().unwrap(), options())
        .await
        .unwrap();
    drop(client);

    let reopened = open_client_with_fee_account(
        database,
        payments,
        fman_state,
        FmanConfig::given_away(),
        TestFiFeeAccountProvider::unavailable(),
    )
    .await;
    reopened.resume().await.unwrap();
    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
}

#[tokio::test]
async fn interrupted_formation_replays_its_persisted_fee_target() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    fman_state
        .block_meta_indices
        .lock()
        .expect("test lock")
        .extend(0..usize::from(MIN_FEDERATION_SIZE));
    let original_provider = TestFiFeeAccountProvider::new(guardian_fee_account(30));
    let client = open_client_with_fee_account(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::given_away(),
        original_provider.clone(),
    )
    .await;
    let operation = tokio::spawn(async move {
        client
            .create_with_pinned_fmans(intent(), locators(), options())
            .await
    });
    fman_state.meta_call_blocked.notified().await;
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    fman_state.release_meta_calls.store(true, Ordering::SeqCst);
    fman_state.meta_call_release.notify_waiters();

    let replacement_provider = TestFiFeeAccountProvider::new(guardian_fee_account(29));
    let reopened = open_client_with_fee_account(
        database,
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
        replacement_provider.clone(),
    )
    .await;
    reopened.resume().await.unwrap();

    assert_eq!(original_provider.requested_federations().len(), 1);
    assert!(replacement_provider.requested_federations().is_empty());
    let recipients = fman_state
        .fee_submissions
        .lock()
        .expect("test lock")
        .values()
        .next()
        .expect("replayed fee target")
        .1
        .clone();
    assert!(recipients.contains(&guardian_fee_account(30).id().to_string()));
    assert!(!recipients.contains(&guardian_fee_account(29).id().to_string()));
}

#[tokio::test]
async fn exact_formation_readback_wins_over_late_already_published() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    fman_state
        .meta_terminal_errors
        .lock()
        .expect("test lock")
        .insert(
            usize::from(MIN_FEDERATION_SIZE) - 1,
            FleetManagerError::FormationMetaAlreadyPublished,
        );
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state,
        FmanConfig::given_away(),
    )
    .await;

    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
}

#[tokio::test]
async fn fee_rate_uses_terminal_maintenance_rejection() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    fman_state
        .meta_terminal_errors
        .lock()
        .expect("test lock")
        .extend(
            (0..usize::from(MIN_FEDERATION_SIZE))
                .map(|index| (index, FleetManagerError::MetaValueInvalid)),
        );

    assert!(matches!(
        client
            .propose_guardian_fees(2_500.try_into().unwrap(), options())
            .await,
        Err(FiError::MaintenanceRejected { .. })
    ));
}

#[tokio::test]
async fn a_transient_consensus_read_failure_is_retried() {
    // An unreachable federation is a reason to wait, not to fail a formation
    // whose directory may already be in consensus.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let reader = TestConsensusReader::new(fman_state.clone());
    reader.fail_next(3);
    let client = open_client_with_reader(
        database,
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
        reader,
    )
    .await;

    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
}

#[tokio::test]
async fn pre_payment_abandon_wipes_a_parked_paid_formation_to_idle() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    assert_eq!(
        formation(&client.status()).phase,
        FormationPhase::AwaitingPaymentReadiness
    );

    client.abandon_formation(options()).await.unwrap();

    assert_eq!(client.status(), FiStatus::Idle);
    assert_eq!(payment_state.recover_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);

    // The wipe is durable: a reopened client sees no formation to resume,
    // and a fresh creation is accepted.
    let reopened = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    assert_eq!(reopened.status(), FiStatus::Idle);
    assert!(matches!(
        reopened.resume().await,
        Err(FiError::NoActiveFormation)
    ));
    reopened
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    assert_eq!(
        formation(&reopened.status()).phase,
        FormationPhase::AwaitingPaymentReadiness
    );
}

#[tokio::test]
async fn pre_payment_abandon_forfeits_accepted_free_seats() {
    // A free formation can hold FMan-accepted seats before any payment
    // exists. Abandoning is still value-safe — no money moved — but the
    // accepted seats are forfeited server-side, not released.
    let database = MemDatabase::new().into_database();
    let (payments, _payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::given_away()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let error = client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            FiError::SeatRefused { .. } | FiError::FleetManager { .. }
        ),
        "{error}"
    );
    assert!(
        fman_state.create_calls.load(Ordering::SeqCst) > 1,
        "sibling free seats were accepted server-side before the refusal",
    );

    client.abandon_formation(options()).await.unwrap();
    assert_eq!(client.status(), FiStatus::Idle);

    let reopened = open_client(database, payments, fman_state, FmanConfig::given_away()).await;
    assert_eq!(reopened.status(), FiStatus::Idle);
}

#[tokio::test]
async fn abandon_after_commercial_authorization_before_outputs_is_value_safe() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let requirements = payment_requirements(&client.status()).clone();
    let authorizations = requirements
        .seats
        .iter()
        .map(|requirement| QuoteAuthorization {
            index: requirement.index,
            quote_id: requirement.quote_id,
        })
        .collect::<Vec<_>>();
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    client
        .inner
        .store
        .authorize_payments(
            &recovery.snapshot.formation_id,
            &requirements.authorization_id,
            &authorizations,
        )
        .await
        .unwrap();

    client.abandon_formation(options()).await.unwrap();
    assert_eq!(client.status(), FiStatus::Idle);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn connector_failure_before_first_output_remains_abandonable() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let requirements = payment_requirements(&client.status()).clone();

    // Attempt one quoted initially. Exact authorization pins that quote, so
    // attempt two is the final presentation connection. That last fallible
    // connection must still be on the abandonable side of the value boundary.
    *fman_state
        .fail_connect_on_attempt
        .lock()
        .expect("test lock") = Some((0, 2));
    let error = client
        .authorize_payments(requirements.authorization_id, options())
        .await
        .expect_err("the injected final connection fails");
    assert!(matches!(error, FiError::FleetManager { .. }), "{error}");
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(!recovery.payment_outputs_started);

    client.abandon_formation(options()).await.unwrap();
    assert_eq!(client.status(), FiStatus::Idle);
}

#[tokio::test]
async fn connector_deadline_before_first_output_remains_abandonable() {
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let requirements = payment_requirements(&client.status()).clone();
    *fman_state
        .hang_connect_on_attempt
        .lock()
        .expect("test lock") = Some((0, 2));
    let short_request = FormationRunOptions::new(FormationRunOptionsConfig {
        poll_interval: Duration::from_millis(10),
        run_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_millis(10),
    })
    .unwrap();

    let error = client
        .authorize_payments(requirements.authorization_id, short_request)
        .await
        .expect_err("the final connection reaches its request deadline");
    assert!(matches!(error, FiError::Timeout(_)), "{error}");
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(!recovery.payment_outputs_started);

    client.abandon_formation(options()).await.unwrap();
    assert_eq!(client.status(), FiStatus::Idle);
}

#[tokio::test]
async fn every_fman_connection_resolves_before_first_wallet_output_poll() {
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let requirements = payment_requirements(&client.status()).clone();
    let effect_log = Arc::new(Mutex::new(Vec::new()));
    *payment_state.effect_log.lock().expect("test lock") = Some(effect_log.clone());
    *fman_state.effect_log.lock().expect("test lock") = Some(effect_log.clone());

    client
        .authorize_payments(requirements.authorization_id, options())
        .await
        .unwrap();

    let effects = effect_log.lock().expect("test effect log");
    let first_output = effects
        .iter()
        .position(|effect| *effect == TestEffect::PaymentOutputPolled)
        .expect("paid formation polls a wallet output");
    let mut connections = vec![0usize; usize::from(MIN_FEDERATION_SIZE)];
    for effect in &effects[..first_output] {
        if let TestEffect::FmanConnected(index) = effect {
            connections[*index] += 1;
        }
    }
    assert!(
        connections.iter().all(|count| *count >= 1),
        "every FMan completed the final preflight before funding: {connections:?}"
    );
}

#[tokio::test]
async fn completed_payment_is_durable_before_the_next_funding_call_starts() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let requirements = payment_requirements(&client.status()).clone();
    let effect_log = Arc::new(Mutex::new(Vec::new()));
    *payment_state.effect_log.lock().expect("test lock") = Some(effect_log.clone());
    let checkpoint_log = effect_log.clone();
    client
        .inner
        .store
        .set_seat_checkpoint_hook(Arc::new(move |_index| {
            checkpoint_log
                .lock()
                .expect("test effect log")
                .push(TestEffect::SeatCheckpointed);
        }));

    client
        .authorize_payments(requirements.authorization_id, options())
        .await
        .unwrap();

    // Both events are appended synchronously: payment entry in the wallet
    // call, and checkpoint completion immediately after the database commit.
    // The old concurrent funding wave therefore records payment two before
    // any checkpoint and deterministically fails this assertion.
    let effects = effect_log.lock().expect("test effect log");
    let payment_entries = effects
        .iter()
        .enumerate()
        .filter_map(|(position, effect)| {
            (*effect == TestEffect::PaymentOutputPolled).then_some(position)
        })
        .collect::<Vec<_>>();
    let first_checkpoint = effects
        .iter()
        .position(|effect| *effect == TestEffect::SeatCheckpointed)
        .expect("paid formation checkpoints its first seat");
    assert!(payment_entries.len() >= 2);
    assert!(
        first_checkpoint < payment_entries[1],
        "payment two entered before payment one's durable checkpoint: {effects:?}"
    );
    drop(effects);

    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(
        recovery
            .snapshot
            .seats
            .iter()
            .all(|seat| seat.seat_id.is_some()),
        "every paid seat is durable after the blocked next payment resumes",
    );
}

#[tokio::test]
async fn stale_lease_cannot_arm_payment_outputs_after_takeover() {
    let database = MemDatabase::new().into_database();
    let now = Arc::new(AtomicU64::new(12_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
    };
    let store = db::FiStore::new_with_lease_clock(database, clock);
    let (payments, payment_state) = TestPayments::new();
    let client = open_client_with_store(
        store.clone(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let requirements = payment_requirements(&client.status()).clone();
    let authorizations = requirements
        .seats
        .iter()
        .map(|requirement| QuoteAuthorization {
            index: requirement.index,
            quote_id: requirement.quote_id,
        })
        .collect::<Vec<_>>();
    let recovery = active_recovery(store.load_recovery(TestIdentity::fi_id()).await.unwrap());
    store
        .authorize_payments(
            &recovery.snapshot.formation_id,
            &requirements.authorization_id,
            &authorizations,
        )
        .await
        .unwrap();

    let stale = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
        .await
        .unwrap();
    stale.renew().await.unwrap();
    now.store(12_006, Ordering::SeqCst);
    let replacement = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
        .await
        .unwrap();

    let error = stale
        .arm_payment_outputs_started(
            &recovery.snapshot.formation_id,
            test_peer_badge_verifier().provenance().into(),
        )
        .await
        .expect_err("a replaced driver cannot arm wallet output generation");
    assert!(matches!(error, FiError::Busy), "{error}");
    let recovery = active_recovery(store.load_recovery(TestIdentity::fi_id()).await.unwrap());
    assert!(!recovery.payment_outputs_started);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    replacement.renew().await.unwrap();
}

#[tokio::test]
async fn stale_lease_cannot_abandon_formation_after_takeover() {
    let database = MemDatabase::new().into_database();
    let now = Arc::new(AtomicU64::new(13_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
    };
    let store = db::FiStore::new_with_lease_clock(database, clock);
    let (payments, _) = TestPayments::new();
    let client = open_client_with_store(
        store.clone(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let recovery = active_recovery(store.load_recovery(TestIdentity::fi_id()).await.unwrap());

    let stale = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
        .await
        .unwrap();
    now.store(13_006, Ordering::SeqCst);
    let replacement = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
        .await
        .unwrap();

    let error = stale
        .abandon_formation(&recovery.snapshot.formation_id)
        .await
        .expect_err("a replaced driver cannot wipe formation state");
    assert!(matches!(error, FiError::Busy), "{error}");
    assert!(matches!(
        store.load_recovery(TestIdentity::fi_id()).await.unwrap(),
        db::FiRecovery::Formation(_)
    ));
    replacement.renew().await.unwrap();
}

#[tokio::test]
async fn expired_lease_cannot_abandon_formation_after_forward_clock_jump() {
    let database = MemDatabase::new().into_database();
    let now = Arc::new(AtomicU64::new(14_000));
    let clock = {
        let now = now.clone();
        Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
    };
    let store = db::FiStore::new_with_lease_clock(database, clock);
    let (payments, _) = TestPayments::new();
    let client = open_client_with_store(
        store.clone(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let recovery = active_recovery(store.load_recovery(TestIdentity::fi_id()).await.unwrap());

    let stale = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
        .await
        .unwrap();
    now.store(14_006, Ordering::SeqCst);

    let error = stale
        .abandon_formation(&recovery.snapshot.formation_id)
        .await
        .expect_err("an expired driver cannot wipe formation state");
    assert!(matches!(error, FiError::Busy), "{error}");
    assert!(matches!(
        store.load_recovery(TestIdentity::fi_id()).await.unwrap(),
        db::FiRecovery::Formation(_)
    ));
    let replacement = store
        .acquire_driver_lease(Duration::from_secs(30), Duration::from_secs(5))
        .await
        .unwrap();
    replacement.renew().await.unwrap();
}

#[tokio::test]
async fn pre_output_cleanup_retains_formation_when_wallet_release_fails() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::paid(),
    )
    .await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let requirements = payment_requirements(&client.status()).clone();
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    client
        .inner
        .store
        .authorize_payments(
            &recovery.snapshot.formation_id,
            &requirements.authorization_id,
            &requirements
                .seats
                .iter()
                .map(|seat| QuoteAuthorization {
                    index: seat.index,
                    quote_id: seat.quote_id,
                })
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    let reservation_id =
        crate::db::payment_reservation_id(&recovery.snapshot.formation_id, &requirements);
    payment_state
        .reservations
        .lock()
        .expect("test lock")
        .insert(
            reservation_id.as_str().to_owned(),
            TestReservation {
                quote_ids: requirements
                    .seats
                    .iter()
                    .map(|seat| seat.quote_id)
                    .collect(),
                started: HashSet::new(),
                terminal: HashSet::new(),
                released: HashSet::new(),
            },
        );
    payment_state
        .fail_whole_release
        .store(true, Ordering::SeqCst);

    assert!(matches!(
        client.abandon_formation(options()).await,
        Err(FiError::Payment(message)) if message.contains("release failure")
    ));
    assert!(matches!(client.status(), FiStatus::Formation(_)));
    assert_eq!(
        payment_state.reservations.lock().expect("test lock").len(),
        1,
        "a failed release keeps the reconstructable wallet hold",
    );

    payment_state
        .fail_whole_release
        .store(false, Ordering::SeqCst);
    client.abandon_formation(options()).await.unwrap();
    assert_eq!(client.status(), FiStatus::Idle);
    assert!(
        payment_state
            .reservations
            .lock()
            .expect("test lock")
            .is_empty()
    );
}

#[tokio::test]
async fn abandon_survives_authorization_invalidation() {
    // Clearing a quote invalidates the aggregate commercial authorization,
    // but the independent output-generation tombstone must keep gating
    // abandon after the authorization itself is gone.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let requirements = payment_requirements(&client.status()).clone();
    let authorizations = requirements
        .seats
        .iter()
        .map(|requirement| QuoteAuthorization {
            index: requirement.index,
            quote_id: requirement.quote_id,
        })
        .collect::<Vec<_>>();
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    client
        .inner
        .store
        .authorize_payments(
            &recovery.snapshot.formation_id,
            &requirements.authorization_id,
            &authorizations,
        )
        .await
        .unwrap();
    let reservation_id =
        crate::db::payment_reservation_id(&recovery.snapshot.formation_id, &requirements);
    client
        .inner
        .store
        .record_payment_reservation(&recovery.snapshot.formation_id, &reservation_id)
        .await
        .unwrap();
    let lease = client
        .inner
        .store
        .acquire_driver_lease(
            options().lease_duration(),
            options().lease_renewal_duration(),
        )
        .await
        .unwrap();
    lease
        .arm_payment_outputs_started(
            &recovery.snapshot.formation_id,
            test_peer_badge_verifier().provenance().into(),
        )
        .await
        .unwrap();
    client
        .inner
        .store
        .release_driver_lease(lease)
        .await
        .unwrap();
    let old_quote = recovery.seats[0]
        .signed_quote
        .clone()
        .expect("journaled paid quote");
    client
        .inner
        .store
        .clear_quote(&recovery.snapshot.formation_id, 0, &old_quote)
        .await
        .unwrap();
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(
        recovery.test_payment_authorization().is_none(),
        "clearing a quote invalidated the aggregate authorization",
    );

    let error = client.abandon_formation(options()).await.unwrap_err();
    assert!(
        matches!(
            error,
            FiError::AbandonUnavailable(AbandonUnavailableReason::PaymentOutputsStarted)
        ),
        "{error}"
    );
}

#[tokio::test]
async fn commercial_authorization_does_not_imply_the_output_tombstone() {
    // Schema 9 deliberately separates commercial quote authorization from
    // the irreversible output-generation boundary. Reconstructing the former
    // must never synthesize the latter.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    let requirements = payment_requirements(&client.status()).clone();
    let authorizations = requirements
        .seats
        .iter()
        .map(|requirement| QuoteAuthorization {
            index: requirement.index,
            quote_id: requirement.quote_id,
        })
        .collect::<Vec<_>>();
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    client
        .inner
        .store
        .authorize_payments(
            &recovery.snapshot.formation_id,
            &requirements.authorization_id,
            &authorizations,
        )
        .await
        .unwrap();
    client
        .inner
        .store
        .clear_authorization_tombstone_for_test()
        .await;

    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(
        recovery.payment_authorization_recorded,
        "the loader derives the tombstone from a present authorization",
    );
    assert!(!recovery.payment_outputs_started);
    client.abandon_formation(options()).await.unwrap();
    assert_eq!(client.status(), FiStatus::Idle);
}

#[tokio::test]
async fn abandon_after_formed_is_a_typed_error() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state, FmanConfig::given_away()).await;
    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();
    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);

    let error = client.abandon_formation(options()).await.unwrap_err();
    assert!(
        matches!(
            error,
            FiError::AbandonUnavailable(AbandonUnavailableReason::AlreadyFormed)
        ),
        "{error}"
    );
    assert!(matches!(client.status(), FiStatus::Formation(_)));
}

#[tokio::test]
async fn abandon_without_an_active_formation_is_a_typed_error() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::given_away(),
    )
    .await;
    assert!(matches!(
        client.abandon_formation(options()).await,
        Err(FiError::NoActiveFormation)
    ));
}

fn capped_paid_intent(max_total_msats: u64) -> FormationIntent {
    intent()
        .with_max_total_msats(max_total_msats)
        .expect("valid test spending cap")
}

fn compatible_intent() -> FormationIntent {
    FormationIntent::new(
        Some(FederationName("Test Federation".to_owned())),
        FederationSize(MIN_FEDERATION_SIZE),
        PlanPreference::InfiniteBestEffort,
        FedimintdVersionRange::new(
            "0.11.1".parse().expect("range minimum parses"),
            "0.11.3".parse().expect("range maximum parses"),
        )
        .expect("test range is ordered"),
    )
    .expect("compatible test intent is valid")
}

#[test]
fn fedimintd_range_and_dkg_identity_enforce_separate_boundaries() {
    let range = FedimintdVersionRange::new(
        "0.11.1-fedi17+fedi".parse().expect("minimum parses"),
        "0.11.3".parse().expect("maximum parses"),
    )
    .expect("ordered release range");

    assert!(range.contains(&"0.11.1-fedi99+fedi".parse().expect("version parses")));
    assert!(range.contains(&"0.11.2+fedi".parse().expect("version parses")));
    assert!(!range.contains(&"0.11.3+fedi".parse().expect("version parses")));
    assert!(
        range.overlaps_dkg(
            &"0.11.9+fedi"
                .parse::<FedimintdVersion>()
                .expect("version parses")
                .dkg_version()
        )
    );
    assert!(
        !range.overlaps_dkg(
            &"0.12.0+fedi"
                .parse::<FedimintdVersion>()
                .expect("version parses")
                .dkg_version()
        )
    );
    assert!(
        FedimintdVersionRange::new(
            "0.11.2".parse().expect("minimum parses"),
            "0.11.2".parse().expect("maximum parses"),
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<FedimintdVersionRange>(serde_json::json!({
            "minimum": {"major": 0, "minor": 11, "patch": 2},
            "maximum_exclusive": {"major": 0, "minor": 11, "patch": 1}
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<FedimintdVersionRange>(serde_json::json!({
            "minimum": {"major": 0, "minor": 11, "patch": 1},
            "maximum_exclusive": {"major": 0, "minor": 12, "patch": 0},
            "include_prereleases": false
        }))
        .is_err()
    );
}

#[test]
fn resolved_intent_requires_an_allowed_fedi_dkg_identity() {
    for version in ["0.11.2", "0.11.2+acme", "0.12.0+fedi"] {
        assert!(
            compatible_intent()
                .resolve_for_dkg(
                    FederationName("Rejected DKG".to_owned()),
                    version
                        .parse::<FedimintdVersion>()
                        .expect("version parses")
                        .dkg_version(),
                )
                .is_err(),
            "{version} must not produce durable formation intent"
        );
    }
}

#[test]
fn spending_cap_rejects_zero_and_roundtrips_through_the_strict_schema() {
    assert!(matches!(
        intent().with_max_total_msats(0),
        Err(FiError::InvalidIntent(_))
    ));
    assert!(
        serde_json::from_value::<FormationIntent>(serde_json::json!({
            "federation_name": null,
            "federation_size": 7,
            "plan": "infinite_best_effort",
            "fedimintd_versions": {"minimum":{"major":0,"minor":11,"patch":1},"maximum_exclusive":{"major":0,"minor":11,"patch":2}},
            "max_total_msats": 0,
        }))
        .is_err()
    );

    // A capped intent roundtrips, and a capless intent serializes without
    // the field so pre-cap consumers keep decoding it.
    let capped = capped_paid_intent(1_234);
    let value = serde_json::to_value(&capped).unwrap();
    assert_eq!(value["max_total_msats"], serde_json::json!(1_234));
    assert_eq!(
        serde_json::from_value::<FormationIntent>(value).unwrap(),
        capped
    );
    let capless = serde_json::to_value(intent()).unwrap();
    assert!(
        capless
            .as_object()
            .unwrap()
            .get("max_total_msats")
            .is_none()
    );
    assert_eq!(
        serde_json::from_value::<FormationIntent>(capless)
            .unwrap()
            .max_total_msats(),
        None
    );
    // Unknown fields stay rejected alongside the evolved optional field.
    assert!(
        serde_json::from_value::<FormationIntent>(serde_json::json!({
            "federation_name": null,
            "federation_size": 7,
            "plan": "infinite_best_effort",
            "fedimintd_versions": {"minimum":{"major":0,"minor":11,"patch":1},"maximum_exclusive":{"major":0,"minor":11,"patch":2}},
            "max_total_msat": 5,
        }))
        .is_err()
    );
}

#[tokio::test]
async fn under_cap_paid_formation_self_authorizes_and_forms() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;

    client
        .create_with_pinned_fmans(
            capped_paid_intent(PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE)),
            locators(),
            options(),
        )
        .await
        .unwrap();

    let status = client.status();
    assert_eq!(formation(&status).phase, FormationPhase::Formed);
    assert!(formation(&status).action_required.is_none());
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "the self-authorized run funded every paid seat without an explicit call",
    );
    // The self-authorization recorded the same durable quote-bound
    // authorization an explicit call would have written.
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    let (_, authorized) = recovery
        .test_payment_authorization()
        .expect("durable aggregate authorization exists");
    assert_eq!(authorized.len(), usize::from(MIN_FEDERATION_SIZE));
}

#[tokio::test]
async fn over_cap_paid_formation_parks_with_both_numbers() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE) - 1;

    client
        .create_with_pinned_fmans(capped_paid_intent(cap), locators(), options())
        .await
        .unwrap();

    let status = client.status();
    assert_eq!(
        formation(&status).phase,
        FormationPhase::AwaitingPaymentReadiness
    );
    let requirements = payment_requirements(&status);
    assert_eq!(
        requirements.total_msats,
        PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(requirements.max_total_msats, Some(cap));
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.create_calls.load(Ordering::SeqCst), 0);

    // The cap is durable intent state: a reopened client resumes to the same
    // parked action carrying the same cap.
    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    reopened.resume().await.unwrap();
    let reopened_status = reopened.status();
    assert_eq!(
        formation(&reopened_status).phase,
        FormationPhase::AwaitingPaymentReadiness
    );
    assert_eq!(
        formation(&reopened_status).intent.max_total_msats,
        Some(cap)
    );
    assert_eq!(
        payment_requirements(&reopened_status).max_total_msats,
        Some(cap)
    );
    // Explicit authorization of the parked over-cap set still works.
    reopened
        .authorize_payments(
            payment_requirements(&reopened_status)
                .authorization_id
                .clone(),
            options(),
        )
        .await
        .unwrap();
    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
}

#[tokio::test]
async fn absent_cap_still_parks_for_explicit_authorization() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state, FmanConfig::paid()).await;

    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    let status = client.status();
    assert_eq!(
        formation(&status).phase,
        FormationPhase::AwaitingPaymentReadiness
    );
    assert_eq!(payment_requirements(&status).max_total_msats, None);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pay_and_create_uses_the_explicit_ready_payer_and_arms_outputs() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);

    client
        .pay_and_create(
            intent(),
            selection_approval(cap),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap();

    let status = formation(&client.status()).clone();
    assert_eq!(status.phase, FormationPhase::Formed);
    assert!(
        fman_state
            .quote_records
            .lock()
            .expect("test lock")
            .iter()
            .all(|record| record.payment_federation_id.as_ref() == Some(&payment_federation_id())),
        "every paid quote must name the explicitly selected payer",
    );
    assert!(status.payment_outputs_started);
    assert!(payment_state.readiness_calls.load(Ordering::SeqCst) >= 1);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn selected_formation_persists_and_enforces_its_compatible_release() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    set_fman_version(&fman_state, 0, "0.11.2-rc.1+fedi");
    let client = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);

    client
        .pay_and_create(
            compatible_intent(),
            compatible_selection_approval(cap),
            payment_federation_id(),
            options(),
        )
        .await
        .expect("every live FMan offers a build in the sealed cohort");

    let formed = formation(&client.status()).clone();
    assert_eq!(formed.phase, FormationPhase::Formed);
    assert_eq!(
        formed.intent.fedimintd_dkg_version,
        fedimintd_version().dkg_version()
    );
    assert_eq!(
        formed
            .intent
            .fedimintd_versions
            .maximum_exclusive()
            .to_string(),
        "0.11.3"
    );
    assert!(
        fman_state
            .quote_records
            .lock()
            .expect("test lock")
            .iter()
            .any(|record| record.fedimintd_version.to_string() == "0.11.2-rc.1+fedi"),
        "same-minor patch drift is used for the exact quote",
    );
}

#[tokio::test]
async fn selected_fman_minor_drift_requires_fresh_selection_before_payment() {
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    set_fman_version(&fman_state, 0, "0.12.0+fedi");
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;

    let error = client
        .pay_and_create(
            compatible_intent(),
            compatible_selection_approval(PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE)),
            payment_federation_id(),
            options(),
        )
        .await
        .expect_err("a different live minor invalidates the selected set");

    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::SelectedFmanUnavailable
        )
    ));
    assert!(matches!(client.status(), FiStatus::Idle));
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert!(
        fman_state
            .quote_records
            .lock()
            .expect("test lock")
            .iter()
            .all(|record| record.fedimintd_version.dkg_version()
                == fedimintd_version().dkg_version())
    );
}

#[tokio::test]
async fn reopen_preserves_the_selected_dkg_identity_and_accepts_patch_skew() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let resolved = compatible_intent()
        .with_max_total_msats(PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE))
        .unwrap()
        .resolve_for_dkg(
            FederationName("Restart Range".to_owned()),
            fedimintd_version().dkg_version(),
        )
        .unwrap();
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            FormationId("restart-range".to_owned()),
            resolved,
            (0..MIN_FEDERATION_SIZE)
                .map(|index| selected_initial_seat(index, test_now_secs() + 120))
                .collect(),
            crate::db::FormationCreationMode::Selected {
                payment_federation_id: Some(payment_federation_id()),
            },
            None,
        )
        .await
        .unwrap();
    drop(client);
    set_fman_version(&fman_state, 0, "0.11.2+fedi");

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let persisted = formation(&reopened.status()).clone();
    assert_eq!(
        persisted.intent.fedimintd_dkg_version,
        fedimintd_version().dkg_version()
    );
    assert_eq!(
        persisted
            .intent
            .fedimintd_versions
            .maximum_exclusive()
            .to_string(),
        "0.11.3"
    );
    reopened
        .resume()
        .await
        .expect("reopen accepts patch skew inside the persisted DKG identity");
    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn persisted_formation_rejects_a_cross_minor_replacement() {
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    registry.advertisements.lock().expect("test lock").push(
        selection::issuer_ad_for_version_and_service_key_at(
            &fman_keys(20),
            &discovery::issuer_keys(0),
            PAYMENT_AMOUNT_MSATS,
            "0.12.0+fedi",
            manager_key(20).x_only_public_key().0,
            test_now_secs(),
        ),
    );
    let (payments, _) = TestPayments::new();
    let client = open_client_with_registry(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(FmanState::default()),
        FmanConfig {
            create_behavior: CreateBehavior::RefuseFirstQuote,
            ..FmanConfig::paid()
        },
        registry,
    )
    .await;

    let error = client
        .pay_and_create(
            compatible_intent(),
            compatible_selection_approval(PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE)),
            payment_federation_id(),
            options(),
        )
        .await
        .expect_err("the original selected guardian refuses its paid presentation");
    assert!(matches!(error, FiError::SeatRefused { .. }));
    let persisted = formation(&client.status()).clone();
    assert_eq!(
        persisted.intent.fedimintd_dkg_version,
        fedimintd_version().dkg_version()
    );
    assert!(matches!(
        persisted.action_required,
        Some(FormationActionRequired::ReplaceGuardians(_))
    ));

    let error = client
        .preview_fman_replacements(crate::FmanDiscoveryOptions::default())
        .await
        .expect_err("replacement discovery keeps the persisted DKG identity");
    assert!(matches!(
        error,
        FiError::InsufficientFmanSeats {
            selected: 0,
            eligible: 0,
            ..
        }
    ));
}

#[tokio::test]
async fn pinned_formation_derives_one_shared_dkg_identity_across_patches() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    set_fman_version(&fman_state, 0, "0.11.2+fedi");
    set_fman_version(&fman_state, 1, "0.11.1-rc.1+fedi");
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state,
        FmanConfig::given_away(),
    )
    .await;

    client
        .create_with_pinned_fmans(compatible_intent(), locators(), options())
        .await
        .expect("the pinned set shares one DKG identity inside the FI range");

    let formed = formation(&client.status()).clone();
    assert_eq!(formed.phase, FormationPhase::Formed);
    assert_eq!(
        formed.intent.fedimintd_dkg_version,
        fedimintd_version().dkg_version()
    );
    assert_eq!(
        formed
            .intent
            .fedimintd_versions
            .maximum_exclusive()
            .to_string(),
        "0.11.3"
    );
}

#[tokio::test]
async fn pinned_formation_rejects_a_non_fedi_vendor_before_persistence() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    set_fman_version(&fman_state, 0, "0.11.1+acme");
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state,
        FmanConfig::given_away(),
    )
    .await;

    assert!(
        client
            .create_with_pinned_fmans(compatible_intent(), locators(), options())
            .await
            .is_err()
    );
    assert!(matches!(client.status(), FiStatus::Idle));
}

#[tokio::test]
async fn selected_connection_failure_retries_before_requesting_replacement() {
    let database = MemDatabase::new().into_database();
    let (payments, _payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    fman_state
        .connect_failures_remaining
        .store(1, Ordering::SeqCst);
    let client = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);

    client
        .pay_and_create(
            intent(),
            selection_approval(cap),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap();

    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    assert!(fman_state.connect_calls.load(Ordering::SeqCst) > usize::from(MIN_FEDERATION_SIZE));
}

#[tokio::test]
async fn selected_availability_transport_failure_reconnects_before_payment() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    fman_state
        .availability_transport_failures_remaining
        .store(1, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);

    client
        .pay_and_create(
            intent(),
            selection_approval(cap),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap();

    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    assert!(
        fman_state.availability_calls.load(Ordering::SeqCst) > usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn selected_quote_transport_failure_reconnects_before_payment() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    fman_state
        .quote_transport_failures_remaining
        .store(1, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);

    client
        .pay_and_create(
            intent(),
            selection_approval(cap),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap();

    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    assert!(fman_state.quote_calls.load(Ordering::SeqCst) > usize::from(MIN_FEDERATION_SIZE));
}

#[tokio::test]
async fn selected_remote_quote_error_cannot_impersonate_local_transport_failure() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig {
            reject_quote: true,
            ..FmanConfig::paid()
        },
    )
    .await;

    let error = client
        .pay_and_create(
            intent(),
            selection_approval(PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE)),
            payment_federation_id(),
            options(),
        )
        .await
        .expect_err("remote service error must return without transport retries");

    assert!(matches!(error, FiError::FleetManager { .. }), "{error}");
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "remote errors must not enter the local transport retry path",
    );
}

#[tokio::test]
async fn selected_acquisition_connection_failure_retries_before_output_generation() {
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    // The first seven connections obtain initial quotes. Fail the first
    // acquisition-barrier connection, while the selected flow is still
    // value-safe and must retry instead of returning a generic transport error.
    fman_state
        .connect_failure_on_call
        .store(usize::from(MIN_FEDERATION_SIZE) + 1, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);

    client
        .pay_and_create(
            intent(),
            selection_approval(cap),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap();

    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    assert!(fman_state.connect_calls.load(Ordering::SeqCst) > 2 * usize::from(MIN_FEDERATION_SIZE));
}

#[tokio::test]
async fn exhausted_selected_connection_retry_is_typed_and_returns_idle() {
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    fman_state
        .connect_failures_remaining
        .store(usize::MAX, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state,
        FmanConfig::paid(),
    )
    .await;

    let error = client
        .pay_and_create(
            intent(),
            selection_approval(1_000),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::SelectedFmanUnavailable
        )
    ));
    assert_eq!(client.status(), FiStatus::Idle);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn post_output_connection_failure_requires_exact_replay_not_replacement() {
    let database = MemDatabase::new().into_database();
    let (payments, _payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    fman_state
        .failed_create_indices
        .lock()
        .expect("test lock")
        .insert(0);
    let client = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);

    client
        .pay_and_create(
            intent(),
            selection_approval(cap),
            payment_federation_id(),
            options(),
        )
        .await
        .expect_err("one authenticated CreateSeat failure interrupts the run");
    assert!(formation(&client.status()).payment_outputs_started);

    fman_state
        .failed_create_indices
        .lock()
        .expect("test lock")
        .clear();
    fman_state
        .connect_failures_remaining
        .store(1, Ordering::SeqCst);
    let error = client.resume().await.unwrap_err();

    assert!(matches!(error, FiError::FleetManager { .. }), "{error}");
    assert!(formation(&client.status()).payment_outputs_started);
}

#[derive(Clone, Copy)]
enum ProvisionalReplacementQuote {
    None,
    Paid,
    Free,
}

async fn persist_provisional_replacement_for_test(
    client: &TestClient,
    valid_until: Timestamp,
    quote: ProvisionalReplacementQuote,
) -> (GuardianReplacementRequirements, u16) {
    let error = client
        .pay_and_create(
            intent(),
            selection_approval(PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE)),
            payment_federation_id(),
            options(),
        )
        .await
        .expect_err("the original selected guardian refuses its paid presentation");
    assert!(matches!(error, FiError::SeatRefused { .. }), "{error:?}");
    let status = client.status();
    let formation = formation(&status).clone();
    let FormationActionRequired::ReplaceGuardians(requirements) = formation
        .action_required
        .clone()
        .expect("terminal refusal requires one replacement")
    else {
        panic!("terminal refusal exposed the wrong recovery action")
    };
    let index = requirements.seats[0].index;
    let replacement_index = usize::from(MAX_FEDERATION_SIZE);
    client
        .inner
        .store
        .replace_guardians(
            &formation.formation_id,
            &requirements,
            &[(
                locator(replacement_index),
                crate::db::FmanAdmission::fresh_peer_badge(
                    test_fman_id(replacement_index),
                    test_peer_badge_verifier().provenance().into(),
                    valid_until,
                ),
            )],
            PAYMENT_AMOUNT_MSATS,
        )
        .await
        .expect("fresh replacement approval is persisted provisionally");

    let signed_quote = match quote {
        ProvisionalReplacementQuote::None => None,
        ProvisionalReplacementQuote::Paid => {
            Some(signed_quote(replacement_index, &formation.intent, [91; 32]))
        }
        ProvisionalReplacementQuote::Free => Some(signed_free_quote(
            replacement_index,
            &formation.intent,
            [92; 32],
        )),
    };
    if let Some(signed_quote) = signed_quote {
        client
            .inner
            .store
            .store_quote(&formation.formation_id, index, signed_quote)
            .await
            .expect("provisional replacement quote is durable before its effect");
    }
    (requirements, index)
}

async fn assert_replacement_preview_still_reachable(
    client: &TestClient,
    requirements: &GuardianReplacementRequirements,
) {
    assert_eq!(
        formation(&client.status()).action_required,
        Some(FormationActionRequired::ReplaceGuardians(
            requirements.clone()
        ))
    );
    match client
        .preview_fman_replacements(crate::FmanDiscoveryOptions::default())
        .await
    {
        Err(FiError::InsufficientFmanSeats { .. }) => {}
        Err(error) => panic!("fresh replacement preview was not reachable: {error:?}"),
        Ok(_) => panic!("empty test registry unexpectedly filled a replacement preview"),
    }
}

#[tokio::test]
async fn unavailable_provisional_replacement_reopens_with_fresh_preview_before_quote() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::paid()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let (requirements, _) = persist_provisional_replacement_for_test(
        &client,
        Timestamp(test_now_secs() + 120),
        ProvisionalReplacementQuote::None,
    )
    .await;
    let payment_calls = payment_state.create_calls.load(Ordering::SeqCst);
    let presentation_calls = fman_state.create_calls.load(Ordering::SeqCst);
    drop(client);

    let reopened = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig {
            accepting_seats: false,
            ..config
        },
    )
    .await;
    let error = reopened
        .resume()
        .await
        .expect_err("replacement is unavailable");
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::SelectedFmanUnavailable
        )
    ));
    drop(reopened);

    let reopened = open_client(database, payments, fman_state.clone(), config).await;
    assert_replacement_preview_still_reachable(&reopened, &requirements).await;
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        payment_calls
    );
    assert_eq!(
        fman_state.create_calls.load(Ordering::SeqCst),
        presentation_calls
    );
}

#[tokio::test]
async fn expired_paid_replacement_releases_exact_hold_and_reopens_with_fresh_preview() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::paid()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let (replacement_requirements, _) = persist_provisional_replacement_for_test(
        &client,
        Timestamp(test_now_secs().saturating_sub(1)),
        ProvisionalReplacementQuote::Paid,
    )
    .await;
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    let requirements = recovery
        .payment_requirements(TestIdentity::fi_id())
        .unwrap()
        .expect("the provisional paid quote has exact requirements");
    let authorizations = requirements
        .seats
        .iter()
        .map(|seat| QuoteAuthorization {
            index: seat.index,
            quote_id: seat.quote_id,
        })
        .collect::<Vec<_>>();
    client
        .inner
        .store
        .authorize_payments(
            &recovery.snapshot.formation_id,
            &requirements.authorization_id,
            &authorizations,
        )
        .await
        .unwrap();
    let verified_quotes = requirements
        .seats
        .iter()
        .map(|requirement| {
            let seat = recovery
                .seats
                .iter()
                .find(|seat| seat.progress.index == requirement.index)
                .expect("payment requirement names its seat");
            seat.signed_quote
                .as_ref()
                .expect("paid replacement quote is durable")
                .verify(&seat.progress.locator.service_pubkey)
                .expect("paid replacement quote verifies")
        })
        .collect::<Vec<_>>();
    let preflight = crate::ExactPaymentPreflight::new(&requirements, &verified_quotes).unwrap();
    let reservation_id =
        crate::db::payment_reservation_id(&recovery.snapshot.formation_id, &requirements);
    payments
        .reserve_payment_requirements(&reservation_id, &preflight)
        .await
        .unwrap();
    client
        .inner
        .store
        .record_payment_reservation(&recovery.snapshot.formation_id, &reservation_id)
        .await
        .unwrap();
    let payment_calls = payment_state.create_calls.load(Ordering::SeqCst);
    let presentation_calls = fman_state.create_calls.load(Ordering::SeqCst);

    let error = client
        .resume()
        .await
        .expect_err("replacement approval expired");
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(SelectionReauthorizationReason::PreviewExpired)
    ));
    assert!(
        !payment_state
            .reservations
            .lock()
            .expect("test lock")
            .contains_key(reservation_id.as_str()),
        "fresh preview is exposed only after the exact unstarted hold is released"
    );
    drop(client);

    let reopened = open_client(database, payments, fman_state.clone(), config).await;
    assert_replacement_preview_still_reachable(&reopened, &replacement_requirements).await;
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        payment_calls
    );
    assert_eq!(
        fman_state.create_calls.load(Ordering::SeqCst),
        presentation_calls
    );
}

#[tokio::test]
async fn verifier_drifted_free_replacement_reopens_with_fresh_preview_before_presentation() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::paid()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let (requirements, _) = persist_provisional_replacement_for_test(
        &client,
        Timestamp(test_now_secs() + 120),
        ProvisionalReplacementQuote::Free,
    )
    .await;
    let payment_calls = payment_state.create_calls.load(Ordering::SeqCst);
    let presentation_calls = fman_state.create_calls.load(Ordering::SeqCst);
    drop(client);

    let staging = peer_badge_verifier(ManifoldEnvironment::Staging);
    let reopened = open_client_with_verifier(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
        staging.clone(),
    )
    .await;
    let error = reopened
        .resume()
        .await
        .expect_err("verifier provenance drifted");
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::VerifierEnvironmentChanged
        )
    ));
    drop(reopened);

    let reopened =
        open_client_with_verifier(database, payments, fman_state.clone(), config, staging).await;
    assert_replacement_preview_still_reachable(&reopened, &requirements).await;
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        payment_calls
    );
    assert_eq!(
        fman_state.create_calls.load(Ordering::SeqCst),
        presentation_calls
    );
}

#[tokio::test]
async fn selected_free_refusal_survives_failed_abandon_and_replays_before_cleanup() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::given_away()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    client.inner.store.fail_abandon_once();

    let error = client
        .pay_and_create(
            intent(),
            selection_approval(1),
            payment_federation_id(),
            options(),
        )
        .await
        .expect_err("injected abandon failure retains exact refusal recovery");
    assert!(matches!(error, FiError::Storage(message) if message.contains("abandon failure")));
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    let refused = recovery
        .seats
        .iter()
        .find(|seat| seat.progress.seat_id.is_none())
        .expect("one refused free seat remains unaccepted");
    let refused_quote = refused
        .signed_quote
        .as_ref()
        .expect("failed abandon retains the exact refused quote")
        .verify(&refused.progress.locator.service_pubkey)
        .expect("retained free quote verifies");
    assert!(refused_quote.terms.payment.is_none());
    assert!(matches!(
        &refused.admission,
        crate::db::FmanAdmission::PeerBadge {
            state: crate::db::AdmissionState::EffectAuthorized {
                effect: crate::db::AdmissionEffect::FreePresentation,
                ..
            },
            ..
        }
    ));
    assert!(!recovery.payment_outputs_started);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    drop(client);

    let reopened = open_client(database, payments, fman_state, config).await;
    let error = reopened
        .resume()
        .await
        .expect_err("replayed refusal returns fresh-selection guidance");
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::SelectedFmanUnavailable
        )
    ));
    assert_eq!(reopened.status(), FiStatus::Idle);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selected_refusal_changes_only_the_proven_unsecured_guardian() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::paid()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let original_cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);

    let error = client
        .pay_and_create(
            intent(),
            selection_approval(original_cap),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, FiError::SeatRefused { .. }), "{error:?}");
    let interrupted = formation(&client.status()).clone();
    let FormationActionRequired::ReplaceGuardians(requirements) = interrupted
        .action_required
        .clone()
        .expect("a verified refusal requires subset replacement")
    else {
        panic!("selected refusal exposed the wrong recovery action")
    };
    assert_eq!(requirements.seats.len(), 1);
    let replaced_index = requirements.seats[0].index;
    for seat in &interrupted.seats {
        assert_eq!(
            seat.fman_name()
                .expect("a badge-vouched row derives a display name"),
            FmanName::from_fman_id(test_fman_id(usize::from(seat.index))),
            "snapshot row {} lost its badge-vouched FMan identity",
            seat.index,
        );
    }
    assert_eq!(
        requirements.seats[0].previous_fman_id,
        Some(test_fman_id(usize::from(replaced_index))),
        "the replacement row names the outgoing FMan",
    );
    let accepted_before = interrupted
        .seats
        .iter()
        .filter_map(|seat| seat.seat_id.clone().map(|seat_id| (seat.index, seat_id)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        accepted_before.len(),
        usize::from(MIN_FEDERATION_SIZE) - 1,
        "every paid and accepted sibling is pinned"
    );

    drop(client);
    let client = open_client(database, payments, fman_state.clone(), config).await;
    let reopened_status = client.status();
    let reopened = formation(&reopened_status);
    assert_eq!(
        reopened.action_required,
        Some(FormationActionRequired::ReplaceGuardians(
            requirements.clone()
        )),
        "the exact subset-replacement requirement survives process restart",
    );
    for (index, seat_id) in &accepted_before {
        assert_eq!(
            reopened.seats[usize::from(*index)].seat_id.as_ref(),
            Some(seat_id),
            "restart lost accepted sibling {index}",
        );
    }

    let retained_index = *accepted_before
        .keys()
        .next()
        .expect("one accepted sibling remains");
    let colliding_approval = FmanReplacementApproval {
        requirements: requirements.clone(),
        verifier_provenance: test_peer_badge_verifier().provenance(),
        seats: vec![crate::selection::ApprovedFmanSeat {
            fman_id: test_fman_id(usize::from(MAX_FEDERATION_SIZE)),
            locator: reopened.seats[usize::from(retained_index)].locator.clone(),
        }],
        max_total_msats: PAYMENT_AMOUNT_MSATS,
        valid_until: Timestamp(test_now_secs() + 120),
    };
    let error = client
        .apply_fman_replacements(colliding_approval, options())
        .await
        .expect_err("a distinct author cannot reuse a retained signing authority");
    assert!(
        matches!(&error, FiError::InvalidFleetManagers(message)
            if message.contains("service signing key")),
        "unexpected collision error: {error:?}",
    );
    assert_eq!(
        formation(&client.status()).action_required,
        Some(FormationActionRequired::ReplaceGuardians(
            requirements.clone()
        )),
        "rejected apply must leave the exact replacement requirement intact",
    );

    let approval = FmanReplacementApproval {
        requirements,
        verifier_provenance: test_peer_badge_verifier().provenance(),
        seats: vec![crate::selection::ApprovedFmanSeat {
            fman_id: test_fman_id(usize::from(MAX_FEDERATION_SIZE)),
            locator: locator(usize::from(MAX_FEDERATION_SIZE)),
        }],
        // Model an advertisement below the eventual real quote so the test
        // also exercises the exact post-replacement authorization action.
        max_total_msats: 1,
        valid_until: Timestamp(test_now_secs() + 120),
    };
    client
        .apply_fman_replacements(approval, options())
        .await
        .unwrap();

    let replaced_status = client.status();
    let replaced = formation(&replaced_status);
    assert_eq!(replaced.phase, FormationPhase::AwaitingPaymentReadiness);
    let Some(FormationActionRequired::AuthorizePayments(parked_payment)) =
        replaced.action_required.clone()
    else {
        panic!("over-cap replacement quote must park an authorization action")
    };
    assert_eq!(
        parked_payment
            .seats
            .iter()
            .map(|seat| seat.fman_id)
            .collect::<Vec<_>>(),
        vec![Some(test_fman_id(usize::from(MAX_FEDERATION_SIZE)))],
        "the parked payment names the incoming FMan",
    );
    assert_eq!(
        replaced.seats[usize::from(replaced_index)].locator,
        locator(usize::from(MAX_FEDERATION_SIZE))
    );
    assert_eq!(
        replaced.seats[usize::from(replaced_index)].fman_id,
        Some(test_fman_id(usize::from(MAX_FEDERATION_SIZE))),
        "the replaced row exposes the incoming FMan's identity",
    );
    for (index, seat_id) in accepted_before {
        assert_eq!(
            replaced.seats[usize::from(index)].seat_id.as_ref(),
            Some(&seat_id),
            "an accepted sibling changed while replacing row {replaced_index}"
        );
    }
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "the exact over-cap replacement quote moves no additional value"
    );

    let authorization_id = payment_requirements(&replaced_status)
        .authorization_id
        .clone();
    client
        .authorize_payments(authorization_id, options())
        .await
        .unwrap();
    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE) + 1,
        "only the renewed replacement quote may generate another payment output",
    );
    let calls_after_completion = fman_state.create_calls.load(Ordering::SeqCst);
    client.resume().await.unwrap();
    assert_eq!(
        fman_state.create_calls.load(Ordering::SeqCst),
        calls_after_completion,
        "formed replay must not repeat any accepted or replacement CreateSeat",
    );
}

#[tokio::test]
async fn selected_paid_replacement_reopens_after_authorized_output_without_new_spend() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::paid()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
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
    let FormationActionRequired::ReplaceGuardians(requirements) = formation(&client.status())
        .action_required
        .clone()
        .expect("the refused selected seat requires replacement")
    else {
        panic!("selected refusal exposed the wrong recovery action")
    };
    assert_eq!(requirements.seats.len(), 1);
    let replaced_index = requirements.seats[0].index;

    let next_funding_call = payment_state.create_calls.load(Ordering::SeqCst) + 1;
    payment_state
        .hang_funding_on_call
        .store(next_funding_call, Ordering::SeqCst);
    let funding_started = payment_state.funding_started.notified();
    let approval = FmanReplacementApproval {
        requirements,
        verifier_provenance: test_peer_badge_verifier().provenance(),
        seats: vec![crate::selection::ApprovedFmanSeat {
            fman_id: test_fman_id(usize::from(MAX_FEDERATION_SIZE)),
            locator: locator(usize::from(MAX_FEDERATION_SIZE)),
        }],
        max_total_msats: PAYMENT_AMOUNT_MSATS,
        valid_until: Timestamp(test_now_secs() + 120),
    };
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .apply_fman_replacements(approval, long_request_options())
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), funding_started)
        .await
        .expect("selected replacement reaches the injected lost wallet result");

    let replacement_quote = *payment_state
        .created_quotes
        .lock()
        .expect("test lock")
        .last()
        .expect("the replacement wallet output is journaled");
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    let interrupted = &recovery.seats[usize::from(replaced_index)];
    assert!(interrupted.progress.seat_id.is_none());
    assert!(matches!(
        &interrupted.admission,
        crate::db::FmanAdmission::PeerBadge {
            state: crate::db::AdmissionState::EffectAuthorized {
                quote_id,
                effect: crate::db::AdmissionEffect::PaidOutput,
            },
            ..
        } if *quote_id == replacement_quote
    ));
    let replacement_signed_quote = interrupted
        .signed_quote
        .clone()
        .expect("the exact replacement quote remains durable");
    assert!(
        fman_state
            .create_records
            .lock()
            .expect("test lock")
            .iter()
            .all(|record| record.quote_id != replacement_quote),
        "the crash point precedes replacement-seat presentation",
    );

    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let spends_before_resume = payment_state.create_calls.load(Ordering::SeqCst);
    assert_eq!(spends_before_resume, next_funding_call);
    let quote_calls_before_resume = fman_state.quote_calls.load(Ordering::SeqCst);
    drop(client);

    let reopened = open_client_with_verifier(
        database,
        payments,
        fman_state.clone(),
        config,
        peer_badge_verifier(ManifoldEnvironment::Staging),
    )
    .await;
    reopened.resume().await.unwrap();

    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        spends_before_resume,
        "recovering a journaled replacement output must not spend again",
    );
    assert_eq!(
        fman_state.quote_calls.load(Ordering::SeqCst),
        quote_calls_before_resume,
        "authorized recovery must not obtain a fresh replacement quote",
    );
    let records = fman_state.create_records.lock().expect("test lock");
    let replacement_presentations = records
        .iter()
        .filter(|record| record.quote_id == replacement_quote)
        .collect::<Vec<_>>();
    assert_eq!(replacement_presentations.len(), 1);
    assert_eq!(
        replacement_presentations[0].signed_quote,
        replacement_signed_quote,
    );
}

#[tokio::test]
async fn selected_payer_failure_returns_to_idle_and_live_approval_can_retry() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    payment_state
        .insufficient_funds
        .store(true, Ordering::SeqCst);
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);
    let approval = selection_approval(cap);

    let error = client
        .pay_and_create(
            intent(),
            approval.clone(),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::SelectedPayerInsufficientFunds
        )
    ));
    assert_eq!(client.status(), FiStatus::Idle);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);

    payment_state
        .insufficient_funds
        .store(false, Ordering::SeqCst);
    client
        .pay_and_create(intent(), approval, payment_federation_id(), options())
        .await
        .unwrap();
    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
}

#[tokio::test]
async fn selected_lost_reservation_result_reconstructs_the_same_wallet_journal() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    payment_state
        .lose_reservation_result_on_call
        .store(1, Ordering::SeqCst);
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);

    let error = client
        .pay_and_create(
            intent(),
            selection_approval(cap),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, FiError::Payment(message)
            if message.contains("lost after journaling")),
        "{error:?}"
    );
    let interrupted_status = client.status();
    let interrupted = formation(&interrupted_status);
    assert!(!interrupted.payment_outputs_started);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        payment_state.reservations.lock().expect("test lock").len(),
        1,
        "the ambiguous result retains the wallet's durable same-id journal",
    );
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(
        recovery.payment_reservation_id.is_none(),
        "the injected response loss precedes the FI reservation checkpoint",
    );
    drop(client);

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    reopened
        .resume()
        .await
        .expect("resume reconstructs the wallet capability under the same id");
    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
    assert_eq!(
        payment_state.reservations.lock().expect("test lock").len(),
        1,
        "recovery reuses the journal instead of creating another reservation",
    );
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        1,
        "normal resume adopts the existing journal instead of reserving again",
    );
    assert!(
        payment_state
            .reservation_recover_calls
            .load(Ordering::SeqCst)
            >= 2,
        "normal resume probes the deterministic id before continuing",
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
    );
}

#[tokio::test]
async fn selected_same_id_reservation_binding_mismatch_never_returns_idle() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    payment_state
        .lose_reservation_result_on_call
        .store(1, Ordering::SeqCst);
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);
    assert!(matches!(
        client
            .pay_and_create(
                intent(),
                selection_approval(cap),
                payment_federation_id(),
                options(),
            )
            .await,
        Err(FiError::Payment(_))
    ));
    {
        let mut reservations = payment_state.reservations.lock().expect("test lock");
        let journal = reservations
            .values_mut()
            .next()
            .expect("the response-loss hook persisted one journal");
        journal.quote_ids[0] = QuoteId([0xff; 32]);
    }
    drop(client);

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let error = reopened.resume().await.unwrap_err();
    assert!(
        matches!(&error, FiError::Payment(message)
            if message.contains("different exact plan")),
        "{error:?}"
    );
    assert!(
        matches!(reopened.status(), FiStatus::Formation(_)),
        "a binding failure has no proof that the durable journal is absent",
    );
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        payment_state.reservations.lock().expect("test lock").len(),
        1,
    );
}

#[tokio::test]
async fn post_refresh_funding_failure_precedes_the_output_tombstone() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    // Reservation is the final exact aggregate barrier after same-terms quote
    // refresh and immediately before output generation.
    payment_state
        .fail_readiness_on_call
        .store(1, Ordering::SeqCst);
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);

    let error = client
        .pay_and_create(
            intent(),
            selection_approval(cap),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::SelectedPayerInsufficientFunds
        )
    ));
    assert_eq!(payment_state.readiness_calls.load(Ordering::SeqCst), 1);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(client.status(), FiStatus::Idle);
}

#[tokio::test]
async fn selected_quote_over_cap_returns_durable_idle_before_outputs() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let exact_total = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);
    let mut observed = client.observe();
    let operation = client.pay_and_create(
        intent(),
        selection_approval(exact_total - 1),
        payment_federation_id(),
        options(),
    );
    tokio::pin!(operation);
    let mut exposed_legacy_authorization = false;

    let error = loop {
        tokio::select! {
            result = &mut operation => break result.unwrap_err(),
            changed = observed.changed() => {
                changed.expect("FI watch remains open");
                exposed_legacy_authorization |= matches!(
                    &*observed.borrow_and_update(),
                    FiStatus::Formation(snapshot)
                        if matches!(
                            snapshot.action_required.as_ref(),
                            Some(FormationActionRequired::AuthorizePayments(_))
                        )
                );
            }
        }
    };

    assert!(
        matches!(
            error,
            FiError::SelectionReauthorizationRequired(
                SelectionReauthorizationReason::QuoteTotalExceedsLimit
            )
        ),
        "unexpected error: {error:?}"
    );
    assert_eq!(client.status(), FiStatus::Idle);
    assert!(
        !exposed_legacy_authorization,
        "selected Pay-and-create must never publish the legacy second authorization action"
    );
    assert_eq!(payment_state.readiness_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn crashed_selected_over_cap_record_reopens_without_legacy_action_and_cleans_on_resume() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let exact_total = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);
    let resolved = intent()
        .with_max_total_msats(exact_total - 1)
        .unwrap()
        .resolve_for_dkg(
            FederationName("Crash Recovery".to_owned()),
            fedimintd_version().dkg_version(),
        )
        .unwrap();
    let formation_id = FormationId("selected-over-cap-crash".to_owned());
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            formation_id.clone(),
            resolved.clone(),
            (0..MIN_FEDERATION_SIZE)
                .map(|index| selected_initial_seat(index, test_now_secs() + 120))
                .collect(),
            crate::db::FormationCreationMode::Selected {
                payment_federation_id: Some(payment_federation_id()),
            },
            None,
        )
        .await
        .unwrap();
    for index in 0..MIN_FEDERATION_SIZE {
        client
            .inner
            .store
            .store_quote(
                &formation_id,
                index,
                signed_quote(
                    usize::from(index),
                    &resolved,
                    [u8::try_from(index).unwrap() + 1; 32],
                ),
            )
            .await
            .unwrap();
    }
    drop(client);

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let status = reopened.status();
    let snapshot = formation(&status);
    assert!(snapshot.action_required.is_none());
    let error = reopened.resume().await.unwrap_err();
    assert!(
        matches!(
            error,
            FiError::SelectionReauthorizationRequired(
                SelectionReauthorizationReason::QuoteTotalExceedsLimit
            )
        ),
        "unexpected error: {error:?}"
    );
    assert_eq!(reopened.status(), FiStatus::Idle);
}

#[tokio::test]
async fn crashed_selected_record_cannot_resume_after_its_preview_expires() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let resolved = intent()
        .with_max_total_msats(1_000)
        .unwrap()
        .resolve_for_dkg(
            FederationName("Expired Crash".to_owned()),
            fedimintd_version().dkg_version(),
        )
        .unwrap();
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            FormationId("selected-expired-crash".to_owned()),
            resolved,
            (0..MIN_FEDERATION_SIZE)
                .map(|index| selected_initial_seat(index, test_now_secs().saturating_sub(1)))
                .collect(),
            crate::db::FormationCreationMode::Selected {
                payment_federation_id: Some(payment_federation_id()),
            },
            None,
        )
        .await
        .unwrap();
    drop(client);

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    assert!(formation(&reopened.status()).action_required.is_none());
    let error = reopened.resume().await.unwrap_err();
    assert!(
        matches!(
            error,
            FiError::SelectionReauthorizationRequired(
                SelectionReauthorizationReason::PreviewExpired
            )
        ),
        "unexpected error: {error:?}"
    );
    assert_eq!(reopened.status(), FiStatus::Idle);
}

#[tokio::test]
async fn selected_reopen_under_changed_verifier_requires_fresh_pre_output_authority() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let formation_id = FormationId("selected-verifier-drift-pre-output".to_owned());
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            formation_id,
            intent()
                .with_max_total_msats(1_000)
                .unwrap()
                .resolve_for_dkg(
                    FederationName("Verifier Drift".to_owned()),
                    fedimintd_version().dkg_version(),
                )
                .unwrap(),
            (0..MIN_FEDERATION_SIZE)
                .map(|index| selected_initial_seat(index, test_now_secs() + 120))
                .collect(),
            crate::db::FormationCreationMode::Selected {
                payment_federation_id: Some(payment_federation_id()),
            },
            None,
        )
        .await
        .unwrap();
    drop(client);

    let reopened = open_client_with_verifier(
        database,
        payments,
        fman_state,
        FmanConfig::paid(),
        peer_badge_verifier(ManifoldEnvironment::Staging),
    )
    .await;
    let error = reopened.resume().await.unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::VerifierEnvironmentChanged
        )
    ));
    assert_eq!(payment_state.readiness_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selected_reopen_after_output_tombstone_uses_durable_admission_provenance() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let formation_id = FormationId("selected-verifier-drift-post-output".to_owned());
    let resolved = intent()
        .with_max_total_msats(1_000)
        .unwrap()
        .resolve_for_dkg(
            FederationName("Verifier Drift Recovery".to_owned()),
            fedimintd_version().dkg_version(),
        )
        .unwrap();
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            formation_id.clone(),
            resolved.clone(),
            (0..MIN_FEDERATION_SIZE)
                .map(|index| selected_initial_seat(index, test_now_secs() + 120))
                .collect(),
            crate::db::FormationCreationMode::Selected {
                payment_federation_id: Some(payment_federation_id()),
            },
            None,
        )
        .await
        .unwrap();
    for index in 0..MIN_FEDERATION_SIZE {
        client
            .inner
            .store
            .store_quote(
                &formation_id,
                index,
                signed_quote(
                    usize::from(index),
                    &resolved,
                    [120 + u8::try_from(index).unwrap(); 32],
                ),
            )
            .await
            .unwrap();
    }
    let requirements = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    )
    .payment_requirements(TestIdentity::fi_id())
    .unwrap()
    .expect("complete selected aggregate");
    client
        .inner
        .store
        .authorize_payments(
            &formation_id,
            &requirements.authorization_id,
            &requirements
                .seats
                .iter()
                .map(|requirement| QuoteAuthorization {
                    index: requirement.index,
                    quote_id: requirement.quote_id,
                })
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    let reservation_id = crate::db::payment_reservation_id(&formation_id, &requirements);
    payment_state
        .reservations
        .lock()
        .expect("test lock")
        .insert(
            reservation_id.as_str().to_owned(),
            TestReservation {
                quote_ids: requirements
                    .seats
                    .iter()
                    .map(|seat| seat.quote_id)
                    .collect(),
                started: HashSet::new(),
                terminal: HashSet::new(),
                released: HashSet::new(),
            },
        );
    client
        .inner
        .store
        .record_payment_reservation(&formation_id, &reservation_id)
        .await
        .unwrap();
    let lease = client
        .inner
        .store
        .acquire_driver_lease(
            options().lease_duration(),
            options().lease_renewal_duration(),
        )
        .await
        .unwrap();
    lease
        .arm_payment_outputs_started(
            &formation_id,
            test_peer_badge_verifier().provenance().into(),
        )
        .await
        .unwrap();
    client
        .inner
        .store
        .release_driver_lease(lease)
        .await
        .unwrap();
    drop(client);

    let reopened = open_client_with_verifier(
        database,
        payments,
        fman_state,
        FmanConfig::paid(),
        peer_badge_verifier(ManifoldEnvironment::Staging),
    )
    .await;
    reopened
        .resume()
        .await
        .expect("post-output recovery uses persisted admission facts");
    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
    assert!(payment_state.create_calls.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn expired_selected_cleanup_reconstructs_and_releases_wallet_reservation_before_idle() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let resolved = intent()
        .with_max_total_msats(1_000)
        .unwrap()
        .resolve_for_dkg(
            FederationName("Expired Reserved Crash".to_owned()),
            fedimintd_version().dkg_version(),
        )
        .unwrap();
    let formation_id = FormationId("selected-expired-reserved-crash".to_owned());
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            formation_id.clone(),
            resolved.clone(),
            (0..MIN_FEDERATION_SIZE)
                .map(|index| selected_initial_seat(index, test_now_secs().saturating_sub(1)))
                .collect(),
            crate::db::FormationCreationMode::Selected {
                payment_federation_id: Some(payment_federation_id()),
            },
            None,
        )
        .await
        .unwrap();
    for index in 0..MIN_FEDERATION_SIZE {
        client
            .inner
            .store
            .store_quote(
                &formation_id,
                index,
                signed_quote(
                    usize::from(index),
                    &resolved,
                    [90 + u8::try_from(index).unwrap(); 32],
                ),
            )
            .await
            .unwrap();
    }
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    let requirements = recovery
        .payment_requirements(TestIdentity::fi_id())
        .unwrap()
        .expect("exact aggregate");
    let authorizations = requirements
        .seats
        .iter()
        .map(|seat| QuoteAuthorization {
            index: seat.index,
            quote_id: seat.quote_id,
        })
        .collect::<Vec<_>>();
    client
        .inner
        .store
        .authorize_payments(
            &formation_id,
            &requirements.authorization_id,
            &authorizations,
        )
        .await
        .unwrap();
    let reservation_id = crate::db::payment_reservation_id(&formation_id, &requirements);
    payment_state
        .reservations
        .lock()
        .expect("test lock")
        .insert(
            reservation_id.as_str().to_owned(),
            TestReservation {
                quote_ids: requirements
                    .seats
                    .iter()
                    .map(|seat| seat.quote_id)
                    .collect(),
                started: HashSet::new(),
                terminal: HashSet::new(),
                released: HashSet::new(),
            },
        );
    client
        .inner
        .store
        .record_payment_reservation(&formation_id, &reservation_id)
        .await
        .unwrap();
    drop(client);

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let error = reopened.resume().await.unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(SelectionReauthorizationReason::PreviewExpired)
    ));
    assert_eq!(reopened.status(), FiStatus::Idle);
    assert!(
        payment_state
            .reservations
            .lock()
            .expect("test lock")
            .is_empty(),
        "FI state reached Idle only after the wallet hold was removed",
    );
    assert_eq!(payment_state.readiness_calls.load(Ordering::SeqCst), 0);
}

async fn seed_lost_selected_reservation_result(
    client: &TestClient,
    payment_state: &Arc<PaymentState>,
    formation_id: FormationId,
    valid_until: u64,
) {
    let resolved = intent()
        .with_max_total_msats(
            PAYMENT_AMOUNT_MSATS
                .checked_mul(u64::from(MIN_FEDERATION_SIZE))
                .expect("test aggregate fits"),
        )
        .unwrap()
        .resolve_for_dkg(
            FederationName("Lost Reservation Result".to_owned()),
            fedimintd_version().dkg_version(),
        )
        .unwrap();
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            formation_id.clone(),
            resolved.clone(),
            (0..MIN_FEDERATION_SIZE)
                .map(|index| selected_initial_seat(index, valid_until))
                .collect(),
            crate::db::FormationCreationMode::Selected {
                payment_federation_id: Some(payment_federation_id()),
            },
            None,
        )
        .await
        .unwrap();
    for index in 0..MIN_FEDERATION_SIZE {
        client
            .inner
            .store
            .store_quote(
                &formation_id,
                index,
                signed_quote(
                    usize::from(index),
                    &resolved,
                    [110 + u8::try_from(index).unwrap(); 32],
                ),
            )
            .await
            .unwrap();
    }
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    let requirements = recovery
        .payment_requirements(TestIdentity::fi_id())
        .unwrap()
        .expect("exact aggregate");
    client
        .inner
        .store
        .authorize_payments(
            &formation_id,
            &requirements.authorization_id,
            &requirements
                .seats
                .iter()
                .map(|seat| QuoteAuthorization {
                    index: seat.index,
                    quote_id: seat.quote_id,
                })
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    let reservation_id = crate::db::payment_reservation_id(&formation_id, &requirements);
    payment_state
        .reservations
        .lock()
        .expect("test lock")
        .insert(
            reservation_id.as_str().to_owned(),
            TestReservation {
                quote_ids: requirements
                    .seats
                    .iter()
                    .map(|seat| seat.quote_id)
                    .collect(),
                started: HashSet::new(),
                terminal: HashSet::new(),
                released: HashSet::new(),
            },
        );
    let persisted = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(
        persisted.payment_reservation_id.is_none(),
        "the wallet commit is lost before the FI reservation-id checkpoint",
    );
}

async fn seed_lost_post_output_replacement_reservation_result(
    client: &TestClient,
    payment_state: &Arc<PaymentState>,
    valid_until: Timestamp,
    wallet_journal_present: bool,
) -> (GuardianReplacementRequirements, crate::PaymentReservationId) {
    let (replacement_requirements, _) = persist_provisional_replacement_for_test(
        client,
        valid_until,
        ProvisionalReplacementQuote::Paid,
    )
    .await;
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(recovery.payment_outputs_started);
    let requirements = recovery
        .payment_requirements(TestIdentity::fi_id())
        .unwrap()
        .expect("the provisional paid replacement has exact requirements");
    client
        .inner
        .store
        .authorize_payments(
            &recovery.snapshot.formation_id,
            &requirements.authorization_id,
            &requirements
                .seats
                .iter()
                .map(|seat| QuoteAuthorization {
                    index: seat.index,
                    quote_id: seat.quote_id,
                })
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    let reservation_id =
        crate::db::payment_reservation_id(&recovery.snapshot.formation_id, &requirements);
    if wallet_journal_present {
        payment_state
            .reservations
            .lock()
            .expect("test lock")
            .insert(
                reservation_id.as_str().to_owned(),
                TestReservation {
                    quote_ids: requirements
                        .seats
                        .iter()
                        .map(|seat| seat.quote_id)
                        .collect(),
                    started: HashSet::new(),
                    terminal: HashSet::new(),
                    released: HashSet::new(),
                },
            );
    }
    let persisted = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(
        persisted.payment_reservation_id.is_none(),
        "the replacement wallet commit is lost before the FI reservation-id checkpoint",
    );
    (replacement_requirements, reservation_id)
}

#[tokio::test]
async fn lost_reserve_result_releases_after_preview_expiry_without_creating_a_journal() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    seed_lost_selected_reservation_result(
        &client,
        &payment_state,
        FormationId("lost-reserve-expired-preview".to_owned()),
        test_now_secs().saturating_sub(1),
    )
    .await;
    drop(client);

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let error = reopened.resume().await.unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(SelectionReauthorizationReason::PreviewExpired)
    ));
    assert_eq!(reopened.status(), FiStatus::Idle);
    assert_eq!(payment_state.whole_release_calls.load(Ordering::SeqCst), 1);
    assert_eq!(payment_state.readiness_calls.load(Ordering::SeqCst), 0);
    assert!(
        payment_state
            .reservations
            .lock()
            .expect("test lock")
            .is_empty(),
        "the exact existing journal is released before FI state reaches Idle",
    );
}

#[tokio::test]
async fn lost_reserve_result_releases_after_verifier_drift_without_creating_a_journal() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    seed_lost_selected_reservation_result(
        &client,
        &payment_state,
        FormationId("lost-reserve-verifier-drift".to_owned()),
        test_now_secs() + 120,
    )
    .await;
    drop(client);

    let reopened = open_client_with_verifier(
        database,
        payments,
        fman_state,
        FmanConfig::paid(),
        peer_badge_verifier(ManifoldEnvironment::Staging),
    )
    .await;
    let error = reopened.resume().await.unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::VerifierEnvironmentChanged
        )
    ));
    assert_eq!(reopened.status(), FiStatus::Idle);
    assert_eq!(payment_state.whole_release_calls.load(Ordering::SeqCst), 1);
    assert_eq!(payment_state.readiness_calls.load(Ordering::SeqCst), 0);
    assert!(
        payment_state
            .reservations
            .lock()
            .expect("test lock")
            .is_empty(),
        "verifier drift cannot strand an ambiguously committed wallet journal",
    );
}

#[tokio::test]
async fn lost_replacement_reserve_result_releases_after_preview_expiry() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::paid()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let (requirements, replacement_reservation_id) =
        seed_lost_post_output_replacement_reservation_result(
            &client,
            &payment_state,
            Timestamp(test_now_secs().saturating_sub(1)),
            true,
        )
        .await;
    let readiness_calls = payment_state.readiness_calls.load(Ordering::SeqCst);
    drop(client);

    let reopened = open_client(database, payments, fman_state, config).await;
    let error = reopened.resume().await.unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(SelectionReauthorizationReason::PreviewExpired)
    ));
    assert_replacement_preview_still_reachable(&reopened, &requirements).await;
    assert_eq!(payment_state.whole_release_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        readiness_calls,
        "recovery probes but never recreates the replacement wallet journal",
    );
    assert!(
        !payment_state
            .reservations
            .lock()
            .expect("test lock")
            .contains_key(replacement_reservation_id.as_str()),
        "the exact replacement hold is released before fresh preview",
    );
}

#[tokio::test]
async fn lost_replacement_reserve_result_releases_after_verifier_drift() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::paid()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let (requirements, replacement_reservation_id) =
        seed_lost_post_output_replacement_reservation_result(
            &client,
            &payment_state,
            Timestamp(test_now_secs() + 120),
            true,
        )
        .await;
    let readiness_calls = payment_state.readiness_calls.load(Ordering::SeqCst);
    drop(client);

    let reopened = open_client_with_verifier(
        database,
        payments,
        fman_state,
        config,
        peer_badge_verifier(ManifoldEnvironment::Staging),
    )
    .await;
    let error = reopened.resume().await.unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::VerifierEnvironmentChanged
        )
    ));
    assert_replacement_preview_still_reachable(&reopened, &requirements).await;
    assert_eq!(payment_state.whole_release_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        readiness_calls,
    );
    assert!(
        !payment_state
            .reservations
            .lock()
            .expect("test lock")
            .contains_key(replacement_reservation_id.as_str()),
    );
}

#[tokio::test]
async fn absent_replacement_reservation_restores_without_wallet_release() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::paid()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let (requirements, replacement_reservation_id) =
        seed_lost_post_output_replacement_reservation_result(
            &client,
            &payment_state,
            Timestamp(test_now_secs().saturating_sub(1)),
            false,
        )
        .await;
    let readiness_calls = payment_state.readiness_calls.load(Ordering::SeqCst);
    drop(client);

    let reopened = open_client(database, payments, fman_state, config).await;
    let error = reopened.resume().await.unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(SelectionReauthorizationReason::PreviewExpired)
    ));
    assert_replacement_preview_still_reachable(&reopened, &requirements).await;
    assert_eq!(payment_state.whole_release_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        readiness_calls,
    );
    assert!(
        !payment_state
            .reservations
            .lock()
            .expect("test lock")
            .contains_key(replacement_reservation_id.as_str()),
        "an absent replacement reservation remains absent",
    );
}

/// Seed a pre-output selected paid formation whose exact aggregate is
/// authorized, journaled by the wallet, and durably checkpointed as the FI
/// reservation, with no release commitment recorded yet.
async fn seed_selected_recorded_reservation(
    client: &TestClient,
    payment_state: &Arc<PaymentState>,
    formation_id: FormationId,
    valid_until: u64,
) -> crate::PaymentReservationId {
    let resolved = intent()
        .with_max_total_msats(
            PAYMENT_AMOUNT_MSATS
                .checked_mul(u64::from(MIN_FEDERATION_SIZE))
                .expect("test aggregate fits"),
        )
        .unwrap()
        .resolve_for_dkg(
            FederationName("Recorded Reservation".to_owned()),
            fedimintd_version().dkg_version(),
        )
        .unwrap();
    client
        .inner
        .store
        .initialize(
            TestIdentity::fi_id(),
            formation_id.clone(),
            resolved.clone(),
            (0..MIN_FEDERATION_SIZE)
                .map(|index| selected_initial_seat(index, valid_until))
                .collect(),
            crate::db::FormationCreationMode::Selected {
                payment_federation_id: Some(payment_federation_id()),
            },
            None,
        )
        .await
        .unwrap();
    for index in 0..MIN_FEDERATION_SIZE {
        client
            .inner
            .store
            .store_quote(
                &formation_id,
                index,
                signed_quote(
                    usize::from(index),
                    &resolved,
                    [130 + u8::try_from(index).unwrap(); 32],
                ),
            )
            .await
            .unwrap();
    }
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    let requirements = recovery
        .payment_requirements(TestIdentity::fi_id())
        .unwrap()
        .expect("exact aggregate");
    client
        .inner
        .store
        .authorize_payments(
            &formation_id,
            &requirements.authorization_id,
            &requirements
                .seats
                .iter()
                .map(|seat| QuoteAuthorization {
                    index: seat.index,
                    quote_id: seat.quote_id,
                })
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    let reservation_id = crate::db::payment_reservation_id(&formation_id, &requirements);
    payment_state
        .reservations
        .lock()
        .expect("test lock")
        .insert(
            reservation_id.as_str().to_owned(),
            TestReservation {
                quote_ids: requirements
                    .seats
                    .iter()
                    .map(|seat| seat.quote_id)
                    .collect(),
                started: HashSet::new(),
                terminal: HashSet::new(),
                released: HashSet::new(),
            },
        );
    client
        .inner
        .store
        .record_payment_reservation(&formation_id, &reservation_id)
        .await
        .unwrap();
    let persisted = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        persisted.payment_reservation_id.as_ref(),
        Some(&reservation_id),
        "the seeded reservation checkpoint is durable",
    );
    assert!(
        !persisted.payment_reservation_release_intended,
        "no release commitment exists before the interrupted cleanup",
    );
    reservation_id
}

#[tokio::test]
async fn interrupted_pre_output_abandon_completes_after_wallet_release() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let formation_id = FormationId("interrupted-abandon-retry".to_owned());
    let reservation_id = seed_selected_recorded_reservation(
        &client,
        &payment_state,
        formation_id.clone(),
        test_now_secs() + 120,
    )
    .await;
    // The injected wipe failure returns after the wallet release committed,
    // which is exactly the state a crash between the two would leave behind.
    client.inner.store.fail_abandon_once();

    let error = client
        .abandon_formation(options())
        .await
        .expect_err("the wipe is interrupted after the wallet release");
    assert!(
        matches!(&error, FiError::Storage(message) if message.contains("abandon failure")),
        "{error:?}"
    );
    assert_eq!(payment_state.whole_release_calls.load(Ordering::SeqCst), 1);
    assert!(
        payment_state
            .reservations
            .lock()
            .expect("test lock")
            .is_empty(),
        "the wallet half of the abandon completed before the interruption",
    );
    let interrupted = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        interrupted.payment_reservation_id.as_ref(),
        Some(&reservation_id),
        "the interrupted wipe retains the durable reservation id",
    );
    assert!(
        interrupted.payment_reservation_release_intended,
        "the release commitment was durably recorded before the wallet call",
    );
    drop(client);

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    reopened
        .abandon_formation(options())
        .await
        .expect("the abandon retry completes the interrupted wipe under the commitment");
    assert_eq!(reopened.status(), FiStatus::Idle);
    assert!(matches!(
        reopened
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
        FiRecovery::Idle
    ));
    assert_eq!(
        payment_state.whole_release_calls.load(Ordering::SeqCst),
        1,
        "expected wallet absence completes the wipe without a second release",
    );
    assert_eq!(
        payment_state
            .reservation_recover_calls
            .load(Ordering::SeqCst),
        2,
        "each abandon attempt probes the deterministic id exactly once",
    );
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        0,
        "no cleanup path may recreate the released wallet journal",
    );
    assert!(
        payment_state
            .reservations
            .lock()
            .expect("test lock")
            .is_empty()
    );
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn interrupted_pre_output_abandon_completes_on_resume() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let reservation_id = seed_selected_recorded_reservation(
        &client,
        &payment_state,
        FormationId("interrupted-abandon-resume".to_owned()),
        test_now_secs() + 120,
    )
    .await;
    client.inner.store.fail_abandon_once();
    let error = client
        .abandon_formation(options())
        .await
        .expect_err("the wipe is interrupted after the wallet release");
    assert!(
        matches!(&error, FiError::Storage(message) if message.contains("abandon failure")),
        "{error:?}"
    );
    let interrupted = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        interrupted.payment_reservation_id.as_ref(),
        Some(&reservation_id)
    );
    assert!(interrupted.payment_reservation_release_intended);
    drop(client);

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    reopened
        .resume()
        .await
        .expect("resume completes the interrupted abandon under the commitment");
    assert_eq!(reopened.status(), FiStatus::Idle);
    assert!(matches!(
        reopened
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
        FiRecovery::Idle
    ));
    assert_eq!(
        payment_state.whole_release_calls.load(Ordering::SeqCst),
        1,
        "resume must not release the already-released reservation again",
    );
    assert_eq!(
        payment_state
            .reservation_recover_calls
            .load(Ordering::SeqCst),
        2,
        "the abandon attempt and the resume each probe the deterministic id once",
    );
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        0,
        "resume must not recreate the released wallet journal",
    );
    assert!(
        payment_state
            .reservations
            .lock()
            .expect("test lock")
            .is_empty()
    );
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn interrupted_replacement_restore_completes_on_resume() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::paid()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let (replacement_requirements, _) = persist_provisional_replacement_for_test(
        &client,
        Timestamp(test_now_secs().saturating_sub(1)),
        ProvisionalReplacementQuote::Paid,
    )
    .await;
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    let requirements = recovery
        .payment_requirements(TestIdentity::fi_id())
        .unwrap()
        .expect("the provisional paid quote has exact requirements");
    let authorizations = requirements
        .seats
        .iter()
        .map(|seat| QuoteAuthorization {
            index: seat.index,
            quote_id: seat.quote_id,
        })
        .collect::<Vec<_>>();
    client
        .inner
        .store
        .authorize_payments(
            &recovery.snapshot.formation_id,
            &requirements.authorization_id,
            &authorizations,
        )
        .await
        .unwrap();
    let verified_quotes = requirements
        .seats
        .iter()
        .map(|requirement| {
            let seat = recovery
                .seats
                .iter()
                .find(|seat| seat.progress.index == requirement.index)
                .expect("payment requirement names its seat");
            seat.signed_quote
                .as_ref()
                .expect("paid replacement quote is durable")
                .verify(&seat.progress.locator.service_pubkey)
                .expect("paid replacement quote verifies")
        })
        .collect::<Vec<_>>();
    let preflight = crate::ExactPaymentPreflight::new(&requirements, &verified_quotes).unwrap();
    let reservation_id =
        crate::db::payment_reservation_id(&recovery.snapshot.formation_id, &requirements);
    payments
        .reserve_payment_requirements(&reservation_id, &preflight)
        .await
        .unwrap();
    client
        .inner
        .store
        .record_payment_reservation(&recovery.snapshot.formation_id, &reservation_id)
        .await
        .unwrap();
    let releases_before = payment_state.whole_release_calls.load(Ordering::SeqCst);
    let readiness_before = payment_state.readiness_calls.load(Ordering::SeqCst);
    let spends_before = payment_state.create_calls.load(Ordering::SeqCst);
    // The injected restore failure returns after the wallet release committed,
    // which is exactly the state a crash between the two would leave behind.
    client.inner.store.fail_restore_once();

    let error = client
        .resume()
        .await
        .expect_err("the atomic restore is interrupted after the wallet release");
    assert!(
        matches!(&error, FiError::Storage(message) if message.contains("restore failure")),
        "{error:?}"
    );
    assert_eq!(
        payment_state.whole_release_calls.load(Ordering::SeqCst),
        releases_before + 1,
    );
    assert!(
        !payment_state
            .reservations
            .lock()
            .expect("test lock")
            .contains_key(reservation_id.as_str()),
        "the wallet half of the restore completed before the interruption",
    );
    let interrupted = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        interrupted.payment_reservation_id.as_ref(),
        Some(&reservation_id),
        "the interrupted restore retains the durable reservation id",
    );
    assert!(
        interrupted.payment_reservation_release_intended,
        "the release commitment was durably recorded before the wallet call",
    );
    drop(client);

    let reopened = open_client(database, payments, fman_state, config).await;
    let error = reopened.resume().await.unwrap_err();
    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(SelectionReauthorizationReason::PreviewExpired)
    ));
    assert_replacement_preview_still_reachable(&reopened, &replacement_requirements).await;
    let restored = active_recovery(
        reopened
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(
        restored.payment_reservation_id.is_none(),
        "the completed restore clears the released reservation id",
    );
    assert!(
        !restored.payment_reservation_release_intended,
        "the completed restore clears the consumed release commitment",
    );
    assert_eq!(
        payment_state.whole_release_calls.load(Ordering::SeqCst),
        releases_before + 1,
        "expected wallet absence completes the restore without a second release",
    );
    assert_eq!(
        payment_state.readiness_calls.load(Ordering::SeqCst),
        readiness_before,
        "no restore path may recreate the released replacement journal",
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        spends_before,
        "completing the interrupted restore moves no additional value",
    );
}

#[tokio::test]
async fn absence_without_release_intent_still_never_returns_idle() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let reservation_id = seed_selected_recorded_reservation(
        &client,
        &payment_state,
        FormationId("absent-without-intent".to_owned()),
        test_now_secs() + 120,
    )
    .await;
    // The wallet loses its journal without any durable release commitment:
    // absence proves corruption, not a completed cleanup.
    payment_state
        .reservations
        .lock()
        .expect("test lock")
        .remove(reservation_id.as_str())
        .expect("the seeded journal is present before the loss");
    drop(client);

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let error = reopened.resume().await.unwrap_err();
    assert!(
        matches!(&error, FiError::Storage(message)
            if message.contains("durable FI reservation is absent from the payment wallet")),
        "{error:?}"
    );
    assert!(
        matches!(reopened.status(), FiStatus::Formation(_)),
        "uncommitted wallet absence must retain the formation",
    );

    let error = reopened.abandon_formation(options()).await.unwrap_err();
    assert!(
        matches!(&error, FiError::Storage(message)
            if message.contains("durable FI reservation is absent from the payment wallet")),
        "{error:?}"
    );
    let retained = active_recovery(
        reopened
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        retained.payment_reservation_id.as_ref(),
        Some(&reservation_id),
        "the fail-closed path keeps the durable reservation id",
    );
    assert!(!retained.payment_reservation_release_intended);
    assert_eq!(payment_state.whole_release_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn release_intent_is_superseded_by_reservation_adoption() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let reservation_id = seed_selected_recorded_reservation(
        &client,
        &payment_state,
        FormationId("intent-superseded-by-adoption".to_owned()),
        test_now_secs() + 120,
    )
    .await;
    payment_state
        .fail_whole_release
        .store(true, Ordering::SeqCst);

    // The commitment becomes durable, but the wallet release itself fails:
    // the journal provably still exists.
    let error = client
        .abandon_formation(options())
        .await
        .expect_err("the wallet refuses the committed release");
    assert!(
        matches!(&error, FiError::Payment(message) if message.contains("release failure")),
        "{error:?}"
    );
    assert_eq!(payment_state.whole_release_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        payment_state.reservations.lock().expect("test lock").len(),
        1,
        "the failed release keeps the wallet journal",
    );
    let interrupted = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        interrupted.payment_reservation_id.as_ref(),
        Some(&reservation_id)
    );
    assert!(
        interrupted.payment_reservation_release_intended,
        "the unconsumed release commitment stays durable after the failed release",
    );

    // Resume finds the journal alive and adopts it, which must durably
    // supersede the stale commitment before the run continues into funding.
    payment_state
        .fail_whole_release
        .store(false, Ordering::SeqCst);
    payment_state
        .hang_funding_on_call
        .store(1, Ordering::SeqCst);
    let funding_started = payment_state.funding_started.notified();
    let running = client.clone();
    let operation = tokio::spawn(async move { running.resume().await });
    tokio::time::timeout(Duration::from_secs(5), funding_started)
        .await
        .expect("resume adopts the live reservation and continues into funding");
    let adopted = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        adopted.payment_reservation_id.as_ref(),
        Some(&reservation_id),
        "adoption keeps the same deterministic reservation",
    );
    assert!(
        !adopted.payment_reservation_release_intended,
        "adopting the live reservation durably clears the stale release commitment",
    );
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let releases_before = payment_state.whole_release_calls.load(Ordering::SeqCst);
    drop(client);

    // A later wallet loss without a fresh commitment must fail closed: the
    // superseded commitment cannot linger and silently authorize a wipe.
    payment_state
        .reservations
        .lock()
        .expect("test lock")
        .remove(reservation_id.as_str())
        .expect("the adopted journal is present before the loss");
    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    let error = reopened.resume().await.unwrap_err();
    assert!(
        matches!(&error, FiError::Storage(message)
            if message.contains("durable FI reservation is absent from the payment wallet")),
        "{error:?}"
    );
    let retained = active_recovery(
        reopened
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        retained.payment_reservation_id.as_ref(),
        Some(&reservation_id)
    );
    assert!(!retained.payment_reservation_release_intended);
    assert_eq!(
        payment_state.whole_release_calls.load(Ordering::SeqCst),
        releases_before,
        "the fail-closed path performs no wallet release",
    );
}

#[tokio::test]
async fn unavailable_explicit_payer_stops_before_quotes_and_persistence() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    payment_state.pay_none.store(true, Ordering::SeqCst);
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;

    let error = client
        .pay_and_create(
            intent(),
            selection_approval(PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE)),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::SelectedPayerUnavailable
        )
    ));
    assert_eq!(client.status(), FiStatus::Idle);
    assert_eq!(fman_state.quote_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn expired_selection_approval_stops_before_payer_or_quote_work() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(database, payments, fman_state.clone(), FmanConfig::paid()).await;
    let mut approval = selection_approval(1_000);
    approval.valid_until = Timestamp(test_now_secs().saturating_sub(1));

    let error = client
        .pay_and_create(intent(), approval, payment_federation_id(), options())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(SelectionReauthorizationReason::PreviewExpired)
    ));
    assert_eq!(client.status(), FiStatus::Idle);
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.quote_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selection_approval_rejects_request_context_drift_before_external_work() {
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let drifted = FormationIntent::new(
        None,
        FederationSize(MIN_FEDERATION_SIZE),
        PlanPreference::InfiniteBestEffort,
        version_range("0.11.2"),
    )
    .unwrap();

    let error = client
        .pay_and_create(
            drifted,
            selection_approval(1_000),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, FiError::InvalidIntent(_)));
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.quote_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selection_approval_rejects_verifier_environment_drift_before_external_work() {
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig::paid(),
    )
    .await;
    let mut approval = selection_approval(1_000);
    approval.verifier_provenance = PeerBadgeVerifier::try_from_profile(
        &ManifoldEnvironment::Staging
            .profile()
            .expect("staging profile resolves"),
    )
    .expect("staging verifier resolves")
    .provenance();

    let error = client
        .pay_and_create(intent(), approval, payment_federation_id(), options())
        .await
        .unwrap_err();

    assert!(matches!(error, FiError::InvalidIntent(_)));
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.quote_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selected_fman_unavailability_requires_a_fresh_set_and_returns_idle() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        database,
        payments,
        fman_state.clone(),
        FmanConfig {
            accepting_seats: false,
            ..FmanConfig::paid()
        },
    )
    .await;

    let error = client
        .pay_and_create(
            intent(),
            selection_approval(1_000),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::SelectedFmanUnavailable
        )
    ));
    assert_eq!(client.status(), FiStatus::Idle);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.quote_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selected_quote_capacity_race_requires_a_fresh_set_and_returns_idle() {
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        fman_state.clone(),
        FmanConfig {
            capacity_exhausted_quote: true,
            ..FmanConfig::paid()
        },
    )
    .await;

    let error = client
        .pay_and_create(
            intent(),
            selection_approval(1_000),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        FiError::SelectionReauthorizationRequired(
            SelectionReauthorizationReason::SelectedFmanUnavailable
        )
    ));
    assert_eq!(client.status(), FiStatus::Idle);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fman_state.quote_calls.load(Ordering::SeqCst), 7);
}

#[tokio::test]
async fn under_cap_self_authorization_survives_reopen_and_resume() {
    // Park the capped formation by interrupting before payment: hang the
    // first CreateSeat so the initial run times out after the durable
    // self-authorization, then prove a reopened resume carries the cap and
    // recorded authorization forward to completion.
    let database = MemDatabase::new().into_database();
    let (payments, _payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::HangFirst,
        ..FmanConfig::paid()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);
    let short = FormationRunOptions::new(FormationRunOptionsConfig {
        poll_interval: Duration::from_millis(10),
        run_timeout: Duration::from_millis(1500),
        request_timeout: Duration::from_millis(500),
    })
    .unwrap();
    let error = client
        .create_with_pinned_fmans(capped_paid_intent(cap), locators(), short)
        .await
        .unwrap_err();
    assert!(matches!(error, FiError::Timeout(_)), "{error}");
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_recovery(TestIdentity::fi_id())
            .await
            .unwrap(),
    );
    assert!(
        recovery.test_payment_authorization().is_some(),
        "the self-authorization was durably recorded before the interruption",
    );
    drop(client);

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    reopened.resume().await.unwrap();
    let status = reopened.status();
    assert_eq!(formation(&status).phase, FormationPhase::Formed);
    assert_eq!(formation(&status).intent.max_total_msats, Some(cap));
}

#[tokio::test]
async fn quote_replacement_after_self_authorization_parks_even_under_cap() {
    // The cap approves the initial aggregate exactly once. The capped run
    // self-authorizes and starts funding, one seat's presentation is refused
    // (the verified refusal clears that exact quote), and resume re-quotes
    // the cleared seat under the same cap: the durable tombstone must park
    // the replacement set for explicit authorization instead of
    // self-authorizing a second aggregate the consumer never saw.
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let config = FmanConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..FmanConfig::paid()
    };
    let client = open_client(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        config,
    )
    .await;
    let cap = PAYMENT_AMOUNT_MSATS * u64::from(MIN_FEDERATION_SIZE);
    let error = client
        .create_with_pinned_fmans(capped_paid_intent(cap), locators(), options())
        .await
        .unwrap_err();
    assert!(matches!(error, FiError::SeatRefused { .. }), "{error}");
    let funded = payment_state.create_calls.load(Ordering::SeqCst);
    assert!(funded > 0, "the self-authorized run started funding");
    drop(client);

    let reopened = open_client(database, payments, fman_state, FmanConfig::paid()).await;
    reopened.resume().await.unwrap();
    let status = reopened.status();
    assert_eq!(
        formation(&status).phase,
        FormationPhase::AwaitingPaymentReadiness,
        "the replacement quote parks despite being under the cap",
    );
    let requirements = payment_requirements(&status).clone();
    assert!(requirements.total_msats <= cap);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        funded,
        "no funding started for the parked replacement quote",
    );

    // Explicit authorization of the parked replacement still completes.
    reopened
        .authorize_payments(requirements.authorization_id, options())
        .await
        .unwrap();
    assert_eq!(formation(&reopened.status()).phase, FormationPhase::Formed);
}

#[tokio::test]
async fn liquidity_operation_listing_is_bounded_paginated_and_fail_closed() {
    let database = MemDatabase::new().into_database();
    let store = db::FiStore::new(database.clone());
    let mut expected_ids = Vec::new();
    for marker in 1..=3 {
        let operation = stored_liquidity_operation(marker);
        expected_ids.push(operation.operation_id.clone());
        store
            .insert_liquidity_operation(operation)
            .await
            .expect("persist operation");
    }
    expected_ids.sort_by(|left, right| left.0.cmp(&right.0));

    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(FmanState::default()),
        FmanConfig::given_away(),
    )
    .await;

    let first = client
        .list_liquidity_operations(None, 2)
        .await
        .expect("first page");
    assert_eq!(
        first
            .operations
            .iter()
            .map(|snapshot| snapshot.operation_id.clone())
            .collect::<Vec<_>>(),
        expected_ids[..2]
    );
    assert_eq!(first.next_after.as_ref(), expected_ids.get(1));

    let second = client
        .list_liquidity_operations(first.next_after.as_ref(), 2)
        .await
        .expect("second page");
    assert_eq!(
        second
            .operations
            .iter()
            .map(|snapshot| snapshot.operation_id.clone())
            .collect::<Vec<_>>(),
        expected_ids[2..]
    );
    assert!(second.next_after.is_none());
    assert!(client.list_liquidity_operations(None, 0).await.is_err());
    assert!(
        client
            .list_liquidity_operations(None, FI_LIQUIDITY_OPERATION_PAGE_MAX + 1)
            .await
            .is_err()
    );
    assert!(
        client
            .list_liquidity_operations(Some(&LiquidityOperationId("not-a-hash".to_owned())), 1,)
            .await
            .is_err()
    );

    let mut corrupt = stored_liquidity_operation(4);
    corrupt.details_payload_hash =
        fedi_decentralized_service_liquidity_manager::Sha256Digest([0; 32]);
    client
        .inner
        .store
        .insert_liquidity_operation(corrupt)
        .await
        .expect("persist corrupt fixture");
    assert!(
        client
            .list_liquidity_operations(None, FI_LIQUIDITY_OPERATION_PAGE_MAX)
            .await
            .is_err(),
        "listing must not project a commitment/id mismatch"
    );
}
#[tokio::test]
async fn post_formation_liquidity_supports_gateway_stability_and_combined_intents() {
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());
    let intents = [
        LiquidityRequestIntent::gateway(10_000, Some(20_000)),
        LiquidityRequestIntent {
            amounts: liquidity_api::LiquidityAmountBounds {
                gateway_min_amount: liquidity_api::Sats(0),
                gateway_max_amount: None,
                stability_min_amount: liquidity_api::Sats(30_000),
                stability_max_amount: Some(liquidity_api::Sats(40_000)),
            },
        },
        LiquidityRequestIntent {
            amounts: liquidity_api::LiquidityAmountBounds {
                gateway_min_amount: liquidity_api::Sats(50_000),
                gateway_max_amount: Some(liquidity_api::Sats(60_000)),
                stability_min_amount: liquidity_api::Sats(70_000),
                stability_max_amount: Some(liquidity_api::Sats(80_000)),
            },
        },
    ];

    for (index, intent) in intents.into_iter().enumerate() {
        // One live operation per federation: every intent exercises its own
        // freshly formed federation.
        let (client, formation_id, fman_state, connector) = formed_client_for_liquidity().await;
        let snapshot = client
            .start_liquidity_for_test(
                &formation_id,
                &provider,
                intent,
                &connector,
                &TestLiquidityVerifier,
            )
            .await
            .unwrap_or_else(|error| panic!("intent {index} failed: {error}"));
        assert_eq!(snapshot.phase, LiquidityOperationPhase::Accepted);
        assert_eq!(snapshot.item_statuses.len(), if index == 2 { 2 } else { 1 });

        let requests = connector.0.requests.lock().expect("test lock");
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(request.payload.fman_endorsement.is_some());
        let trust = request
            .payload
            .fman_trust_material
            .as_ref()
            .expect("all FMan trust material is carried");
        assert_eq!(trust.len(), usize::from(MIN_FEDERATION_SIZE));
        assert_eq!(
            trust
                .iter()
                .map(|response| response.material.fman_pubkey.clone())
                .collect::<BTreeSet<_>>()
                .len(),
            usize::from(MIN_FEDERATION_SIZE),
            "one signed response is carried for every distinct FMan",
        );
        drop(requests);

        let trust_requests = fman_state
            .trust_material_requests
            .lock()
            .expect("test lock");
        assert_eq!(trust_requests.len(), usize::from(MIN_FEDERATION_SIZE));
        assert_eq!(
            trust_requests
                .iter()
                .map(|(index, _)| *index)
                .collect::<HashSet<_>>()
                .len(),
            usize::from(MIN_FEDERATION_SIZE),
            "each distinct FMan is queried once per exact request",
        );
        assert!(
            trust_requests
                .iter()
                .all(|(_, peer_ids)| peer_ids.len() == 1)
        );
    }
}

#[test]
fn liquidity_intent_rejects_a_maximum_for_an_unrequested_source() {
    for intent in [
        LiquidityRequestIntent {
            amounts: liquidity_api::LiquidityAmountBounds {
                gateway_min_amount: liquidity_api::Sats(10_000),
                gateway_max_amount: None,
                stability_min_amount: liquidity_api::Sats(0),
                stability_max_amount: Some(liquidity_api::Sats(20_000)),
            },
        },
        LiquidityRequestIntent {
            amounts: liquidity_api::LiquidityAmountBounds {
                gateway_min_amount: liquidity_api::Sats(0),
                gateway_max_amount: Some(liquidity_api::Sats(20_000)),
                stability_min_amount: liquidity_api::Sats(10_000),
                stability_max_amount: None,
            },
        },
    ] {
        assert!(intent.validate().is_err());
    }
}

#[test]
fn liquidity_endpoint_admission_allows_untrusted_query_hints_around_exact_alpn() {
    let endpoint = liquidity_provider_endpoint();
    let alpn =
        String::from_utf8_lossy(liquidity_api::PUBLIC_LIQUIDITY_API_ALPN).replace('/', "%2F");
    let url = liquidity_api::Url(format!(
        "iroh://{}?region=west&alpn={alpn}&transport=relay",
        endpoint.id
    ));

    let (admitted_url, admitted_endpoint) =
        crate::liquidity::admit_endpoint(std::slice::from_ref(&url))
            .expect("exact ALPN is authoritative");
    assert_eq!(admitted_url, url);
    assert_eq!(admitted_endpoint, endpoint);
}

#[tokio::test]
async fn liquidity_recovery_adopts_a_lost_ack_before_any_replay() {
    let (client, formation_id, _fman_state, connector) = formed_client_for_liquidity().await;
    connector.0.lose_first_ack.store(true, Ordering::SeqCst);
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());

    let error = client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(90_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect_err("the first acknowledgement is lost");
    assert!(matches!(error, FiError::Liquidity(_)));
    let prepared = client
        .list_liquidity_operations(None, 10)
        .await
        .expect("durable operation is discoverable");
    assert_eq!(prepared.operations.len(), 1);
    assert_eq!(
        prepared.operations[0].phase,
        LiquidityOperationPhase::Prepared
    );

    let recovered = client
        .resume_liquidity_for_test(
            &prepared.operations[0].operation_id,
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect("status-first recovery adopts the accepted allocation");
    assert_eq!(recovered.phase, LiquidityOperationPhase::Accepted);
    assert_eq!(
        *connector.0.calls.lock().expect("test lock"),
        vec!["request", "status"],
        "a found status forbids replaying RequestLiquidity",
    );
    assert_eq!(connector.0.requests.lock().expect("test lock").len(), 1);
}

#[tokio::test]
async fn completed_gateway_is_registered_with_a_threshold_and_durably_verified() {
    let (client, formation_id, fman_state, connector) = formed_client_for_liquidity().await;
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());
    let operation = client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(90_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .unwrap();
    let gateway_api = GatewayApiUrl::try_from(
        "iroh://8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c",
    )
    .unwrap();
    {
        let mut allocations = connector.0.allocations.lock().expect("test lock");
        let allocation = allocations
            .get_mut(&operation.details_payload_hash)
            .expect("provider allocation exists");
        complete_gateway_allocation(allocation, gateway_api.clone());
    }

    let completed = client
        .resume_liquidity_for_test(&operation.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .unwrap();
    assert!(completed.gateway_view_verified);
    let registrations = fman_state.gateway_registrations.lock().expect("test lock");
    assert_eq!(registrations.len(), usize::from(MIN_FEDERATION_SIZE));
    assert!(
        registrations
            .iter()
            .all(|(_, url)| url == gateway_api.as_str())
    );
}

#[tokio::test]
async fn completed_gateway_recovery_uses_durable_evidence_before_provider_connection() {
    let (client, formation_id, fman_state, connector) = formed_client_for_liquidity().await;
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());
    let operation = client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(95_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect("start liquidity");
    let gateway_api = GatewayApiUrl::try_from(
        "iroh://d5611c6297fe6fdb1a6875d17f81ea5968436fdf899e70a6a30d8f9b6f37c181",
    )
    .expect("gateway API");
    let allocation = {
        let mut allocations = connector.0.allocations.lock().expect("test lock");
        let allocation = allocations
            .get_mut(&operation.details_payload_hash)
            .expect("provider allocation exists");
        complete_gateway_allocation(allocation, gateway_api.clone());
        allocation.clone()
    };
    let durable_status = sign_liquidity_payload(
        liquidity_api::PublicRpcPayloadDomain::GetAllocationStatusResponse,
        liquidity_api::GetAllocationStatusResponse {
            version: liquidity_api::PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
            provider_pubkey: provider,
            issued_at: liquidity_api::Timestamp(test_now_secs()),
            status: allocation,
        },
        &liquidity_provider_keys(),
    );
    client
        .inner
        .store
        .store_liquidity_status(&operation.operation_id, durable_status)
        .await
        .expect("persist verified provider completion before the simulated crash");
    client
        .inner
        .ports
        .registry
        .advertisements
        .lock()
        .expect("test lock")
        .clear();
    connector.0.fail_next_connect.store(true, Ordering::SeqCst);

    let recovered = client
        .resume_liquidity_for_test(&operation.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .expect("durable completion attaches without the provider");

    assert!(recovered.gateway_view_verified);
    assert_eq!(
        *connector.0.calls.lock().expect("test lock"),
        vec!["request"],
        "resume performs no provider status call",
    );
    assert!(
        client
            .inner
            .ports
            .registry
            .advertisements
            .lock()
            .expect("test lock")
            .is_empty(),
        "the completed provider is no longer advertised",
    );
    assert!(
        connector.0.fail_next_connect.load(Ordering::SeqCst),
        "the injected provider connection failure was never consumed",
    );
    assert_eq!(
        fman_state
            .gateway_registrations
            .lock()
            .expect("test lock")
            .len(),
        usize::from(MIN_FEDERATION_SIZE),
    );
}

#[tokio::test]
async fn liquidity_recovery_replays_the_exact_commitment_only_after_not_found() {
    let (client, formation_id, _fman_state, connector) = formed_client_for_liquidity().await;
    connector
        .0
        .fail_first_before_allocation
        .store(true, Ordering::SeqCst);
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());

    client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(100_000, Some(110_000)),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect_err("first request fails before provider acceptance");
    let prepared = client
        .list_liquidity_operations(None, 10)
        .await
        .expect("durable operation is discoverable")
        .operations
        .pop()
        .expect("one prepared operation");
    let recovered = client
        .resume_liquidity_for_test(&prepared.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .expect("NotFound permits exact replay");
    assert_eq!(recovered.phase, LiquidityOperationPhase::Accepted);
    assert_eq!(
        *connector.0.calls.lock().expect("test lock"),
        vec!["request", "status", "request"]
    );
    let requests = connector.0.requests.lock().expect("test lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(
        liquidity_api::request_liquidity_details_hash_for_request(&requests[0].payload)
            .expect("first request hashes"),
        liquidity_api::request_liquidity_details_hash_for_request(&requests[1].payload)
            .expect("replay hashes"),
    );
    assert_eq!(
        requests[0].payload.details_payload_hash,
        requests[1].payload.details_payload_hash,
    );
    assert_eq!(
        requests[0].payload.federation_details,
        requests[1].payload.federation_details,
    );
}

#[tokio::test]
async fn liquidity_recovery_does_not_replay_on_ambiguous_status_failure() {
    let (client, formation_id, _fman_state, connector) = formed_client_for_liquidity().await;
    connector
        .0
        .fail_first_before_allocation
        .store(true, Ordering::SeqCst);
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());
    client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(120_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect_err("first request fails before acceptance");
    let operation = client
        .list_liquidity_operations(None, 10)
        .await
        .expect("list operation")
        .operations
        .pop()
        .expect("prepared operation");
    *connector.0.status_error.lock().expect("test lock") =
        Some(liquidity_api::ServiceErrorCode::Unavailable);
    client
        .resume_liquidity_for_test(&operation.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .expect_err("ambiguous status failure must remain retryable");
    assert_eq!(
        *connector.0.calls.lock().expect("test lock"),
        vec!["request", "status"]
    );
    assert_eq!(connector.0.requests.lock().expect("test lock").len(), 1);
}

#[tokio::test]
async fn liquidity_rejects_invalid_fman_and_provider_proofs_without_accepting() {
    let (client, formation_id, fman_state, connector) = formed_client_for_liquidity().await;
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());

    fman_state
        .corrupt_trust_material_signature
        .store(true, Ordering::SeqCst);
    client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(130_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect_err("invalid FMan trust signature fails closed");
    assert!(connector.0.requests.lock().expect("test lock").is_empty());
    let operations = client
        .list_liquidity_operations(None, 10)
        .await
        .expect("the refused acknowledgement remains recoverable");
    assert_eq!(operations.operations.len(), 1);
    assert_eq!(
        operations.operations[0].phase,
        LiquidityOperationPhase::Prepared
    );

    for (index, fault) in [
        LiquidityResponseFault::InvalidSignature,
        LiquidityResponseFault::WrongHash,
        LiquidityResponseFault::WrongItemSet,
        LiquidityResponseFault::BelowMinimum,
        LiquidityResponseFault::AboveMaximum,
    ]
    .into_iter()
    .enumerate()
    {
        // One live operation per federation: every fault exercises its own
        // freshly formed federation.
        let (client, formation_id, _fman_state, connector) = formed_client_for_liquidity().await;
        *connector.0.response_fault.lock().expect("test lock") = Some(fault);
        let minimum = 140_000 + index as u64;
        client
            .start_liquidity_for_test(
                &formation_id,
                &provider,
                LiquidityRequestIntent::gateway(
                    minimum,
                    (fault == LiquidityResponseFault::AboveMaximum).then_some(minimum),
                ),
                &connector,
                &TestLiquidityVerifier,
            )
            .await
            .unwrap_err();
        assert_eq!(connector.0.requests.lock().expect("test lock").len(), 1);
        let operations = client
            .list_liquidity_operations(None, 10)
            .await
            .expect("the rejected acknowledgement remains recoverable");
        assert_eq!(operations.operations.len(), 1);
        assert_eq!(
            operations.operations[0].phase,
            LiquidityOperationPhase::Prepared
        );
    }
}

#[tokio::test]
async fn liquidity_status_rejects_a_signed_amount_outside_persisted_bounds() {
    let (client, formation_id, _fman_state, connector) = formed_client_for_liquidity().await;
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());
    connector.0.lose_first_ack.store(true, Ordering::SeqCst);
    *connector.0.response_fault.lock().expect("test lock") =
        Some(LiquidityResponseFault::BelowMinimum);

    client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(200_000, Some(210_000)),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect_err("the provider allocation exists but its acknowledgement is lost");
    let operation = client
        .list_liquidity_operations(None, 10)
        .await
        .expect("prepared operation is durable")
        .operations
        .pop()
        .expect("one prepared operation");
    client
        .resume_liquidity_for_test(&operation.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .expect_err("signed status below the persisted minimum fails closed");
    assert_eq!(
        *connector.0.calls.lock().expect("test lock"),
        vec!["request", "status"]
    );
}

#[tokio::test]
async fn second_live_liquidity_request_for_a_federation_is_refused_until_resumed() {
    let (client, formation_id, _fman_state, connector) = formed_client_for_liquidity().await;
    connector
        .0
        .fail_first_before_allocation
        .store(true, Ordering::SeqCst);
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());
    client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(300_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect_err("the first request fails ambiguously after the commitment is durable");
    let prepared = client
        .list_liquidity_operations(None, 10)
        .await
        .expect("durable operation is discoverable")
        .operations
        .pop()
        .expect("one prepared operation");

    let error = client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(300_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect_err("a live operation forbids minting a second request identity");
    let operation_id = match error {
        FiError::LiquidityOperationExists { operation_id } => operation_id,
        other => panic!("expected the live-operation error, got {other}"),
    };
    assert_eq!(operation_id, prepared.operation_id);
    assert_eq!(
        connector.0.requests.lock().expect("test lock").len(),
        1,
        "the refused retry never reached the provider",
    );
    let current = client
        .current_liquidity_operation()
        .await
        .expect("the canonical live operation is readable")
        .expect("the live operation the error named is the current one");
    assert_eq!(current.operation_id, operation_id);
    assert_eq!(current.phase, LiquidityOperationPhase::Prepared);

    let resumed = client
        .resume_liquidity_for_test(&prepared.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .expect("the named operation resumes to completion");
    assert_eq!(resumed.phase, LiquidityOperationPhase::Accepted);

    // The accepted allocation still owns the federation's provider capacity.
    let error = client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(310_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect_err("an accepted operation is still the federation's one allocation");
    assert!(
        matches!(error, FiError::LiquidityOperationExists { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn provider_not_found_cannot_overturn_durable_acceptance_evidence() {
    let (client, formation_id, _fman_state, connector) = formed_client_for_liquidity().await;
    connector.0.lose_first_ack.store(true, Ordering::SeqCst);
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());
    client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(220_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect_err("the first acknowledgement is lost");
    let operation = client
        .list_liquidity_operations(None, 10)
        .await
        .expect("durable operation is discoverable")
        .operations
        .pop()
        .expect("one prepared operation");
    let recovered = client
        .resume_liquidity_for_test(&operation.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .expect("status-first recovery adopts the accepted allocation");
    assert_eq!(recovered.phase, LiquidityOperationPhase::Accepted);

    // The provider then durably loses (or disclaims) the allocation.
    connector.0.allocations.lock().expect("test lock").clear();
    let error = client
        .resume_liquidity_for_test(&operation.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .expect_err("journal evidence outranks the provider's NotFound");
    assert!(matches!(error, FiError::Liquidity(_)), "{error}");
    assert!(error.to_string().contains("disclaims"), "{error}");
    assert_eq!(
        client
            .liquidity_status(&operation.operation_id)
            .await
            .expect("durable projection remains readable")
            .phase,
        LiquidityOperationPhase::Accepted,
        "the durable acceptance is retained unchanged",
    );
    assert_eq!(
        *connector.0.calls.lock().expect("test lock"),
        vec!["request", "status", "status"],
        "a disclaimed evidenced allocation is never replayed",
    );
    assert_eq!(connector.0.requests.lock().expect("test lock").len(), 1);
}

#[tokio::test]
async fn provider_rejection_is_terminal_and_frees_the_federation() {
    let (client, formation_id, _fman_state, connector) = formed_client_for_liquidity().await;
    *connector.0.response_fault.lock().expect("test lock") = Some(LiquidityResponseFault::Rejected);
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());
    let rejected = client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(230_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect("a signed rejection is a durable terminal outcome");
    assert_eq!(rejected.phase, LiquidityOperationPhase::Rejected);
    assert_eq!(
        rejected.rejection_code.as_deref(),
        Some("insufficient_capacity")
    );

    let resumed = client
        .resume_liquidity_for_test(&rejected.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .expect("a rejected operation resumes as its terminal snapshot");
    assert_eq!(resumed.phase, LiquidityOperationPhase::Rejected);
    assert_eq!(
        *connector.0.calls.lock().expect("test lock"),
        vec!["request"],
        "a terminal rejection performs no provider work on resume",
    );
    assert_eq!(
        client
            .current_liquidity_operation()
            .await
            .expect("the canonical live operation is readable"),
        None,
        "a terminal rejection leaves no current operation",
    );

    // The terminal rejection frees the federation for a fresh request.
    let fresh = client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(240_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect("a rejected federation may start fresh");
    assert_eq!(fresh.phase, LiquidityOperationPhase::Accepted);
    assert_eq!(
        client
            .current_liquidity_operation()
            .await
            .expect("the canonical live operation is readable"),
        Some(fresh),
        "the fresh accepted operation is the federation's current one",
    );
}

#[tokio::test]
async fn liquidity_store_refuses_conflicting_or_regressing_provider_evidence() {
    fn unsigned<T>(payload: T) -> liquidity_api::Signed<T> {
        liquidity_api::Signed {
            payload,
            proof: liquidity_api::PayloadProof {
                signature: liquidity_api::Signature(vec![0; 64]),
            },
        }
    }

    let store = db::FiStore::new(MemDatabase::new().into_database());
    let operation = stored_liquidity_operation(1);
    let operation_id = operation.operation_id.clone();
    let provider = operation.commitment.provider_pubkey.clone();
    let hash = operation.details_payload_hash;
    store
        .insert_liquidity_operation(operation)
        .await
        .expect("persist operation");

    let accepted_status = liquidity_api::GetAllocationStatusResponse {
        version: liquidity_api::PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
        provider_pubkey: provider.clone(),
        issued_at: liquidity_api::Timestamp(1_000),
        status: liquidity_api::AllocationStatus {
            details_payload_hash: hash,
            provider_pubkey: provider.clone(),
            item_statuses: Vec::new(),
        },
    };
    store
        .store_liquidity_status(&operation_id, unsigned(accepted_status.clone()))
        .await
        .expect("first durable status stores");
    store
        .store_liquidity_status(&operation_id, unsigned(accepted_status.clone()))
        .await
        .expect("an identical status is idempotent");

    let mut older = accepted_status.clone();
    older.issued_at = liquidity_api::Timestamp(999);
    store
        .store_liquidity_status(&operation_id, unsigned(older))
        .await
        .expect_err("an older status is a rollback");

    let mut contradiction = accepted_status.clone();
    contradiction.status.item_statuses = allocation_items(&liquidity_api::LiquidityAmountBounds {
        gateway_min_amount: liquidity_api::Sats(1),
        gateway_max_amount: None,
        stability_min_amount: liquidity_api::Sats(0),
        stability_max_amount: None,
    });
    store
        .store_liquidity_status(&operation_id, unsigned(contradiction))
        .await
        .expect_err("a same-timestamp different payload is contradictory");

    // A rejection cannot overwrite durable accepted-status evidence.
    let rejection = liquidity_api::RequestLiquidityResponse {
        version: liquidity_api::PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
        details_payload_hash: hash,
        provider_pubkey: provider.clone(),
        issued_at: liquidity_api::Timestamp(1_001),
        outcome: liquidity_api::RequestLiquidityOutcome::Rejected(liquidity_api::PublicRejection {
            code: liquidity_api::PublicRejectionCode::RequestExpired,
            reason: None,
        }),
    };
    let error = store
        .store_liquidity_response(&operation_id, unsigned(rejection))
        .await
        .expect_err("a rejection conflicts with durable acceptance evidence");
    assert!(
        error
            .to_string()
            .contains("conflicting with durable acceptance evidence"),
        "{error}"
    );

    let accepted_response = liquidity_api::RequestLiquidityResponse {
        version: liquidity_api::PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
        details_payload_hash: hash,
        provider_pubkey: provider.clone(),
        issued_at: liquidity_api::Timestamp(1_002),
        outcome: liquidity_api::RequestLiquidityOutcome::Accepted(
            liquidity_api::AllocationStatus {
                details_payload_hash: hash,
                provider_pubkey: provider,
                item_statuses: Vec::new(),
            },
        ),
    };
    store
        .store_liquidity_response(&operation_id, unsigned(accepted_response.clone()))
        .await
        .expect("the first response stores");
    store
        .store_liquidity_response(&operation_id, unsigned(accepted_response.clone()))
        .await
        .expect("an identical response is idempotent");
    let mut differing = accepted_response;
    differing.issued_at = liquidity_api::Timestamp(1_003);
    store
        .store_liquidity_response(&operation_id, unsigned(differing))
        .await
        .expect_err("a second differing response conflicts");
}

#[tokio::test]
async fn gateway_view_verification_is_bound_to_the_current_durable_url() {
    fn unsigned<T>(payload: T) -> liquidity_api::Signed<T> {
        liquidity_api::Signed {
            payload,
            proof: liquidity_api::PayloadProof {
                signature: liquidity_api::Signature(vec![0; 64]),
            },
        }
    }

    let store = db::FiStore::new(MemDatabase::new().into_database());
    let operation = stored_liquidity_operation(2);
    let operation_id = operation.operation_id.clone();
    let provider = operation.commitment.provider_pubkey.clone();
    let hash = operation.details_payload_hash;
    store
        .insert_liquidity_operation(operation)
        .await
        .expect("persist operation");
    let gateway_a = GatewayApiUrl::try_from(
        "iroh://059457bb866519cb35f68e344862c1caee22fc75b6e0c13fc855563efc80089a",
    )
    .expect("gateway A");
    let gateway_b = GatewayApiUrl::try_from(
        "iroh://fa13e542abac58f369ca4c3ec48a6b33b23d5bc694bf8ad53f107caf4f6e17b1",
    )
    .expect("gateway B");
    let mut allocation = liquidity_api::AllocationStatus {
        details_payload_hash: hash,
        provider_pubkey: provider.clone(),
        item_statuses: allocation_items(&liquidity_api::LiquidityAmountBounds {
            gateway_min_amount: liquidity_api::Sats(2),
            gateway_max_amount: None,
            stability_min_amount: liquidity_api::Sats(0),
            stability_max_amount: None,
        }),
    };
    complete_gateway_allocation(&mut allocation, gateway_a.clone());
    let status_a = liquidity_api::GetAllocationStatusResponse {
        version: liquidity_api::PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
        provider_pubkey: provider.clone(),
        issued_at: liquidity_api::Timestamp(1_000),
        status: allocation.clone(),
    };
    store
        .store_liquidity_status(&operation_id, unsigned(status_a))
        .await
        .expect("store completion A");
    store
        .mark_liquidity_gateway_view_verified(&operation_id, &gateway_a)
        .await
        .expect("bind readback to A");
    assert!(
        store
            .load_liquidity_operation(&operation_id)
            .await
            .expect("load A")
            .snapshot()
            .expect("snapshot A")
            .gateway_view_verified
    );

    complete_gateway_allocation(&mut allocation, gateway_b.clone());
    let status_b = liquidity_api::GetAllocationStatusResponse {
        version: liquidity_api::PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
        provider_pubkey: provider,
        issued_at: liquidity_api::Timestamp(1_001),
        status: allocation,
    };
    store
        .store_liquidity_status(&operation_id, unsigned(status_b))
        .await
        .expect("new signed URL clears A's proof");
    assert!(
        !store
            .load_liquidity_operation(&operation_id)
            .await
            .expect("load B")
            .snapshot()
            .expect("snapshot B")
            .gateway_view_verified
    );
    store
        .mark_liquidity_gateway_view_verified(&operation_id, &gateway_a)
        .await
        .expect_err("A cannot inherit B's readback");
    store
        .mark_liquidity_gateway_view_verified(&operation_id, &gateway_b)
        .await
        .expect("B can be marked only after B's exact readback");
}

#[tokio::test]
async fn liquidity_reopen_resumes_the_exact_persisted_commitment() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    registry
        .advertisements
        .lock()
        .expect("test lock")
        .push(liquidity_provider_event());
    let client = open_client_with_registry(
        database.clone(),
        payments.clone(),
        fman_state.clone(),
        FmanConfig::given_away(),
        registry,
    )
    .await;
    client
        .pay_and_create(
            intent(),
            selection_approval(1),
            payment_federation_id(),
            options(),
        )
        .await
        .expect("form selected test federation");
    let formation_id = formation(&client.status()).formation_id.clone();
    let connector =
        TestLiquidityConnector(TestLiquidityProviderState::new(client.inner.store.clone()));
    connector.0.fail_next_connect.store(true, Ordering::SeqCst);
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());
    client
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(250_000, Some(260_000)),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect_err("the connection fails after the commitment is durable");
    assert!(
        connector.0.calls.lock().expect("test lock").is_empty(),
        "no provider call was ever sent",
    );
    let persisted = client
        .list_liquidity_operations(None, 10)
        .await
        .expect("durable operation is discoverable")
        .operations
        .pop()
        .expect("one prepared operation");
    assert_eq!(persisted.phase, LiquidityOperationPhase::Prepared);
    drop(client);

    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    registry
        .advertisements
        .lock()
        .expect("test lock")
        .push(liquidity_provider_event());
    let reopened = open_client_with_registry(
        database,
        payments,
        fman_state,
        FmanConfig::given_away(),
        registry,
    )
    .await;
    reopened
        .resume()
        .await
        .expect("reconcile the formed federation after reopen");
    assert_eq!(
        reopened
            .current_liquidity_operation()
            .await
            .expect("the canonical live operation is readable after reopen")
            .as_ref(),
        Some(&persisted),
        "reopen recovery finds the prepared operation without paging",
    );
    let recovered = reopened
        .resume_liquidity_for_test(&persisted.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .expect("recovery replays the exact persisted commitment");
    assert_eq!(recovered.phase, LiquidityOperationPhase::Accepted);
    assert_eq!(
        *connector.0.calls.lock().expect("test lock"),
        vec!["status", "request"],
        "recovery is status-first and replays exactly once",
    );
    let stored = reopened
        .inner
        .store
        .load_liquidity_operation(&persisted.operation_id)
        .await
        .expect("stored operation loads after reopen");
    let requests = connector.0.requests.lock().expect("test lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].payload.details_payload_hash,
        persisted.details_payload_hash,
    );
    assert_eq!(
        liquidity_api::request_liquidity_details_hash_for_request(&requests[0].payload)
            .expect("replayed request hashes"),
        persisted.details_payload_hash,
        "the replay carries the exact persisted semantic identity",
    );
    assert_eq!(
        requests[0].payload.federation_details,
        stored.commitment.federation_details,
    );
    assert_eq!(requests[0].payload.expires_at, stored.commitment.expires_at);
}

#[tokio::test]
async fn late_stale_base_responses_do_not_fail_an_adopted_directory() {
    // A threshold can adopt the target while slower guardians in the same
    // concurrent wave are still checking the old base. Those late guardians
    // correctly report staleness; the FI must read consensus before deciding
    // whether the operation failed.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let threshold = test_federation_seats().consensus_threshold() as usize;
    fman_state
        .stale_meta_indices
        .lock()
        .expect("test lock")
        .extend(threshold..usize::from(MIN_FEDERATION_SIZE));
    let client = open_client(
        database,
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
    )
    .await;

    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    assert_eq!(
        fman_state.meta_submissions.lock().expect("test lock").len(),
        threshold,
        "the threshold target should be accepted despite late stale responses"
    );
}

#[tokio::test]
async fn an_all_stale_wave_rereads_rebases_and_replays_one_identical_base() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let fman_state = Arc::new(FmanState::default());
    let reader = TestConsensusReader::new(fman_state.clone());
    let initial = serde_json::to_vec(&serde_json::json!({ "external": "first" })).unwrap();
    let changed = serde_json::to_vec(&serde_json::json!({ "external": "second" })).unwrap();
    // The final config read validates the directory before the durable target
    // is pinned. Advance after the following metadata-base read so the first
    // proposal wave is stale rather than consuming the fixture on validation.
    reader.change_base_after_reads(initial.clone(), changed.clone(), 2);
    let client = open_client_with_reader(
        database,
        payments,
        fman_state.clone(),
        FmanConfig::given_away(),
        reader,
    )
    .await;

    client
        .create_with_pinned_fmans(intent(), locators(), options())
        .await
        .unwrap();

    assert_eq!(formation(&client.status()).phase, FormationPhase::Formed);
    let bases = fman_state.meta_request_bases.lock().expect("test lock");
    let seats = usize::from(MIN_FEDERATION_SIZE);
    assert_eq!(bases.len(), seats * 2, "one all-stale wave and one retry");
    // The advance from `initial` to `changed` is one adoption: the fake
    // consensus revision moves 0 -> 1 with it.
    let initial_base = MetaConsensusBase::from_consensus(Some((0, &initial)));
    let changed_base = MetaConsensusBase::from_consensus(Some((1, &changed)));
    assert!(bases[..seats].iter().all(|(_, base)| *base == initial_base));
    assert!(bases[seats..].iter().all(|(_, base)| *base == changed_base));
    assert_eq!(
        fman_state.meta_submissions.lock().expect("test lock").len(),
        seats,
        "the rebased retry sends an identical accepted request to every seat"
    );
}
