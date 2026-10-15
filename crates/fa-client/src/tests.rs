mod backup_restore;
mod callback;
mod crash_db;
mod discovery;
mod maintenance;
mod migration_qa;
mod payment_crash;
mod recovery_policy;
mod selection;

use crate::formation::Timeouts;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::pending;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bitcoin::secp256k1::{PublicKey as BitcoinPublicKey, SecretKey as BitcoinSecretKey};
use bitcoin_hashes::{Hash as _, sha256};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_nostr::setup_payment_federations::{
    SETUP_PAYMENT_FEDERATIONS_D_TAG, SETUP_PAYMENT_FEDERATIONS_EVENT_KIND,
};
use fedi_decentralized_nostr_clients::{FaNostrClient, NostrClientError, NostrClientResult};
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_service_liquidity_manager as liquidity_api;
use fedi_decentralized_service_manifold_guardian::*;
use fedi_iroh_rpc::iroh::{EndpointAddr, SecretKey as IrohSecretKey};
use fedimint_core::PeerId;
use fedimint_core::db::{IRawDatabaseExt as _, mem_impl::MemDatabase};
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::util::SafeUrl;
use nostr_sdk::{Event, EventBuilder, Keys, Kind, PublicKey, Tag};
use secp256k1::{Keypair, SECP256K1, SecretKey};
use stability_pool_common::{Account, AccountType};
use tokio::sync::Notify;

use crate::liquidity::TestLiquidityBadgeVerifier;
use crate::state::ResolvedSeatPurchase;

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
struct TestFaFeeAccountProvider {
    account: Option<Account>,
    requested_federations: Arc<Mutex<Vec<FedimintFederationId>>>,
}

impl TestFaFeeAccountProvider {
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

impl Default for TestFaFeeAccountProvider {
    fn default() -> Self {
        Self::new(guardian_fee_account(30))
    }
}

impl FaFeeAccountProvider for TestFaFeeAccountProvider {
    fn formed_federation_fee_account(
        &self,
        federation_id: &FedimintFederationId,
    ) -> Result<Account, FaFeeAccountError> {
        self.requested_federations
            .lock()
            .expect("test lock")
            .push(*federation_id);
        self.account
            .clone()
            .ok_or_else(|| FaFeeAccountError::new("test FA fee account unavailable"))
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
        crate::ports::protocol_keypair(&Self.scoped_root())
    }

