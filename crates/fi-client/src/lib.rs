//! Consumer-neutral Federation Initiator state engine.

#![allow(async_fn_in_trait)]

mod db;
mod discovery;
mod error;
mod formation;
mod guardian_fee_ppm;
mod liquidity;
mod maintenance;
mod ports;
mod selection;
mod setup_payment_federations;
mod state;
mod unavailable;

use std::sync::Arc;

use fedi_decentralized_manifold_environment::ManifoldEnvironmentProfile;
use fedi_decentralized_nostr_clients::FiNostrClient;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedimint_core::db::Database;
use nostr_sdk::PublicKey;
use stability_pool_common::Account;
use tokio::sync::{Mutex, watch};

pub use discovery::{
    AdvertisementRejection, EligibleFmanCandidate, FMAN_ADVERTISEMENT_MAX_AGE,
    FMAN_ADVERTISEMENT_MAX_HOLDER_AUTHORIZATIONS, FMAN_DISCOVERY_TIMEOUT,
    FmanCandidateRequirements, FmanDiscovery, FmanDiscoveryOptions, FmanRegistryQuery,
    InsecureUntrustedPinnedFman, InsecureUntrustedPinnedFmanDiscovery, RejectedAdvertisement,
};
pub use error::{
    AbandonUnavailableReason, Capability, FiError, FiErrorCode, FiResult,
    SelectionReauthorizationReason,
};
pub use fedi_decentralized_nostr::fman::{ApiEndpoint, Availability};
pub use fedi_decentralized_service_fleet_manager::{
    DkgCompletionCallback, FederationId, FederationMetadataIconUrl, FederationMetadataName,
    FederationMetadataUpdate, FederationMetadataWelcomeMessage, FederationName, FederationSize,
    FedimintdVersion, FiId, FiSignature, FmanName, GatewayApiUrl, GuardianCode,
    InvalidFederationMetadataValue, InvalidGatewayApiUrl, InviteCode, Locator, QuoteId, SeatId,
    Timestamp,
};
pub use fedi_decentralized_service_liquidity_manager::{
    AllocationItemStatus, LiquidityAmountBounds, Pubkey, Sats, Sha256Digest, SourceType,
};
pub use fedimint_core::config::FederationId as FedimintFederationId;
pub use formation::{
    FormationRunOptions, FormationRunOptionsConfig, FormationTimingField,
    InvalidFormationRunOptions,
};
pub use guardian_fee_ppm::{GuardianFeePpm, InvalidGuardianFeePpm};
pub use liquidity::{
    AdmittedLiquidityProvider, FI_LIQUIDITY_DISCOVERY_TIMEOUT, FI_LIQUIDITY_MAX_ADVERTISEMENT_AGE,
    FI_LIQUIDITY_MAX_HOLDER_AUTHORIZATIONS, FI_LIQUIDITY_OPERATION_PAGE_MAX,
    FI_LIQUIDITY_REQUEST_VALIDITY, FI_LIQUIDITY_RPC_TIMEOUT, FI_LIQUIDITY_TRUST_MATERIAL_VALIDITY,
    LiquidityDiscovery, LiquidityOperationId, LiquidityOperationPage, LiquidityOperationPhase,
    LiquidityOperationSnapshot, LiquidityProviderRejection, LiquidityRequestIntent,
};
#[cfg(test)]
pub(crate) use maintenance::first_three_maintenance_retry_delays;
pub use maintenance::{
    InvalidMaintenanceRunOptions, MaintenanceRunOptions, MaintenanceRunOptionsConfig,
    MaintenanceTimingField,
};
pub use ports::{
    ExactPaymentPreflight, ExactSeatPaymentPreflight, FederationConsensusError,
    FederationConsensusReader, FederationConsensusSnapshot, FiFeeAccountError,
    FiFeeAccountProvider, FiIdentity, FiPaymentError, FiPayments, FleetManagerCallError,
    FleetManagerConnector, FleetManagerConnectorError, LiquidityProviderConnector,
    LiquidityProviderConnectorError, PaymentReservationRecovery, PreparedSeatPayment,
    SeatPaymentRecovery, SettledSeatRefund,
};
pub use selection::{
    FMAN_SELECTION_PREVIEW_VALIDITY, FMAN_SELECTION_PROBE_TIMEOUT, FmanReplacementApproval,
    FmanReplacementPreview, FmanSelectionApproval, FmanSelectionPreview, FmanSelectionQuery,
    FmanSelectionRequest, SeatProvenance, SelectedFmanSeat, VerifiedBadgeFacts, VerifiedCandidate,
};
pub use setup_payment_federations::AdmittedSetupPaymentFederation;
/// Exact SPv2 account type returned by [`FiFeeAccountProvider`].
///
/// This re-export keeps the capability boundary on `fi-client`'s exact
/// dependency identity even when the consumer workspace also contains another
/// revision of `stability-pool-common`.
pub use stability_pool_common::Account as GuardianFeeAccount;
pub use state::{
    FiStatus, FormationActionRequired, FormationFreshness, FormationId, FormationIntent,
    FormationPhase, FormationSnapshot, GuardianReplacementId, GuardianReplacementRequirements,
    GuardianReplacementSeat, MAX_FEDERATION_SIZE, MAX_FEDERATION_SIZE_EXCLUSIVE,
    MAX_GUARDIAN_FEE_PPM, MIN_FEDERATION_SIZE, PaymentAuthorizationId, PaymentRequirements,
    PaymentReservationId, PlanPreference, ResolvedFormationIntent, SeatPaymentRequirement,
    SeatPhase, SeatProgress,
};
pub use unavailable::{
    UnavailableFederationConsensusReader, UnavailableFiFeeAccountProvider,
    UnavailableFleetManagerConnector, UnavailablePayments, UnavailableRegistry,
};

