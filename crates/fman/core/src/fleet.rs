//! L4 seat lifecycle: allocation and the in-memory registry of seats.
//!
//! SQLite is the serialization point for cross-seat admission and the
//! lifetime port cursor. The registry map is a `std` `RwLock` held only for
//! lookup/insert. Everything about one particular seat after its
//! allocation — the lifecycle commands and the per-seat task that serializes
//! them — lives on `Seat`. SQLite records decisions made by that task.
//! Bounded live facts and outstanding payment hand-offs are loaded by
//! [`Fleet::open`].

mod payout;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use fedi_decentralized_domain::Pubkey;
use fedi_decentralized_service_fleet_manager::{
    CreateSeatResponse, FederationId, FiId, GuardianTelemetrySeat, Plan, QuoteId, QuoteTerms,
    RefundTransaction, RefusalReason, SafeEventJournal, SeatId, SeatScopedFiRequest,
    SignedResponse, TelemetryCapability, VerifiedFiRequest,
};
use tokio::sync::Notify;

use crate::guardian_fee::{
    AccountId, Collected, FederationFeeStatus, FeePolicy, GuardianFeeAccountKey, GuardianFeeVault,
    Remittance,
};
use crate::wallet::EcashPayoutWorker;
use crate::wallet::{ClaimOutcome, EcashClaimWorker, EcashWallet, Msats, VerifiedLockedPayment};

use crate::backup::BackupSink;
use crate::backup_worker::BackupWorker;
use crate::db::{Db, NewPayment, NewSeat, Offer, QuoteSettings, SeatAdmissionResult, SeatRecord};
use crate::facts::{PortBase, SeatPorts};
use crate::identity::RootMnemonic;
use crate::payout_wire::WalletDrainStatusWire;
use crate::push_callback::{CompletionCallbackInvoker, CompletionHookWorker, PushGatewayOrigin};
use crate::seat::{
    PaymentClaimStatus, Seat, SeatDurableState, SeatReport, SeatRuntimeDependencies, SeatSummary,
    SeatVerbError,
};
use crate::seat_process::{RespawnPolicy, SeatProcessConfig, SeatProcessSpawner};

// No `Debug`: [`SeatProcessConfig`] may hold a bitcoind password.
#[derive(Clone)]
pub struct FleetConfig {
    /// The environment this data root was bound to before onboarding.
    pub manifold_environment: fedi_decentralized_manifold_environment::ManifoldEnvironment,
    /// First port block base; a release constant, not an operator knob.
    /// Already a [`PortBase`] so an unusable base fails at configuration
    /// time instead of silently exhausting the port grid.
    pub first_port_base: PortBase,
    pub respawn: RespawnPolicy,
    pub process: SeatProcessConfig,
    /// Whether this environment's profile carries a setup-payment publisher.
    /// Without one no common set can ever be admitted, so a price could
    /// never be paid; `set_offered_price` refuses rather than selling seats
    /// no one can buy.
    pub setup_payments_configured: bool,
    /// How often the backup worker rescans every seat with nothing marked
    /// ([`crate::backup_worker::DEFAULT_SCAN_INTERVAL`]).
    pub backup_scan_interval: Duration,
    /// Exact public push-gateway origin accepted for FI callback capabilities.
    /// `None` keeps ordinary FMan operation available but rejects callbacks.
    pub push_gateway_origin: Option<PushGatewayOrigin>,
    /// Probe/retry cadence while one DKG completion callback is pending.
    pub push_callback_retry_interval: Duration,
    /// Network capability supplied by the binary composition root.
    pub completion_callback_invoker: Arc<dyn CompletionCallbackInvoker>,
    /// Process-boundary capability used to start supervised seat children.
    pub process_spawner: SeatProcessSpawner,
}

/// Payment has already been authenticated and verified offline. Fleet owns
/// only the serialized allocation/refusal decision that follows verification.
pub struct VerifiedCreateSeat {
    pub fi_id: FiId,
    pub quote_id: QuoteId,
    pub quote_terms: QuoteTerms,
    /// `Paid` iff the quote is paid — the boundary verified that
    /// correlation along with the payment itself, so the fleet never
    /// re-derives paid-ness from the price.
    pub payment: VerifiedPayment,
}

/// The already-verified payment accompanying a creation: free, or an
/// offline-verified locked payment carrying everything its own settlement
/// needs — the claim's addressing, the evidence to persist, and (inside
/// the opaque wallet payment) the fee-checked FI refund outputs. The
/// refund transaction is instantiated only after the durable admission
/// decision returns a refusal.
pub enum VerifiedPayment {
    Free,
    Locked {
        federation_id: FederationId,
        payment: VerifiedLockedPayment,
    },
    /// Test-only paid payment: its claim is a no-op and the refund bytes are
    /// supplied directly, so allocation tests can exercise the paid decision
    /// paths without any wallet cryptography.
    #[cfg(test)]
    TestRefund {
        federation_id: FederationId,
        transaction: RefundTransaction,
    },
}

impl VerifiedPayment {
    /// The payment ledger row persisted with an accepted paid seat.
    fn accepted_payment(&self) -> Option<NewPayment> {
        match self {
            VerifiedPayment::Locked { payment, .. } => Some(NewPayment {
                evidence: payment.claim_evidence().clone(),
            }),
            VerifiedPayment::Free => None,
            #[cfg(test)]
            VerifiedPayment::TestRefund { .. } => Some(NewPayment {
                evidence: crate::wallet::EcashClaimEvidence::test(0),
            }),
        }
    }