    fn fa_id() -> FaId {
        FaId(Self::keypair().x_only_public_key().0)
    }
}

impl FaIdentity for TestIdentity {
    fn scoped_root(&self) -> fedimint_derive_secret::DerivableSecret {
        fedimint_derive_secret::DerivableSecret::new_root(&[7; 64], b"fa-client-test-root")
    }
}

#[derive(Clone, Copy)]
struct OtherIdentity;

impl FaIdentity for OtherIdentity {
    fn scoped_root(&self) -> fedimint_derive_secret::DerivableSecret {
        fedimint_derive_secret::DerivableSecret::new_root(&[8; 64], b"fa-client-test-root")
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

impl FaNostrClient for TestRegistry {
    async fn fetch_mg_advertisement(
        &self,
        _mg_pubkey: PublicKey,
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

    async fn fetch_mg_advertisements(&self, _timeout: Duration) -> NostrClientResult<Vec<Event>> {
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
    MgConnected(usize),
    PaymentOutputPolled,
    SeatCheckpointed,
}

#[derive(Default)]
struct PaymentState {
    payable_calls: AtomicUsize,
    recover_calls: AtomicUsize,
    create_calls: AtomicUsize,
    refund_calls: AtomicUsize,
    funded_quotes: Mutex<HashSet<QuoteId>>,
    created_quotes: Mutex<Vec<QuoteId>>,
    settled_quotes: Mutex<HashSet<QuoteId>>,
    refund_contexts: Mutex<Vec<QuoteId>>,
    reject_recovery: AtomicBool,
    rejected_quotes: Mutex<HashSet<QuoteId>>,
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
    first_payment_acceptance_delay: Mutex<Duration>,
    payment_signatures_delay: Mutex<Duration>,
    hang_first_refund: AtomicBool,
    refund_delay: Mutex<Duration>,
    refund_started: Notify,
    pay_none: AtomicBool,
    insufficient_funds: AtomicBool,
    /// Balance the purchase fund check sees, or `None` for enough.
    checked_balance_msats: Mutex<Option<u64>>,
    /// Prices of every purchase fund check.
    fund_checks: Mutex<Vec<Vec<u64>>>,
    /// One-based payment call that finds insufficient funds.
    fail_readiness_on_call: AtomicUsize,
    reverse_payable: AtomicBool,
    /// Model a payer whose complete spendable balance is one ecash note.
    /// Funding removes that note until consensus returns its change, so a
    /// concurrently polled sibling observes no spendable input.
    single_note_wallet: AtomicBool,
    single_note_balance_msats: AtomicU64,
    single_note_fee_msats: AtomicU64,
    /// Settle under a generation the quote did not name, as a wallet whose
    /// dispatch disagrees with the terms it was handed would.
    settle_wrong_generation: AtomicBool,
    effect_log: Mutex<Option<Arc<Mutex<Vec<TestEffect>>>>>,
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
            // The test MG quotes mintv1 terms.
            settled_under: if self.0.settle_wrong_generation.load(Ordering::SeqCst) {
                MintGeneration::MintV2
            } else {
                MintGeneration::MintV1
            },
            refund_context: quote_id,
        }
    }
}

impl FaPayments for TestPayments {
    type RefundContext = QuoteId;

    async fn payable_federations(
        &self,
        admitted: &[FederationId],
    ) -> Result<Vec<FederationId>, FaPaymentError> {
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

    async fn check_seat_funds(
        &self,
        _federation_id: &FederationId,
        prices_msats: &[u64],
    ) -> Result<(), FaPaymentError> {
        self.0
            .fund_checks
            .lock()
            .expect("test lock")
            .push(prices_msats.to_vec());
        match *self.0.checked_balance_msats.lock().expect("test lock") {
            Some(balance) if prices_msats.iter().sum::<u64>() > balance => Err(
                FaPaymentError::insufficient_funds("the balance cannot fund the seats"),
            ),
            _ => Ok(()),
        }
    }

    async fn prepare_quote_refund(
        &self,
        _federation_id: &FederationId,
        _plan: &Plan,
    ) -> Result<RefundIssuance, FaPaymentError> {
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
        quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<SeatPaymentRecovery<Self::RefundContext>, FaPaymentError> {
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
            return Err(FaPaymentError::new("injected recovery failure"));
        }
        if self.0.reject_recovery.load(Ordering::SeqCst)
            || self
                .0
                .rejected_quotes
                .lock()
                .expect("test lock")
                .contains(&quote_id)
        {
            return Ok(SeatPaymentRecovery::Rejected);
        }
        if self
            .0
            .funded_quotes
            .lock()
            .expect("test lock")
            .contains(&quote_id)
        {
            let delay = *self.0.payment_signatures_delay.lock().expect("test lock");
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            Ok(SeatPaymentRecovery::Prepared(self.prepared(quote_id)))
        } else {
            Ok(SeatPaymentRecovery::NotStarted)
        }
    }

    async fn create_seat_payment(
        &self,
        quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<PreparedSeatPayment<Self::RefundContext>, FaPaymentError> {
        if let Some(log) = self.0.effect_log.lock().expect("test lock").as_ref() {
            log.lock()
                .expect("test effect log")
                .push(TestEffect::PaymentOutputPolled);
        }
        let call = self.0.create_calls.fetch_add(1, Ordering::SeqCst);
        let quote_id = quote.quote_id();
        if self.0.insufficient_funds.load(Ordering::SeqCst)
            || self.0.fail_readiness_on_call.load(Ordering::SeqCst) == call + 1
        {
            return Err(FaPaymentError::insufficient_funds(
                "insufficient funds including fees",
            ));
        }
        if self.0.single_note_wallet.load(Ordering::SeqCst) {
            let Plan::InfiniteBestEffort { price_msats } = quote.terms.request.plan else {
                panic!("single-note test payer supports one-time prices");
            };
            let debit_msats = price_msats
                .checked_add(self.0.single_note_fee_msats.load(Ordering::SeqCst))
                .expect("test payment debit fits u64");
            let note_msats = self.0.single_note_balance_msats.swap(0, Ordering::SeqCst);
            if note_msats == 0 {
                return Err(FaPaymentError::new(
                    "the payer's only ecash note is reserved by another payment",
                ));
            }
            if note_msats < debit_msats {
                self.0
                    .single_note_balance_msats
                    .store(note_msats, Ordering::SeqCst);
                return Err(FaPaymentError::insufficient_funds(
                    "single ecash note cannot cover the payment plus fees",
                ));
            }
            // Make the note unavailable to sibling wallet futures until this
            // accepted transaction's change becomes spendable.
            tokio::task::yield_now().await;
            self.0
                .single_note_balance_msats
                .store(note_msats - debit_msats, Ordering::SeqCst);
        }
        let inserted = self
            .0
            .funded_quotes
            .lock()
            .expect("test lock")
            .insert(quote_id);
        if !inserted {
            return Err(FaPaymentError::new(
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
        if call == 0 {
            let delay = *self
                .0
                .first_payment_acceptance_delay
                .lock()
                .expect("test lock");
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let delay = *self.0.payment_signatures_delay.lock().expect("test lock");
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
        Ok(self.prepared(quote_id))
    }

    async fn settle_seat_refund(
        &self,
        context: Self::RefundContext,
        _refund: RefundTransaction,
    ) -> Result<(), FaPaymentError> {
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
            return Err(FaPaymentError::new("refund context did not match"));
        }
        self.0
            .settled_quotes
            .lock()
            .expect("test lock")
            .insert(context);
        self.0
            .refund_contexts
            .lock()
            .expect("test lock")
            .push(context);
        if self.0.hang_first_refund.swap(false, Ordering::SeqCst) {
            self.0.refund_started.notify_one();
            return pending().await;
        }
        let delay = *self.0.refund_delay.lock().expect("test lock");
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CreateBehavior {
    Accept,
    HangFirst,
    RefuseFirstQuote,
}

#[derive(Clone, Copy)]
struct MgConfig {
    /// What this MG offers its seats at. Zero is a give-away: it quotes no
    /// payment terms and expects no payment federation.
    price_msats: u64,
    accepting_seats: bool,
    federation_size: FederationSize,
    hang_availability: bool,
    create_behavior: CreateBehavior,
    reject_quote: bool,
    invalid_quote_signature: bool,
    capacity_exhausted_quote: bool,
}

impl MgConfig {
    /// The MG offers one priced plan and the FA arranges a payment for it.
    fn paid() -> Self {
        Self {
            price_msats: PAYMENT_AMOUNT_MSATS,
            accepting_seats: true,
            federation_size: FederationSize(MIN_FEDERATION_SIZE),
            hang_availability: false,
            create_behavior: CreateBehavior::Accept,
            reject_quote: false,
            invalid_quote_signature: false,
            capacity_exhausted_quote: false,
        }
    }

    /// The bootstrap offer: seats at zero, for an FA with no way to pay.
    fn given_away() -> Self {
        Self {
            price_msats: 0,
            ..Self::paid()
        }
    }
}

#[derive(Clone)]
struct QuoteRecord {
    index: usize,
    plan: Plan,
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
struct MgState {
    connect_calls: AtomicUsize,
    connect_attempts: Mutex<HashMap<usize, usize>>,
    fail_connect_on_attempt: Mutex<Option<(usize, usize)>>,
    hang_connect_on_attempt: Mutex<Option<(usize, usize)>>,
    hang_connect: AtomicBool,
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
    /// MGs asked for a guardian code that carries the federation name.
    named_dkg_code_indices: Mutex<Vec<usize>>,
    unavailable_dkg_code_indices: Mutex<HashSet<usize>>,
    decommissioned_indices: Mutex<HashSet<usize>>,
    /// Seats report that DKG has not started.
    report_status_new: AtomicBool,
    /// Codes carry the federation name the request names, as real MGs do.
    name_sensitive_codes: AtomicBool,
    restart_calls: AtomicUsize,
    report_dkg_already_started: AtomicBool,
    start_callbacks: Mutex<Vec<Option<DkgCompletionCallback>>>,
    invite_calls: AtomicUsize,
    quote_records: Mutex<Vec<QuoteRecord>>,
    /// Per-MG price overrides for mixed paid/free and changed-price tests.
    price_overrides_msats: Mutex<HashMap<usize, u64>>,
    /// Plans every MG offers ahead of its own price.
    extra_plans: Mutex<Vec<Plan>>,
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
    meta_terminal_errors: Mutex<HashMap<usize, ManifoldGuardianError>>,
    /// Wakes cancellation tests after a fake accepts a metadata proposal.
    meta_submission_changed: Notify,
    attestation_calls: AtomicUsize,
    /// Replacement MG index -> original federation peer id it operates.
    attested_peer_overrides: Mutex<HashMap<usize, usize>>,
    /// Seat 0 attests a peer id outside the federation's config.
    attest_foreign_peer: AtomicBool,
    /// Seat 0 signs an account different from its earlier signed acceptance.
    attest_wrong_fee_account: AtomicBool,
    /// One entry per public trust-material request, preserving the MG queried.
    trust_material_requests: Mutex<Vec<usize>>,
    /// Corrupt the first MG trust-material response signature.
    corrupt_trust_material_signature: AtomicBool,
}

#[derive(Clone)]
struct TestConnector {
    state: Arc<MgState>,
    config: MgConfig,
}

impl ManifoldGuardianConnector for TestConnector {
    type Client = TestMg;

    async fn connect(
        &self,
        locator: &Locator,
    ) -> Result<Self::Client, ManifoldGuardianConnectorError> {
        let call = self.state.connect_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.state.hang_connect.load(Ordering::SeqCst) {
            return pending().await;
        }
        if self
            .state
            .connect_failure_on_call
            .compare_exchange(call, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Err(ManifoldGuardianConnectorError::new(
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
            return Err(ManifoldGuardianConnectorError::new(
                "injected connection failure",
            ));
        }
        let index = (0..=usize::from(MAX_FEDERATION_SIZE))
            .find(|index| manager_key(*index).x_only_public_key().0 == locator.service_pubkey)
            .ok_or_else(|| ManifoldGuardianConnectorError::new("unknown test locator"))?;
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
            return Err(ManifoldGuardianConnectorError::new(
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
                .push(TestEffect::MgConnected(index));
        }
        if self
            .state
            .offline_indices
            .lock()
            .expect("test lock")
            .contains(&index)
        {
            return Err(ManifoldGuardianConnectorError::new(
                "test Manifold Guardian is offline",
            ));
        }
        Ok(TestMg {
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
    ) -> Result<FmResult<GetAvailabilityResponse>, ManifoldGuardianCallError> {
        if self
            .state
            .availability_transport_failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            self.state.availability_calls.fetch_add(1, Ordering::SeqCst);
            return Err(ManifoldGuardianCallError::new(
                "injected availability stream loss",
            ));
        }
        Ok(client.get_availability(request).await)
    }

    async fn get_quote(
        &self,
        client: &Self::Client,
        request: GetQuoteRequest,
    ) -> Result<FmResult<SignedResponse<GetQuoteResponse>>, ManifoldGuardianCallError> {
        if self
            .state
            .quote_transport_failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            self.state.quote_calls.fetch_add(1, Ordering::SeqCst);
            return Err(ManifoldGuardianCallError::new("injected quote stream loss"));
        }
        Ok(client.get_quote(request).await)
    }
}

struct TestMg {
    index: usize,
    manager_key: Keypair,
    state: Arc<MgState>,
    config: MgConfig,
}

impl TestMg {
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

impl ManifoldGuardianService for TestMg {
    async fn get_availability(
        &self,
        _request: GetAvailabilityRequest,
    ) -> FmResult<GetAvailabilityResponse> {
        self.state.availability_calls.fetch_add(1, Ordering::SeqCst);
        if self.config.hang_availability {
            return pending().await;
        }
        let mut plans = self.state.extra_plans.lock().expect("test lock").clone();
        plans.push(Plan::InfiniteBestEffort {
            price_msats: self.price_msats(),
        });
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
        let signed_plan = request.plan.clone();
        if self.config.reject_quote {
            return Err(ManifoldGuardianError::Other(
                "test daemon rejects the selected payment federation".to_owned(),
            ));
        }
        if self.config.capacity_exhausted_quote {
            return Err(ManifoldGuardianError::CapacityExhausted);
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
        let signing_key = if self.config.invalid_quote_signature {
            manager_key(30)
        } else {
            self.manager_key
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
            &signing_key,
        )?;
        let quote_id = signed_quote
            .verify(&signing_key.x_only_public_key().0)?
            .quote_id();
        self.state
            .quote_records
            .lock()
            .expect("test lock")
            .push(QuoteRecord {
                index: self.index,
                plan: signed_plan,
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
        // Seat ids sort by MG, so formation numbers guardians in MG order as
        // the fakes do.
        let mut seat_bytes = quote_id.0;
        seat_bytes[0] = u8::try_from(self.index).expect("small test index");
        let seat_id = SeatId::from(QuoteId(seat_bytes));
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
            return Err(ManifoldGuardianError::Other(
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
            return Err(ManifoldGuardianError::Other(
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
        request: SignedRequest<GetDkgCodeRequest>,
    ) -> FmResult<GetDkgCodeResponse> {
        self.state.dkg_code_calls.fetch_add(1, Ordering::SeqCst);
        if self
            .state
            .decommissioned_indices
            .lock()
            .expect("test lock")
            .contains(&self.index)
        {
            return Err(ManifoldGuardianError::WrongState {
                status: ServiceStatus::Decommissioned,
            });
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock follows Unix epoch")
            .as_secs();
        let federation_name = request
            .verify(Timestamp(now))
            .expect("test FA request verifies")
            .into_inner()
            .federation_name;
        if federation_name.is_some() {
            self.state
                .named_dkg_code_indices
                .lock()
                .expect("test lock")
                .push(self.index);
        }
        // A real MG writes the leader-only federation name into its code.
        if self.state.name_sensitive_codes.load(Ordering::SeqCst) {
            return Ok(GetDkgCodeResponse {
                guardian_code: GuardianCode(match federation_name {
                    Some(name) => format!("guardiancode{}-{}", self.index, name.0),
                    None => format!("guardiancode{}", self.index),
                }),
            });
        }
        if self
            .state
            .unavailable_dkg_code_indices
            .lock()
            .expect("test lock")
            .contains(&self.index)
        {
            return Err(ManifoldGuardianError::SeatUnavailable);
        }
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
            .expect("test FA request verifies")
            .into_inner();
        self.state
            .start_callbacks
            .lock()
            .expect("test lock")
            .push(request.completion_callback);
        if self.state.report_dkg_already_started.load(Ordering::SeqCst) {
            Err(ManifoldGuardianError::WrongState {
                status: ServiceStatus::DkgInProcess,
            })
        } else {
            Ok(StartDkgResponse)
        }
    }

    async fn decommission_seat(
        &self,
        _request: SignedRequest<
            fedi_decentralized_service_manifold_guardian::DecommissionSeatRequest,
        >,
    ) -> FmResult<fedi_decentralized_service_manifold_guardian::DecommissionSeatResponse> {
        Ok(
            fedi_decentralized_service_manifold_guardian::DecommissionSeatResponse {
                already_decommissioned: false,
            },
        )
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
            status: if self.state.report_status_new.load(Ordering::SeqCst) {
                ServiceStatus::New
            } else {
                ServiceStatus::Running
            },
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
            attestation.proof.signature = mg_keys(self.index).sign_schnorr(&message);
        }
        Ok(GetPeerAttestationResponse {
            mg_peer_attestation: attestation,
            seat_endpoint_proof: fedi_decentralized_domain::SeatEndpointProof {
                signature: vec![0; 64],
            },
        })
    }

    async fn get_fman_trust_material(
        &self,
        _request: GetMgTrustMaterialRequest,
    ) -> FmResult<GetMgTrustMaterialResponse> {
        self.state
            .trust_material_requests
            .lock()
            .expect("test lock")
            .push(self.index);
        let now = test_now_secs();
        let keys = mg_keys(self.index);
        let material = fedi_decentralized_domain::MgTrustMaterial {
            mg_pubkey: fedi_decentralized_domain::Pubkey(keys.public_key().to_string()),
            issued_at: Timestamp(now),
            expires_at: Timestamp(
                now + crate::liquidity::FA_LIQUIDITY_TRUST_MATERIAL_VALIDITY.as_secs(),
            ),
            public_api_urls: vec![fedi_decentralized_domain::Url(format!(
                "iroh://{}",
                locator(self.index).endpoint_addr.id
            ))],
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
            mg_keys(self.index + 1)
        } else {
            keys
        };
        Ok(GetMgTrustMaterialResponse {
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
            return Err(ManifoldGuardianError::MetaConsensusChanged);
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
                return Err(ManifoldGuardianError::MetaConsensusChanged);
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
            return Err(ManifoldGuardianError::MetaConsensusChanged);
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
                return Err(ManifoldGuardianError::MetaConsensusChanged);
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
            fedi_decentralized_service_manifold_guardian::GuardianFeeAccount::try_from(
                guardian_verification_fee_account,
            )
            .unwrap();
        if request.guardian_verification_fee_account != guardian_verification_fee_account {
            return Err(ManifoldGuardianError::GuardianVerificationFeeAccountMismatch);
        }
        let bindings = fedi_decentralized_domain::MgSeatBindings::new(
            request
                .seat_bindings
                .iter()
                .map(|binding| binding.attestation.clone()),
        )
        .map_err(|error| ManifoldGuardianError::Other(error.to_string()))?;
        let canonical_bindings = bindings
            .canonical_string()
            .map_err(|error| ManifoldGuardianError::Other(error.to_string()))?;
        let mut recipients = bindings
            .seat_bindings()
            .iter()
            .map(|binding| {
                GuardianFeeRecipient::new(
                    fedi_decentralized_service_manifold_guardian::GuardianFeeAccount::try_from(
                        binding.attestation.guardian_fee_account.clone(),
                    )
                    .expect("test attestation account is valid"),
                    GUARDIAN_GUARDIAN_FEE_WEIGHT,
                )
            })
            .collect::<Vec<_>>();
        recipients.push(GuardianFeeRecipient::new(
            request.fa_fee_account,
            FA_GUARDIAN_FEE_WEIGHT,
        ));
        recipients.push(GuardianFeeRecipient::new(
            guardian_verification_fee_account.clone(),
            GUARDIAN_VERIFICATION_FEE_WEIGHT,
        ));
        recipients.sort_by_key(|recipient| recipient.account.as_account().id());
        let recipients = canonical_guardian_fee_recipient_list(&recipients)
            .map_err(|error| ManifoldGuardianError::Other(error.to_string()))?;
        self.state
            .meta_submissions
            .lock()
            .expect("test lock")
            .insert(
                self.index,
                (
                    fedi_decentralized_domain::MG_SEAT_BINDINGS_META_FIELD_KEY.to_owned(),
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

fn unsupported() -> ManifoldGuardianError {
    ManifoldGuardianError::Other("test operation unavailable".to_owned())
}

/// The final config the test federation converges on.
///
/// Shared with the MG fake's attestations, so the peer set `fa-client`
/// derives from this config is exactly the one the attestations bind to.
fn test_federation_config() -> fedimint_core::config::ClientConfig {
    let mut config =
        fedi_decentralized_domain::test_support::test_config(usize::from(MIN_FEDERATION_SIZE));
    config.global.meta.insert(
        FEDERATION_NAME_META_FIELD_KEY.to_owned(),
        "Test Federation".to_owned(),
    );
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

/// The MG operating keys for one seat.
///
/// Signing goes through nostr's `secp256k1`, not this crate's: they are
/// different major versions, and `MgPeerAttestation::verify` uses nostr's.
fn mg_keys(index: usize) -> Keys {
    // Production derives the Nostr/attestation identity and the RPC
    // commitment key from distinct HKDF labels. Keep the fixture domains
    // distinct so identity/locator conflation cannot pass tests.
    let byte = u8::try_from(index + 80).expect("small test index");
    Keys::new(nostr_sdk::SecretKey::from_slice(&[byte; 32]).expect("valid test secret"))
}

/// A genuinely signed attestation binding seat `index` to its MG.
fn test_attestation(index: usize) -> fedi_decentralized_domain::MgPeerAttestation {
    test_attestation_for_peer(index, index)
}

/// A fresh replacement MG identity binding the peer row it takes over.
fn test_attestation_for_peer(
    operator_index: usize,
    peer_index: usize,
) -> fedi_decentralized_domain::MgPeerAttestation {
    let seats = test_federation_seats();
    let seat = &seats.seats()[peer_index];
    let keys = mg_keys(operator_index);
    let attestation = fedi_decentralized_domain::MgPeerAttestationStatement {
        mg_pubkey: fedi_decentralized_domain::Pubkey(keys.public_key().to_string()),
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

    fedi_decentralized_domain::MgPeerAttestation {
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
/// `MgSeatBindings::new` and only fails when checked against the config.
fn foreign_peer_attestation() -> fedi_decentralized_domain::MgPeerAttestation {
    let seats = test_federation_seats();
    let keys = mg_keys(0);
    let attestation = fedi_decentralized_domain::MgPeerAttestationStatement {
        mg_pubkey: fedi_decentralized_domain::Pubkey(keys.public_key().to_string()),
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

    fedi_decentralized_domain::MgPeerAttestation {
        version: fedi_decentralized_domain::ProtocolV1,
        attestation,
        proof: fedi_decentralized_domain::SchnorrSignatureProof {
            signature: keys.sign_schnorr(&message),
        },
    }
}

/// Consensus reader backed by what the MG fakes actually accepted.
///
/// It reports a directory only once a consensus threshold of seats hold
/// byte-identical bytes, mirroring the real rule that a meta value becomes
/// consensus only when threshold guardians submit the same value. A fake that
/// echoed back whatever was written would make the readback vacuous — the
/// exact failure the port's contract warns about.
#[derive(Clone)]
struct TestConsensusReader {
    state: Arc<MgState>,
    config: fedimint_core::config::ClientConfig,
    failures: Arc<AtomicUsize>,
    hangs: Arc<AtomicUsize>,
    offline_peers: Arc<Mutex<BTreeSet<PeerId>>>,
    forced_value: Arc<Mutex<Option<String>>>,
    advance_meta_after_next_read: Arc<Mutex<Option<Vec<u8>>>>,
    reads_before_meta_advance: Arc<AtomicUsize>,
    revision_bump_after_next_read: Arc<AtomicU64>,
}

impl TestConsensusReader {
    fn new(state: Arc<MgState>) -> Self {
        Self {
            state,
            config: test_federation_config(),
            failures: Arc::new(AtomicUsize::new(0)),
            hangs: Arc::new(AtomicUsize::new(0)),
            offline_peers: Arc::new(Mutex::new(BTreeSet::new())),
            forced_value: Arc::new(Mutex::new(None)),
            advance_meta_after_next_read: Arc::new(Mutex::new(None)),
            reads_before_meta_advance: Arc::new(AtomicUsize::new(0)),
            revision_bump_after_next_read: Arc::new(AtomicU64::new(0)),
        }
    }

    fn check_invite(&self, invite: &InviteCode) -> Result<(), FederationConsensusError> {
        let peers = invite.0.parse::<FedimintInviteCode>().unwrap().peers();
        let offline = self.offline_peers.lock().expect("test lock");
        if peers.keys().all(|peer| offline.contains(peer)) {
            return Err(FederationConsensusError::new(
                "invite guardians are unreachable",
            ));
        }
        Ok(())
    }

    /// Report `value` as consensus regardless of what any seat accepted.
    fn force_value(&self, value: &str) {
        *self.forced_value.lock().expect("test lock") = Some(value.to_owned());
    }

    /// Fail the next `count` reads, as an unreachable federation would.
    fn fail_next(&self, count: usize) {
        self.failures.store(count, Ordering::SeqCst);
    }

    /// Never answer the next `count` reads, as a restarting guardian.
    fn hang_next(&self, count: usize) {
        self.hangs.store(count, Ordering::SeqCst);
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
                    && key == fedi_decentralized_domain::MG_SEAT_BINDINGS_META_FIELD_KEY
            })
            .map(|((_, value), _)| value)
    }

    fn agreed_maintenance_field(&self) -> Option<(String, String)> {
        let submissions = self.state.meta_submissions.lock().expect("test lock");
        let threshold = test_federation_seats().consensus_threshold() as usize;
        let mut counts: HashMap<(String, String), usize> = HashMap::new();
        for entry in submissions.values() {
            if entry.0 != fedi_decentralized_domain::MG_SEAT_BINDINGS_META_FIELD_KEY {
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
        invite_code: &InviteCode,
    ) -> Result<FederationConsensusSnapshot, FederationConsensusError> {
        self.check_invite(invite_code)?;
        if self
            .failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(FederationConsensusError::new("test federation unreachable"));
        }
        if self
            .hangs
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            std::future::pending::<()>().await;
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
                fedi_decentralized_domain::MG_SEAT_BINDINGS_META_FIELD_KEY: value,
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
            if !fields.contains_key(fedi_decentralized_domain::MG_SEAT_BINDINGS_META_FIELD_KEY)
                && let (Some(value), Some((send_ppm, recipients))) =
                    (self.agreed_value(), self.agreed_fee_policy())
            {
                fields.insert(
                    fedi_decentralized_domain::MG_SEAT_BINDINGS_META_FIELD_KEY.to_owned(),
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
        invite_code: &InviteCode,
    ) -> Result<Vec<GatewayApiUrl>, FederationConsensusError> {
        self.check_invite(invite_code)?;
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

fn set_mg_version(state: &MgState, index: usize, version: &str) {
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
            if federation == 0 {
                test_federation_config().calculate_federation_id()
            } else {
                format!("{federation:064x}")
                    .parse()
                    .expect("valid test federation id")
            },
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

/// A verified seat for the test MG at `index`, advertised at `price_msats`.
fn selected_seat(index: usize, price_msats: u64) -> crate::selection::SelectedMgSeat {
    let now = test_now_secs();
    crate::selection::SelectedMgSeat {
        candidate: crate::selection::VerifiedCandidate {
            mg_id: test_mg_id(index),
            api_endpoints: Vec::new(),
            locator: locator(index),
            advertised_price_msats: price_msats,
            availability: fedi_decentralized_nostr::mg::Availability {
                fedimintd_version: fedimintd_version(),
                federation_sizes: vec![MIN_FEDERATION_SIZE],
            },
            issued_at: Timestamp(now),
            expires_at: Timestamp(now + 3_600),
            badge: crate::selection::VerifiedBadgeFacts {
                issuer: test_mg_id(60),
                holder: test_mg_id(index),
                subject: test_mg_id(index),
                badge: fedi_decentralized_domain::TrustScoreBadgeV1 {
                    trust_level: 10,
                    holder_pubkey: test_mg_id(index),
                },
            },
        },
        provenance: SeatProvenance::FediAttested,
    }
}

/// A fresh preview of `seats`, each `(index, advertised price)`.
fn preview_of(
    request: MgSelectionRequest,
    seats: impl IntoIterator<Item = (usize, u64)>,
) -> MgSelectionPreview {
    let minimum = request.fedimintd_versions().minimum();
    let fedimintd_dkg_version = format!("{minimum}+fedi")
        .parse::<FedimintdVersion>()
        .expect("a release with the Fedi vendor is valid SemVer")
        .dkg_version();
    MgSelectionPreview::for_test(
        request,
        fedimintd_dkg_version,
        test_peer_badge_verifier().provenance(),
        seats
            .into_iter()
            .map(|(index, price_msats)| selected_seat(index, price_msats))
            .collect(),
        Timestamp(test_now_secs() + 120),
    )
}

/// A fresh preview of the seven test MGs, each at the paid test price.
fn selection_preview() -> MgSelectionPreview {
    preview_of(
        MgSelectionRequest::new(
            FederationSize(MIN_FEDERATION_SIZE),
            fedimintd_version_range(),
            PlanPreference::InfiniteBestEffort,
        )
        .expect("valid test selection request"),
        (0..usize::from(MIN_FEDERATION_SIZE)).map(|index| (index, PAYMENT_AMOUNT_MSATS)),
    )
}

/// [`selection_preview`] for [`compatible_intent`].
fn compatible_selection_preview() -> MgSelectionPreview {
    preview_of(
        compatible_intent().request,
        (0..usize::from(MIN_FEDERATION_SIZE)).map(|index| (index, PAYMENT_AMOUNT_MSATS)),
    )
}

/// The index of a locator built by [`locator`].
fn locator_index(seat_locator: &Locator) -> usize {
    (0..64)
        .find(|index| locator(*index) == *seat_locator)
        .expect("test locators come from locator()")
}

impl TestClient {
    /// Buy one seat at each locator, as if a fresh preview had selected
    /// them, and drive only the purchase. A priced MG is paid from the test
    /// payment federation when this FA can pay. Without a creation this
    /// starts one for `intent`; with one, it buys the missing seats.
    async fn buy_seats_at(
        &self,
        intent: &TestIntent,
        locators: Vec<Locator>,
        options: Timeouts,
    ) -> FaResult<()> {
        self.request_seats_at(intent, locators, None).await?;
        self.drive_purchase_for_test(options).await
    }

    /// Save the purchase of one seat at each locator without driving it.
    async fn request_seats_at(
        &self,
        intent: &TestIntent,
        locators: Vec<Locator>,
        completion_callback: Option<DkgCompletionCallback>,
    ) -> FaResult<()> {
        let payer = (self.inner.ports.mg_connector.config.price_msats > 0
            && self.inner.setup_payment_publisher.is_some())
        .then(payment_federation_id);
        if self.status().creation.is_some() {
            let preview = self.missing_preview_at(locators).await;
            self.buy_seats(&preview, payer).await
        } else {
            self.create_federation(
                intent.federation_name.clone(),
                completion_callback,
                &self.preview_at(intent, locators),
                payer,
            )
            .await
        }
    }

    /// A fresh preview of one seat at each locator, at the price its test MG
    /// offers now.
    fn preview_at(&self, intent: &TestIntent, locators: Vec<Locator>) -> MgSelectionPreview {
        preview_of(intent.request.clone(), self.live_prices(&locators))
    }

    /// A fresh preview of the creation's missing seats at `locators`.
    async fn missing_preview_at(&self, locators: Vec<Locator>) -> MgSelectionPreview {
        let creation = self
            .inner
            .store
            .load_creation(TestIdentity::fa_id())
            .await
            .unwrap()
            .expect("a creation needs seats");
        let bought = u16::try_from(
            self.inner
                .store
                .pool_seats(TestIdentity::fa_id())
                .await
                .unwrap()
                .len(),
        )
        .unwrap();
        let request = MgSelectionRequest::new(
            creation.federation_size,
            creation.fedimintd_versions,
            creation.plan,
        )
        .unwrap()
        .missing_seats(
            self.inner
                .store
                .bought_seats(TestIdentity::fa_id())
                .await
                .unwrap(),
            bought,
        );
        preview_of(request, self.live_prices(&locators))
    }

    fn live_prices(&self, locators: &[Locator]) -> Vec<(usize, u64)> {
        let connector = &self.inner.ports.mg_connector;
        let overrides = connector
            .state
            .price_overrides_msats
            .lock()
            .expect("test lock");
        locators
            .iter()
            .map(|seat_locator| {
                let index = locator_index(seat_locator);
                let price_msats = overrides
                    .get(&index)
                    .copied()
                    .unwrap_or(connector.config.price_msats);
                (index, price_msats)
            })
            .collect()
    }

    async fn buy_and_form_at(
        &self,
        intent: TestIntent,
        locators: Vec<Locator>,
        options: Timeouts,
    ) -> FaResult<()> {
        self.request_seats_at(&intent, locators, None).await?;
        self.drive_all_for_test(options).await
    }

    async fn buy_and_form_at_with_callback(
        &self,
        intent: TestIntent,
        locators: Vec<Locator>,
        completion_callback: DkgCompletionCallback,
        options: Timeouts,
    ) -> FaResult<()> {
        self.request_seats_at(&intent, locators, Some(completion_callback))
            .await?;
        self.drive_all_for_test(options).await
    }
}

/// The seats and federation a test buys and forms in one call.
#[derive(Clone, Debug)]
struct TestIntent {
    federation_name: Option<FederationName>,
    request: MgSelectionRequest,
}

impl TestIntent {
    fn new(
        federation_name: Option<FederationName>,
        federation_size: FederationSize,
        plan: PlanPreference,
        fedimintd_versions: FedimintdVersionRange,
    ) -> FaResult<Self> {
        if let Some(name) = &federation_name {
            crate::state::validate_federation_name(name)?;
        }
        Ok(Self {
            federation_name,
            request: MgSelectionRequest::new(federation_size, fedimintd_versions, plan)?,
        })
    }

    fn resolve(self, fedimintd_dkg_version: FedimintdDkgVersion) -> FaResult<ResolvedSeatPurchase> {
        Ok(ResolvedSeatPurchase {
            federation_size: self.request.federation_size(),
            plan: self.request.plan(),
            fedimintd_versions: self.request.fedimintd_versions().clone(),
            fedimintd_dkg_version,
        })
    }
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    async fn buy_and_form(
        &self,
        intent: TestIntent,
        preview: &MgSelectionPreview,
        payment_federation_id: FederationId,
        options: Timeouts,
    ) -> FaResult<()> {
        self.create_federation(
            intent.federation_name,
            None,
            preview,
            Some(payment_federation_id),
        )
        .await?;
        self.drive_all_for_test(options).await
    }

    async fn buy_and_form_with_callback(
        &self,
        intent: TestIntent,
        preview: &MgSelectionPreview,
        payment_federation_id: FederationId,
        completion_callback: DkgCompletionCallback,
        options: Timeouts,
    ) -> FaResult<()> {
        self.create_federation(
            intent.federation_name,
            Some(completion_callback),
            preview,
            Some(payment_federation_id),
        )
        .await?;
        self.drive_all_for_test(options).await
    }

    async fn buy_and_form_without_payer(
        &self,
        intent: TestIntent,
        preview: &MgSelectionPreview,
        options: Timeouts,
    ) -> FaResult<()> {
        self.create_federation(intent.federation_name, None, preview, None)
            .await?;
        self.drive_all_for_test(options).await
    }

    /// Create the test federation from `preview` and drive only its
    /// purchase.
    async fn create_and_buy(
        &self,
        preview: &MgSelectionPreview,
        payment_federation_id: Option<FederationId>,
        options: Timeouts,
    ) -> FaResult<()> {
        self.create_federation(
            intent().federation_name,
            None,
            preview,
            payment_federation_id,
        )
        .await?;
        self.drive_purchase_for_test(options).await
    }

    /// [`Self::drive_with_options`] with the default options.
    async fn drive(&self) -> FaResult<()> {
        self.drive_with_options(Timeouts::default()).await
    }

    /// The import step of a backup lookup, on a payload the relays returned.
    async fn import_backup_for_test(
        &self,
        payload: Option<crate::backup::FaBackupPayload>,
    ) -> FaResult<()> {
        let options = Timeouts::default();
        let fa_id = self.fa_id()?;
        let run = options;
        self.import_backup(payload, &staging_recovery(), fa_id, run)
            .await
    }

    /// Drive the running purchase, or without one every later piece of
    /// work, as [`Self::drive_all_for_test`] does.
    async fn drive_with_options(&self, options: Timeouts) -> FaResult<()> {
        let purchase_only = self
            .inner
            .store
            .load_creation(self.fa_id()?)
            .await?
            .is_some_and(|creation| creation.purchase.is_some());
        self.drive_steps_for_test(options, purchase_only).await
    }

    /// Do the run loop's work until none is left, without its retries, and
    /// return the first error. Each step is the work `run` would choose.
    async fn drive_all_for_test(&self, options: Timeouts) -> FaResult<()> {
        self.drive_steps_for_test(options, false).await
    }

    /// [`Self::drive_with_options`], limited to the running purchase.
    async fn drive_purchase_for_test(&self, options: Timeouts) -> FaResult<()> {
        self.drive_steps_for_test(options, true).await
    }

    async fn drive_steps_for_test(&self, options: Timeouts, purchase_only: bool) -> FaResult<()> {
        let _run = self.inner.run_guard.try_lock().map_err(|_| FaError::Busy)?;
        let fa_id = self.fa_id()?;
        loop {
            let status = self.status();
            if status.recovery.is_some() {
                return Ok(());
            }
            let creation = self.inner.store.load_creation(fa_id).await?;
            let buying = creation
                .as_ref()
                .is_some_and(|creation| creation.purchase.is_some());
            if purchase_only && !buying {
                return Ok(());
            }
            let Some(recovery) = creation.filter(|creation| buying || creation.is_full()) else {
                return Ok(());
            };
            let run = options;
            if buying {
                self.drive_purchase(recovery, fa_id, run).await?;
            } else {
                self.drive_formation(recovery, fa_id, run).await?;
            }
        }
    }
}

fn intent() -> TestIntent {
    TestIntent::new(
        Some(FederationName("Test Federation".to_owned())),
        FederationSize(MIN_FEDERATION_SIZE),
        PlanPreference::InfiniteBestEffort,
        fedimintd_version_range(),
    )
    .unwrap()
}

#[test]
fn formation_intent_accepts_product_size_boundaries_only() {
    for size in [MIN_FEDERATION_SIZE, MAX_FEDERATION_SIZE_EXCLUSIVE - 1] {
        assert!(
            TestIntent::new(
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
            TestIntent::new(
                None,
                FederationSize(size),
                PlanPreference::InfiniteBestEffort,
                fedimintd_version_range(),
            )
            .is_err()
        );
    }
}

fn resolved_intent() -> ResolvedSeatPurchase {
    intent()
        .resolve(fedimintd_version().dkg_version())
        .expect("valid test intent")
}

fn resolved_intent_with_size(federation_size: FederationSize) -> ResolvedSeatPurchase {
    let mut intent = resolved_intent();
    intent.federation_size = federation_size;
    intent
}

fn test_mg_id(index: usize) -> nostr_sdk::PublicKey {
    mg_keys(index).public_key()
}

fn selected_initial_seat(index: u16) -> crate::db::PickedSeat {
    crate::db::PickedSeat {
        locator: locator(usize::from(index)),
        mg_id: test_mg_id(usize::from(index)),
        holder: test_mg_id(usize::from(index)),
        plan: Plan::InfiniteBestEffort {
            price_msats: PAYMENT_AMOUNT_MSATS,
        },
    }
}

fn signed_quote(
    manager_index: usize,
    intent: &ResolvedSeatPurchase,
    quote_nonce: [u8; 32],
) -> SignedResponse<GetQuoteResponse> {
    signed_quote_at_price(manager_index, intent, quote_nonce, PAYMENT_AMOUNT_MSATS)
}

fn signed_quote_at_price(
    manager_index: usize,
    intent: &ResolvedSeatPurchase,
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
                    fa_id: TestIdentity::fa_id(),
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
        endpoint_hint: Some(Url("iroh://provider".to_owned())),
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
    store: db::FaStore,
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
    fn new(store: db::FaStore) -> Arc<Self> {
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
            .expect("FA persisted the exact operation before provider mutation");
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
    Arc<MgState>,
    TestLiquidityConnector,
) {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
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
        mg_state.clone(),
        MgConfig::given_away(),
        registry,
    )
    .await;
    client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .expect("form selected test federation");
    let formation_id = federation(&client.status()).formation_id.clone();
    let provider = TestLiquidityProviderState::new(client.inner.store.clone());
    (
        client,
        formation_id,
        mg_state,
        TestLiquidityConnector(provider),
    )
}

#[tokio::test]
async fn schema_11_partial_purchase_stays_a_purchase() {
    let database = MemDatabase::new().into_database();
    crate::db::FaStore::new(database.clone())
        .install_schema_11_fixture_for_test(crate::db::test_support::Schema11Fixture::Purchase)
        .await;
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;

    let status = client.status();
    assert_eq!(
        purchase(&status).federation_size,
        FederationSize(MIN_FEDERATION_SIZE)
    );
    // The accepted seats are in the pool, and the rest are still bought.
    assert_eq!(pending_indices(&status), [3, 4, 5, 6]);
    assert_eq!(accepted_seats(&status).len(), 3);
    assert!(!is_forming(&status));
    assert!(status.federation.is_none());
}

#[tokio::test]
async fn schema_11_seat_never_bound_leaves_without_a_previewed_plan() {
    let database = MemDatabase::new().into_database();
    crate::db::FaStore::new(database.clone())
        .install_schema_11_unbound_seat_for_test(4)
        .await;
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;

    // Schema 11 kept no plan, so the seat has no approved price to request.
    let status = client.status();
    assert_eq!(pending_indices(&status), [3, 5, 6]);
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        recovery.reauthorization,
        Some(SelectionReauthorizationReason::SeatUnavailable)
    );
    assert_eq!(accepted_seats(&status).len(), 3);
}

#[tokio::test]
async fn schema_11_driver_lease_row_is_removed() {
    use fedimint_core::db::IDatabaseTransactionOpsCore;

    let database = MemDatabase::new().into_database();
    crate::db::FaStore::new(database.clone())
        .install_schema_11_fixture_for_test(crate::db::test_support::Schema11Fixture::Purchase)
        .await;
    let mut dbtx = database.begin_transaction().await;
    dbtx.raw_insert_bytes(&[0x02], b"stale lease")
        .await
        .unwrap();
    dbtx.commit_tx().await;
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database.clone(),
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;

    purchase(&client.status());
    let mut dbtx = database.begin_transaction_nc().await;
    assert_eq!(dbtx.raw_get_bytes(&[0x02]).await.unwrap(), None);
}

#[tokio::test]
async fn schema_11_seat_waiting_for_replacement_leaves_the_purchase() {
    let database = MemDatabase::new().into_database();
    crate::db::FaStore::new(database.clone())
        .install_schema_11_replacement_for_test(3, None)
        .await;
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database.clone(),
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;

    let status = client.status();
    assert_eq!(pending_indices(&status), [4, 5, 6]);
    let recovery = active_recovery(
        crate::db::FaStore::new(database)
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    assert!(
        recovery
            .slots
            .iter()
            .filter(|seat| seat.seat_id().is_none())
            .all(|seat| seat.state.bound_quote().is_some()),
        "the unpaid seats keep their bound quotes",
    );
    assert_eq!(accepted_seats(&status).len(), 3);
}

#[tokio::test]
async fn schema_11_applied_replacement_leaves_the_purchase() {
    let database = MemDatabase::new().into_database();
    crate::db::FaStore::new(database.clone())
        .install_schema_11_replacement_for_test(3, Some((locator(7), test_mg_id(7))))
        .await;
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;

    // The replacement had no effect yet and no wallet reservation, so it
    // leaves with the seat it replaced.
    let status = client.status();
    assert_eq!(pending_indices(&status), [4, 5, 6]);
    assert!(
        purchase(&status)
            .slots
            .iter()
            .all(|seat| seat.mg_id != test_mg_id(7))
    );
    assert_eq!(accepted_seats(&status).len(), 3);
}

#[tokio::test]
async fn schema_11_dkg_moves_seats_into_a_formation_that_finishes() {
    let database = MemDatabase::new().into_database();
    crate::db::FaStore::new(database.clone())
        .install_schema_11_fixture_for_test(crate::db::test_support::Schema11Fixture::Dkg)
        .await;
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;

    let status = client.status();
    assert!(!is_buying(&status));
    let creation = status
        .creation
        .as_ref()
        .expect("the migration keeps a creation");
    assert_eq!(
        creation.federation_name,
        FederationName("Test Federation".to_owned())
    );
    assert_eq!(
        creation.federation_size,
        FederationSize(MIN_FEDERATION_SIZE)
    );
    // Every seat keeps its schema 11 index, so the pool and the formation
    // list them in that order.
    let pool = accepted_seats(&status)
        .iter()
        .map(|seat| seat.seat_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(pool.len(), usize::from(MIN_FEDERATION_SIZE));
    let formation = formation(&status);
    assert_eq!(
        formation
            .slots
            .iter()
            .map(|seat| (seat.index, seat.seat_id.clone()))
            .collect::<Vec<_>>(),
        pool.into_iter()
            .enumerate()
            .map(|(index, seat_id)| (u16::try_from(index).unwrap(), seat_id))
            .collect::<Vec<_>>()
    );
    assert_eq!(formation_phase(&status), FormationPhase::PreparingDkg);
    assert!(formation_invite(&status).is_none());
    assert_eq!(
        formation.federation_name,
        FederationName("Test Federation".to_owned())
    );
    // The six codes that arrived stay cached against their seats; only the
    // seat stored at index 5 still needs its code.
    for seat in &formation.slots {
        let seat_id = seat.seat_id.as_ref().unwrap().to_string();
        assert_eq!(
            seat.guardian_code.is_none(),
            seat.index == 5,
            "seat {seat_id}"
        );
        assert_eq!(
            seat.index == 5,
            seat_id.starts_with("bfd0b333"),
            "seat {seat_id}"
        );
    }

    // The FA reads the invite from the first seat of the formation.
    let first_mg = (0..usize::from(MIN_FEDERATION_SIZE))
        .find(|index| test_mg_id(*index) == formation.slots[0].mg_id)
        .unwrap();

    // Each MG re-reports its deterministic code, so a cached code moved to
    // the wrong seat fails the run.
    client.drive().await.unwrap();
    // Schema 11 named the code of its seat at index 0, which belongs to MG 0.
    // That seat stays first, though it is not the first in seat-id order.
    assert_eq!(
        *mg_state.named_dkg_code_indices.lock().expect("test lock"),
        [0]
    );
    assert_eq!(formation.slots[0].mg_id, test_mg_id(0));
    assert!(
        formation
            .slots
            .iter()
            .any(|seat| seat.seat_id < formation.slots[0].seat_id)
    );
    let status = client.status();
    assert!(!is_forming(&status));
    assert!(status.creation.is_none());
    assert!(accepted_seats(&status).is_empty());
    let federation = federation(&status);
    assert_eq!(federation.federation_invite, test_invite(first_mg));
    client.inner.store.backup_payload().await.unwrap();
}

#[tokio::test]
async fn schema_11_dkg_complete_resumes_at_metadata_with_its_invite() {
    for fixture in [
        crate::db::test_support::Schema11Fixture::DkgComplete,
        crate::db::test_support::Schema11Fixture::PublishingSeatBindings,
    ] {
        let database = MemDatabase::new().into_database();
        crate::db::FaStore::new(database.clone())
            .install_schema_11_fixture_for_test(fixture)
            .await;
        let (payments, _) = TestPayments::new();
        let mg_state = Arc::new(MgState::default());
        let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;

        let status = client.status();
        assert!(!is_buying(&status), "{fixture:?}");
        assert!(status.federation.is_none(), "{fixture:?}");
        assert!(status.creation.is_some(), "{fixture:?}");
        assert_eq!(
            accepted_seats(&status).len(),
            usize::from(MIN_FEDERATION_SIZE)
        );
        let formation = formation(&status);
        assert_eq!(
            formation_phase(&status),
            match fixture {
                crate::db::test_support::Schema11Fixture::DkgComplete => {
                    FormationPhase::DkgComplete
                }
                _ => FormationPhase::PublishingSeatBindings,
            },
            "{fixture:?}"
        );
        assert_eq!(
            formation_invite(&status),
            Some(test_invite(0)),
            "{fixture:?}"
        );
        assert!(
            formation
                .slots
                .iter()
                .all(|seat| seat.guardian_code.is_some()),
            "{fixture:?}"
        );

        client.drive().await.unwrap();
        let status = client.status();
        assert!(status.creation.is_none(), "{fixture:?}");
        assert!(accepted_seats(&status).is_empty(), "{fixture:?}");
        assert_eq!(
            federation(&status).federation_invite,
            test_invite(0),
            "{fixture:?}"
        );
        assert_eq!(
            mg_state.start_callbacks.lock().unwrap().len(),
            0,
            "{fixture:?}"
        );
        assert_eq!(
            mg_state.restart_calls.load(Ordering::SeqCst),
            0,
            "{fixture:?}"
        );
        if matches!(
            fixture,
            crate::db::test_support::Schema11Fixture::PublishingSeatBindings
        ) {
            // The saved target is proposed exactly, not rebuilt.
            let consensus: serde_json::Value = serde_json::from_slice(
                &crate::db::test_support::schema_11_formed_meta_consensus_for_test(),
            )
            .unwrap();
            let saved = consensus[fedi_decentralized_domain::MG_SEAT_BINDINGS_META_FIELD_KEY]
                .as_str()
                .unwrap()
                .to_owned();
            let submissions = mg_state.meta_submissions.lock().unwrap();
            assert!(!submissions.is_empty());
            for (_, value) in submissions.values() {
                assert_eq!(value, &saved);
            }
        }
    }
}

#[tokio::test]
async fn schema_11_formed_federation_keeps_its_record_and_rechecks() {
    for fixture in [
        crate::db::test_support::Schema11Fixture::Formed,
        crate::db::test_support::Schema11Fixture::FormedWithoutFederationRecord,
    ] {
        let database = MemDatabase::new().into_database();
        crate::db::FaStore::new(database.clone())
            .install_schema_11_fixture_for_test(fixture)
            .await;
        let (payments, _) = TestPayments::new();
        let state = Arc::new(MgState::default());
        *state.meta_consensus_raw.lock().unwrap() =
            Some(crate::db::test_support::schema_11_formed_meta_consensus_for_test());
        let client = open_client(database, payments, state, MgConfig::paid()).await;

        let status = client.status();
        assert!(!is_buying(&status), "{fixture:?}");
        assert!(!is_forming(&status), "{fixture:?}");
        let federation = federation(&status);
        assert_eq!(federation.federation_invite, test_invite(0), "{fixture:?}");
        assert_eq!(federation.seats.len(), usize::from(MIN_FEDERATION_SIZE));
        // Only the federation record remains.
        assert!(status.creation.is_none(), "{fixture:?}");
        assert!(accepted_seats(&status).is_empty(), "{fixture:?}");
        let store = &client.inner.store;
        let fa_id = TestIdentity::fa_id();
        assert!(store.load_creation(fa_id).await.unwrap().is_none());
        assert!(store.load_purchase(fa_id).await.unwrap().is_none());
        assert!(
            store
                .pool_seats(TestIdentity::fa_id())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(!store.holds_creation_rows_for_test().await, "{fixture:?}");
        let backup = client.inner.store.backup_payload().await.unwrap();
        assert_eq!(backup.payload.federation_invite, test_invite(0));

        client.drive().await.unwrap();
    }
}

#[tokio::test]
async fn the_read_invite_survives_a_reopen_and_stays_out_of_the_backup() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();
    let saved = federation(&client.status()).federation_invite.clone();
    let read = client
        .inner
        .store
        .read_invite(&saved)
        .await
        .expect("formation stores the guardian addresses it read");
    assert_ne!(read, saved, "the read invite lists every guardian");
    drop(client);

    let reopened = open_client(database, payments, mg_state, MgConfig::given_away()).await;
    assert_eq!(reopened.consensus_invite(&saved).await, read);
    let backup = reopened.inner.store.backup_payload().await.unwrap();
    assert_eq!(backup.payload.federation_invite, saved);
}

#[tokio::test]
async fn backup_payload_is_lean_stable_and_requires_confirmed_formed_metadata() {
    let (client, _, _, _) = formed_client_for_liquidity().await;
    let first = client.inner.store.backup_payload().await.unwrap();
    let second = client.inner.store.backup_payload().await.unwrap();
    assert_eq!(first.payload.snapshot_generation, 1);
    assert_eq!(second.payload.snapshot_generation, 1);
    assert_eq!(first.created_at, second.created_at);
    assert_eq!(first.payload.federation_invite, test_invite(0));
    assert_eq!(first.payload.seats.len(), usize::from(MIN_FEDERATION_SIZE));
    assert!(first.payload.seats.iter().all(|seat| {
        seat.seat_id.to_string().len() == 64 && seat.locator.service_pubkey.serialize().len() == 32
    }));
    assert!(first.payload.liquidity.is_none());

    // The purpose-built type cannot carry internal rows: its serialized field
    // set pins the inclusion/exclusion boundary directly.
    let value = serde_json::to_value(&first.payload).unwrap();
    let fields = value
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        fields,
        BTreeSet::from([
            "federation_invite".to_owned(),
            "schema_version".to_owned(),
            "seats".to_owned(),
            "snapshot_generation".to_owned(),
        ])
    );
    let seat_fields = value["seats"][0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        seat_fields,
        BTreeSet::from([
            "fman_identity".to_owned(),
            "locator".to_owned(),
            "seat_id".to_owned(),
        ])
    );
}

async fn open_with_backup_recovery_hint(
    database: fedimint_core::db::Database,
    hint: BackupRecoveryHint,
) -> TestClient {
    let (payments, _) = TestPayments::new();
    let state = Arc::new(MgState::default());
    FaClient::open_with_manifold_profile_and_recovery(
        database,
        TestIdentity,
        payments,
        TestRegistry::default(),
        TestConnector {
            state: state.clone(),
            config: MgConfig::given_away(),
        },
        test_peer_badge_verifier(),
        TestConsensusReader::new(state),
        TestFaFeeAccountProvider::default(),
        ManifoldEnvironment::Staging.profile().unwrap(),
        hint,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn backup_lookup_blocks_actions_until_it_ends() {
    let database = MemDatabase::new().into_database();
    let fresh = open_with_backup_recovery_hint(database.clone(), BackupRecoveryHint::Skip).await;
    assert_eq!(fresh.status(), FaStatus::default());
    drop(fresh);

    let restored =
        open_with_backup_recovery_hint(database.clone(), BackupRecoveryHint::RestoredMnemonic)
            .await;
    assert_eq!(restored.status(), recovering(None));
    assert!(matches!(
        restored.claim_run_guard().await,
        Err(FaError::Busy)
    ));
    // A relay quorum without a backup ends the lookup for good.
    restored.import_backup_for_test(None).await.unwrap();
    assert_eq!(restored.status(), FaStatus::default());
    assert!(restored.claim_run_guard().await.is_ok());
    drop(restored);

    let checked =
        open_with_backup_recovery_hint(database, BackupRecoveryHint::RestoredMnemonic).await;
    assert_eq!(checked.status(), FaStatus::default());
}

#[tokio::test]
async fn run_loop_retries_a_backup_lookup_without_a_relay_quorum() {
    let state = Arc::new(MgState::default());
    let (payments, _) = TestPayments::new();
    let client = FaClient::open_inner(
        MemDatabase::new().into_database(),
        FaClientPorts {
            identity: TestIdentity,
            payments,
            registry: TestRegistry::default(),
            mg_connector: TestConnector {
                state: state.clone(),
                config: MgConfig::given_away(),
            },
            consensus_reader: TestConsensusReader::new(state),
            fa_fee_account_provider: Arc::new(TestFaFeeAccountProvider::default()),
        },
        test_peer_badge_verifier(),
        None,
        None,
        Some(crate::backup_worker::BackupSource {
            relays: Vec::new(),
            required_reads: 1,
            completed: staging_recovery(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(client.status(), recovering(None));
    let mut state = client.observe();
    let failed = state.wait_for(|state| *state != FaState::Restoring(RecoveryCheck::default()));
    tokio::select! {
        () = client.run_with(options()) => unreachable!("the run loop does not return"),
        result = tokio::time::timeout(Duration::from_secs(5), failed) => {
            result.expect("the loop runs the lookup").unwrap();
        }
    }
    assert_eq!(client.status(), recovering(Some(FaErrorCode::Registry)));
    assert!(
        !client
            .inner
            .store
            .recovery_completed(&staging_recovery())
            .await
    );
    assert!(matches!(client.claim_run_guard().await, Err(FaError::Busy)));
}

#[tokio::test]
async fn empty_backup_recovery_completion_survives_reopen_and_is_environment_scoped() {
    let database = MemDatabase::new().into_database();
    let store = crate::db::FaStore::new(database.clone());
    let completed = crate::db::RecoveryCompletedKey::new(ManifoldEnvironment::Production);
    let different_environment = crate::db::RecoveryCompletedKey::new(ManifoldEnvironment::Staging);
    assert!(!store.recovery_completed(&completed).await);
    store.complete_empty_recovery(&completed).await.unwrap();
    let reopened = crate::db::FaStore::new(database);
    assert!(reopened.recovery_completed(&completed).await);
    assert!(!reopened.recovery_completed(&different_environment).await);
    assert!(reopened.load_status(TestIdentity::fa_id()).await.unwrap() == FaStatus::default());
}

#[tokio::test]
async fn verified_backup_import_commits_federation_and_lookup_together() {
    let (source, _, state, _) = formed_client_for_liquidity().await;
    let keys = crate::backup::FaBackupKeys::derive(&source.inner.ports.identity.scoped_root());
    let encrypted = keys
        .seal(&source.inner.store.backup_payload().await.unwrap().payload)
        .unwrap();
    let mut newer_payload = source.inner.store.backup_payload().await.unwrap().payload;
    newer_payload.snapshot_generation = 4;
    let newer = keys.seal(&newer_payload).unwrap();
    let invalid = crate::backup::EncryptedFaBackup::from_bytes(b"not base64".to_vec()).err();
    assert!(invalid.is_some());
    let (payments, _) = TestPayments::new();
    let restored = open_client(
        MemDatabase::new().into_database(),
        payments,
        state,
        MgConfig::given_away(),
    )
    .await;
    let payload = [&encrypted, &newer]
        .into_iter()
        .filter_map(|candidate| keys.open(candidate).ok())
        .max_by_key(|payload| payload.snapshot_generation)
        .unwrap();
    assert_eq!(payload.snapshot_generation, 4);
    restored
        .import_backup_for_test(Some(payload))
        .await
        .unwrap();
    assert!(
        restored
            .inner
            .store
            .recovery_completed(&staging_recovery())
            .await,
        "the import and the completed lookup commit together"
    );
    let snapshot = restored.status().federation.expect("restored status");
    assert_eq!(snapshot.seats.len(), usize::from(MIN_FEDERATION_SIZE));
    assert!(
        restored
            .buy_and_form_at(intent(), locators(), options())
            .await
            .is_err()
    );
    assert!(matches!(
        restored.import_backup_for_test(Some(newer_payload)).await,
        Err(FaError::Storage(_))
    ));
}

#[tokio::test]
async fn backup_import_keeps_cached_setup_payment_policy() {
    let (source, _, _, _) = formed_client_for_liquidity().await;
    let payload = source.inner.store.backup_payload().await.unwrap().payload;
    let store = db::FaStore::new(MemDatabase::new().into_database());
    let cached = setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]);
    store
        .store_setup_payment_federations_event(cached.clone())
        .await
        .unwrap();
    store
        .import_backup(TestIdentity::fa_id(), payload, None, &staging_recovery())
        .await
        .unwrap();
    assert!(store.recovery_completed(&staging_recovery()).await);
    assert_eq!(
        store
            .load_setup_payment_federations_event()
            .await
            .unwrap()
            .id,
        cached.id
    );
}

#[tokio::test]
async fn legacy_pending_backup_import_is_looked_up_again() {
    let (source, _, _, _) = formed_client_for_liquidity().await;
    let payload = source.inner.store.backup_payload().await.unwrap().payload;
    let database = MemDatabase::new().into_database();
    let store = db::FaStore::new(database.clone());
    store
        .write_pending_import_for_test(TestIdentity::fa_id(), payload)
        .await;
    store
        .complete_empty_recovery(&staging_recovery())
        .await
        .unwrap();
    let reopened =
        open_with_backup_recovery_hint(database, BackupRecoveryHint::RestoredMnemonic).await;
    assert_eq!(reopened.status(), recovering(None));
    assert!(
        reopened
            .inner
            .store
            .load_federation(TestIdentity::fa_id())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn restored_federation_name_uses_consensus_without_a_liquidity_backup() {
    let (source, _, state, _) = formed_client_for_liquidity().await;
    let payload = source.inner.store.backup_payload().await.unwrap().payload;
    assert!(payload.liquidity.is_none());
    for name in ["Test Federation", "Renamed Federation"] {
        if name == "Renamed Federation" {
            source
                .update_federation_metadata_with(
                    FederationMetadataUpdate::name(name).unwrap(),
                    Timeouts::default(),
                )
                .await
                .unwrap();
        }
        let (payments, _) = TestPayments::new();
        let restored = open_client(
            MemDatabase::new().into_database(),
            payments,
            state.clone(),
            MgConfig::given_away(),
        )
        .await;
        restored
            .import_backup_for_test(Some(payload.clone()))
            .await
            .unwrap();
        let snapshot = restored.status().federation.expect("restored status");
        assert_eq!(
            snapshot.federation_name,
            Some(FederationName(name.to_owned()))
        );
    }
}

#[test]
fn federation_name_uses_config_only_when_metadata_name_is_absent() {
    for (meta, expected) in [
        (None, "Test Federation"),
        (Some("{}"), "Test Federation"),
        (Some(r#"{"federation_name":"Renamed"}"#), "Renamed"),
    ] {
        let snapshot = FederationConsensusSnapshot {
            config: test_federation_config(),
            meta_value: meta.map(|value| value.as_bytes().to_vec()),
            meta_revision: meta.map(|_| 0),
            network: fedi_decentralized_domain::BitcoinNetwork::Regtest,
        };
        assert_eq!(
            crate::formation::federation_name(&snapshot).unwrap(),
            Some(FederationName(expected.to_owned()))
        );
        let mut missing = snapshot;
        missing.config.global.meta.clear();
        missing.meta_value = None;
        assert_eq!(crate::formation::federation_name(&missing).unwrap(), None);
        for invalid in [
            "{",
            r#"{"federation_name":42}"#,
            r#"{"federation_name":null}"#,
        ] {
            missing.config = test_federation_config();
            missing.meta_value = Some(invalid.as_bytes().to_vec());
            assert!(crate::formation::federation_name(&missing).is_err());
        }
    }
}

#[tokio::test]
async fn restored_backup_gives_usable_authority_and_hydrates_liquidity() {
    let (source, formation_id, mg_state, connector) = formed_client_for_liquidity().await;
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());
    source
        .start_liquidity_for_test(
            &formation_id,
            &provider,
            LiquidityRequestIntent::gateway(250_000, Some(260_000)),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect("provider accepts an allocation that is still pending");
    let source_operation = source
        .list_liquidity_operations(None, 10)
        .await
        .unwrap()
        .operations
        .pop()
        .expect("source accepted operation");
    let keys = crate::backup::FaBackupKeys::derive(&source.inner.ports.identity.scoped_root());
    let encrypted = keys
        .seal(&source.inner.store.backup_payload().await.unwrap().payload)
        .unwrap();

    let registry = TestRegistry::default();
    registry
        .advertisements
        .lock()
        .expect("test lock")
        .push(liquidity_provider_event());
    let (payments, _) = TestPayments::new();
    let database = MemDatabase::new().into_database();
    let restored = open_client_with_registry(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::given_away(),
        registry.clone(),
    )
    .await;
    let mut payload = keys.open(&encrypted).unwrap();
    // A generation the restored FA could not reach by counting from one.
    payload.snapshot_generation = 7;
    let fa_id = restored.inner.ports.identity.public_key().unwrap();
    let restored_formation_id =
        crate::formation::restored_formation_id(fa_id, &payload.federation_invite).unwrap();
    let restored_generation = payload.snapshot_generation;

    let connect_calls = mg_state.connect_calls.load(Ordering::SeqCst);
    mg_state
        .connect_failures_remaining
        .store(usize::MAX, Ordering::SeqCst);
    restored
        .import_backup_for_test(Some(payload.clone()))
        .await
        .expect("verify restored seats through consensus while managers are unreachable");
    let reconciled = restored
        .status()
        .federation
        .expect("reconciled restored status");
    assert_eq!(reconciled.formation_id, restored_formation_id);
    assert_eq!(mg_state.connect_calls.load(Ordering::SeqCst), connect_calls);
    let republished = restored.inner.store.backup_payload().await.unwrap();
    assert_eq!(republished.payload.snapshot_generation, restored_generation);
    assert_eq!(
        republished.payload.liquidity.as_ref().map(|commitment| {
            liquidity_api::request_liquidity_details_hash(commitment).unwrap()
        }),
        Some(source_operation.details_payload_hash),
        "the restored writer can publish its current lean authority",
    );

    // Reopen from the storage that predates the federation record.
    restored
        .inner
        .store
        .write_legacy_federation_storage_for_test(Some(payload))
        .await;
    drop(restored);
    let restored = open_client_with_registry(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        registry,
    )
    .await;
    assert_eq!(
        restored
            .inner
            .store
            .backup_payload()
            .await
            .unwrap()
            .payload
            .snapshot_generation,
        restored_generation,
        "a migrated import keeps its eligibility and generation baseline",
    );
    restored
        .inner
        .ports
        .consensus_reader
        .offline_peers
        .lock()
        .unwrap()
        .insert(PeerId::from(0));
    mg_state
        .connect_failures_remaining
        .store(0, Ordering::SeqCst);
    let hydrated = restored
        .current_liquidity_operation()
        .await
        .unwrap()
        .expect("pending commitment is hydrated");
    assert_eq!(hydrated.operation_id, source_operation.operation_id);
    assert_eq!(
        hydrated.details_payload_hash,
        source_operation.details_payload_hash
    );
    assert_eq!(hydrated.endpoint_hint, None);
    {
        let mut allocations = connector.0.allocations.lock().unwrap();
        complete_gateway_allocation(
            allocations.get_mut(&hydrated.details_payload_hash).unwrap(),
            GatewayApiUrl::try_from("https://gateway.example/").unwrap(),
        );
    }
    let resumed = restored
        .resume_liquidity_for_test(&hydrated.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .expect("restored authority resumes and verifies the same request through other guardians");
    assert_eq!(resumed.phase, LiquidityOperationPhase::Accepted);
    assert!(resumed.gateway_view_verified);
    assert_eq!(connector.0.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn restored_recovery_rejects_unverified_consensus_and_changed_seats() {
    for fault in [
        "identity",
        "directory",
        "seat",
        "unreachable",
        "alternate_identity",
    ] {
        let (source, _, state, _) = formed_client_for_liquidity().await;
        let mut payload = source.inner.store.backup_payload().await.unwrap().payload;
        let mut reader = TestConsensusReader::new(state.clone());
        match fault {
            "identity" => {
                reader
                    .config
                    .global
                    .api_endpoints
                    .remove(&fedimint_core::PeerId::from(0));
            }
            "directory" => {
                reader.adopt(b"{}".to_vec());
                reader.force_value("{}");
            }
            "seat" => payload.seats[0].mg_identity = mg_keys(20).public_key(),
            "unreachable" => reader.fail_next(usize::MAX),
            "alternate_identity" => {
                reader
                    .offline_peers
                    .lock()
                    .unwrap()
                    .extend((0..MIN_FEDERATION_SIZE - 1).map(PeerId::from));
                state.disagreeing_invite.store(true, Ordering::SeqCst);
            }
            _ => unreachable!(),
        }
        let (payments, _) = TestPayments::new();
        let restored = open_client_with_reader(
            MemDatabase::new().into_database(),
            payments,
            state.clone(),
            MgConfig::given_away(),
            reader,
        )
        .await;
        let calls = state.connect_calls.load(Ordering::SeqCst);
        let error = restored
            .import_backup_for_test(Some(payload))
            .await
            .expect_err(fault);
        if fault == "identity" {
            assert!(
                error
                    .to_string()
                    .contains("identity differs from the saved invite")
            );
        }
        // A rejected backup leaves nothing behind, and the lookup retries.
        assert!(restored.status().federation.is_none());
        assert!(
            restored
                .inner
                .store
                .load_federation(TestIdentity::fa_id())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !restored
                .inner
                .store
                .recovery_completed(&staging_recovery())
                .await
        );
        assert!(
            restored
                .current_liquidity_operation()
                .await
                .unwrap()
                .is_none()
        );
        if !matches!(fault, "unreachable" | "alternate_identity") {
            assert_eq!(state.connect_calls.load(Ordering::SeqCst), calls);
        }
    }
}

#[tokio::test]
async fn restored_recovery_dials_saved_managers_when_the_invite_peers_are_down() {
    let (source, _, state, _) = formed_client_for_liquidity().await;
    let payload = source.inner.store.backup_payload().await.unwrap().payload;
    let reader = TestConsensusReader::new(state.clone());
    reader
        .offline_peers
        .lock()
        .unwrap()
        .extend((0..MIN_FEDERATION_SIZE - 1).map(PeerId::from));
    let (payments, _) = TestPayments::new();
    let restored = open_client_with_reader(
        MemDatabase::new().into_database(),
        payments,
        state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    restored
        .import_backup_for_test(Some(payload))
        .await
        .unwrap();
}

#[tokio::test]
async fn backup_relay_progress_is_independent_and_workers_do_not_block_fa_operations() {
    let (client, _, _, _) = formed_client_for_liquidity().await;
    let hash = sha256::Hash::from_byte_array([9; 32]);
    client
        .inner
        .store
        .record_backup_confirmation(
            "wss://first.example",
            db::BackupRelayConfirmation {
                document_hash: hash,
                generation: 3,
                event_id: "event-a".to_owned(),
                confirmed_at_secs: 7,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        client
            .inner
            .store
            .backup_confirmation("wss://first.example")
            .await
            .unwrap()
            .generation,
        3
    );
    assert!(
        client
            .inner
            .store
            .backup_confirmation("wss://second.example")
            .await
            .is_none()
    );

    client
        .inner
        .store
        .record_backup_confirmation(
            "wss://first.example",
            db::BackupRelayConfirmation {
                document_hash: sha256::Hash::from_byte_array([1; 32]),
                generation: 2,
                event_id: "stale-event".to_owned(),
                confirmed_at_secs: 8,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        client
            .inner
            .store
            .backup_confirmation("wss://first.example")
            .await
            .unwrap()
            .event_id,
        "event-a"
    );
    let backup_tasks = fedimint_core::task::TaskGroup::new();
    crate::backup_worker::spawn_workers(
        &backup_tasks,
        client.inner.store.clone(),
        client.inner.ports.identity.scoped_root(),
        vec![nostr_sdk::RelayUrl::parse("ws://127.0.0.1:1").unwrap()],
    );
    // The unreachable relay is owned solely by background delivery; it does
    // not take the FA operation guard or extend local backup reads with network I/O.
    tokio::time::timeout(Duration::from_secs(1), client.inner.store.backup_payload())
        .await
        .expect("ordinary FA operation did not wait for relay")
        .unwrap();
    backup_tasks.shutdown_join_all(None).await.unwrap();
}

fn options() -> Timeouts {
    Timeouts {
        poll: Duration::from_millis(1),
        long: Duration::from_secs(2),
        request: Duration::from_millis(200),
    }
}

/// A run long enough for every selected MG to use up its retry budget.
fn unreachable_mg_options(request_timeout: Duration) -> Timeouts {
    Timeouts {
        poll: Duration::from_millis(1),
        long: Duration::from_secs(20 * 60),
        request: request_timeout,
    }
}

fn long_request_options() -> Timeouts {
    Timeouts {
        poll: Duration::from_millis(1),
        long: Duration::from_secs(2),
        request: Duration::from_secs(30),
    }
}

type TestClient =
    FaClient<TestIdentity, TestPayments, TestRegistry, TestConnector, TestConsensusReader>;

trait TestSetupPaymentSelection {
    async fn select_setup_payment_federation_for_test(
        &self,
        request_timeout: Duration,
    ) -> FaResult<FederationId>;
}

impl TestSetupPaymentSelection for TestClient {
    async fn select_setup_payment_federation_for_test(
        &self,
        request_timeout: Duration,
    ) -> FaResult<FederationId> {
        let options = Timeouts {
            poll: Duration::from_millis(1),
            long: Duration::from_secs(2),
            request: request_timeout,
        };
        let run = options;
        self.payable_setup_payment_federations(run)
            .await
            .and_then(|(policy_ids, payable)| {
                policy_ids
                    .into_iter()
                    .find(|federation_id| payable.contains(federation_id))
                    .ok_or_else(|| FaError::Payment("no payable policy member".to_owned()))
            })
    }
}

async fn open_client(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<MgState>,
    config: MgConfig,
) -> TestClient {
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    open_client_with_registry(database, payments, state, config, registry).await
}

/// An FA with no deployment-pinned setup-payment publisher: it has no
/// authenticated set of federations to fund from, so it can only take seats an
/// MG gives away.
async fn open_client_that_cannot_pay(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<MgState>,
    config: MgConfig,
) -> TestClient {
    FaClient::open_inner(
        database,
        FaClientPorts {
            identity: TestIdentity,
            payments,
            registry: TestRegistry::default(),
            mg_connector: TestConnector {
                state: state.clone(),
                config,
            },
            consensus_reader: TestConsensusReader::new(state),
            fa_fee_account_provider: Arc::new(TestFaFeeAccountProvider::default()),
        },
        test_peer_badge_verifier(),
        None,
        Some(guardian_fee_account(31)),
        None,
    )
    .await
    .expect("open test FA client")
}

async fn open_client_with_reader(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<MgState>,
    config: MgConfig,
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
    state: Arc<MgState>,
    config: MgConfig,
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
    state: Arc<MgState>,
    config: MgConfig,
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
        TestFaFeeAccountProvider::default(),
    )
    .await
}

async fn open_client_with_fee_account(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<MgState>,
    config: MgConfig,
    fa_fee_account_provider: TestFaFeeAccountProvider,
) -> TestClient {
    open_client_with_registry_reader_and_fee_account(
        database,
        payments,
        state.clone(),
        config,
        TestRegistry::default(),
        TestConsensusReader::new(state),
        fa_fee_account_provider,
    )
    .await
}

async fn open_client_with_registry_reader_and_fee_account(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<MgState>,
    config: MgConfig,
    registry: TestRegistry,
    consensus_reader: TestConsensusReader,
    fa_fee_account_provider: TestFaFeeAccountProvider,
) -> TestClient {
    FaClient::open_inner(
        database,
        FaClientPorts {
            identity: TestIdentity,
            payments,
            registry,
            mg_connector: TestConnector { state, config },
            consensus_reader,
            fa_fee_account_provider: Arc::new(fa_fee_account_provider),
        },
        test_peer_badge_verifier(),
        Some(setup_payment_keys().public_key()),
        Some(guardian_fee_account(31)),
        None,
    )
    .await
    .expect("open test FA client")
}

async fn open_client_with_verifier(
    database: fedimint_core::db::Database,
    payments: TestPayments,
    state: Arc<MgState>,
    config: MgConfig,
    verifier: PeerBadgeVerifier,
) -> TestClient {
    let registry = TestRegistry::default();
    registry
        .candidates
        .lock()
        .expect("test lock")
        .push(setup_payment_event(test_now_secs(), &[PAYMENT_INVITE]));
    FaClient::open_inner(
        database,
        FaClientPorts {
            identity: TestIdentity,
            payments,
            registry,
            mg_connector: TestConnector {
                state: state.clone(),
                config,
            },
            consensus_reader: TestConsensusReader::new(state),
            fa_fee_account_provider: Arc::new(TestFaFeeAccountProvider::default()),
        },
        verifier,
        Some(setup_payment_keys().public_key()),
        Some(guardian_fee_account(31)),
        None,
    )
    .await
    .expect("open test FA client with explicit verifier")
}

async fn open_client_with_store(
    store: db::FaStore,
    payments: TestPayments,
    state: Arc<MgState>,
    config: MgConfig,
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
    store: db::FaStore,
    payments: TestPayments,
    state: Arc<MgState>,
    config: MgConfig,
    registry: TestRegistry,
) -> TestClient {
    let status = store.load_status(TestIdentity::fa_id()).await.unwrap();
    let progress = Arc::new(crate::run::Progress::new(status));
    FaClient {
        inner: Arc::new(FaClientInner {
            store,
            ports: FaClientPorts {
                identity: TestIdentity,
                payments,
                registry,
                mg_connector: TestConnector {
                    state: state.clone(),
                    config,
                },
                consensus_reader: TestConsensusReader::new(state),
                fa_fee_account_provider: Arc::new(TestFaFeeAccountProvider::default()),
            },
            progress,
            control: crate::run::RunControl::default(),
            run_guard: Arc::new(tokio::sync::Mutex::new(())),
            backup_source: None,
            peer_badge_verifier: test_peer_badge_verifier(),
            setup_payment_publisher: Some(setup_payment_keys().public_key()),
            guardian_verification_fee_account: Some(guardian_fee_account(31)),
            backup_tasks: fedimint_core::task::TaskGroup::new(),
        }),
    }
}

fn staging_recovery() -> crate::db::RecoveryCompletedKey {
    crate::db::RecoveryCompletedKey::new(ManifoldEnvironment::Staging)
}

fn recovering(last_error: Option<FaErrorCode>) -> FaStatus {
    FaStatus {
        recovery: Some(RecoveryCheck { last_error }),
        ..FaStatus::default()
    }
}

fn creation(status: &FaStatus) -> &CreationSnapshot {
    status.creation.as_ref().expect("expected a creation")
}

/// The creation, which buys seats now.
fn purchase(status: &FaStatus) -> &CreationSnapshot {
    let creation = creation(status);
    assert_eq!(creation.stage, CreationStage::Buying, "expected a purchase");
    creation
}

fn is_buying(status: &FaStatus) -> bool {
    status
        .creation
        .as_ref()
        .is_some_and(|creation| creation.stage == CreationStage::Buying)
}

fn is_forming(status: &FaStatus) -> bool {
    status
        .creation
        .as_ref()
        .is_some_and(|creation| matches!(creation.stage, CreationStage::Forming { .. }))
}

/// The creation, once every slot holds an accepted seat.
fn formation(status: &FaStatus) -> &CreationSnapshot {
    let creation = creation(status);
    assert!(
        is_forming(status),
        "expected a formation, got {:?}",
        creation.stage
    );
    creation
}

fn formation_phase(status: &FaStatus) -> FormationPhase {
    match formation(status).stage {
        CreationStage::Forming { phase, .. } => phase,
        _ => unreachable!(),
    }
}

fn formation_invite(status: &FaStatus) -> Option<InviteCode> {
    match &formation(status).stage {
        CreationStage::Forming { invite_code, .. } => invite_code.clone(),
        _ => unreachable!(),
    }
}

/// The slots the purchase still buys.
fn pending_indices(status: &FaStatus) -> Vec<u16> {
    purchase(status)
        .slots
        .iter()
        .filter(|slot| slot.seat_id.is_none())
        .map(|slot| slot.index)
        .collect()
}

/// The seats the MGs accepted, in slot order.
fn accepted_seats(status: &FaStatus) -> Vec<SeatProgress> {
    status
        .creation
        .iter()
        .flat_map(|creation| &creation.slots)
        .filter(|slot| slot.seat_id.is_some())
        .cloned()
        .collect()
}

/// Why the creation needs a new preview, if it does.
fn reauthorization(status: &FaStatus) -> Option<SelectionReauthorizationReason> {
    match status.creation.as_ref()?.stage {
        CreationStage::NeedsSeats {
            reauthorization, ..
        } => reauthorization,
        _ => None,
    }
}

/// The purchase ended before it bought a seat, and the creation asks for a
/// new preview of every seat.
fn assert_needs_new_preview(client: &TestClient) {
    let FaState::Creating(creation) = client.state() else {
        panic!("expected a creation, got {:?}", client.state());
    };
    assert_needs_seats(client, creation.federation_size.0);
}

/// The purchase ended without `missing` of its seats, and the creation asks
/// for a new preview of them.
fn assert_needs_seats(client: &TestClient, missing: u16) {
    let status = client.status();
    assert!(
        !is_buying(&status) && !is_forming(&status) && status.federation.is_none(),
        "{status:?}"
    );
    match client.state() {
        FaState::Creating(creation) => {
            assert_eq!(
                accepted_seats(&status).len(),
                usize::from(creation.federation_size.0 - missing)
            );
            assert!(
                matches!(
                    creation.stage,
                    CreationStage::NeedsSeats {
                        count,
                        reauthorization: Some(_),
                    } if count == missing
                ),
                "{:?}",
                creation.stage
            );
        }
        state => panic!("expected a creation that needs seats, got {state:?}"),
    }
}

/// The purchase finished and its seats wait in the pool.
fn bought(status: &FaStatus) {
    assert!(!is_buying(status), "purchase still active");
    assert_eq!(
        accepted_seats(status).len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

/// The purchase finished without the rows at `dropped`, and exactly the
/// other selected MGs' seats wait in the pool.
fn bought_without(status: &FaStatus, dropped: &[u16]) {
    assert!(!is_buying(status), "purchase still active");
    let expected = (0..MIN_FEDERATION_SIZE)
        .filter(|index| !dropped.contains(index))
        .map(|index| locator(usize::from(index)))
        .collect::<Vec<_>>();
    let mut pooled = accepted_seats(status)
        .iter()
        .map(|seat| seat.locator.clone())
        .collect::<Vec<_>>();
    pooled.sort_by_key(|pooled| expected.iter().position(|locator| locator == pooled));
    assert_eq!(pooled, expected);
}

/// The indices of the slots the purchase still buys.
fn seat_indices(recovery: &crate::db::CreationRecovery) -> Vec<u16> {
    recovery.pending()
}

fn federation(status: &FaStatus) -> &FederationSnapshot {
    status.federation.as_ref().expect("expected a federation")
}

async fn active_formation(store: &db::FaStore) -> crate::db::CreationRecovery {
    store
        .load_formation(TestIdentity::fa_id())
        .await
        .unwrap()
        .expect("expected a formation")
}

fn active_recovery(recovery: Option<crate::db::CreationRecovery>) -> crate::db::CreationRecovery {
    recovery.expect("expected an active purchase")
}

impl crate::db::FaStore {
    /// The creation, while it buys seats.
    async fn load_purchase(&self, fa_id: FaId) -> FaResult<Option<crate::db::CreationRecovery>> {
        Ok(self
            .load_creation(fa_id)
            .await?
            .filter(|creation| creation.purchase.is_some()))
    }

    /// The creation, once every slot holds an accepted seat.
    async fn load_formation(&self, fa_id: FaId) -> FaResult<Option<crate::db::CreationRecovery>> {
        Ok(self
            .load_creation(fa_id)
            .await?
            .filter(crate::db::CreationRecovery::is_full))
    }

    /// The seats of the creation's accepted slots, in slot order.
    async fn pool_seats(&self, fa_id: FaId) -> FaResult<Vec<SeatId>> {
        Ok(self
            .load_creation(fa_id)
            .await?
            .iter()
            .flat_map(|creation| &creation.slots)
            .filter_map(|slot| slot.seat_id().cloned())
            .collect())
    }

    /// Bind a fresh seat to its quote, as a run does before it pays.
    async fn bind_fresh_quote(
        &self,
        formation_id: &FormationId,
        index: u16,
        quote: SignedResponse<GetQuoteResponse>,
    ) -> FaResult<()> {
        self.bind_quote(formation_id, index, None, quote).await
    }
}

const SEEDED_PURCHASE_ID: &str = "seeded-selected-purchase";

/// Seed a selected paid purchase whose run bound every seat to its exact
/// quote and stopped before any wallet call. A test wallet told that a quote
/// is funded or rejected then models a payment the stopped run started.
async fn seed_bound_selected_purchase(client: &TestClient) -> Vec<QuoteId> {
    seed_bound_purchase(client, SEEDED_PURCHASE_ID, 200).await
}

/// Seed a selected paid purchase whose run has not bound any seat yet.
async fn seed_purchase(client: &TestClient, formation_id: &str) {
    client
        .inner
        .store
        .initialize_for_test(
            TestIdentity::fa_id(),
            FormationId(formation_id.to_owned()),
            intent().resolve(fedimintd_version().dkg_version()).unwrap(),
            (0..MIN_FEDERATION_SIZE)
                .map(selected_initial_seat)
                .collect(),
            Some(payment_federation_id()),
        )
        .await
        .unwrap();
}

async fn seed_bound_purchase(client: &TestClient, formation_id: &str, nonce: u8) -> Vec<QuoteId> {
    seed_purchase(client, formation_id).await;
    let store = &client.inner.store;
    let formation_id = FormationId(formation_id.to_owned());
    let resolved = intent().resolve(fedimintd_version().dkg_version()).unwrap();
    let mut quote_ids = Vec::new();
    for index in 0..MIN_FEDERATION_SIZE {
        let quote = signed_quote(
            usize::from(index),
            &resolved,
            [nonce + u8::try_from(index).unwrap(); 32],
        );
        quote_ids.push(
            quote
                .verify(&locator(usize::from(index)).service_pubkey)
                .unwrap()
                .quote_id(),
        );
        store
            .bind_quote(&formation_id, index, None, quote)
            .await
            .unwrap();
    }
    quote_ids
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
        Arc::new(MgState::default()),
        MgConfig::paid(),
        registry,
    )
    .await;

    let admitted = client
        .admitted_setup_payment_federations_with(options())
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
            .select_setup_payment_federation_for_test(options().request)
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
            Arc::new(MgState::default()),
            MgConfig::paid(),
            registry,
        )
        .await;

        let error = client
            .select_setup_payment_federation_for_test(options().request)
            .await
            .expect_err("missing or empty policy stops paid selection");
        assert!(matches!(error, FaError::Payment(_)));
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
        Arc::new(MgState::default()),
        MgConfig::paid(),
        registry,
    )
    .await;

    let error = client
        .select_setup_payment_federation_for_test(options().request)
        .await
        .expect_err("wallet holdings only filter authenticated policy");
    assert!(matches!(error, FaError::Payment(_)));
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
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;

    let admitted = client
        .admitted_setup_payment_federations_with(options())
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
        Arc::new(MgState::default()),
        MgConfig::paid(),
        registry,
    )
    .await;

    assert!(
        client
            .admitted_setup_payment_federations_with(options())
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
        Arc::new(MgState::default()),
        MgConfig::paid(),
        registry,
    )
    .await;
    assert_eq!(
        client
            .select_setup_payment_federation_for_test(options().request)
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
        Arc::new(MgState::default()),
        MgConfig::paid(),
        rollback_registry.clone(),
    )
    .await;
    assert_eq!(
        reopened
            .select_setup_payment_federation_for_test(options().request)
            .await
            .expect("rollback cannot replace last-known-good policy"),
        payment_federation_id()
    );

    rollback_registry.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        reopened
            .select_setup_payment_federation_for_test(options().request)
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
        Arc::new(MgState::default()),
        MgConfig::paid(),
        registry,
    )
    .await;
    client
        .select_setup_payment_federation_for_test(options().request)
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
        Arc::new(MgState::default()),
        MgConfig::paid(),
        malformed_registry,
    )
    .await;

    assert_eq!(
        reopened
            .select_setup_payment_federation_for_test(options().request)
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
        Arc::new(MgState::default()),
        MgConfig::paid(),
        registry,
    )
    .await;

    assert!(matches!(
        client
            .select_setup_payment_federation_for_test(options().request)
            .await,
        Err(FaError::Payment(_))
    ));
}

#[tokio::test]
async fn daemon_rejection_of_selected_common_set_member_is_quote_failure() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let state = Arc::new(MgState::default());
    let client = open_client(
        database,
        payments,
        state.clone(),
        MgConfig {
            reject_quote: true,
            ..MgConfig::paid()
        },
    )
    .await;

    let error = client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .expect_err("daemon-local policy rejection is actionable quote failure");
    assert!(matches!(
        &error,
        FaError::Mg(MgFailure {
            operation: MgOperation::GetQuote,
            cause: MgCause::Other,
            ..
        })
    ));
    assert_eq!(
        state.quote_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn authenticated_common_set_selection_is_sent_in_paid_quotes() {
    let (payments, _) = TestPayments::new();
    let state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        state.clone(),
        MgConfig::paid(),
    )
    .await;

    client
        .buy_and_form_at(intent(), locators(), options())
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
            TestIntent::new(
                None,
                FederationSize(size),
                PlanPreference::InfiniteBestEffort,
                fedimintd_version_range(),
            ),
            Err(FaError::InvalidIntent(_))
        ));
    }
    assert!(
        TestIntent::new(
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
            TestIntent::new(
                Some(FederationName(name)),
                FederationSize(MIN_FEDERATION_SIZE),
                PlanPreference::InfiniteBestEffort,
                fedimintd_version_range(),
            ),
            Err(FaError::InvalidIntent(_))
        ));
    }

    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    let mut outside_request = client.preview_at(&intent(), locators());
    outside_request.fedimintd_dkg_version = "0.11.0+fedi"
        .parse::<FedimintdVersion>()
        .expect("version parses")
        .dkg_version();
    assert!(matches!(
        client
            .create_and_buy(&outside_request, Some(payment_federation_id()), options())
            .await,
        Err(FaError::InvalidIntent(_))
    ));
    assert_eq!(client.status(), FaStatus::default());
    assert!(
        client
            .inner
            .store
            .load_purchase(TestIdentity.public_key().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(mg_state.availability_calls.load(Ordering::SeqCst), 0);
}

/// The deployment bootstrap: the first federation forms before any ecash to
/// pay for it exists, so an FA with no setup-payment publisher forms against
/// MGs offering their seats at zero, and never asks its wallet for anything.
#[tokio::test]
async fn an_fa_that_cannot_pay_forms_against_seats_offered_at_zero() {
    let (payments, payment_state) = TestPayments::new();
    let client = open_client_that_cannot_pay(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(MgState::default()),
        MgConfig::given_away(),
    )
    .await;

    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    let status = client.status();
    let formed = federation(&status);
    assert_eq!(formed.federation_invite, test_invite(0));
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

/// The wallet states the protocol it settled under, and the FA compares it
/// against the quote's signed terms: funds locked under a generation the quote
/// did not name never reach a `CreateSeat`.
#[tokio::test]
async fn a_payment_settled_under_the_wrong_generation_is_never_presented() {
    let (payments, payment_state) = TestPayments::new();
    payment_state
        .settle_wrong_generation
        .store(true, Ordering::SeqCst);
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;

    let error = client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .expect_err("a payment that disagrees with the quote is not presented");
    assert!(
        matches!(&error, FaError::ManifoldGuardian { message, .. }
            if message.contains("payment generation did not match")),
        "unexpected error: {error:?}"
    );
    assert_eq!(mg_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn default_name_final_status_and_formed_reconciliation_are_typed() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;
    let unnamed = TestIntent::new(
        None,
        FederationSize(MIN_FEDERATION_SIZE),
        PlanPreference::InfiniteBestEffort,
        fedimintd_version_range(),
    )
    .unwrap();
    client
        .buy_and_form_at(unnamed, locators(), options())
        .await
        .unwrap();

    let formed_status = client.status();
    let formed = federation(&formed_status);
    assert_eq!(formed.federation_invite, test_invite(0));
    let generated_name = formed.federation_name.clone().expect("formed name");
    assert_eq!(generated_name.0.split_whitespace().count(), 2);

    let formed_state = client.state();
    let json = serde_json::to_value(&formed_state).unwrap();
    assert_eq!(json["active"]["federation_name"], generated_name.0);
    assert_eq!(
        serde_json::from_value::<FaState>(json).unwrap(),
        formed_state
    );

    // Reconciliation adopts the consensus name, which a real federation took
    // from DKG; the fake config has a fixed name, so set it in the metadata.
    {
        let mut raw = mg_state.meta_consensus_raw.lock().unwrap();
        let mut meta: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(raw.as_deref().expect("formation published metadata")).unwrap();
        meta.insert(
            FEDERATION_NAME_META_FIELD_KEY.to_owned(),
            generated_name.0.clone().into(),
        );
        *raw = Some(serde_json::to_vec(&meta).unwrap());
    }
    let quote_calls = mg_state.quote_calls.load(Ordering::SeqCst);
    let create_calls = mg_state.create_calls.load(Ordering::SeqCst);
    let status_calls = mg_state.status_calls.load(Ordering::SeqCst);
    let invite_calls = mg_state.invite_calls.load(Ordering::SeqCst);
    let connect_calls = mg_state.connect_calls.load(Ordering::SeqCst);
    mg_state
        .connect_failures_remaining
        .store(usize::MAX, Ordering::SeqCst);
    let reopened = open_client(database, payments, mg_state.clone(), MgConfig::given_away()).await;
    let reloaded_status = reopened.status();
    let reloaded = federation(&reloaded_status);
    assert_eq!(reloaded.federation_name, Some(generated_name));

    // A reopened federation has no work, so it calls no MG.
    reopened.drive().await.unwrap();
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), quote_calls);
    assert_eq!(mg_state.create_calls.load(Ordering::SeqCst), create_calls);
    assert_eq!(mg_state.status_calls.load(Ordering::SeqCst), status_calls);
    assert_eq!(mg_state.invite_calls.load(Ordering::SeqCst), invite_calls);
    assert_eq!(mg_state.connect_calls.load(Ordering::SeqCst), connect_calls);
}

#[tokio::test]
async fn an_already_started_reply_resumes_through_status_instead_of_stranding_formation() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state
        .report_dkg_already_started
        .store(true, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;

    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    federation(&client.status());
    assert_eq!(
        mg_state.start_callbacks.lock().expect("test lock").len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn record_dkg_complete_rejects_incomplete_seats_without_changing_recovery_state() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state, MgConfig::given_away()).await;
    client
        .buy_seats_at(&intent(), locators(), options())
        .await
        .unwrap();
    let store = &client.inner.store;
    // The full pool is a formation that has not run yet.
    let formation_id = active_formation(store).await.formation_id;
    // Every seat but the first has its guardian code.
    for index in 1..MIN_FEDERATION_SIZE {
        store
            .record_guardian_code(
                &formation_id,
                index,
                GuardianCode(format!("guardiancode{index}")),
            )
            .await
            .unwrap();
    }

    assert!(matches!(
        client
            .inner
            .store
            .record_dkg_complete(&formation_id, test_invite(0))
            .await,
        Err(FaError::Storage(_))
    ));
    let status = client
        .inner
        .store
        .load_status(TestIdentity::fa_id())
        .await
        .unwrap();
    assert_eq!(formation_phase(&status), FormationPhase::PreparingDkg);
    assert!(formation_invite(&status).is_none());
}

#[tokio::test]
async fn formation_rejects_disagreeing_final_federation_identities() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state.disagreeing_invite.store(true, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;

    assert!(matches!(
        client
            .buy_and_form_at(intent(), locators(), options())
            .await,
        Err(FaError::InvalidManifoldGuardians(message))
            if message.contains("different federation identities")
    ));
    assert_eq!(
        formation_phase(&client.status()),
        FormationPhase::DkgUnderway
    );
    assert!(formation_invite(&client.status()).is_none());
}

#[tokio::test]
async fn concurrent_driver_is_rejected() {
    let (payments, _) = TestPayments::new();
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;
    let _guard = client.inner.run_guard.lock().await;
    assert!(matches!(client.drive().await, Err(FaError::Busy)));
}

#[test]
fn formation_autocommit_errors_are_mapped_by_failure_kind() {
    assert!(matches!(
        db::map_formation_tx_error(fedimint_core::db::AutocommitError::CommitFailed {
            attempts: 10,
            last_error: fedimint_core::db::DatabaseError::TransactionConsumed,
        }),
        FaError::Storage(message) if message == "updating FA formation transaction failed"
    ));
    assert!(matches!(
        db::map_formation_tx_error(fedimint_core::db::AutocommitError::CommitFailed {
            attempts: 10,
            last_error: fedimint_core::db::DatabaseError::WriteConflict,
        }),
        FaError::Storage(message) if message == "updating FA formation transaction failed"
    ));
    assert!(matches!(
        db::map_formation_tx_error(fedimint_core::db::AutocommitError::ClosureError {
            attempts: 1,
            error: FaError::Busy,
        }),
        FaError::Busy
    ));
}

#[tokio::test]
async fn initialized_formation_reloads_as_unsynced_typed_status() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    client
        .inner
        .store
        .initialize_for_test(
            TestIdentity::fa_id(),
            FormationId("formation1".to_owned()),
            resolved_intent_with_size(FederationSize(1)),
            vec![selected_initial_seat(0)],
            None,
        )
        .await
        .unwrap();

    let reopened = open_client(database, payments, mg_state, MgConfig::paid()).await;
    let status = reopened.status();
    let snapshot = purchase(&status);
    assert_eq!(snapshot.formation_id, FormationId("formation1".to_owned()));
    assert_eq!(snapshot.freshness, FormationFreshness::Unsynced);
    assert_eq!(snapshot.slots[0].freshness, FormationFreshness::Unsynced);
    assert_eq!(snapshot.slots[0].phase, SeatPhase::Selected);
}

#[tokio::test]
async fn persisted_formation_rejects_a_different_identity_before_observation() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    client
        .inner
        .store
        .initialize_for_test(
            TestIdentity::fa_id(),
            FormationId("ownedformation".to_owned()),
            resolved_intent_with_size(FederationSize(1)),
            vec![selected_initial_seat(0)],
            None,
        )
        .await
        .unwrap();

    let reopened = FaClient::open(
        database,
        OtherIdentity,
        payments,
        TestRegistry::default(),
        TestConnector {
            state: mg_state.clone(),
            config: MgConfig::paid(),
        },
        test_peer_badge_verifier(),
        TestConsensusReader::new(mg_state),
        TestFaFeeAccountProvider::default(),
    )
    .await;
    assert!(
        matches!(reopened, Err(FaError::Storage(message)) if message.contains("different identity"))
    );
}

#[tokio::test]
async fn only_a_paid_bound_quote_moves_and_only_to_a_paid_quote() {
    let store = db::FaStore::new(MemDatabase::new().into_database());
    let formation_id = FormationId("quote-moves".to_owned());
    let intent = resolved_intent_with_size(FederationSize(2));
    store
        .initialize_for_test(
            TestIdentity::fa_id(),
            formation_id.clone(),
            intent.clone(),
            vec![selected_initial_seat(0), selected_initial_seat(1)],
            Some(payment_federation_id()),
        )
        .await
        .unwrap();
    let first = signed_quote(0, &intent, [1; 32]);
    store
        .bind_fresh_quote(&formation_id, 0, first.clone())
        .await
        .unwrap();

    // A paid quote moves to a new paid quote, but only from the bound one.
    let second = signed_quote(0, &intent, [3; 32]);
    assert!(
        store
            .bind_quote(&formation_id, 0, None, second.clone())
            .await
            .is_err()
    );
    store
        .bind_quote(&formation_id, 0, Some(&first), second.clone())
        .await
        .unwrap();
    // An accepted seat keeps its quote.
    store
        .record_seat_accepted(
            &formation_id,
            0,
            SeatId::from(QuoteId([0x05; 32])),
            guardian_fee_account(32),
        )
        .await
        .unwrap();
    assert!(
        store
            .bind_quote(
                &formation_id,
                0,
                Some(&second),
                signed_quote(0, &intent, [4; 32]),
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn accepting_one_seat_preserves_a_sibling_journaled_quote() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;
    let formation_id = FormationId("twoseatinterleaving".to_owned());
    let intent = resolved_intent_with_size(FederationSize(2));
    client
        .inner
        .store
        .initialize_for_test(
            TestIdentity::fa_id(),
            formation_id.clone(),
            intent.clone(),
            vec![selected_initial_seat(0), selected_initial_seat(1)],
            Some(payment_federation_id()),
        )
        .await
        .unwrap();
    let first_quote = signed_quote(0, &intent, [21; 32]);
    let second_quote = signed_quote(1, &intent, [22; 32]);
    client
        .inner
        .store
        .bind_fresh_quote(&formation_id, 0, first_quote.clone())
        .await
        .unwrap();
    client
        .inner
        .store
        .bind_fresh_quote(&formation_id, 1, second_quote.clone())
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
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    // The accepted seat moved to the pool, and its sibling kept its quote.
    assert_eq!(seat_indices(&recovery), [1]);
    assert_eq!(recovery.slots[1].state.bound_quote(), Some(&second_quote));
    assert_eq!(
        client
            .inner
            .store
            .pool_seats(TestIdentity::fa_id())
            .await
            .unwrap(),
        [SeatId::from(QuoteId([0x01; 32]))]
    );
}

#[tokio::test]
async fn concurrent_two_seat_acceptance_retries_and_derives_completion_from_durable_siblings() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;
    let store = &client.inner.store;
    let formation_id = FormationId("concurrentseatacceptance".to_owned());
    // Two of the four slots are picked, so their acceptance ends the purchase
    // without a formation.
    let intent = resolved_intent();
    store
        .initialize_for_test(
            TestIdentity::fa_id(),
            formation_id.clone(),
            intent.clone(),
            vec![selected_initial_seat(0), selected_initial_seat(1)],
            Some(payment_federation_id()),
        )
        .await
        .unwrap();
    for index in 0..2 {
        store
            .bind_fresh_quote(
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

    // Both seats are in the pool, which ended the purchase.
    let fa_id = TestIdentity::fa_id();
    assert!(store.load_purchase(fa_id).await.unwrap().is_none());
    assert_eq!(
        store.pool_seats(fa_id).await.unwrap(),
        [
            SeatId::from(QuoteId([0x02; 32])),
            SeatId::from(QuoteId([0x03; 32]))
        ]
    );
}

#[tokio::test]
async fn bound_unpaid_seats_pay_fresh_quotes_on_reopen() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    let required = seed_bound_selected_purchase(&client)
        .await
        .into_iter()
        .collect::<HashSet<_>>();
    assert_eq!(required.len(), usize::from(MIN_FEDERATION_SIZE));
    drop(client);

    // The bound quotes were never paid, so reopen pays a fresh quote at the
    // previewed price for each seat.
    let reopened = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;
    reopened.drive().await.unwrap();
    bought(&reopened.status());
    let refreshed = quote_ids(&mg_state.quote_records.lock().expect("test lock"));
    assert_eq!(refreshed.len(), usize::from(MIN_FEDERATION_SIZE));
    assert!(required.is_disjoint(&refreshed));
    assert_eq!(
        payment_state.recover_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(
        mg_state.create_calls.load(Ordering::SeqCst),
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
        mg_state.quote_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
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
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;
    client
        .buy_seats_at(&intent(), locators(), options())
        .await
        .expect("one note is reused only after each accepted payment returns spendable change");

    bought(&client.status());
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
async fn partial_seat_recovery_keeps_the_remaining_payment_after_a_dropped_seat() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::paid(),
    )
    .await;
    let store = &client.inner.store;
    let formation_id = FormationId("partialpaymentreadiness".to_owned());
    let intent = resolved_intent_with_size(FederationSize(3));
    store
        .initialize_for_test(
            TestIdentity::fa_id(),
            formation_id.clone(),
            intent.clone(),
            (0..3).map(selected_initial_seat).collect(),
            Some(payment_federation_id()),
        )
        .await
        .unwrap();
    let quotes = (0..3)
        .map(|index| signed_quote(index, &intent, [51 + u8::try_from(index).unwrap(); 32]))
        .collect::<Vec<_>>();
    for (index, quote) in (0..).zip(&quotes) {
        store
            .bind_fresh_quote(&formation_id, index, quote.clone())
            .await
            .unwrap();
    }
    store
        .record_seat_accepted(
            &formation_id,
            0,
            SeatId::from(QuoteId([0x04; 32])),
            guardian_fee_account(32),
        )
        .await
        .unwrap();
    store
        .empty_slots(
            &formation_id,
            vec![(
                1,
                crate::db::SlotState::Bound {
                    quote: quotes[1].clone(),
                },
            )],
            SelectionReauthorizationReason::SeatUnavailable,
        )
        .await
        .unwrap();

    let recovered = store.load_status(TestIdentity::fa_id()).await.unwrap();
    assert_eq!(pending_indices(&recovered), [2]);
    assert_eq!(
        accepted_seats(&recovered)
            .into_iter()
            .map(|seat| seat.seat_id)
            .collect::<Vec<_>>(),
        [Some(SeatId::from(QuoteId([0x04; 32])))]
    );
    let recovery = active_recovery(store.load_purchase(TestIdentity::fa_id()).await.unwrap());
    assert_eq!(seat_indices(&recovery), [2]);
    assert_eq!(recovery.slots[1].state.bound_quote(), Some(&quotes[2]));
}

#[tokio::test]
async fn a_closed_mg_stops_before_quote_or_value_moving_payment_work() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let mut config = MgConfig::paid();
    config.accepting_seats = false;
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        config,
    )
    .await;

    assert!(matches!(
        client
            .buy_and_form_at(intent(), locators(), options())
            .await
            .as_ref(),
        Err(FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SeatUnavailable,
            failure: Some(MgFailure {
                operation: MgOperation::GetAvailability,
                cause: MgCause::NotAcceptingSeats,
                ..
            }),
        })
    ));
    assert_needs_new_preview(&client);
    assert_eq!(
        mg_state.availability_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), 0);
    assert_eq!(mg_state.create_calls.load(Ordering::SeqCst), 0);
    // Only the explicit payer's readiness was checked before quoting; no
    // value-moving wallet work followed.
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 1);
    assert_eq!(payment_state.recover_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn advertised_size_capability_stops_formation_before_quotes() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let mut config = MgConfig::paid();
    config.federation_size = FederationSize(10);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        config,
    )
    .await;

    let error = client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SeatUnavailable,
            failure: Some(MgFailure {
                operation: MgOperation::GetAvailability,
                cause: MgCause::UnsupportedFederationSize,
                ..
            }),
        }
    ));
    assert_eq!(
        mg_state.availability_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), 0);
    assert_eq!(mg_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn external_capability_calls_obey_request_timeout() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let mut config = MgConfig::paid();
    config.hang_availability = true;
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        config,
    )
    .await;
    let timeout_options = unreachable_mg_options(Duration::from_millis(10));

    assert!(matches!(
        client
            .buy_and_form_at(intent(), locators(), timeout_options)
            .await
            .as_ref(),
        Err(FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SeatUnavailable,
            failure: Some(MgFailure {
                operation: MgOperation::GetAvailability,
                cause: MgCause::Timeout,
                ..
            }),
        })
    ));
    assert_needs_new_preview(&client);
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), 0);
    assert_eq!(mg_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 1);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn lost_create_seat_response_replays_exact_quote_without_refunding_twice() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let mut config = MgConfig::paid();
    config.create_behavior = CreateBehavior::HangFirst;
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
    let create_started = mg_state.create_started.notified();
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .buy_seats_at(&intent(), locators(), long_request_options())
            .await
    });
    create_started.await;
    let hung_quote = mg_state
        .hung_quote
        .lock()
        .expect("test lock")
        .expect("hung quote");
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    assert_eq!(
        mg_state.quote_calls.load(Ordering::SeqCst),
        1,
        "seats are quoted one at a time"
    );

    let reopened = open_client(database, payments, mg_state.clone(), config).await;
    reopened.drive().await.unwrap();
    bought(&reopened.status());
    assert_eq!(
        mg_state.quote_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "the paid seat replays its quote"
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
    let records = mg_state.create_records.lock().expect("test lock");
    let replayed = records
        .iter()
        .filter(|record| record.quote_id == hung_quote)
        .collect::<Vec<_>>();
    assert_eq!(replayed.len(), 2);
    assert_eq!(replayed[0].signed_quote, replayed[1].signed_quote);
}

#[tokio::test(start_paused = true)]
async fn slow_refund_settlement_finishes_before_the_seat_is_dropped() {
    let (payments, payment_state) = TestPayments::new();
    *payment_state.refund_delay.lock().expect("test lock") = Duration::from_secs(95);
    let mg_state = Arc::new(MgState::default());
    let config = MgConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..MgConfig::paid()
    };
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        config,
    )
    .await;
    let error = client
        .buy_and_form_at(intent(), locators(), Timeouts::default())
        .await
        .unwrap_err();
    assert!(
        matches!(
            &error,
            FaError::SelectionReauthorizationRequired {
                failure: Some(MgFailure {
                    cause: MgCause::OfferChanged,
                    ..
                }),
                ..
            }
        ),
        "{error:?}"
    );
    let refused_quote = mg_state.refused_quote.lock().expect("test lock").unwrap();
    assert_eq!(payment_state.refund_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        payment_state
            .refund_contexts
            .lock()
            .expect("test lock")
            .as_slice(),
        &[refused_quote],
    );
    assert_needs_seats(&client, 1);
}

#[tokio::test]
async fn repeated_refund_settlement_replays_exact_context_before_the_seat_is_dropped() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let mut config = MgConfig::paid();
    config.create_behavior = CreateBehavior::RefuseFirstQuote;
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
    payment_state
        .hang_first_refund
        .store(true, Ordering::SeqCst);
    let refund_started = payment_state.refund_started.notified();
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .buy_seats_at(&intent(), locators(), long_request_options())
            .await
    });
    refund_started.await;
    let refused_quote = mg_state
        .refused_quote
        .lock()
        .expect("test lock")
        .expect("refused quote");
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());

    let reopened = open_client(database, payments, mg_state.clone(), config).await;
    let replay_error = reopened.drive().await.unwrap_err();
    assert!(
        matches!(
            &replay_error,
            FaError::SelectionReauthorizationRequired {
                failure: Some(MgFailure {
                    cause: MgCause::OfferChanged,
                    ..
                }),
                ..
            }
        ),
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
    let records = mg_state.create_records.lock().expect("test lock");
    let replayed = records
        .iter()
        .filter(|record| record.quote_id == refused_quote)
        .collect::<Vec<_>>();
    assert_eq!(replayed.len(), 2);
    assert_eq!(replayed[0].signed_quote, replayed[1].signed_quote);
    assert_needs_seats(&reopened, 1);
}

#[tokio::test]
async fn lost_free_create_seat_response_replays_exact_quote_and_allocation() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let mut config = MgConfig::given_away();
    config.create_behavior = CreateBehavior::HangFirst;
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
    let create_started = mg_state.create_started.notified();
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .buy_and_form_at(intent(), locators(), long_request_options())
            .await
    });
    create_started.await;
    let hung_quote = mg_state
        .hung_quote
        .lock()
        .expect("test lock")
        .expect("hung quote");
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let quote_calls_before_resume = mg_state.quote_calls.load(Ordering::SeqCst);

    assert_eq!(quote_calls_before_resume, 1);

    let reopened = open_client(database, payments, mg_state.clone(), config).await;
    reopened.drive().await.unwrap();
    bought(&reopened.status());
    assert_eq!(
        mg_state.quote_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "the bound seat is not quoted again",
    );
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    let records = mg_state.create_records.lock().expect("test lock");
    let replayed = records
        .iter()
        .filter(|record| record.quote_id == hung_quote)
        .collect::<Vec<_>>();
    assert_eq!(replayed.len(), 2);
    assert_eq!(replayed[0].signed_quote, replayed[1].signed_quote);
    assert_eq!(replayed[0].seat_id, replayed[1].seat_id);
    assert_eq!(
        mg_state.allocated_quotes.lock().expect("test lock").len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn formed_fa_proposes_and_confirms_the_fixed_4_1_1_policy() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let fa_account = guardian_fee_account(30);
    let fee_account_provider = TestFaFeeAccountProvider::new(fa_account.clone());
    let client = open_client_with_fee_account(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        fee_account_provider.clone(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    client
        .propose_guardian_fees_with(GuardianFeePpm::MANIFOLD_DEFAULT, options())
        .await
        .unwrap();

    let formed_status = client.status();
    let formed_invite = &federation(&formed_status).federation_invite;
    let formed_federation_id = formed_invite
        .0
        .parse::<FedimintInviteCode>()
        .expect("formed invite parses")
        .federation_id();
    assert_eq!(
        fee_account_provider.requested_federations(),
        vec![formed_federation_id],
        "fa-client chooses the provider lookup identity from durable formed state"
    );

    let submissions = mg_state.fee_submissions.lock().expect("test lock");
    assert_eq!(submissions.len(), usize::from(MIN_FEDERATION_SIZE));
    let distinct = submissions.values().cloned().collect::<HashSet<_>>();
    assert_eq!(
        distinct.len(),
        1,
        "every MG receives identical policy bytes"
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
        .find(|entry| entry["account"] == serde_json::to_value(&fa_account).unwrap())
        .expect("FA role account is present");
    assert_eq!(fi["weight"], 4);
    assert_eq!(fi["account"], serde_json::to_value(&fa_account).unwrap());
    let guardian_verification_fee_account = guardian_fee_account(31);
    let guardian_verification_fee = entries
        .iter()
        .find(|entry| {
            entry["account"] == serde_json::to_value(&guardian_verification_fee_account).unwrap()
        })
        .expect("Guardian Verification Fee account is present");
    assert_eq!(guardian_verification_fee["weight"], 1);
}

#[tokio::test]
async fn formation_names_the_mg_with_a_divergent_guardian_verification_fee_account() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let divergent_index = 2;
    let divergent_account = guardian_fee_account(29);
    mg_state
        .formation_guardian_verification_fee_accounts
        .lock()
        .expect("test lock")
        .insert(divergent_index, divergent_account.clone());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;

    let error = client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        FaError::Mg(MgFailure { mg, operation: MgOperation::ProposeFormationMeta,
            cause: MgCause::GuardianVerificationFeeAccountMismatch })
            if *mg == manager_key(2).x_only_public_key().0
    ));
    assert!(
        !mg_state
            .fee_submissions
            .lock()
            .expect("test lock")
            .contains_key(&divergent_index),
        "the divergent MG must reject before submitting a vote"
    );
}

#[tokio::test]
async fn unavailable_fa_fee_account_fails_before_any_guardian_vote() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client_with_fee_account(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        TestFaFeeAccountProvider::unavailable(),
    )
    .await;
    assert!(matches!(
        client
            .buy_and_form_at(intent(), locators(), options())
            .await,
        Err(FaError::CapabilityUnavailable(Capability::FeeArrangement))
    ));
    assert!(
        mg_state
            .fee_submissions
            .lock()
            .expect("test lock")
            .is_empty(),
        "an unavailable consumer account must fail before any guardian vote"
    );
}

#[tokio::test]
async fn fa_and_guardian_accounts_must_be_distinct() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let colliding_with_seat_zero = guardian_fee_account(32);
    let client = open_client_with_fee_account(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        TestFaFeeAccountProvider::new(colliding_with_seat_zero.clone()),
    )
    .await;
    assert!(matches!(
        client.buy_and_form_at(intent(), locators(), options()).await,
        Err(FaError::InvalidManifoldGuardians(message))
            if message.contains("do not form a canonical recipient set")
    ));
    assert!(
        mg_state
            .fee_submissions
            .lock()
            .expect("test lock")
            .is_empty()
    );
}

#[tokio::test]
async fn guardian_verification_fee_and_fa_accounts_must_be_distinct() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let shared_account = guardian_fee_account(31);
    let client = open_client_with_fee_account(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        TestFaFeeAccountProvider::new(shared_account.clone()),
    )
    .await;

    assert!(matches!(
        client.buy_and_form_at(intent(), locators(), options()).await,
        Err(FaError::InvalidManifoldGuardians(message))
            if message.contains("do not form a canonical recipient set")
    ));
    assert!(
        mg_state
            .fee_submissions
            .lock()
            .expect("test lock")
            .is_empty()
    );
}

#[tokio::test]
async fn a_guardian_fee_below_the_admitted_minimum_never_reaches_a_guardian_vote() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();
    let admitted = mg_state.fee_submissions.lock().expect("test lock").clone();

    // The publisher raises the floor above the 1,500-ppm default. It rides the
    // same durable setup-payment publication the payer set already comes from,
    // so the FA reads it without a fetch and without a new stored field.
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

    // Every MG would vote this down. Refusing it here is what turns a
    // tolerated per-guardian rejection — which the wave retries until the run
    // deadline — into the one answer that names the reason.
    for send_ppm in [0, 1, 2_499] {
        assert!(
            matches!(
                client
                    .propose_guardian_fees_with(
                        send_ppm.try_into().unwrap(),
                        options(),
                    )
                    .await,
                Err(FaError::InvalidIntent(message))
                    if message.contains("below the published minimum of 2500")
            ),
            "{send_ppm} ppm is below the admitted minimum"
        );
    }
    assert_eq!(
        *mg_state.fee_submissions.lock().expect("test lock"),
        admitted,
        "a refused rate must never reach a guardian vote"
    );

    // The floor itself is a rate an FA may propose.
    client
        .propose_guardian_fees_with(2_500.try_into().unwrap(), options())
        .await
        .unwrap();
    let raw = mg_state
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
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();
    let admitted = mg_state.fee_submissions.lock().expect("test lock").clone();
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
            .propose_guardian_fees_with(GuardianFeePpm::MANIFOLD_DEFAULT, options())
            .await,
        Err(FaError::InvalidIntent(message))
            if message.contains("below the published minimum of 6000")
    ));
    assert_eq!(
        *mg_state.fee_submissions.lock().expect("test lock"),
        admitted,
        "an explicit no-op rate proposal is still gated before guardian votes"
    );
}

#[tokio::test]
async fn the_default_guardian_fee_minimum_applies_before_any_publication_is_admitted() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();
    let admitted = mg_state.fee_submissions.lock().expect("test lock").clone();

    // A given-away formation never needed a payer, so no publication carrying
    // a floor has been admitted. Falling back to zero here is exactly what
    // would let an FA propose 0 ppm, so the published default stands in.
    let default_minimum = fedi_decentralized_domain::DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM;
    assert_eq!(client.min_guardian_fee_ppm().await, default_minimum);
    let default_minimum =
        u32::try_from(default_minimum).expect("the published default fits the ppm domain");

    assert!(matches!(
        client
            .propose_guardian_fees_with(
                (default_minimum - 1).try_into().unwrap(),
                options(),
            )
            .await,
        Err(FaError::InvalidIntent(message))
            if message.contains("below the published minimum of 1500")
    ));
    assert_eq!(
        *mg_state.fee_submissions.lock().expect("test lock"),
        admitted
    );

    // Anything above the floor is untouched: the 5,000-ppm Manifold default
    // proposes exactly as it did before the floor existed.
    client
        .propose_guardian_fees_with(GuardianFeePpm::MANIFOLD_DEFAULT, options())
        .await
        .unwrap();
    let raw = mg_state
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
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let client = open_client_with_reader(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader.clone(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    mg_state.meta_submissions.lock().expect("test lock").clear();
    mg_state
        .meta_request_bases
        .lock()
        .expect("test lock")
        .clear();
    let formed = mg_state
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
    let revision_before = mg_state.meta_consensus_revision.load(Ordering::SeqCst);
    client
        .propose_guardian_fees_with(2_500.try_into().unwrap(), options())
        .await
        .unwrap();

    let bases = mg_state.meta_request_bases.lock().expect("test lock");
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
    let final_raw = mg_state
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
    let mg_state = Arc::new(MgState::default());
    let client = FaClient::open(
        MemDatabase::new().into_database(),
        TestIdentity,
        payments,
        TestRegistry::default(),
        TestConnector {
            state: mg_state,
            config: MgConfig::given_away(),
        },
        test_peer_badge_verifier(),
        TestConsensusReader::new(Arc::new(MgState::default())),
        UnavailableFaFeeAccountProvider,
    )
    .await
    .unwrap();

    assert!(matches!(
        client
            .propose_guardian_fees_with((MAX_GUARDIAN_FEE_PPM + 1).try_into().unwrap(), options(),)
            .await,
        Err(FaError::InvalidIntent(_))
    ));
    assert!(matches!(
        client
            .propose_guardian_fees_with(GuardianFeePpm::MANIFOLD_DEFAULT, options(),)
            .await,
        Err(FaError::NoActiveFormation)
    ));
}

#[tokio::test]
async fn selected_free_lost_create_seat_reopens_after_verifier_drift_with_exact_presentation() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let mut config = MgConfig::given_away();
    config.create_behavior = CreateBehavior::HangFirst;
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
    let create_started = mg_state.create_started.notified();
    let running = client.clone();
    let operation = tokio::spawn(async move {
        running
            .buy_and_form(
                intent(),
                &running.preview_at(&intent(), locators()),
                payment_federation_id(),
                long_request_options(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), create_started)
        .await
        .expect("selected free CreateSeat reaches the injected lost response");
    let hung_quote = mg_state
        .hung_quote
        .lock()
        .expect("test lock")
        .expect("hung quote");
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    let interrupted = recovery
        .slots
        .iter()
        .find(|seat| {
            seat.state.bound_quote().is_some_and(|signed| {
                signed
                    .verify(&seat.locator.service_pubkey)
                    .is_ok_and(|quote| quote.quote_id() == hung_quote)
            })
        })
        .expect("the interrupted selected seat remains durable");
    assert!(
        interrupted
            .state
            .bound_quote()
            .unwrap()
            .verify(&interrupted.locator.service_pubkey)
            .unwrap()
            .terms
            .payment
            .is_none(),
        "the interrupted seat is bound to its free quote",
    );
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let quote_calls_before_resume = mg_state.quote_calls.load(Ordering::SeqCst);
    drop(client);

    let reopened = open_client_with_verifier(
        database,
        payments,
        mg_state.clone(),
        config,
        peer_badge_verifier(ManifoldEnvironment::Staging),
    )
    .await;
    reopened.drive().await.unwrap();

    // The purchase bound its first seat, so it needs no fresh admission.
    bought(&reopened.status());
    assert_eq!(
        mg_state.quote_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "the bound seat is not quoted again",
    );
    assert_eq!(quote_calls_before_resume, 1);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    let records = mg_state.create_records.lock().expect("test lock");
    let replayed = records
        .iter()
        .filter(|record| record.quote_id == hung_quote)
        .collect::<Vec<_>>();
    assert_eq!(replayed.len(), 2);
    assert_eq!(replayed[0].signed_quote, replayed[1].signed_quote);
    assert_eq!(replayed[0].seat_id, replayed[1].seat_id);
    assert_eq!(
        mg_state.allocated_quotes.lock().expect("test lock").len(),
        usize::from(MIN_FEDERATION_SIZE),
    );
}

#[tokio::test]
async fn a_refused_free_seat_leaves_and_its_quote_is_never_presented_again() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let config = MgConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..MgConfig::given_away()
    };
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;

    let error = client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SeatUnavailable,
            failure: Some(MgFailure {
                operation: MgOperation::CreateSeat,
                cause: MgCause::OfferChanged,
                ..
            })
        }
    ));
    assert_needs_seats(&client, 1);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    let refused_quote = mg_state
        .refused_quote
        .lock()
        .expect("test lock")
        .expect("one signed free refusal was returned");
    let presentations = |quote_id| {
        mg_state
            .create_records
            .lock()
            .expect("test lock")
            .iter()
            .filter(|record| record.quote_id == quote_id)
            .count()
    };
    assert_eq!(presentations(refused_quote), 1);
    let presentations_before_reopen = mg_state.create_calls.load(Ordering::SeqCst);
    let dropped = (0..usize::from(MIN_FEDERATION_SIZE))
        .find(|index| {
            accepted_seats(&client.status())
                .iter()
                .all(|seat| seat.locator != locator(*index))
        })
        .expect("one seat left the purchase");
    drop(client);

    let reopened = open_client(database, payments, mg_state.clone(), MgConfig::given_away()).await;
    reopened.drive().await.unwrap();
    assert_eq!(
        mg_state.create_calls.load(Ordering::SeqCst),
        presentations_before_reopen,
        "reopen presents no seat again",
    );

    reopened
        .buy_and_form_at(intent(), vec![locator(dropped)], options())
        .await
        .expect("a newly approved seat completes the creation");
    federation(&reopened.status());
    assert_eq!(
        presentations(refused_quote),
        1,
        "the new purchase never presents the refused quote again",
    );
}

#[tokio::test]
async fn consensus_carrying_a_different_directory_never_completes_formation() {
    // The readback exists to prove the exact bytes reached consensus. A
    // federation reporting some other value is not "done" — it is not done
    // yet, and the run must keep waiting rather than declare success.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    reader.force_value("{\"seat_bindings\":[],\"version\":1}");
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;

    let waited = tokio::time::timeout(
        Duration::from_secs(1),
        client.buy_and_form_at(intent(), locators(), options()),
    )
    .await;

    assert!(
        waited.is_err(),
        "expected the run to keep waiting, got {waited:?}"
    );
    // It really did submit; only consensus disagreed.
    assert_eq!(
        mg_state.meta_submissions.lock().expect("test lock").len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn run_loop_forms_a_creation_and_yields_to_a_user_action_mid_work() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state
        .block_meta_indices
        .lock()
        .expect("test lock")
        .extend(0..usize::from(MIN_FEDERATION_SIZE));
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;
    let runner = client.clone();
    let run = tokio::spawn(async move { runner.run_with(options()).await });
    let mut observed = client.observe();

    // Saving the creation wakes the idle loop, which buys and forms.
    client
        .create_federation(
            intent().federation_name,
            None,
            &client.preview_at(&intent(), locators()),
            None,
        )
        .await
        .unwrap();
    // The loop holds the run guard while the metadata vote is blocked.
    mg_state.meta_call_blocked.notified().await;
    let refused = tokio::time::timeout(Duration::from_secs(2), client.cancel_creation())
        .await
        .expect("the busy loop yields the run guard to a user action");
    assert!(
        matches!(
            refused,
            Err(FaError::AbandonUnavailable(
                crate::AbandonUnavailableReason::SeatsBought
            ))
        ),
        "{refused:?}"
    );

    // The loop takes the guard back and finishes the formation by itself.
    mg_state.release_meta_calls.store(true, Ordering::SeqCst);
    mg_state.meta_call_release.notify_waiters();
    tokio::time::timeout(
        Duration::from_secs(5),
        observed.wait_for(|state| matches!(state, FaState::Active(_))),
    )
    .await
    .expect("the loop continues after the user action")
    .unwrap();
    let status = client.status();
    assert!(status.creation.is_none());
    run.abort();
}

#[tokio::test]
async fn a_cancelled_waiting_action_does_not_stop_the_loop() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    *mg_state.blocked_quote_index.lock().expect("test lock") = Some(0);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;
    let runner = client.clone();
    let run = tokio::spawn(async move { runner.run_with(options()).await });
    let mut observed = client.observe();
    client
        .create_federation(
            intent().federation_name,
            None,
            &client.preview_at(&intent(), locators()),
            None,
        )
        .await
        .unwrap();
    mg_state.quote_blocked.notified().await;

    // The action starts to wait for the guard the loop holds, then the
    // consumer drops it before the loop yields.
    {
        let mut claim = std::pin::pin!(client.claim_run_guard());
        assert!(futures::poll!(claim.as_mut()).is_pending());
    }

    mg_state.release_quotes.store(true, Ordering::SeqCst);
    mg_state.quote_release.notify_waiters();
    tokio::time::timeout(
        Duration::from_secs(5),
        observed.wait_for(|state| matches!(state, FaState::Active(_))),
    )
    .await
    .expect("the loop continues after the dropped action")
    .unwrap();
    run.abort();
}

#[tokio::test]
async fn a_partial_pool_buys_only_its_gap_and_refuses_cancel() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let config = MgConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..MgConfig::paid()
    };
    let client = open_client(database, payments, mg_state.clone(), config).await;
    // One paid seat is refused after payment, so the purchase ends with six.
    client
        .create_and_buy(
            &client.preview_at(&intent(), locators()),
            Some(payment_federation_id()),
            options(),
        )
        .await
        .unwrap_err();
    client.drive().await.unwrap();
    let pool = accepted_seats(&client.status());
    assert_eq!(pool.len(), usize::from(MIN_FEDERATION_SIZE) - 1);
    let dropped = (0..usize::from(MIN_FEDERATION_SIZE))
        .find(|index| pool.iter().all(|seat| seat.mg_id != test_mg_id(*index)))
        .unwrap();
    // Only the one missing seat can be bought, not two, nor a full set.
    let other = (dropped + 1) % usize::from(MIN_FEDERATION_SIZE);
    for preview in [
        client
            .missing_preview_at(vec![locator(dropped), locator(other)])
            .await,
        client.preview_at(&intent(), locators()),
    ] {
        assert!(matches!(
            client
                .buy_seats(&preview, Some(payment_federation_id()))
                .await,
            Err(FaError::InvalidIntent(_))
        ));
    }
    // A purchase of the missing seat has started but not paid.
    client
        .request_seats_at(&intent(), vec![locator(dropped)], None)
        .await
        .unwrap();
    assert_eq!(
        pending_indices(&client.status()),
        [u16::try_from(dropped).unwrap()]
    );

    let error = client.cancel_creation().await.unwrap_err();

    assert!(
        matches!(
            error,
            FaError::AbandonUnavailable(crate::AbandonUnavailableReason::SeatsBought)
        ),
        "{error:?}"
    );
    // The refusal leaves the creation, its pool and its running purchase.
    let status = client.status();
    assert!(status.creation.is_some());
    let seat_ids = |seats: &[SeatProgress]| {
        seats
            .iter()
            .map(|seat| seat.seat_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(seat_ids(&accepted_seats(&status)), seat_ids(&pool));
    assert!(is_buying(&status));
    assert!(
        client
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap()
            .is_some()
    );

    client.drive_all_for_test(options()).await.unwrap();
    federation(&client.status());
    assert!(matches!(
        client.cancel_creation().await,
        Err(FaError::NoActiveFormation)
    ));
}

#[tokio::test]
async fn unfinished_setup_reopens_at_its_checkpoint_without_repeating_dkg() {
    for phase in [
        FormationPhase::DkgComplete,
        FormationPhase::PublishingSeatBindings,
    ] {
        let database = MemDatabase::new().into_database();
        let (payments, payment_state) = TestPayments::new();
        let state = Arc::new(MgState::default());
        let reader = TestConsensusReader::new(state.clone());
        if phase == FormationPhase::DkgComplete {
            reader.fail_next(usize::MAX);
        } else {
            reader.force_value("{}");
        }
        let client = open_client_with_reader(
            database.clone(),
            payments.clone(),
            state.clone(),
            MgConfig::given_away(),
            reader,
        )
        .await;
        // The formation waits for consensus until the test stops it.
        let waited = tokio::time::timeout(
            Duration::from_secs(1),
            client.buy_and_form_at(intent(), locators(), options()),
        )
        .await;
        assert!(waited.is_err(), "{waited:?}");
        assert_eq!(formation_phase(&client.status()), phase);
        drop(client);

        let reopened = open_client(database, payments, state.clone(), MgConfig::given_away()).await;
        assert_eq!(formation_phase(&reopened.status()), phase);
        assert!(matches!(
            reopened.cancel_creation().await,
            Err(FaError::AbandonUnavailable(
                crate::AbandonUnavailableReason::SeatsBought
            ))
        ));
        assert!(matches!(
            reopened.update_federation_metadata_with(
                FederationMetadataUpdate::name("Unfinished").unwrap(), Timeouts::default(),
            ).await,
            Err(FaError::MaintenanceWrongState { phase: Some(actual) }) if actual == phase
        ));
        assert!(reopened.inner.store.backup_payload().await.is_err());
        let starts = state.start_callbacks.lock().unwrap().len();
        let seats = state.create_calls.load(Ordering::SeqCst);
        // Let the fake consensus adopt the submitted directory on the next read.
        *state.meta_consensus_raw.lock().unwrap() = None;
        reopened.drive().await.unwrap();
        federation(&reopened.status());
        assert_eq!(state.start_callbacks.lock().unwrap().len(), starts);
        assert_eq!(state.create_calls.load(Ordering::SeqCst), seats);
        assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn an_mg_claiming_a_foreign_peer_is_not_pinned_for_recovery() {
    // The FA must reject the invalid directory before persisting its target:
    // recovery replays target bytes exactly and cannot repair a foreign peer
    // once that target is durable.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state.attest_foreign_peer.store(true, Ordering::SeqCst);
    let client = open_client(database, payments, mg_state.clone(), MgConfig::given_away()).await;

    let result = client
        .buy_and_form_at(intent(), locators(), options())
        .await;

    assert!(
        matches!(&result, Err(FaError::InvalidManifoldGuardians(message))
            if message.contains("names peer 9") && message.contains("not a guardian seat")),
        "expected the directory to be rejected against the config, got {result:?}"
    );
    assert!(
        mg_state
            .meta_submissions
            .lock()
            .expect("test lock")
            .is_empty(),
        "an invalid directory must not reach a guardian vote"
    );

    mg_state.attest_foreign_peer.store(false, Ordering::SeqCst);
    client.drive().await.unwrap();
    federation(&client.status());
}

#[tokio::test]
async fn an_attested_fee_account_must_match_the_signed_seat_acceptance() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state
        .attest_wrong_fee_account
        .store(true, Ordering::SeqCst);
    let client = open_client(database, payments, mg_state, MgConfig::given_away()).await;

    let result = client
        .buy_and_form_at(intent(), locators(), options())
        .await;

    assert_eq!(
        result.as_ref().unwrap_err().code(),
        FaErrorCode::InvalidManifoldGuardians
    );
    assert!(
        matches!(result.as_ref(), Err(FaError::Mg(MgFailure {
                mg, operation: MgOperation::GetPeerAttestation,
                cause: MgCause::InvalidResponse,
            })) if *mg == manager_key(0).x_only_public_key().0),
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
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    let after_formation = mg_state.attestation_calls.load(Ordering::SeqCst);
    assert_eq!(after_formation, usize::from(MIN_FEDERATION_SIZE));

    let reopened = open_client(database, payments, mg_state.clone(), MgConfig::given_away()).await;
    reopened.drive().await.unwrap();

    assert_eq!(
        mg_state.attestation_calls.load(Ordering::SeqCst),
        after_formation,
        "resume refetched attestations instead of replaying the persisted directory"
    );
}

#[tokio::test]
async fn formed_resume_accepts_a_later_rate_without_fee_account_capabilities() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();
    client
        .propose_guardian_fees_with(2_500.try_into().unwrap(), options())
        .await
        .unwrap();
    drop(client);

    let reopened = open_client_with_fee_account(
        database,
        payments,
        mg_state,
        MgConfig::given_away(),
        TestFaFeeAccountProvider::unavailable(),
    )
    .await;
    reopened.drive().await.unwrap();
    federation(&reopened.status());
}

#[tokio::test]
async fn interrupted_formation_replays_its_persisted_fee_target() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state
        .block_meta_indices
        .lock()
        .expect("test lock")
        .extend(0..usize::from(MIN_FEDERATION_SIZE));
    let original_provider = TestFaFeeAccountProvider::new(guardian_fee_account(30));
    let client = open_client_with_fee_account(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::given_away(),
        original_provider.clone(),
    )
    .await;
    let operation = tokio::spawn(async move {
        client
            .buy_and_form_at(intent(), locators(), options())
            .await
    });
    mg_state.meta_call_blocked.notified().await;
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    mg_state.release_meta_calls.store(true, Ordering::SeqCst);
    mg_state.meta_call_release.notify_waiters();

    let replacement_provider = TestFaFeeAccountProvider::new(guardian_fee_account(29));
    let reopened = open_client_with_fee_account(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        replacement_provider.clone(),
    )
    .await;
    reopened.drive().await.unwrap();

    assert_eq!(original_provider.requested_federations().len(), 1);
    assert!(replacement_provider.requested_federations().is_empty());
    let recipients = mg_state
        .fee_submissions
        .lock()
        .expect("test lock")
        .values()
        .next()
        .expect("replayed fee target")
        .1
        .clone();
    let recipients: serde_json::Value = serde_json::from_str(&recipients).unwrap();
    let recipients = recipients["recipients"].as_array().unwrap();
    assert!(recipients.iter().any(|entry| {
        entry["account"] == serde_json::to_value(guardian_fee_account(30)).unwrap()
    }));
    assert!(!recipients.iter().any(|entry| {
        entry["account"] == serde_json::to_value(guardian_fee_account(29)).unwrap()
    }));
}

#[tokio::test]
async fn exact_formation_readback_wins_over_late_already_published() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state
        .meta_terminal_errors
        .lock()
        .expect("test lock")
        .insert(
            usize::from(MIN_FEDERATION_SIZE) - 1,
            ManifoldGuardianError::FormationMetaAlreadyPublished,
        );
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state,
        MgConfig::given_away(),
    )
    .await;

    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();
    federation(&client.status());
}

#[tokio::test]
async fn fee_rate_uses_terminal_maintenance_rejection() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
    )
    .await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();
    mg_state
        .meta_terminal_errors
        .lock()
        .expect("test lock")
        .extend(
            (0..usize::from(MIN_FEDERATION_SIZE))
                .map(|index| (index, ManifoldGuardianError::MetaValueInvalid)),
        );

    assert!(matches!(
        client
            .propose_guardian_fees_with(2_500.try_into().unwrap(), options())
            .await,
        Err(FaError::MaintenanceRejected { .. })
    ));
}

#[tokio::test]
async fn a_transient_consensus_read_failure_is_retried() {
    // An unreachable federation is a reason to wait, not to fail a formation
    // whose directory may already be in consensus.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    reader.fail_next(3);
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;

    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    federation(&client.status());
}

#[tokio::test]
async fn a_consensus_read_that_times_out_is_retried() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    reader.hang_next(1);
    let client = open_client_with_reader(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;

    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    federation(&client.status());
}

#[tokio::test]
async fn pre_payment_abandon_wipes_a_parked_paid_formation_to_idle() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    seed_purchase(&client, SEEDED_PURCHASE_ID).await;
    drop(client);
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;

    client.cancel_creation().await.unwrap();

    assert_eq!(client.status(), FaStatus::default());
    assert_eq!(payment_state.recover_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(mg_state.create_calls.load(Ordering::SeqCst), 0);

    // The wipe is durable: a reopened client sees nothing to resume, and a
    // fresh creation is accepted.
    let reopened = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;
    assert_eq!(reopened.status(), FaStatus::default());
    reopened
        .buy_seats_at(&intent(), locators(), options())
        .await
        .unwrap();
    bought(&reopened.status());
}

#[tokio::test]
async fn cancel_after_seats_are_bound_before_payment_is_value_safe() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state, MgConfig::paid()).await;
    seed_bound_selected_purchase(&client).await;

    client.cancel_creation().await.unwrap();
    assert_eq!(client.status(), FaStatus::default());
    assert!(
        client
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn completed_payment_is_durable_before_the_next_funding_call_starts() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;
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
        .buy_seats_at(&intent(), locators(), options())
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

    bought(&client.status());
}

#[tokio::test]
async fn abandon_after_formed_is_a_typed_error() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state, MgConfig::given_away()).await;
    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();
    federation(&client.status());

    let error = client.cancel_creation().await.unwrap_err();
    assert!(matches!(error, FaError::NoActiveFormation), "{error}");
    federation(&client.status());
}

#[tokio::test]
async fn abandon_without_an_active_formation_is_a_typed_error() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let client = open_client(
        database,
        payments,
        Arc::new(MgState::default()),
        MgConfig::given_away(),
    )
    .await;
    assert!(matches!(
        client.cancel_creation().await,
        Err(FaError::NoActiveFormation)
    ));
}

fn compatible_intent() -> TestIntent {
    TestIntent::new(
        Some(FederationName("Test Federation".to_owned())),
        FederationSize(MIN_FEDERATION_SIZE),
        PlanPreference::InfiniteBestEffort,
        FedimintdVersionRange::new(
            "0.12.0".parse().expect("range minimum parses"),
            "0.12.3".parse().expect("range maximum parses"),
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

#[tokio::test(start_paused = true)]
async fn slow_payment_preparation_and_recovery_keep_the_exact_payment() {
    for interrupt in [false, true] {
        let database = MemDatabase::new().into_database();
        let (payments, payment_state) = TestPayments::new();
        // Acceptance fits the ordinary 30-second request cap, but collecting
        // signatures afterward does not. Recovery itself also exceeds that cap.
        *payment_state
            .first_payment_acceptance_delay
            .lock()
            .expect("test lock") = Duration::from_millis(29_600);
        *payment_state
            .payment_signatures_delay
            .lock()
            .expect("test lock") = Duration::from_secs(95);
        let mg_state = Arc::new(MgState::default());
        let store = db::FaStore::new(database);
        let client = open_client_with_store(
            store.clone(),
            payments.clone(),
            mg_state.clone(),
            MgConfig::paid(),
        )
        .await;
        let options = Timeouts {
            long: Duration::from_secs(if interrupt { 40 } else { 600 }),
            ..Timeouts::default()
        };
        let result = client.buy_and_form_at(intent(), locators(), options).await;
        if interrupt {
            // Each seat has its own run timeout, so every payment starts and
            // times out once.
            assert!(matches!(result, Err(FaError::Timeout(_))), "{result:?}");
            let paid_quotes = payment_state
                .created_quotes
                .lock()
                .expect("test lock")
                .clone();
            assert_eq!(paid_quotes.len(), usize::from(MIN_FEDERATION_SIZE));
            drop(client);
            let reopened =
                open_client_with_store(store, payments, mg_state, MgConfig::paid()).await;
            reopened
                .drive()
                .await
                .expect("recover signatures without another payment");
            bought(&reopened.status());
            assert_eq!(
                *payment_state.created_quotes.lock().expect("test lock"),
                paid_quotes,
                "recovery pays no quote again",
            );
        } else {
            result.expect("payment preparation can outlast a network request");
            federation(&client.status());
        }
        assert_eq!(
            payment_state.create_calls.load(Ordering::SeqCst),
            usize::from(MIN_FEDERATION_SIZE),
            "fund each seat once, including after an interrupted payment",
        );
    }
}

#[tokio::test]
async fn previewed_paid_formation_self_authorizes_and_forms() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;

    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    federation(&client.status());
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "the self-authorized run funded every paid seat without an explicit call",
    );
}

#[tokio::test]
async fn buy_seats_uses_the_explicit_ready_payer_and_arms_outputs() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;

    client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap();

    federation(&client.status());
    assert!(
        mg_state
            .quote_records
            .lock()
            .expect("test lock")
            .iter()
            .all(|record| record.payment_federation_id.as_ref() == Some(&payment_federation_id())),
        "every paid quote must name the explicitly selected payer",
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn selected_formation_persists_and_enforces_its_compatible_release() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    set_mg_version(&mg_state, 0, "0.12.1+fedi");
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;

    client
        .create_and_buy(
            &compatible_selection_preview(),
            Some(payment_federation_id()),
            options(),
        )
        .await
        .expect("every live MG offers a build in the previewed cohort");

    let status = client.status();
    bought(&status);
    assert!(
        client
            .inner
            .store
            .load_creation(TestIdentity::fa_id())
            .await
            .unwrap()
            .expect("the creation forms next")
            .slots
            .iter()
            .all(|slot| matches!(
                &slot.state,
                crate::db::SlotState::Accepted { fedimintd_version: version, .. }
                    if version.dkg_version() == fedimintd_version().dkg_version()
            ))
    );
    assert!(
        mg_state
            .quote_records
            .lock()
            .expect("test lock")
            .iter()
            .any(|record| record.fedimintd_version.to_string() == "0.12.1+fedi"),
        "same-minor patch drift is used for the exact quote",
    );
}

#[tokio::test]
async fn selected_mg_minor_drift_requires_fresh_selection_before_payment() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    set_mg_version(&mg_state, 0, "0.13.0+fedi");
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;

    let error = client
        .buy_and_form(
            compatible_intent(),
            &compatible_selection_preview(),
            payment_federation_id(),
            options(),
        )
        .await
        .expect_err("a different live minor invalidates the selected set");

    assert!(matches!(
        &error,
        FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SeatUnavailable,
            failure: Some(MgFailure {
                operation: MgOperation::GetAvailability,
                cause: MgCause::UnsupportedVersion,
                ..
            })
        }
    ));
    assert_needs_seats(&client, 1);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE) - 1
    );
    assert!(
        mg_state
            .quote_records
            .lock()
            .expect("test lock")
            .iter()
            .all(|record| record.fedimintd_version.dkg_version()
                == fedimintd_version().dkg_version())
    );
}

/// Buy a fresh preview after MG 3 changed its live price to
/// `live_price_msats`, and check that its seat leaves unquoted and unpaid.
async fn assert_changed_live_price_requires_a_fresh_preview(live_price_msats: u64) {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    let preview = client.preview_at(&intent(), locators());
    mg_state
        .price_overrides_msats
        .lock()
        .expect("test lock")
        .insert(3, live_price_msats);

    let error = client
        .create_and_buy(&preview, Some(payment_federation_id()), options())
        .await
        .expect_err("a changed live price invalidates the preview");

    assert!(
        matches!(
            &error,
            FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SeatUnavailable,
                failure: Some(MgFailure {
                    operation: MgOperation::GetAvailability,
                    cause: MgCause::PlanNotOffered,
                    ..
                }),
            }
        ),
        "{error:?}"
    );
    assert_needs_seats(&client, 1);
    assert!(
        mg_state
            .quote_records
            .lock()
            .expect("test lock")
            .iter()
            .all(|record| record.index != 3),
        "the MG with the changed price is never asked for a quote",
    );
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE) - 1,
        "only the seats at their previewed price are paid",
    );
}

#[tokio::test]
async fn a_higher_live_price_requires_a_fresh_preview_before_payment() {
    assert_changed_live_price_requires_a_fresh_preview(PAYMENT_AMOUNT_MSATS + 1).await;
}

#[tokio::test]
async fn a_lower_live_price_also_requires_a_fresh_preview_before_payment() {
    assert_changed_live_price_requires_a_fresh_preview(PAYMENT_AMOUNT_MSATS - 1).await;
}

#[tokio::test]
async fn a_quote_priced_above_its_previewed_plan_requires_a_fresh_preview() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    // MG 3 echoes the previewed plan but signs a price one above it.
    *mg_state.changed_quote_index.lock().expect("test lock") = Some(3);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;

    let error = client
        .buy_seats_at(&intent(), locators(), options())
        .await
        .expect_err("a quote above the previewed price is never paid");

    assert!(
        matches!(
            &error,
            FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SeatUnavailable,
                failure: Some(MgFailure {
                    operation: MgOperation::GetQuote,
                    cause: MgCause::InvalidResponse,
                    ..
                }),
            }
        ),
        "{error:?}"
    );
    assert_needs_seats(&client, 1);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE) - 1,
        "the seat with the higher quote is never paid",
    );
}