use crate::db::FiStore;
use crate::ports::FiClientPorts;

/// Stateful Federation Initiator client.
pub struct FiClient<I, P, N, F, C> {
    inner: Arc<FiClientInner<I, P, N, F, C>>,
}

struct FiClientInner<I, P, N, F, C> {
    store: FiStore,
    ports: FiClientPorts<I, P, N, F, C>,
    progress: watch::Sender<FiStatus>,
    run_guard: Mutex<()>,
    peer_badge_verifier: PeerBadgeVerifier,
    setup_payment_publisher: Option<PublicKey>,
    fedi_guardian_fee_account: Option<Account>,
}

impl<I, P, N, F, C> Clone for FiClient<I, P, N, F, C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
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
    /// Open FI state from a consumer-provided, already-namespaced database.
    ///
    /// The same stable identity must be supplied on every reopen. Durable
    /// state is identity-bound before it is observed. Mutating operations also
    /// acquire a database-wide expiring lease, so separately opened clients
    /// cannot concurrently perform external effects.
    ///
    /// Storage schemas are intentionally fail-closed. Pre-schema-9 records
    /// predate one or more identity, recovery-tombstone, or selected-flow
    /// discriminators and, because this API is pre-launch, must be reset rather
    /// than guessed or adopted by a new key.
    ///
    /// The mandatory verifier authenticates every PeerBadge the selection
    /// walk examines through [`Self::preview_fman_selection`]. The resulting
    /// sealed approval is consumed by [`Self::pay_and_create`]. The fee-account
    /// provider and deployment Fedi account are explicit capabilities for the
    /// formation fee arrangement.
    #[allow(
        clippy::too_many_arguments,
        reason = "security-critical FI capability ports stay explicit at the public construction boundary"
    )]
    pub async fn open(
        database: Database,
        identity: I,
        payments: P,
        registry: N,
        fman_connector: F,
        peer_badge_verifier: PeerBadgeVerifier,
        consensus_reader: C,
        fi_fee_account_provider: impl FiFeeAccountProvider,
    ) -> FiResult<Self> {
        Self::open_inner(
            database,
            FiClientPorts {
                identity,
                payments,
                registry,
                fman_connector,
                consensus_reader,
                fi_fee_account_provider: Arc::new(fi_fee_account_provider),
            },
            peer_badge_verifier,
            None,
            None,
        )
        .await
    }

    /// Open FI state with a deployment-pinned setup-payment publisher.
    ///
    /// Paid quote selection authenticates kind-37707 publications against this
    /// key. Free selected formation through
    /// [`Self::create_without_payer`] does not require setup-payment policy.
    ///
    /// The mandatory verifier authenticates every PeerBadge the selection
    /// walk examines through [`Self::preview_fman_selection`]. The resulting
    /// sealed approval is consumed by [`Self::pay_and_create`]. The fee-account
    /// provider is an explicit local capability even though this constructor
    /// has no deployment Fedi account and therefore cannot arrange fees.
    #[allow(
        clippy::too_many_arguments,
        reason = "security-critical FI capability ports stay explicit at the public construction boundary"
    )]
    pub async fn open_with_setup_payment_publisher(
        database: Database,
        identity: I,
        payments: P,
        registry: N,
        fman_connector: F,
        peer_badge_verifier: PeerBadgeVerifier,
        consensus_reader: C,
        fi_fee_account_provider: impl FiFeeAccountProvider,
        setup_payment_publisher: PublicKey,
        fedi_guardian_fee_account: Option<Account>,
    ) -> FiResult<Self> {
        Self::open_inner(
            database,
            FiClientPorts {
                identity,
                payments,
                registry,
                fman_connector,
                consensus_reader,
                fi_fee_account_provider: Arc::new(fi_fee_account_provider),
            },
            peer_badge_verifier,
            Some(setup_payment_publisher),
            fedi_guardian_fee_account,
        )
        .await
    }

    /// Open FI state from one canonical Manifold deployment profile.
    ///
    /// This is the production integration boundary: the same profile supplies
    /// the setup-payment publisher and the deployment-owned Fedi guardian-fee
    /// account. The separate fee-account provider resolves the formed
    /// federation consumer's own local account. In production either profile
    /// value or that local lookup may be absent until configuration and joined
    /// state exist; the dependent operation then fails closed.
    #[allow(clippy::too_many_arguments)]
    pub async fn open_with_manifold_profile(
        database: Database,
        identity: I,
        payments: P,
        registry: N,
        fman_connector: F,
        peer_badge_verifier: PeerBadgeVerifier,
        consensus_reader: C,
        fi_fee_account_provider: impl FiFeeAccountProvider,
        profile: ManifoldEnvironmentProfile,
    ) -> FiResult<Self> {
        let setup_payment_publisher = profile.setup_payment_publisher().copied();
        let fedi_guardian_fee_account = profile.fedi_guardian_fee_account().cloned();
        Self::open_inner(
            database,
            FiClientPorts {
                identity,
                payments,
                registry,
                fman_connector,
                consensus_reader,
                fi_fee_account_provider: Arc::new(fi_fee_account_provider),
            },
            peer_badge_verifier,
            setup_payment_publisher,
            fedi_guardian_fee_account,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_inner(
        database: Database,
        ports: FiClientPorts<I, P, N, F, C>,
        peer_badge_verifier: PeerBadgeVerifier,
        setup_payment_publisher: Option<PublicKey>,
        fedi_guardian_fee_account: Option<Account>,
    ) -> FiResult<Self> {
        let store = FiStore::new(database);
        let fi_id = ports.identity.public_key().map_err(FiError::Identity)?;
        let status = store.load_status(fi_id).await?;
        let (progress, _) = watch::channel(status);
        Ok(Self {
            inner: Arc::new(FiClientInner {
                store,
                ports,
                progress,
                run_guard: Mutex::new(()),
                peer_badge_verifier,
                setup_payment_publisher,
                fedi_guardian_fee_account,
            }),
        })
    }

    /// Subscribe to the latest FI state.
    #[must_use]
    pub fn observe(&self) -> watch::Receiver<FiStatus> {
        self.inner.progress.subscribe()
    }

    /// Return the latest FI state.
    #[must_use]
    pub fn status(&self) -> FiStatus {
        self.inner.progress.borrow().clone()
    }

    /// Legacy registry-backed creation entry point.
    ///
    /// MVP automatic selection uses [`Self::preview_fman_selection`] followed
    /// by [`Self::pay_and_create`]. This older unapproved registry-create shape
    /// deliberately remains unavailable because it has no verified preview,
    /// payer, spending cap, or sealed selection approval.
    pub async fn create(&self, _intent: FormationIntent) -> FiResult<()> {
        let _run = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        let _fi_pubkey = self
            .inner
            .ports
            .identity
            .public_key()
            .map_err(FiError::Identity)?;
        let _ = (
            &self.inner.ports.payments,
            &self.inner.ports.registry,
            &self.inner.ports.fman_connector,
        );
        self.publish_error(FiErrorCode::CapabilityUnavailable);
        Err(FiError::CapabilityUnavailable(Capability::Registry))
    }

    /// Report that the legacy unapproved registry-create shape is unavailable
    /// without accessing any consumer capability. Use the preview and
    /// Pay-and-create APIs for MVP creation.
    pub fn preflight_create(_intent: &FormationIntent) -> FiResult<()> {
        Err(FiError::CapabilityUnavailable(Capability::Registry))
    }

    /// Continue the active formation from its durable recovery state.
    ///
    /// `Ok(())` does not necessarily mean that formation completed. In
    /// particular, a successful call can stop at
    /// [`FormationPhase::AwaitingPaymentReadiness`]. Read the aggregate
    /// [`FormationActionRequired`] from [`Self::status`] or [`Self::observe`].
    /// An [`FormationActionRequired::AuthorizePayments`] action requires the
    /// displayed [`PaymentAuthorizationId`]. A
    /// [`FormationActionRequired::ReplaceGuardians`] action requires a fresh
    /// verified excluding-set preview and sealed replacement approval through
    /// [`Self::apply_fman_replacements`].
    ///
    /// Resuming never grants payment authorization. It first recovers operations
    /// for the exact stored authorized quotes. It can carry that authorization to
    /// refreshed quote IDs only when every authorized commercial term is
    /// unchanged; changed terms expose a new payment action, which needs a new
    /// authorization.
    ///
    /// A stored [`FormationPhase::Formed`] state is also not treated as proof of
    /// current remote state: resume reconnects to the Fleet Managers, reconciles
    /// their status and common invite, and rejects a changed federation identity.
    /// Inspect [`FormationFreshness`] through status observation when presenting
    /// persisted state before that reconciliation completes.
    ///
    /// That reconciliation is not read-only. Before republishing `Formed`,
    /// resume passes through [`FormationPhase::PublishingSeatBindings`]: it
    /// submits the FMan seat-binding directory to every seat and reads the
    /// value back from consensus, so a formation whose publish was interrupted
    /// completes here rather than at a second DKG. The submitted bytes are the
    /// ones persisted on the first attempt, so repeated resumes replay one
    /// identical value and consensus can converge on it. Resume reaches
    /// `Formed` only once the readback equals what was written.
    ///
    /// The returned [`std::future::Future`] owns no background task. Dropping it
    /// cancels local work, but completed durable checkpoints remain recoverable:
    /// reopen with the same database, identity, and wallet through
    /// [`Self::open`] (or [`Self::open_with_setup_payment_publisher`]), then call
    /// [`Self::resume`] again.
    pub async fn resume(&self) -> FiResult<()> {
        let _run = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        let options = FormationRunOptions::default();
        options.validate_for_start(&self.inner.store)?;
        let fi_id = self
            .inner
            .ports
            .identity
            .public_key()
            .map_err(FiError::Identity)?;
        let (deadline, lease) = formation::start_driver_run(&self.inner.store, options).await?;
        let result = async {
            let recovery = self.inner.store.load_recovery(fi_id).await?;
            match recovery {
                db::FiRecovery::Idle => Err(FiError::NoActiveFormation),
                db::FiRecovery::Formation(recovery) => {
                    self.inner
                        .progress
                        .send_replace(FiStatus::Formation(recovery.snapshot.clone()));
                    self.resume_pinned(*recovery, options, deadline, &lease)
                        .await
                }
            }
        }
        .await;
        formation::finish_driver_run(result, self.inner.store.release_driver_lease(lease).await)
    }

    fn publish_error(&self, error: FiErrorCode) {
        if let FiStatus::Formation(mut snapshot) = self.status() {
            snapshot.last_error = Some(error);
            self.inner
                .progress
                .send_replace(FiStatus::Formation(snapshot));
        }
    }
}

#[cfg(test)]
mod tests;