    /// A paid payment's refund settlement material: the federation it
    /// settles against and the signed refund transaction. `None` for free.
    fn into_refund(self) -> Option<(FederationId, RefundTransaction)> {
        match self {
            VerifiedPayment::Free => None,
            VerifiedPayment::Locked {
                federation_id,
                payment,
                ..
            } => Some((federation_id, payment.into_refund_transaction())),
            #[cfg(test)]
            VerifiedPayment::TestRefund {
                federation_id,
                transaction,
            } => Some((federation_id, transaction)),
        }
    }
}

/// Operator-facing state of one accepted payment federation.
#[derive(Clone, Debug)]
pub struct PaymentFederationStatus {
    pub federation_id: FederationId,
    /// Member of the currently admitted common setup-payment set. A
    /// `false` here is a durable wallet leftover of a removed member. After a
    /// restart its client may remain dormant: listing stays fail-closed, while
    /// sweep still requires reopening work not implemented here.
    pub accepted: bool,
    pub receivable: bool,
    /// Explicit, fail-closed wallet value and operation projection.
    pub wallet: WalletDrainStatusWire,
}

/// The operator-controlled input to quoting, and nothing else. Quote terms are
/// a function of this value and the request; the offer epoch labels the
/// exact value under which they were priced.
/// The shared availability view used by RPC and to gate advertising.
///
/// Capacity leaves the daemon only as the RPC boolean and as the decision to
/// publish or skip an advertisement. The seat count remains operator-private.
pub(crate) struct AvailabilitySnapshot {
    pub(crate) accepting_seats: bool,
    pub(crate) plans: Vec<Plan>,
}

pub struct FleetNostrHost {
    fleet: Arc<Fleet>,
    iroh_endpoint_id: String,
    /// Commitment-signing service pubkey, the same value the locator
    /// carries, so advertisement and locator can never disagree.
    service_pubkey: secp256k1::XOnlyPublicKey,
}

impl FleetNostrHost {
    pub fn new(
        fleet: Arc<Fleet>,
        iroh_endpoint_id: String,
        service_pubkey: secp256k1::XOnlyPublicKey,
    ) -> Self {
        Self {
            fleet,
            iroh_endpoint_id,
            service_pubkey,
        }
    }

    /// What a directory runtime reads out of the daemon each cycle.
    ///
    /// Read-only and whole: `None` suppresses publication while the FMan is not
    /// accepting seats; otherwise the snapshot is everything an advertisement
    /// needs, so no runtime assembles one from reads that could disagree.
    pub async fn advertisement(&self) -> Option<crate::directory::AdvertisementSnapshot> {
        let snapshot = self.fleet.availability_snapshot().await;
        snapshot
            .accepting_seats
            .then_some(crate::directory::AdvertisementSnapshot {
                iroh_endpoint_id: self.iroh_endpoint_id.clone(),
                service_pubkey: self.service_pubkey,
                plans: snapshot.plans,
            })
    }

    /// Wait until daemon-owned state that contributes to the advertisement may
    /// have changed. Notifications coalesce: the publisher always rebuilds one
    /// complete snapshot instead of replaying individual mutations.
    pub async fn advertisement_changed(&self) {
        self.fleet.advertisement_changed.notified().await;
    }
}

/// Couples durable retention of the admitted setup-payment publication to
/// this fleet's quote policy: the nostr boundary's admission calls back into
/// this store, which replaces the retained event and the derived accepted
/// membership — bumping the offer epoch when a member was removed — in one
/// database transaction ([`Db::replace_setup_payment_policy`]).
pub struct FleetSetupPaymentPolicyStore {
    fleet: Arc<Fleet>,
}

/// Durable cache owned by the fleet and populated only by explicit
/// Holder-authorization enrollment refreshes from the Nostr boundary.
pub struct FleetHolderAuthorizationStore {
    db: Db,
}

impl FleetHolderAuthorizationStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn for_fleet(fleet: &Fleet) -> Self {
        Self {
            db: fleet.db.clone(),
        }
    }

    pub async fn load(&self, max_issued_at: u64) -> anyhow::Result<Vec<String>> {
        Ok(self
            .db
            .bounded_holder_authorization_event_jsons(max_issued_at)
            .await?)
    }

    pub async fn merge(
        &self,
        events: &[(Vec<u8>, u64, String)],
        max_issued_at: u64,
    ) -> anyhow::Result<()> {
        self.db
            .merge_holder_authorization_events(events, max_issued_at)
            .await?;
        Ok(())
    }
}

impl FleetSetupPaymentPolicyStore {
    pub fn new(fleet: Arc<Fleet>) -> Self {
        Self { fleet }
    }

    /// Load the durably retained complete event JSON, if any.
    pub async fn load(&self) -> anyhow::Result<Option<String>> {
        Ok(self.fleet.db.setup_payment_event_json().await?)
    }