#[tokio::test]
async fn quotes_are_requested_at_exactly_the_previewed_price() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    // Every MG also offers the same plan at another price, listed first.
    *mg_state.extra_plans.lock().expect("test lock") = vec![Plan::InfiniteBestEffort {
        price_msats: PAYMENT_AMOUNT_MSATS * 3,
    }];
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;

    client
        .buy_seats_at(&intent(), locators(), options())
        .await
        .unwrap();

    bought(&client.status());
    let records = mg_state.quote_records.lock().expect("test lock");
    assert_eq!(records.len(), usize::from(MIN_FEDERATION_SIZE));
    assert!(records.iter().all(|record| record.plan
        == Plan::InfiniteBestEffort {
            price_msats: PAYMENT_AMOUNT_MSATS,
        }));
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn reopen_preserves_the_selected_dkg_identity_and_accepts_patch_skew() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    let resolved = compatible_intent()
        .resolve(fedimintd_version().dkg_version())
        .unwrap();
    client
        .inner
        .store
        .initialize_for_test(
            TestIdentity::fa_id(),
            FormationId("restart-range".to_owned()),
            resolved,
            (0..MIN_FEDERATION_SIZE)
                .map(selected_initial_seat)
                .collect(),
            Some(payment_federation_id()),
        )
        .await
        .unwrap();
    drop(client);
    set_mg_version(&mg_state, 0, "0.12.2+fedi");

    let reopened = open_client(database, payments, mg_state, MgConfig::paid()).await;
    assert_eq!(
        purchase(&reopened.status()).fedimintd_dkg_version,
        Some(fedimintd_version().dkg_version())
    );
    let persisted = active_recovery(
        reopened
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    assert_eq!(
        persisted.fedimintd_versions.maximum_exclusive().to_string(),
        "0.12.3"
    );
    reopened
        .drive()
        .await
        .expect("reopen accepts patch skew inside the persisted DKG identity");
    bought(&reopened.status());
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn selected_connection_failure_retries_before_requesting_replacement() {
    let database = MemDatabase::new().into_database();
    let (payments, _payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state
        .connect_failures_remaining
        .store(1, Ordering::SeqCst);
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;

    client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap();

    federation(&client.status());
    assert!(mg_state.connect_calls.load(Ordering::SeqCst) > usize::from(MIN_FEDERATION_SIZE));
}

#[tokio::test]
async fn selected_availability_transport_failure_reconnects_before_payment() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state
        .availability_transport_failures_remaining
        .store(1, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;

    client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap();

    federation(&client.status());
    assert!(mg_state.availability_calls.load(Ordering::SeqCst) > usize::from(MIN_FEDERATION_SIZE));
}

#[tokio::test]
async fn selected_quote_transport_failure_reconnects_before_payment() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state
        .quote_transport_failures_remaining
        .store(1, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;

    client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap();

    federation(&client.status());
    assert!(mg_state.quote_calls.load(Ordering::SeqCst) > usize::from(MIN_FEDERATION_SIZE));
}

#[tokio::test]
async fn selected_remote_quote_error_cannot_impersonate_local_transport_failure() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig {
            reject_quote: true,
            ..MgConfig::paid()
        },
    )
    .await;

    let error = client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .expect_err("remote service error must return without transport retries");

    assert!(
        matches!(
            &error,
            FaError::Mg(MgFailure {
                operation: MgOperation::GetQuote,
                cause: MgCause::Other,
                ..
            })
        ),
        "{error}"
    );
    assert_eq!(
        mg_state.quote_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "remote errors must not enter the local transport retry path",
    );
}

#[tokio::test]
async fn a_failed_presentation_connection_is_retried_by_the_next_run() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    // The preflight connects once to every seat. Then each seat connects once
    // for its quote and once for its presentation. Fail the presentation
    // connection of the fourth seat after its payment.
    mg_state
        .connect_failure_on_call
        .store(usize::from(MIN_FEDERATION_SIZE) + 8, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;

    let error = client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            &error,
            FaError::Mg(MgFailure {
                operation: MgOperation::Connect,
                cause: MgCause::Transport,
                ..
            })
        ),
        "{error:?}"
    );
    assert_eq!(
        purchase(&client.status())
            .slots
            .iter()
            .filter(|seat| seat.seat_id.is_none())
            .count(),
        1,
        "the other seats are bought",
    );

    client.drive_all_for_test(options()).await.unwrap();
    federation(&client.status());
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "the paid seat is presented again without a second payment",
    );
}

#[tokio::test]
async fn a_selected_connection_timeout_retries_in_the_quote_phase() {
    use tracing::instrument::WithSubscriber as _;

    // Attempt one obtains the quote.
    for attempt in [1] {
        let (payments, payment_state) = TestPayments::new();
        let state = Arc::new(MgState::default());
        *state.hang_connect_on_attempt.lock().unwrap() = Some((3, attempt));
        let client = open_client(
            MemDatabase::new().into_database(),
            payments,
            state.clone(),
            MgConfig::paid(),
        )
        .await;
        let capture = FormationLogCapture::default();
        client
            .buy_and_form(
                intent(),
                &client.preview_at(&intent(), locators()),
                payment_federation_id(),
                formation_log_options(Duration::from_millis(50)),
            )
            .with_subscriber(capture.subscriber())
            .await
            .unwrap();

        federation(&client.status());
        assert!(state.connect_attempts.lock().unwrap()[&3] > attempt);
        assert_eq!(
            payment_state.create_calls.load(Ordering::SeqCst),
            usize::from(MIN_FEDERATION_SIZE),
            "a retried connection must not duplicate payment outputs",
        );
        let logs = capture.text();
        assert!(
            logs.lines().any(|line| {
                line.contains("formation request failed")
                    && line.contains(&format!(
                        "mg_pubkey={}",
                        manager_key(3).x_only_public_key().0
                    ))
                    && line.contains("operation=connect")
                    && line.contains("cause=timeout")
                    && line.contains("outcome=\"retried\"")
            }),
            "attempt {attempt}: {logs}",
        );
    }
}