    /// Atomically retain a newly admitted event (already authenticated and
    /// strictly above the stored one in replacement order) together with its
    /// admitted membership.
    ///
    /// The event and its policy consequences — the derived accepted
    /// membership, and a fresh offer epoch when a member was removed — land in
    /// one database transaction, so a crash can never retain a replacement
    /// without its policy effect. This is not the only offer-epoch writer, so
    /// it composes with concurrent bumps rather than owning the epoch.
    pub async fn replace(
        &self,
        event_json: &str,
        admitted: &fedi_decentralized_domain::AdmittedSetupPaymentFederations,
    ) -> anyhow::Result<()> {
        let member_ids: Vec<FederationId> = admitted
            .iter()
            .map(|(federation_id, _)| FederationId(federation_id.0.clone()))
            .collect();
        self.fleet
            .db
            .replace_setup_payment_policy(event_json, &member_ids)
            .await?;
        Ok(())
    }
}

pub struct Fleet {
    config: FleetConfig,
    db: Db,
    identity: Arc<RootMnemonic>,
    wallet: Arc<dyn EcashWallet>,
    /// The reconciling publisher of recovery documents
    /// (SPEC-nostr-backup-restore). A worker with no sink is inert, which is
    /// a test-only construction: the daemon's relay comes from the Manifold
    /// environment profile.
    backup: Arc<BackupWorker>,
    completion_hooks: CompletionHookWorker,
    fedimint_connectors: fedimint_connectors::ConnectorRegistry,
    claims: Arc<dyn EcashClaimWorker>,
    payouts: Arc<dyn EcashPayoutWorker>,
    seats: RwLock<HashMap<SeatId, Arc<Seat>>>,
    telemetry_generation: AtomicU64,
    telemetry_registration_changed: Notify,
    /// Edge trigger for the Nostr runtime to rebuild its advertisement. This is
    /// deliberately independent of the offer epoch, whose only job is quote
    /// validation.
    advertisement_changed: Notify,
}

/// Failure while resolving a capability-scoped guardian metrics target.
#[derive(Debug)]
pub enum TelemetryAccessError {
    /// Capability mismatch.
    Unauthorized,
    /// The capability is valid but the child has not reached Running, is
    /// decommissioned, or cannot currently report its lifecycle.
    Unavailable,
}

impl Fleet {
    /// Open the store, rebuild the in-memory registry from it, and respawn
    /// every created seat's child. Ordinary ceremony phase is rederived by
    /// probing whenever a verb needs it. Completion callbacks are the explicit
    /// exception: their pending/operator-blocked/terminal outcome is durable,
    /// and one fleet-wide worker reconstructs delivery independently of seats.
    pub async fn open(
        db: Db,
        config: FleetConfig,
        wallet: Arc<dyn EcashWallet>,
    ) -> anyhow::Result<Self> {
        // A discarding sink, not an absent one: every fleet has a backup
        // worker, and callers without a relay — tests — get one whose
        // "relay" confirms everything and stores nothing.
        Self::open_with_wallet(
            db,
            config,
            async |_| Ok(wallet),
            async |_| Ok(Arc::new(crate::backup::DiscardBackupSink) as _),
        )
        .await
    }

    /// Like [`Fleet::open`], but the wallet is built from the install's
    /// root identity (which lives in this fleet's database, so it is not
    /// known before open).
    pub async fn open_with_wallet(
        db: Db,
        config: FleetConfig,
        wallet: impl AsyncFnOnce(&RootMnemonic) -> anyhow::Result<Arc<dyn EcashWallet>>,
        backup: impl AsyncFnOnce(&RootMnemonic) -> anyhow::Result<Arc<dyn BackupSink>>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !config.push_callback_retry_interval.is_zero(),
            "push callback retry interval must be nonzero"
        );
        tokio::fs::create_dir_all(&config.process.data_root)
            .await
            .map_err(|err| {
                anyhow!(
                    "create fleet data root {}: {err}",
                    config.process.data_root.display()
                )
            })?;
        db.bind_manifold_environment(config.manifold_environment)
            .await?;
        anyhow::ensure!(
            db.onboarding_stage().await? == crate::db::OnboardingStage::Complete,
            "this Fleet Manager has not completed onboarding"
        );
        // A fleet is opened against an identity that already exists. Acquiring
        // one is onboarding's job ([`crate::onboarding`]), and it happens
        // before this: every key below derives from the phrase, so there is
        // nothing here that could run without it.
        let identity = Arc::new(
            db.load_identity()
                .await?
                .ok_or_else(|| anyhow!("this Fleet Manager has not been onboarded"))?,
        );
        let telemetry_generation = db.telemetry_capability_generation().await?;
        let wallet = wallet(&identity).await?;
        let claims = wallet.clone().start_claim_worker(db.clone());
        let payout_identity = identity.clone();
        let payouts = wallet.clone().start_payout_worker(
            db.clone(),
            Arc::new(move |seat_id| payout_identity.derive_guardian_fee_account_key(seat_id)),
        );
        // The worker's first scan covers the whole fleet, so startup needs no
        // per-seat marking: anything whose durable state moved while the
        // daemon was down — or whose last publish confirmed but crashed
        // before its record — is found by comparison, and everything else
        // costs a hash, not a relay round trip.
        let backup = BackupWorker::new(backup(&identity).await?, config.backup_scan_interval);
        db.validate_completion_callbacks().await?;
        backup.spawn(db.clone(), config.process.clone());
        let completion_hooks = CompletionHookWorker::new(
            db.clone(),
            config.push_gateway_origin.clone(),
            config.push_callback_retry_interval,
            config.completion_callback_invoker.clone(),
        );
        let fedimint_connectors =
            fedimint_connectors::ConnectorRegistry::build_from_server_defaults()
                .bind()
                .await
                .context("bind native Fedimint API connectors")?;
        let mut map = HashMap::new();
        for seat in db.list_seats().await? {
            let SeatRecord {
                facts,
                decommissioned_at_ms,
            } = seat;
            let formed_invite = db.formed_federation_invite(&facts.seat_id).await?;
            let ports =
                SeatPorts::from_base(facts.seat_no.port_base(config.first_port_base).ok_or_else(
                    || anyhow!("stored seat_no {} exceeds port grid", facts.seat_no.0),
                )?);
            let keys = identity.derive_seat_keys(&facts.seat_id);
            let seat = Seat::start(
                SeatDurableState {
                    facts,
                    formed_invite,
                    decommissioned_at_ms,
                },
                SeatRuntimeDependencies {
                    db: db.clone(),
                    process: config.process.clone(),
                    policy: config.respawn,
                    keys,
                    own_fman_pubkey: attestation_pubkey(&identity),
                    ports,
                    fedimint_connectors: fedimint_connectors.clone(),
                    backup: backup.clone(),
                    completion_hooks: completion_hooks.wake_handle(),
                    process_spawner: config.process_spawner.clone(),
                },
            );
            map.insert(seat.facts().seat_id.clone(), seat);
        }