#[tokio::test(start_paused = true)]
async fn exhausted_selected_connection_retry_is_typed_and_returns_idle() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state
        .connect_failures_remaining
        .store(usize::MAX, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state,
        MgConfig::paid(),
    )
    .await;

    let error = client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            unreachable_mg_options(Duration::from_millis(200)),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        &error,
        FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SeatUnavailable,
            failure: Some(MgFailure {
                operation: MgOperation::Connect,
                cause: MgCause::Transport,
                ..
            })
        }
    ));
    let FaError::SelectionReauthorizationRequired {
        failure: Some(failure),
        ..
    } = &error
    else {
        panic!("reauthorization retains the MG failure: {error:?}");
    };
    assert_eq!(failure.operation, MgOperation::Connect);
    assert_eq!(failure.cause, MgCause::Transport);
    assert_eq!(error.code(), FaErrorCode::SelectionReauthorizationRequired);
    assert_needs_new_preview(&client);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn post_output_connection_failure_requires_exact_replay_not_replacement() {
    let database = MemDatabase::new().into_database();
    let (payments, _payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    mg_state
        .failed_create_indices
        .lock()
        .expect("test lock")
        .insert(0);
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;

    client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .expect_err("one authenticated CreateSeat failure interrupts the run");

    mg_state
        .failed_create_indices
        .lock()
        .expect("test lock")
        .clear();
    mg_state
        .connect_failures_remaining
        .store(1, Ordering::SeqCst);
    let error = client.drive().await.unwrap_err();

    assert!(
        matches!(
            &error,
            FaError::Mg(MgFailure {
                operation: MgOperation::Connect,
                cause: MgCause::Transport,
                ..
            })
        ),
        "{error}"
    );
}

#[tokio::test]
async fn a_refused_paid_seat_leaves_and_the_rest_complete() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let config = MgConfig {
        create_behavior: CreateBehavior::RefuseFirstQuote,
        ..MgConfig::paid()
    };
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;

    let error = client
        .create_and_buy(
            &client.preview_at(&intent(), locators()),
            Some(payment_federation_id()),
            options(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            &error,
            FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SeatUnavailable,
                failure: Some(MgFailure {
                    cause: MgCause::OfferChanged,
                    ..
                }),
            }
        ),
        "{error:?}"
    );
    let status = client.status();
    assert!(!is_buying(&status));
    let dropped = (0..MIN_FEDERATION_SIZE)
        .find(|index| {
            accepted_seats(&status)
                .iter()
                .all(|seat| seat.locator != locator(usize::from(*index)))
        })
        .expect("one seat left the purchase");
    bought_without(&status, &[dropped]);
    assert_eq!(payment_state.refund_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        usize::from(MIN_FEDERATION_SIZE),
        "every seat is paid once",
    );
    let FaState::Creating(creation) = client.state() else {
        panic!("the creation still needs the dropped seat");
    };
    assert_eq!(
        creation.stage,
        CreationStage::NeedsSeats {
            count: 1,
            reauthorization: Some(SelectionReauthorizationReason::SeatUnavailable),
        },
    );

    drop(client);
    let client = open_client(database, payments, mg_state, config).await;
    bought_without(&client.status(), &[dropped]);
}

#[tokio::test]
async fn buy_seats_rejects_a_preview_that_a_finished_purchase_made_stale() {
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state,
        MgConfig::given_away(),
    )
    .await;
    client
        .request_seats_at(&intent(), locators(), None)
        .await
        .unwrap();
    // A preview of the missing seats, taken while the first purchase runs.
    let spare = (0..usize::from(MIN_FEDERATION_SIZE))
        .map(|index| locator(usize::from(MIN_FEDERATION_SIZE) + index))
        .collect();
    let stale = client.missing_preview_at(spare).await;

    // The run loop holds the guard, so buy_seats waits for it.
    let guard = client.inner.run_guard.lock().await;
    client.inner.control.driving.store(true, Ordering::SeqCst);
    let buying = tokio::spawn({
        let client = client.clone();
        async move { client.buy_seats(&stale, None).await }
    });
    tokio::task::yield_now().await;
    let recovery = client
        .inner
        .store
        .load_purchase(TestIdentity::fa_id())
        .await
        .unwrap()
        .unwrap();
    client
        .drive_purchase(recovery, TestIdentity::fa_id(), options())
        .await
        .unwrap();
    assert_eq!(
        client
            .inner
            .store
            .pool_seats(TestIdentity::fa_id())
            .await
            .unwrap()
            .len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
    client.inner.control.driving.store(false, Ordering::SeqCst);
    drop(guard);

    let error = buying.await.unwrap().unwrap_err();
    assert!(matches!(error, FaError::InvalidIntent(_)), "{error:?}");
    assert!(!is_buying(&client.status()));
}