        let fleet = Self {
            config,
            db,
            identity,
            wallet,
            backup,
            completion_hooks,
            fedimint_connectors,
            claims,
            payouts,
            seats: RwLock::new(map),
            telemetry_generation: AtomicU64::new(telemetry_generation),
            telemetry_registration_changed: Notify::new(),
            advertisement_changed: Notify::new(),
        };
        Ok(fleet)
    }

    pub fn identity(&self) -> &RootMnemonic {
        &self.identity
    }

    /// One root-derived bearer for every telemetry resource owned by this
    /// FMan. It is handed only to the enrollment transport.
    pub fn telemetry_capability(&self) -> TelemetryCapability {
        self.telemetry_registration_capability().1
    }

    /// Return one coherent generation and capability pair for registration.
    ///
    /// Repeated reads are idempotent until an owner explicitly rotates the
    /// FMan-wide capability.
    pub fn telemetry_registration_capability(&self) -> (u64, TelemetryCapability) {
        let generation = self.telemetry_generation.load(Ordering::SeqCst);
        let capability = self.identity.derive_telemetry_capability(generation);
        (generation, capability)
    }

    /// Rotate the one FMan-wide telemetry capability and wake registration.
    /// The previous capability stops authorizing every telemetry surface
    /// before this returns.
    pub async fn reenroll_telemetry(&self) -> anyhow::Result<()> {
        let generation = self.db.rotate_telemetry_capability_generation().await?;
        // Concurrent owner-local commands may finish on different runtime
        // threads. Never let a later continuation reinstall an older durable
        // generation in memory.
        self.telemetry_generation
            .fetch_max(generation, Ordering::SeqCst);
        self.telemetry_registration_changed.notify_one();
        Ok(())
    }

    /// Wait until an operator rotation requires immediate re-registration.
    pub async fn telemetry_registration_changed(&self) {
        self.telemetry_registration_changed.notified().await;
    }

    /// Snapshot all known seats and their invite when formation has produced
    /// one. Seat identity is resource discovery, not an authorization scope.
    pub fn telemetry_seats(&self) -> Vec<GuardianTelemetrySeat> {
        let seats = self
            .seats
            .read()
            .expect("seat registry lock is never poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut advertised = Vec::with_capacity(seats.len());
        for seat in seats {
            advertised.push(GuardianTelemetrySeat {
                seat_id: seat.facts().seat_id.clone(),
                invite_code: seat.cached_invite_code(),
            });
        }
        advertised.sort_by(|left, right| left.seat_id.cmp(&right.seat_id));
        advertised
    }

    /// Validate the common FMan telemetry bearer without inspecting a seat or
    /// journal selection.
    pub fn authorize_telemetry(
        &self,
        supplied: &TelemetryCapability,
    ) -> Result<(), TelemetryAccessError> {
        let expected = self.telemetry_capability();
        if constant_time_eq::constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
            Ok(())
        } else {
            Err(TelemetryAccessError::Unauthorized)
        }
    }

    /// Validate the common bearer before selecting a running seat's fixed
    /// loopback metrics port.
    pub async fn authorize_telemetry_scrape(
        &self,
        seat_id: &SeatId,
        supplied: &TelemetryCapability,
    ) -> Result<u16, TelemetryAccessError> {
        self.authorize_telemetry(supplied)?;
        let seat = self
            .seat_by_id(seat_id)
            .ok_or(TelemetryAccessError::Unavailable)?;
        let report = seat
            .report()
            .await
            .map_err(|_| TelemetryAccessError::Unavailable)?;
        if !matches!(
            report,
            SeatReport::Active {
                phase: crate::seat::SeatPhase::Running { .. },
                ..
            }
        ) {
            return Err(TelemetryAccessError::Unavailable);
        }
        Ok(seat.metrics_port())
    }

    pub fn safe_event_journals(&self) -> Vec<SafeEventJournal> {
        let mut journals = self
            .seats
            .read()
            .expect("seat registry lock is never poisoned")
            .keys()
            .cloned()
            .map(|seat_id| SafeEventJournal::Seat { seat_id })
            .collect::<Vec<_>>();
        journals.sort();
        journals.insert(0, SafeEventJournal::Fman);
        journals
    }

    pub fn safe_event_journal_dir(&self, journal: &SafeEventJournal) -> Option<PathBuf> {
        match journal {
            SafeEventJournal::Fman => Some(self.config.process.data_root.join("safe-events/fman")),
            SafeEventJournal::Seat { seat_id } => self.seat_by_id(seat_id).map(|seat| {
                crate::seat_process::safe_event_dir(&self.config.process, seat.facts().seat_no)
            }),
        }
    }

    /// Shared database pool handed to deep infrastructure modules that own
    /// their private tables and queries.
    pub fn database_pool(&self) -> sqlx::SqlitePool {
        self.db.pool().clone()
    }

    pub fn config(&self) -> &FleetConfig {
        &self.config
    }

    /// Stop every supervised child. Used by the daemon before process exit and
    /// by embedders before dropping the runtime; `Drop` remains best-effort.
    pub async fn shutdown(&self) {
        self.completion_hooks.shutdown().await;
        self.claims.shutdown().await;
        let seats = self
            .seats
            .read()
            .expect("seat registry lock is never poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for seat in seats {
            seat.stop().await;
        }
    }

    /// One coherent epoch for quote policy and operator reads.
    pub(crate) async fn operator_settings(&self) -> QuoteSettings {
        self.db
            .offer_snapshot(self.config.first_port_base)
            .await
            .expect("stored quote settings are valid")
            .offer
            .settings
    }

    pub async fn offered_plans(&self) -> Vec<Plan> {
        self.operator_settings().await.plans()
    }

    /// The payment wallet, for the boundary's quote/verification calls.
    /// The fleet's own job stays the serialized allocation decision;
    /// wallet operations that need no such serialization go straight to
    /// the wallet.
    pub fn wallet(&self) -> &Arc<dyn EcashWallet> {
        &self.wallet
    }

    /// Replace the operator's offer: a price to sell seats at, or nothing to
    /// stop selling. There is one plan to offer, so a price is the whole
    /// offer; [`Fleet::offered_plans`] renders it as the wire states it.
    pub async fn set_offered_price(&self, price: Option<Msats>) -> anyhow::Result<()> {
        if matches!(price, Some(Msats(price)) if price > 0)
            && !self.config.setup_payments_configured
        {
            anyhow::bail!(
                "this environment has no setup-payment publisher, so a paid seat could never \
                 be paid for; a zero price or no price remains available"
            );
        }
        self.db.set_offered_price(price).await?;
        self.advertisement_changed.notify_one();
        Ok(())
    }

    pub async fn max_seats(&self) -> u32 {
        self.db
            .max_seats()
            .await
            .expect("stored max seats are valid")
    }

    pub async fn set_max_seats(&self, max_seats: u32) -> anyhow::Result<()> {
        self.db.set_max_seats(max_seats).await?;
        self.advertisement_changed.notify_one();
        Ok(())
    }

    pub async fn available_slots(&self) -> u32 {
        self.db
            .offer_snapshot(self.config.first_port_base)
            .await
            .expect("offer snapshot")
            .slots
    }

    /// Capacity, epoch, and quote settings used to mint a quote, read in
    /// one database transaction.
    pub(crate) async fn quote_offer(&self) -> Option<Offer> {
        let snapshot = self
            .db
            .offer_snapshot(self.config.first_port_base)
            .await
            .expect("offer snapshot");
        (snapshot.slots > 0).then_some(snapshot.offer)
    }

    /// What the advertisement and `GetAvailability` say: whether a seat
    /// would be allocated right now. False with no free capacity, and false
    /// when the operator has configured no offer. Payment-policy membership and
    /// retained wallet-client readiness are RPC concerns, not advertisement
    /// availability; `GetQuote` remains authoritative.
    /// No advertisement is produced while this is false; an earlier one ages
    /// out at its signed expiry.
    pub(crate) async fn availability_snapshot(&self) -> AvailabilitySnapshot {
        let snapshot = self
            .db
            .offer_snapshot(self.config.first_port_base)
            .await
            .expect("offer snapshot");
        let settings = snapshot.offer.settings;
        AvailabilitySnapshot {
            accepting_seats: snapshot.slots > 0 && settings.price.is_some(),
            plans: settings.plans(),
        }
    }

    /// Make the single durable allocation decision for a preverified quote.
    pub async fn create_seat(
        &self,
        input: VerifiedCreateSeat,
        sign: impl FnOnce(&SeatId) -> anyhow::Result<SignedResponse<CreateSeatResponse>>,
        sign_refusal: impl FnOnce(
            RefusalReason,
            Option<&RefundTransaction>,
        ) -> anyhow::Result<SignedResponse<CreateSeatResponse>>,
    ) -> anyhow::Result<SignedResponse<CreateSeatResponse>> {
        let seat_id = SeatId::from(input.quote_id);
        // Signing can fail and must do so before the durable acceptance. SQLite
        // then decides replay, epoch validity, capacity, allocation, payment
        // persistence, and last-slot epoch rotation under one writer boundary.
        let commitment = sign(&seat_id)?;
        let plan = input.quote_terms.request.plan;
        let federation_size = input.quote_terms.request.federation_size;
        let payment = input.payment.accepted_payment();
        let paid = payment.is_some();
        let result = self
            .db
            .admit_seat(
                NewSeat {
                    seat_id: seat_id.clone(),
                    fi_id: input.fi_id,
                    plan,
                    federation_size,
                    payment,
                },
                input.quote_terms.offer_epoch,
                self.config.first_port_base,
            )
            .await?;

        let (durable, inserted) = match result {
            SeatAdmissionResult::OfferChanged => {
                let refund = input.payment.into_refund().map(|(_, refund)| refund);
                return sign_refusal(RefusalReason::OfferChanged, refund.as_ref());
            }
            SeatAdmissionResult::Inserted(facts) => (
                SeatDurableState {
                    facts,
                    formed_invite: None,
                    decommissioned_at_ms: None,
                },
                true,
            ),
            SeatAdmissionResult::Existing(facts) => (
                SeatDurableState {
                    formed_invite: self.db.formed_federation_invite(&facts.seat_id).await?,
                    decommissioned_at_ms: self.db.decommissioned_at_ms(&facts.seat_id).await?,
                    facts,
                },
                false,
            ),
        };

        let (_, installed) = self.ensure_seat_runtime(durable);
        if inserted || installed {
            // Hint, never await: an FI's CreateSeat must not wait on the
            // operator's relay, and neither must anything downstream of it.
            self.backup.mark();
        }
        if paid || !inserted {
            self.claims.mark();
        }
        Ok(commitment)
    }

    /// Publish one durable seat into the process registry. Fresh admission
    /// installs synchronously after commit; a durable replay can first hydrate
    /// lifecycle state to repair an older interrupted publication.
    fn ensure_seat_runtime(&self, durable: SeatDurableState) -> (Arc<Seat>, bool) {
        let SeatDurableState {
            facts,
            formed_invite,
            decommissioned_at_ms,
        } = durable;
        if let Some(seat) = self
            .seats
            .read()
            .expect("seat registry lock is never poisoned")
            .get(&facts.seat_id)
            .cloned()
        {
            return (seat, false);
        }

        let ports = SeatPorts::from_base(
            facts
                .seat_no
                .port_base(self.config.first_port_base)
                .expect("admitted seat_no has a port block"),
        );
        let keys = self.identity.derive_seat_keys(&facts.seat_id);
        let mut seats = self
            .seats
            .write()
            .expect("seat registry lock is never poisoned");
        if let Some(seat) = seats.get(&facts.seat_id).cloned() {
            return (seat, false);
        }
        let seat = Seat::start(
            SeatDurableState {
                facts,
                formed_invite,
                decommissioned_at_ms,
            },
            SeatRuntimeDependencies {
                db: self.db.clone(),
                process: self.config.process.clone(),
                policy: self.config.respawn,
                keys,
                own_fman_pubkey: attestation_pubkey(&self.identity),
                ports,
                fedimint_connectors: self.fedimint_connectors.clone(),
                backup: self.backup.clone(),
                completion_hooks: self.completion_hooks.wake_handle(),
                process_spawner: self.config.process_spawner.clone(),
            },
        );
        seats.insert(seat.facts().seat_id.clone(), seat.clone());
        (seat, true)
    }

    /// Operator view of one seat, including terminal seats: durable facts
    /// plus the live report.
    pub async fn admin_seat_status(
        &self,
        seat_id: &SeatId,
    ) -> anyhow::Result<Option<(SeatSummary, SeatReport)>> {
        let Some(seat) = self.seat_by_id(seat_id) else {
            return Ok(None);
        };
        let report = seat.report().await.map_err(anyhow::Error::new)?;
        Ok(Some((self.seat_summary(&seat).await?, report)))
    }

    /// Operator listing of every durable seat.
    pub async fn seat_summaries(&self) -> anyhow::Result<Vec<SeatSummary>> {
        let seats: Vec<_> = self
            .seats
            .read()
            .expect("seat registry lock is never poisoned")
            .values()
            .cloned()
            .collect();
        let mut summaries = Vec::with_capacity(seats.len());
        for seat in seats {
            summaries.push(self.seat_summary(&seat).await?);
        }
        summaries.sort_by_key(|summary| summary.created_at_ms);
        Ok(summaries)
    }

    async fn seat_summary(&self, seat: &Seat) -> anyhow::Result<SeatSummary> {
        let claim = match self.db.payment(&seat.facts().seat_id).await? {
            None => PaymentClaimStatus::NotPaid,
            Some(payment) => match (payment.outcome, payment.outcome_at_ms) {
                (Some(ClaimOutcome::Success), Some(at_ms)) => PaymentClaimStatus::Success { at_ms },
                (Some(ClaimOutcome::AlreadySpent), Some(at_ms)) => {
                    PaymentClaimStatus::AlreadySpent { at_ms }
                }
                _ => PaymentClaimStatus::Pending,
            },
        };
        let backup = self
            .db
            .backup_publication(&seat.facts().seat_id, self.backup.format_version())
            .await?
            .map(|record| crate::seat::SeatBackupStatus {
                published_at_ms: record.published_at_ms,
                archive_confirmed: record.archive_digest.is_some(),
            });
        let completion_callback = self
            .db
            .completion_callback_status(&seat.facts().seat_id)
            .await?
            .unwrap_or(crate::facts::CompletionCallbackStatus::NotConfigured);
        Ok(seat.summary(claim, completion_callback, backup))
    }

    /// The backup worker's last completed reconciliation pass, for the
    /// operator's health view. `None` before the first scan finishes (or with
    /// no relay configured); a stale timestamp means the worker is wedged.
    pub fn backup_scan(&self) -> Option<crate::backup_worker::BackupScanOutcome> {
        self.backup.last_scan()
    }

    /// Operator decommission (`Seat::decommission`): frees the live
    /// capacity slot while the historical port block remains allocated
    /// forever.
    pub async fn decommission_seat(&self, seat_id: &SeatId) -> anyhow::Result<bool> {
        let seat = self
            .seat_by_id(seat_id)
            .ok_or_else(|| anyhow!("unknown seat"))?;
        let decommissioned = seat.decommission().await?;
        if decommissioned {
            // Hint a republish so the document carries the tombstone promptly:
            // a restore that read the previous version would bring this
            // guardian back into a federation the operator deliberately left.
            // Nothing waits on it — the decommission is already durable
            // locally, and an unreachable relay must not delay an operator
            // retiring a seat.
            self.backup.mark();
            self.advertisement_changed.notify_one();
        }
        Ok(decommissioned)
    }

    /// Operator listing of payment federations with wallet health and
    /// drain state: every accepted member of the common set plus wallet-only
    /// leftovers of removed members. Wallet query failures remain explicit and
    /// prevent a drained conclusion without failing the whole listing.
    pub async fn payment_federation_statuses(&self) -> Vec<PaymentFederationStatus> {
        let accepted = self.operator_settings().await.payment_federations;
        let mut listed: Vec<FederationId> = accepted.clone();
        for retained in self.wallet.retained_federation_ids().await {
            if !listed.contains(&retained) {
                listed.push(retained);
            }
        }
        let mut statuses = Vec::new();
        for federation_id in listed {
            let receivable = self.wallet.receivable(&federation_id).await;
            let wallet = self.payouts.payment_drain_status(&federation_id).await;
            statuses.push(PaymentFederationStatus {
                accepted: accepted.contains(&federation_id),
                federation_id,
                receivable,
                wallet,
            });
        }
        statuses
    }

    pub async fn payout_destination(&self) -> anyhow::Result<Option<String>> {
        Ok(self.db.payout_destination().await?)
    }

    pub async fn set_payout_destination(&self, destination: Option<&str>) -> anyhow::Result<()> {
        if let Some(destination) = destination {
            anyhow::ensure!(
                !destination.trim().is_empty(),
                "payout destination is empty"
            );
            anyhow::ensure!(destination.len() <= 1024, "payout destination is too long");
        }
        self.db.set_payout_destination(destination).await?;
        Ok(())
    }

    /// What payers have remitted into this seat's guardian-fee account, and
    /// what is withdrawable now.
    pub async fn guardian_fee_status(
        &self,
        seat_id: &SeatId,
    ) -> anyhow::Result<FederationFeeStatus> {
        let invite_code = self.seat_federation(seat_id).await?;
        self.guardian_fee_vault()?
            .status(&invite_code, seat_id, &self.guardian_fee_key(seat_id))
            .await
    }

    /// The account payers must remit this seat's share to. This is the value
    /// that belongs in the federation's guardian-fee metadata.
    ///
    /// Answerable for a seat that has no federation yet — deliberately, since
    /// the account has to be known before the ceremony that would carry it
    /// ([SPEC-guardian-fee-policy](../../specs/SPEC-guardian-fee-policy.md)).
    pub fn guardian_fee_account(&self, seat_id: &SeatId) -> anyhow::Result<String> {
        let account = self.guardian_fee_key(seat_id).account();
        Ok(serde_json::to_string(&account)?)
    }

    /// The account id this seat's recipient entry must carry.
    pub(crate) fn guardian_fee_account_id(&self, seat_id: &SeatId) -> AccountId {
        self.guardian_fee_key(seat_id).account().id()
    }

    /// The full account descriptor this seat requires in fee metadata.
    pub(crate) fn guardian_fee_account_descriptor(
        &self,
        seat_id: &SeatId,
    ) -> stability_pool_client::common::Account {
        self.guardian_fee_key(seat_id).account()
    }

    /// The wallet's fee vault, or the one error every fee verb gives when
    /// this install has no wallet that can reach a guarded federation.
    fn guardian_fee_vault(&self) -> anyhow::Result<&dyn GuardianFeeVault> {
        self.wallet
            .guardian_fees()
            .ok_or_else(|| anyhow!("guardian-fee collection is unavailable"))
    }

    /// This seat's remittance account key, derived from the mnemonic and the
    /// seat id alone.
    fn guardian_fee_key(&self, seat_id: &SeatId) -> GuardianFeeAccountKey {
        self.identity.derive_guardian_fee_account_key(seat_id)
    }

    /// Recent remittances into this seat's guardian-fee account, newest
    /// first, with each payer's sealed breakdown opened.
    pub async fn guardian_fee_remittances(
        &self,
        seat_id: &SeatId,
        limit: u64,
    ) -> anyhow::Result<Vec<Remittance>> {
        let invite_code = self.seat_federation(seat_id).await?;
        self.guardian_fee_vault()?
            .remittances(
                &invite_code,
                seat_id,
                &self.guardian_fee_key(seat_id),
                limit,
            )
            .await
    }

    /// Everything payers have ever remitted into this seat's guardian-fee
    /// account, whether it is still in the pool or has long since been swept
    /// out.
    ///
    /// The lifetime figure a dashboard needs, read as its own scalar so no
    /// consumer is tempted to total the windowed list from
    /// [`Self::guardian_fee_remittances`].
    pub async fn guardian_fee_total_remitted(
        &self,
        seat_id: &SeatId,
    ) -> anyhow::Result<fedimint_core::Amount> {
        let invite_code = self.seat_federation(seat_id).await?;
        self.guardian_fee_vault()?
            .total_remitted(&invite_code, seat_id, &self.guardian_fee_key(seat_id))
            .await
    }

    /// Read-only destruction-safety projection for one seat's collected-fee wallet.
    pub async fn guardian_fee_drain_status(
        &self,
        seat_id: &SeatId,
    ) -> anyhow::Result<WalletDrainStatusWire> {
        let invite_code = fedi_decentralized_service_fleet_manager::InviteCode(
            self.seat_federation(seat_id).await?.to_string(),
        );
        self.payouts
            .guardian_drain_status(&invite_code, seat_id)
            .await
    }

    /// Move everything remitted so far out of the pool. Locked deposits leave
    /// only at the next cycle turnover, so the result distinguishes what was
    /// claimed from what a later collection will pick up.
    pub async fn guardian_fee_collect(&self, seat_id: &SeatId) -> anyhow::Result<Collected> {
        let invite_code = self.seat_federation(seat_id).await?;
        self.guardian_fee_vault()?
            .collect(&invite_code, seat_id, &self.guardian_fee_key(seat_id))
            .await
    }

    /// What this seat's federation metadata currently says about guardian
    /// fees, and whether this FMan is still a named recipient.
    pub async fn guardian_fee_policy(&self, seat_id: &SeatId) -> anyhow::Result<FeePolicy> {
        let seat = self
            .seat_by_id(seat_id)
            .ok_or_else(|| anyhow!("unknown seat"))?;
        seat.guardian_fee_policy(self.guardian_fee_account_id(seat_id))
            .await
            .map_err(anyhow::Error::new)
    }

    /// Collected ecash sitting in the guardian-fee client, not yet swept.
    pub async fn guardian_fee_ecash_balance(
        &self,
        seat_id: &SeatId,
    ) -> anyhow::Result<fedimint_core::Amount> {
        let invite_code = self.seat_federation(seat_id).await?;
        self.guardian_fee_vault()?
            .ecash_balance(&invite_code, seat_id, &self.guardian_fee_key(seat_id))
            .await
    }

    /// The federation a seat guards, as a client invite code.
    ///
    /// Collection is an ordinary client operation
    /// ([REQ-guardian-fee-remittance](../../../../specs/REQ-guardian-fee-remittance.md)),
    /// so this is the only thing guardian-fee work takes from the seat: the
    /// same public invite code any member of the federation holds.
    async fn seat_federation(
        &self,
        seat_id: &SeatId,
    ) -> anyhow::Result<fedimint_core::invite_code::InviteCode> {
        let seat = self
            .seat_by_id(seat_id)
            .ok_or_else(|| anyhow!("unknown seat"))?;
        let report = seat.report().await.map_err(anyhow::Error::new)?;
        let SeatReport::Active {
            phase: crate::seat::SeatPhase::Running { invite_code },
            ..
        } = report
        else {
            anyhow::bail!("seat has no federation yet: guardian fees start after DKG completes");
        };
        invite_code
            .0
            .parse()
            .map_err(|err| anyhow!("seat reported an unparsable invite code: {err}"))
    }

    fn seat_by_id(&self, seat_id: &SeatId) -> Option<Arc<Seat>> {
        self.seats
            .read()
            .expect("seat registry lock is never poisoned")
            .get(seat_id)
            .cloned()
    }

    /// The only crate-visible seat-selection path: resolve the verified
    /// request's typed seat id and check the request's signer owns that
    /// seat's immutable durable row. Missing seats and wrong owners are
    /// intentionally indistinguishable, and both exit before any other
    /// seat state (lifecycle, policy, unsupported-verb) is consulted.
    /// `seat_by_id` stays module-private, so FI-facing code cannot select
    /// a seat except through this comparison.
    pub(crate) fn authorize<T: SeatScopedFiRequest>(
        &self,
        request: &VerifiedFiRequest<T>,
    ) -> Result<Arc<Seat>, SeatVerbError> {
        let seat = self
            .seat_by_id(request.seat_id())
            .ok_or(SeatVerbError::UnknownSeat)?;
        if seat.facts().fi_id != *request.signer() {
            return Err(SeatVerbError::UnknownSeat);
        }
        Ok(seat)
    }
}

/// The public key this install signs peer attestations with — the same
/// service Nostr identity [`crate::service`] signs `GetPeerAttestation`
/// responses under. A seat-binding directory this fleet's guardians vote for
/// must bind each of their seats to exactly this key.
fn attestation_pubkey(identity: &RootMnemonic) -> Pubkey {
    Pubkey(
        identity
            .derive_service_nostr_keys()
            .public_key()
            .to_string(),
    )
}

#[cfg(test)]
#[path = "../tests/fleet.rs"]
mod tests;