#[tokio::test]
async fn a_short_payer_ends_the_purchase_before_any_quote_or_payment() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let seats = u64::from(MIN_FEDERATION_SIZE);
    *payment_state.checked_balance_msats.lock().unwrap() = Some(seats * PAYMENT_AMOUNT_MSATS - 1);
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;

    let error = client
        .create_and_buy(
            &client.preview_at(&intent(), locators()),
            Some(payment_federation_id()),
            options(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SelectedPayerInsufficientFunds,
                ..
            }
        ),
        "{error:?}"
    );
    assert_needs_seats(&client, MIN_FEDERATION_SIZE);
    // A reopened FA still shows why the purchase ended.
    let reopened = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    let FaState::Creating(creation) = reopened.state() else {
        panic!("the creation needs seats");
    };
    assert_eq!(
        creation.stage,
        CreationStage::NeedsSeats {
            count: MIN_FEDERATION_SIZE,
            reauthorization: Some(SelectionReauthorizationReason::SelectedPayerInsufficientFunds),
        },
    );
    drop(reopened);
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), 0);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        *payment_state.fund_checks.lock().unwrap(),
        vec![vec![PAYMENT_AMOUNT_MSATS; usize::from(MIN_FEDERATION_SIZE)]],
    );

    // Exactly the total is enough, and the check runs once per purchase.
    *payment_state.checked_balance_msats.lock().unwrap() = Some(seats * PAYMENT_AMOUNT_MSATS);
    client
        .request_seats_at(&intent(), locators(), None)
        .await
        .unwrap();
    client.drive_all_for_test(options()).await.unwrap();
    federation(&client.status());
    assert_eq!(payment_state.fund_checks.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_repriced_seat_leaves_before_the_fund_check_and_any_payment() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    let preview = client.preview_at(&intent(), locators());
    mg_state
        .price_overrides_msats
        .lock()
        .unwrap()
        .insert(2, PAYMENT_AMOUNT_MSATS + 1);

    let error = client
        .create_and_buy(&preview, Some(payment_federation_id()), options())
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SeatUnavailable,
                ..
            }
        ),
        "{error:?}"
    );
    bought_without(&client.status(), &[2]);
    assert_eq!(
        *payment_state.fund_checks.lock().unwrap(),
        vec![vec![
            PAYMENT_AMOUNT_MSATS;
            usize::from(MIN_FEDERATION_SIZE) - 1
        ]],
    );
    assert!(
        mg_state
            .quote_records
            .lock()
            .unwrap()
            .iter()
            .all(|record| record.index != 2),
        "the repriced MG is never asked for a quote",
    );
}

#[tokio::test]
async fn insufficient_funds_ends_the_purchase_with_the_seats_it_bought() {
    let (payments, payment_state) = TestPayments::new();
    // The third payment finds the payer short.
    payment_state
        .fail_readiness_on_call
        .store(3, Ordering::SeqCst);
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;

    let error = client
        .buy_seats_at(&intent(), locators(), options())
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            FaError::SelectionReauthorizationRequired {
                reason: SelectionReauthorizationReason::SelectedPayerInsufficientFunds,
                failure: None,
            }
        ),
        "{error:?}"
    );
    assert_needs_seats(&client, MIN_FEDERATION_SIZE - 2);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        mg_state.quote_calls.load(Ordering::SeqCst),
        3,
        "the seats after the failed payment are never quoted",
    );
    assert_eq!(
        reauthorization(&client.status()),
        Some(SelectionReauthorizationReason::SelectedPayerInsufficientFunds)
    );
}

#[tokio::test]
async fn cancel_is_refused_while_a_bound_seat_payment_started() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state,
        MgConfig::paid(),
    )
    .await;
    let quote_ids = seed_bound_selected_purchase(&client).await;
    payment_state
        .funded_quotes
        .lock()
        .expect("test lock")
        .insert(quote_ids[3]);

    let error = client.cancel_creation().await.unwrap_err();

    assert!(
        matches!(
            error,
            FaError::AbandonUnavailable(crate::AbandonUnavailableReason::PaymentOutputsStarted)
        ),
        "{error:?}"
    );
    assert!(
        client
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn selected_payer_failure_returns_to_idle_and_live_approval_can_retry() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    payment_state
        .insufficient_funds
        .store(true, Ordering::SeqCst);
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state, MgConfig::paid()).await;
    let approval = selection_preview();

    let error = client
        .buy_and_form(intent(), &approval, payment_federation_id(), options())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SelectedPayerInsufficientFunds,
            failure: None
        }
    ));
    assert_needs_new_preview(&client);
    assert_eq!(
        payment_state.create_calls.load(Ordering::SeqCst),
        1,
        "the other seats leave without a payment attempt",
    );

    payment_state
        .insufficient_funds
        .store(false, Ordering::SeqCst);
    // The creation stays, so the retry buys its seats with the same approval.
    client
        .buy_seats(&approval, Some(payment_federation_id()))
        .await
        .unwrap();
    client.drive_all_for_test(options()).await.unwrap();
    federation(&client.status());
}

#[tokio::test]
async fn a_bound_seat_reopens_under_a_changed_verifier() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database.clone(),
        payments.clone(),
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    seed_bound_purchase(&client, "selected-verifier-drift-post-output", 120).await;
    drop(client);

    let reopened = open_client_with_verifier(
        database,
        payments,
        mg_state,
        MgConfig::paid(),
        peer_badge_verifier(ManifoldEnvironment::Staging),
    )
    .await;
    reopened
        .drive()
        .await
        .expect("a bound seat needs no fresh admission");
    bought(&reopened.status());
    assert!(payment_state.create_calls.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn unavailable_explicit_payer_stops_before_quotes_and_persistence() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    payment_state.pay_none.store(true, Ordering::SeqCst);
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;

    let error = client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SelectedPayerUnavailable,
            failure: None
        }
    ));
    assert_eq!(client.status(), FaStatus::default());
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn expired_selection_approval_stops_before_payer_or_quote_work() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(database, payments, mg_state.clone(), MgConfig::paid()).await;
    let fresh = selection_preview();
    let approval = MgSelectionPreview::for_test(
        fresh.request.clone(),
        fresh.fedimintd_dkg_version.clone(),
        fresh.verifier_provenance,
        fresh.seats().to_vec(),
        Timestamp(test_now_secs().saturating_sub(1)),
    );

    let error = client
        .buy_and_form(intent(), &approval, payment_federation_id(), options())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SeatUnavailable,
            failure: None
        }
    ));
    assert_eq!(client.status(), FaStatus::default());
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selection_approval_rejects_verifier_environment_drift_before_external_work() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig::paid(),
    )
    .await;
    let mut approval = selection_preview();
    approval.verifier_provenance = PeerBadgeVerifier::try_from_profile(
        &ManifoldEnvironment::Staging
            .profile()
            .expect("staging profile resolves"),
    )
    .expect("staging verifier resolves")
    .provenance();

    let error = client
        .buy_and_form(intent(), &approval, payment_federation_id(), options())
        .await
        .unwrap_err();

    assert!(matches!(error, FaError::InvalidIntent(_)));
    assert_eq!(payment_state.payable_calls.load(Ordering::SeqCst), 0);
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selected_mg_unavailability_requires_a_fresh_set_and_returns_idle() {
    let database = MemDatabase::new().into_database();
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        database,
        payments,
        mg_state.clone(),
        MgConfig {
            accepting_seats: false,
            ..MgConfig::paid()
        },
    )
    .await;

    let error = client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        &error,
        FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SeatUnavailable,
            failure: Some(MgFailure {
                operation: MgOperation::GetAvailability,
                cause: MgCause::NotAcceptingSeats,
                ..
            })
        }
    ));
    assert_needs_new_preview(&client);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selected_quote_capacity_race_requires_a_fresh_set_and_returns_idle() {
    let (payments, payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        mg_state.clone(),
        MgConfig {
            capacity_exhausted_quote: true,
            ..MgConfig::paid()
        },
    )
    .await;

    let error = client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        &error,
        FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SeatUnavailable,
            failure: Some(MgFailure {
                operation: MgOperation::GetQuote,
                cause: MgCause::CapacityExhausted,
                ..
            })
        }
    ));
    assert_needs_new_preview(&client);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(mg_state.quote_calls.load(Ordering::SeqCst), 7);
}

#[tokio::test]
async fn a_bound_seat_survives_reopen_and_resume() {
    // Hang the first CreateSeat so the initial run times out after it bound
    // and paid that seat, then prove a reopened resume completes it.
    let database = MemDatabase::new().into_database();
    let (payments, _payment_state) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let config = MgConfig {
        create_behavior: CreateBehavior::HangFirst,
        ..MgConfig::paid()
    };
    let client = open_client(database.clone(), payments.clone(), mg_state.clone(), config).await;
    let short = Timeouts {
        poll: Duration::from_millis(10),
        long: Duration::from_millis(1500),
        request: Duration::from_millis(500),
    };
    let error = client
        .buy_and_form_at(intent(), locators(), short)
        .await
        .unwrap_err();
    assert!(
        matches!(
            &error,
            FaError::Mg(MgFailure {
                operation: MgOperation::CreateSeat,
                cause: MgCause::Timeout,
                ..
            })
        ),
        "{error}"
    );
    let recovery = active_recovery(
        client
            .inner
            .store
            .load_purchase(TestIdentity::fa_id())
            .await
            .unwrap(),
    );
    assert!(
        recovery
            .slots
            .iter()
            .any(|seat| seat.seat_id().is_none() && seat.state.bound_quote().is_some()),
        "the hung seat stays bound to its quote",
    );
    drop(client);

    let reopened = open_client(database, payments, mg_state, MgConfig::paid()).await;
    reopened.drive().await.unwrap();
    let status = reopened.status();
    bought(&status);
}

#[tokio::test]
async fn liquidity_operation_listing_is_bounded_paginated_and_fail_closed() {
    let database = MemDatabase::new().into_database();
    let store = db::FaStore::new(database.clone());
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
        Arc::new(MgState::default()),
        MgConfig::given_away(),
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
            .list_liquidity_operations(None, FA_LIQUIDITY_OPERATION_PAGE_MAX + 1)
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
            .list_liquidity_operations(None, FA_LIQUIDITY_OPERATION_PAGE_MAX)
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
        let (client, formation_id, mg_state, connector) = formed_client_for_liquidity().await;
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
        let endorsement = request
            .payload
            .mg_endorsement
            .as_ref()
            .expect("request carries one admission endorsement");
        assert!(
            (0..usize::from(MIN_FEDERATION_SIZE))
                .map(test_attestation)
                .any(|attestation| {
                    attestation.attestation == endorsement.attestation.attestation
                }),
            "the endorsement attestation statement comes from the verified consensus directory",
        );
        assert_eq!(
            endorsement
                .trust
                .holder_authorization
                .authorization
                .subject_pubkey
                .0
                .to_string(),
            endorsement.attestation.attestation.mg_pubkey.0,
            "the live holder authorization is for the directory attestation's MG",
        );
        let trust = request
            .payload
            .mg_trust_material
            .as_ref()
            .expect("all MG trust material is carried");
        assert_eq!(trust.len(), usize::from(MIN_FEDERATION_SIZE));
        assert_eq!(
            trust
                .iter()
                .map(|response| response.material.mg_pubkey.clone())
                .collect::<BTreeSet<_>>()
                .len(),
            usize::from(MIN_FEDERATION_SIZE),
            "one signed response is carried for every distinct MG",
        );
        drop(requests);

        let trust_requests = mg_state.trust_material_requests.lock().expect("test lock");
        assert_eq!(trust_requests.len(), usize::from(MIN_FEDERATION_SIZE));
        assert_eq!(
            trust_requests.iter().copied().collect::<HashSet<_>>().len(),
            usize::from(MIN_FEDERATION_SIZE),
            "each distinct MG is queried once per exact request",
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
    let (client, formation_id, _mg_state, connector) = formed_client_for_liquidity().await;
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
    assert!(matches!(error, FaError::Liquidity(_)));
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
    let (client, formation_id, mg_state, connector) = formed_client_for_liquidity().await;
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
    let registrations = mg_state.gateway_registrations.lock().expect("test lock");
    assert_eq!(registrations.len(), usize::from(MIN_FEDERATION_SIZE));
    assert!(
        registrations
            .iter()
            .all(|(_, url)| url == gateway_api.as_str())
    );
}

#[tokio::test]
async fn completed_gateway_recovery_uses_durable_evidence_before_provider_connection() {
    let (client, formation_id, mg_state, connector) = formed_client_for_liquidity().await;
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
        mg_state
            .gateway_registrations
            .lock()
            .expect("test lock")
            .len(),
        usize::from(MIN_FEDERATION_SIZE),
    );
}

#[tokio::test]
async fn liquidity_recovery_replays_the_exact_commitment_only_after_not_found() {
    let (client, formation_id, _mg_state, connector) = formed_client_for_liquidity().await;
    client
        .update_federation_metadata_with(
            FederationMetadataUpdate::name("First rename").unwrap(),
            Timeouts::default(),
        )
        .await
        .unwrap();
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
    client
        .update_federation_metadata_with(
            FederationMetadataUpdate::name("Second rename").unwrap(),
            Timeouts::default(),
        )
        .await
        .unwrap();
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
        requests[0].payload.federation_details.federation_name.0,
        "First rename"
    );
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
    let (client, formation_id, _mg_state, connector) = formed_client_for_liquidity().await;
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
async fn liquidity_rejects_invalid_mg_and_provider_proofs_without_accepting() {
    let (client, formation_id, mg_state, connector) = formed_client_for_liquidity().await;
    let provider = liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string());

    mg_state
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
        .expect_err("invalid MG trust signature fails closed");
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
        let (client, formation_id, _mg_state, connector) = formed_client_for_liquidity().await;
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
    let (client, formation_id, _mg_state, connector) = formed_client_for_liquidity().await;
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
    let (client, formation_id, _mg_state, connector) = formed_client_for_liquidity().await;
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
        FaError::LiquidityOperationExists { operation_id } => operation_id,
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
        matches!(error, FaError::LiquidityOperationExists { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn provider_not_found_cannot_overturn_durable_acceptance_evidence() {
    let (client, formation_id, _mg_state, connector) = formed_client_for_liquidity().await;
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
    assert!(matches!(error, FaError::Liquidity(_)), "{error}");
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
    let (client, formation_id, _mg_state, connector) = formed_client_for_liquidity().await;
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

    let store = db::FaStore::new(MemDatabase::new().into_database());
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

    let store = db::FaStore::new(MemDatabase::new().into_database());
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
    let mg_state = Arc::new(MgState::default());
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
        mg_state.clone(),
        MgConfig::given_away(),
        registry,
    )
    .await;
    client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .expect("form selected test federation");
    let formation_id = federation(&client.status()).formation_id.clone();
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
        mg_state,
        MgConfig::given_away(),
        registry,
    )
    .await;
    reopened
        .drive()
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
    // correctly report staleness; the FA must read consensus before deciding
    // whether the operation failed.
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let threshold = test_federation_seats().consensus_threshold() as usize;
    mg_state
        .stale_meta_indices
        .lock()
        .expect("test lock")
        .extend(threshold..usize::from(MIN_FEDERATION_SIZE));
    let client = open_client(database, payments, mg_state.clone(), MgConfig::given_away()).await;

    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    federation(&client.status());
    assert_eq!(
        mg_state.meta_submissions.lock().expect("test lock").len(),
        threshold,
        "the threshold target should be accepted despite late stale responses"
    );
}

#[tokio::test]
async fn an_all_stale_wave_rereads_rebases_and_replays_one_identical_base() {
    let database = MemDatabase::new().into_database();
    let (payments, _) = TestPayments::new();
    let mg_state = Arc::new(MgState::default());
    let reader = TestConsensusReader::new(mg_state.clone());
    let initial = serde_json::to_vec(&serde_json::json!({ "external": "first" })).unwrap();
    let changed = serde_json::to_vec(&serde_json::json!({ "external": "second" })).unwrap();
    // The final config read validates the directory before the durable target
    // is pinned. Advance after the following metadata-base read so the first
    // proposal wave is stale rather than consuming the fixture on validation.
    reader.change_base_after_reads(initial.clone(), changed.clone(), 2);
    let client = open_client_with_reader(
        database,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;

    client
        .buy_and_form_at(intent(), locators(), options())
        .await
        .unwrap();

    federation(&client.status());
    let bases = mg_state.meta_request_bases.lock().expect("test lock");
    let seats = usize::from(MIN_FEDERATION_SIZE);
    assert_eq!(bases.len(), seats * 2, "one all-stale wave and one retry");
    // The advance from `initial` to `changed` is one adoption: the fake
    // consensus revision moves 0 -> 1 with it.
    let initial_base = MetaConsensusBase::from_consensus(Some((0, &initial)));
    let changed_base = MetaConsensusBase::from_consensus(Some((1, &changed)));
    assert!(bases[..seats].iter().all(|(_, base)| *base == initial_base));
    assert!(bases[seats..].iter().all(|(_, base)| *base == changed_base));
    assert_eq!(
        mg_state.meta_submissions.lock().expect("test lock").len(),
        seats,
        "the rebased retry sends an identical accepted request to every seat"
    );
}

// Keep the subscriber attached to the future, not a process-global default:
// formation tests run concurrently and each seat's diagnostics must stay isolated.
#[derive(Clone, Default)]
struct FormationLogCapture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for FormationLogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("test log lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl FormationLogCapture {
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        // A callsite first registered by a parallel test without a scoped
        // subscriber must not cache Interest::never for our capture.
        static TRACING: std::sync::Once = std::sync::Once::new();
        TRACING.call_once(|| {
            tracing::subscriber::set_global_default(tracing_subscriber::registry())
                .expect("test tracing registry is installed once");
        });
        let capture = self.clone();
        tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || capture.clone())
            .finish()
    }

    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("test log lock").clone()).unwrap()
    }
}

fn formation_log_options(request_timeout: Duration) -> Timeouts {
    Timeouts {
        poll: Duration::from_millis(1),
        // Leave room for parallel-suite scheduling; only the request bound is under test.
        long: Duration::from_secs(30),
        request: request_timeout,
    }
}

#[tokio::test]
async fn formation_request_logs_bind_concurrent_seats_and_exclude_remote_secrets() {
    use tracing::instrument::WithSubscriber as _;

    const SECRET: &str = "SENTINEL-private-remote-payload-refund-secret";
    let state = Arc::new(MgState::default());
    state
        .meta_terminal_errors
        .lock()
        .unwrap()
        .insert(2, ManifoldGuardianError::Other(SECRET.to_owned()));
    state.hang_meta_indices.lock().unwrap().insert(5);
    let (payments, _) = TestPayments::new();
    let reader = TestConsensusReader::new(state.clone());
    reader.force_value("test-consensus-does-not-adopt-proposals");
    let client = open_client_with_reader(
        MemDatabase::new().into_database(),
        payments,
        state.clone(),
        MgConfig::given_away(),
        reader,
    )
    .await;
    let capture = FormationLogCapture::default();
    let options = formation_log_options(Duration::from_millis(20));
    assert!(
        client
            .buy_and_form_at(intent(), locators(), options)
            .with_subscriber(capture.subscriber())
            .await
            .is_err()
    );
    // The failed formation remains recoverable.
    let snapshot = formation(&client.status()).clone();
    let logs = capture.text();
    let failures = logs
        .lines()
        .filter(|line| {
            line.contains("safe_to_share=true") && line.contains("formation request failed")
        })
        .collect::<Vec<_>>();
    for (index, cause) in [(2, "other"), (5, "timeout")] {
        let expected_key = manager_key(index).x_only_public_key().0.to_string();
        assert!(
            failures.iter().any(|line| {
                line.contains(&format!("mg_pubkey={expected_key}"))
                    && line.contains(&format!("formation_id={}", snapshot.formation_id.0))
                    && line.contains("operation=propose_formation_meta")
                    && line.contains(&format!("cause={cause}"))
            }),
            "{logs}"
        );
    }
    // Closed causes exclude remote text from both public events and final errors.
    assert!(!logs.contains(SECRET), "{logs}");
    for index in [0, 1, 3, 4, 6] {
        assert!(
            !failures.iter().any(|line| line.contains(&format!(
                "mg_pubkey={}",
                manager_key(index).x_only_public_key().0
            ))),
            "successful sibling was blamed: {logs}"
        );
    }
    assert!(
        failures.iter().all(|line| !line.contains("guardiancode")),
        "{logs}"
    );
    assert!(
        failures.iter().all(|line| !line.contains("endpoint_addr")),
        "{logs}"
    );
}

#[tokio::test]
async fn formation_connect_timeout_logs_identity_without_unassigned_seat_id() {
    use tracing::instrument::WithSubscriber as _;

    let state = Arc::new(MgState::default());
    *state.hang_connect_on_attempt.lock().unwrap() = Some((3, 1));
    let (payments, _) = TestPayments::new();
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        state,
        MgConfig::given_away(),
    )
    .await;
    let capture = FormationLogCapture::default();
    let options = formation_log_options(Duration::from_millis(20));
    // The selected seat is still value-safe, so the timed-out connection is
    // logged and retried.
    client
        .buy_and_form_at(intent(), locators(), options)
        .with_subscriber(capture.subscriber())
        .await
        .unwrap();
    let logs = capture.text();
    let line = logs
        .lines()
        .find(|line| line.contains("formation request failed"))
        .unwrap_or_else(|| panic!("missing request event: {logs}"));
    assert!(
        line.contains(&format!(
            "mg_pubkey={}",
            manager_key(3).x_only_public_key().0
        )),
        "{logs}"
    );
    assert!(line.contains("operation=connect"), "{logs}");
    assert!(line.contains("cause=timeout"), "{logs}");
    assert!(!line.contains("seat_id="), "{logs}");
    assert!(!line.contains("endpoint_addr"), "{logs}");
}

#[tokio::test]
async fn formation_quote_logs_both_nested_transport_and_service_errors() {
    use tracing::instrument::WithSubscriber as _;

    for transport in [true, false] {
        let state = Arc::new(MgState::default());
        let mut config = MgConfig::given_away();
        let payload = if transport {
            state
                .quote_transport_failures_remaining
                .store(1, Ordering::SeqCst);
            "injected quote stream loss"
        } else {
            config.reject_quote = true;
            "test daemon rejects the selected payment federation"
        };
        let (payments, _) = TestPayments::new();
        let client = open_client(MemDatabase::new().into_database(), payments, state, config).await;
        let capture = FormationLogCapture::default();
        let options = formation_log_options(Duration::from_millis(200));
        // A selected seat retries a transport loss before outputs, while a
        // service refusal returns.
        let result = client
            .buy_and_form_at(intent(), locators(), options)
            .with_subscriber(capture.subscriber())
            .await;
        assert_eq!(result.is_ok(), transport, "{result:?}");
        let logs = capture.text();
        assert!(
            logs.lines().any(|line| {
                line.contains("formation request failed")
                    && line.contains(&format!(
                        "mg_pubkey={}",
                        manager_key(0).x_only_public_key().0
                    ))
                    && line.contains("operation=get_quote")
                    && line.contains(if transport {
                        "cause=transport"
                    } else {
                        "cause=other"
                    })
                    && line.contains(if transport {
                        "outcome=\"retried\""
                    } else {
                        "outcome=\"returned\""
                    })
                    && !line.contains("seat_id=")
            }),
            "{logs}"
        );
        assert!(
            logs.lines()
                .filter(|line| {
                    line.contains("safe_to_share=true") && line.contains("formation request failed")
                })
                .all(|line| !line.contains(payload)),
            "{logs}"
        );
        assert!(!logs.contains(payload), "remote text leaked: {logs}");
    }
}

#[tokio::test]
async fn formation_acquisition_reconnect_logs_mg_identity() {
    use tracing::instrument::WithSubscriber as _;

    let state = Arc::new(MgState::default());
    state.failed_create_indices.lock().unwrap().insert(3);
    let (payments, _) = TestPayments::new();
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        state.clone(),
        MgConfig::given_away(),
    )
    .await;
    assert!(
        client
            .buy_and_form_at(intent(), locators(), options())
            .await
            .is_err()
    );
    let next_attempt = state.connect_attempts.lock().unwrap()[&3] + 1;
    *state.hang_connect_on_attempt.lock().unwrap() = Some((3, next_attempt));
    let capture = FormationLogCapture::default();
    let options = formation_log_options(Duration::from_millis(20));
    assert!(matches!(
        client
            .drive_with_options(options)
            .with_subscriber(capture.subscriber())
            .await
            .as_ref(),
        Err(FaError::Mg(MgFailure {
            operation: MgOperation::Connect,
            cause: MgCause::Timeout,
            ..
        }))
    ));
    let logs = capture.text();
    assert!(
        logs.lines().any(|line| {
            line.contains("formation request failed")
                && line.contains(&format!(
                    "mg_pubkey={}",
                    manager_key(3).x_only_public_key().0
                ))
                && line.contains("operation=connect")
                && line.contains("cause=timeout")
        }),
        "{logs}"
    );
}

#[tokio::test]
async fn a_never_ready_seat_keeps_the_formation_waiting_for_its_code() {
    let state = Arc::new(MgState::default());
    state.unavailable_dkg_code_indices.lock().unwrap().insert(5);
    let (payments, _) = TestPayments::new();
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        state,
        MgConfig::given_away(),
    )
    .await;
    // No step has a deadline, so the formation waits for the child of seat 5
    // until something else stops it.
    let waited = tokio::time::timeout(
        Duration::from_secs(1),
        client.buy_and_form_at(intent(), locators(), options()),
    )
    .await;
    assert!(waited.is_err(), "{waited:?}");
    let snapshot = formation(&client.status()).clone();
    for (index, seat) in snapshot.slots.iter().enumerate() {
        assert_eq!(
            seat.guardian_code.is_some(),
            index != 5,
            "only the never-ready child should lack its code"
        );
    }
}

/// The index of the formation seat whose guardian code names the federation.
async fn named_seat_index(client: &TestClient) -> usize {
    let formation = client
        .inner
        .store
        .load_formation(TestIdentity::fa_id())
        .await
        .unwrap()
        .expect("the pool is full");
    (0..usize::from(MIN_FEDERATION_SIZE))
        .find(|index| formation.slots[0].locator == locator(*index))
        .expect("the named seat is a test MG")
}

#[tokio::test]
async fn a_decommissioned_pool_seat_leaves_before_dkg_and_a_new_seat_forms() {
    let state = Arc::new(MgState::default());
    state.name_sensitive_codes.store(true, Ordering::SeqCst);
    let (payments, _) = TestPayments::new();
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        state.clone(),
        MgConfig::given_away(),
    )
    .await;
    client
        .request_seats_at(&intent(), locators(), None)
        .await
        .unwrap();
    client.drive_purchase_for_test(options()).await.unwrap();
    // The named seat ends, so another seat must carry the federation name.
    let ended = named_seat_index(&client).await;
    state.decommissioned_indices.lock().unwrap().insert(ended);

    client.drive_all_for_test(options()).await.unwrap();
    let status = client.status();
    bought_without(&status, &[u16::try_from(ended).unwrap()]);
    assert!(!is_forming(&status));
    assert!(state.start_callbacks.lock().unwrap().is_empty());
    let FaState::Creating(creation) = client.state() else {
        panic!("the creation needs a new seat");
    };
    assert_eq!(
        creation.stage,
        CreationStage::NeedsSeats {
            count: 1,
            reauthorization: Some(SelectionReauthorizationReason::SeatUnavailable),
        },
    );

    let spare = usize::from(MIN_FEDERATION_SIZE);
    client
        .request_seats_at(&intent(), vec![locator(spare)], None)
        .await
        .unwrap();
    client.drive_all_for_test(options()).await.unwrap();
    federation(&client.status());
}

#[tokio::test]
async fn a_decommissioned_pool_seat_stays_once_dkg_may_have_started() {
    let state = Arc::new(MgState::default());
    let (payments, _) = TestPayments::new();
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        state.clone(),
        MgConfig::given_away(),
    )
    .await;
    client
        .request_seats_at(&intent(), locators(), None)
        .await
        .unwrap();
    client.drive_purchase_for_test(options()).await.unwrap();
    // Every cached code means a crashed run may have sent StartDkg.
    let formation = client
        .inner
        .store
        .load_formation(TestIdentity::fa_id())
        .await
        .unwrap()
        .unwrap();
    for seat in &formation.slots {
        let mg = (0..usize::from(MIN_FEDERATION_SIZE))
            .find(|index| seat.locator == locator(*index))
            .unwrap();
        client
            .inner
            .store
            .record_guardian_code(
                &formation.formation_id,
                seat.index,
                GuardianCode(format!("guardiancode{mg}")),
            )
            .await
            .unwrap();
    }
    state.decommissioned_indices.lock().unwrap().insert(1);

    let error = client.drive_all_for_test(options()).await.unwrap_err();
    assert!(
        matches!(
            &error,
            FaError::Mg(MgFailure {
                operation: MgOperation::GetDkgCode,
                cause: MgCause::WrongState(ServiceStatus::Decommissioned),
                ..
            })
        ),
        "{error:?}"
    );
    assert_eq!(
        client
            .inner
            .store
            .pool_seats(TestIdentity::fa_id())
            .await
            .unwrap()
            .len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
}

#[tokio::test]
async fn a_decommissioned_pool_seat_leaves_after_codes_while_no_seat_started_dkg() {
    let state = Arc::new(MgState::default());
    let (payments, _) = TestPayments::new();
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        state.clone(),
        MgConfig::given_away(),
    )
    .await;
    client
        .request_seats_at(&intent(), locators(), None)
        .await
        .unwrap();
    client.drive_purchase_for_test(options()).await.unwrap();
    // Every cached code means a crashed run may have sent StartDkg.
    let formation = client
        .inner
        .store
        .load_formation(TestIdentity::fa_id())
        .await
        .unwrap()
        .unwrap();
    for seat in &formation.slots {
        let mg = (0..usize::from(MIN_FEDERATION_SIZE))
            .find(|index| seat.locator == locator(*index))
            .unwrap();
        client
            .inner
            .store
            .record_guardian_code(
                &formation.formation_id,
                seat.index,
                GuardianCode(format!("guardiancode{mg}")),
            )
            .await
            .unwrap();
    }
    state.decommissioned_indices.lock().unwrap().insert(1);
    state.report_status_new.store(true, Ordering::SeqCst);

    client.drive_all_for_test(options()).await.unwrap();
    bought_without(&client.status(), &[1]);
    assert!(state.start_callbacks.lock().unwrap().is_empty());
}

#[test]
fn flat_mg_failure_preserves_cause_and_separate_reauthorization_policy() {
    let mg = manager_key(4).x_only_public_key().0;
    for (cause, code) in [
        (MgCause::Timeout, FaErrorCode::Timeout),
        (MgCause::Transport, FaErrorCode::ManifoldGuardian),
        (
            MgCause::InvalidResponse,
            FaErrorCode::InvalidManifoldGuardians,
        ),
        (
            MgCause::FormationMetaAlreadyPublished,
            FaErrorCode::InvalidManifoldGuardians,
        ),
    ] {
        let failure = MgFailure {
            mg,
            operation: MgOperation::GetQuote,
            cause,
        };
        let error = FaError::Mg(failure.clone());
        assert_eq!(error.code(), code);
        let policy = FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SeatUnavailable,
            failure: Some(failure.clone()),
        };
        assert_eq!(policy.code(), FaErrorCode::SelectionReauthorizationRequired);
        assert!(matches!(policy, FaError::SelectionReauthorizationRequired {
            reason: SelectionReauthorizationReason::SeatUnavailable,
            failure: Some(actual),
        } if actual == failure));
    }
}

#[tokio::test(start_paused = true)]
async fn selected_connect_timeout_exhaustion_retains_blame_and_returns_idle() {
    let (payments, payment_state) = TestPayments::new();
    let state = Arc::new(MgState::default());
    state.hang_connect.store(true, Ordering::SeqCst);
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        state.clone(),
        MgConfig::paid(),
    )
    .await;
    let error = client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            unreachable_mg_options(Duration::from_millis(10)),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, FaError::SelectionReauthorizationRequired {
        reason: SelectionReauthorizationReason::SeatUnavailable,
        failure: Some(MgFailure { mg, operation: MgOperation::Connect, cause: MgCause::Timeout }),
    } if mg == manager_key(0).x_only_public_key().0));
    assert!(state.connect_calls.load(Ordering::SeqCst) > usize::from(MIN_FEDERATION_SIZE));
    assert_needs_new_preview(&client);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_selected_quote_returns_to_selection_with_blame() {
    let (payments, payment_state) = TestPayments::new();
    let state = Arc::new(MgState::default());
    let client = open_client(
        MemDatabase::new().into_database(),
        payments,
        state.clone(),
        MgConfig {
            invalid_quote_signature: true,
            ..MgConfig::paid()
        },
    )
    .await;
    let error = client
        .buy_and_form(
            intent(),
            &client.preview_at(&intent(), locators()),
            payment_federation_id(),
            options(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, FaError::SelectionReauthorizationRequired {
        reason: SelectionReauthorizationReason::SeatUnavailable,
        failure: Some(MgFailure { mg, operation: MgOperation::GetQuote, cause: MgCause::InvalidResponse }),
    } if mg == manager_key(0).x_only_public_key().0));
    assert_needs_new_preview(&client);
    assert_eq!(payment_state.create_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.create_calls.load(Ordering::SeqCst), 0);
}
