//! Consumer-neutral Federation Ambassador state engine.

#![allow(async_fn_in_trait)]

mod backup;
mod backup_worker;
mod db;
mod decommission;
mod discovery;
mod error;
mod formation;
mod guardian_fee_ppm;
mod liquidity;
mod maintenance;
mod meta_write;
mod mg;
mod ports;
mod purchase;
mod run;
mod selection;
mod setup_payment_federations;
mod state;
mod unavailable;

use std::sync::Arc;

use fedi_decentralized_manifold_environment::ManifoldEnvironmentProfile;
use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedimint_core::db::Database;
use fedimint_core::task::TaskGroup;
use nostr_sdk::PublicKey;
use stability_pool_common::Account;
use tokio::sync::{Mutex, watch};

use crate::state::FaStatus;

pub use decommission::DecommissionOutcome;
pub use discovery::{
    AdvertisementRejection, EligibleMgCandidate, MG_ADVERTISEMENT_MAX_AGE,
    MG_ADVERTISEMENT_MAX_HOLDER_AUTHORIZATIONS, MG_DISCOVERY_TIMEOUT, MgCandidateRequirements,
    MgDiscovery, MgDiscoveryOptions, MgRegistryQuery, RejectedAdvertisement,
};
pub use error::{
    AbandonUnavailableReason, Capability, FaError, FaErrorCode, FaResult, MgCause, MgFailure,
    MgOperation, SelectionReauthorizationReason,
};
pub use fedi_decentralized_nostr::mg::{ApiEndpoint, Availability};
pub use fedi_decentralized_service_liquidity_manager::{
    AllocationItemStatus, LiquidityAmountBounds, Pubkey, Sats, Sha256Digest, SourceType,
};
pub use fedi_decentralized_service_manifold_guardian::{
    DkgCompletionCallback, FaId, FaSignature, FederationId, FederationMetadataIconUrl,
    FederationMetadataName, FederationMetadataTermsUrl, FederationMetadataUpdate,
    FederationMetadataWelcomeMessage, FederationName, FederationSize, FedimintdDkgVersion,
    FedimintdVersion, FedimintdVersionCore, GatewayApiUrl, GuardianCode,
    InvalidFederationMetadataValue, InvalidGatewayApiUrl, InviteCode, Locator, MgName, QuoteId,
    SeatId, Timestamp,
};
pub use fedimint_core::config::FederationId as FedimintFederationId;
pub use formation::DkgRestartResult;
pub use guardian_fee_ppm::{GuardianFeePpm, InvalidGuardianFeePpm};
pub use liquidity::{
    AdmittedLiquidityProvider, FA_LIQUIDITY_DISCOVERY_TIMEOUT, FA_LIQUIDITY_MAX_ADVERTISEMENT_AGE,
    FA_LIQUIDITY_MAX_HOLDER_AUTHORIZATIONS, FA_LIQUIDITY_OPERATION_PAGE_MAX,
    FA_LIQUIDITY_REQUEST_VALIDITY, FA_LIQUIDITY_RPC_TIMEOUT, FA_LIQUIDITY_TRUST_MATERIAL_VALIDITY,
    LiquidityDiscovery, LiquidityOperationId, LiquidityOperationPage, LiquidityOperationPhase,
    LiquidityOperationSnapshot, LiquidityProviderRejection, LiquidityRequestIntent,
};
#[cfg(test)]
pub(crate) use maintenance::first_three_maintenance_retry_delays;
pub use ports::{
    FaFeeAccountError, FaFeeAccountProvider, FaIdentity, FaPaymentError, FaPayments,
    FederationConsensusError, FederationConsensusReader, FederationConsensusSnapshot,
    LiquidityProviderConnector, LiquidityProviderConnectorError, ManifoldGuardianCallError,
    ManifoldGuardianConnector, ManifoldGuardianConnectorError, PreparedSeatPayment,
    SeatPaymentRecovery,
};
pub use selection::{
    MG_SELECTION_PREVIEW_VALIDITY, MG_SELECTION_PROBE_TIMEOUT, MgSelectionPreview,
    MgSelectionQuery, MgSelectionRequest, SeatProvenance, SelectedMgSeat, VerifiedBadgeFacts,
    VerifiedCandidate,
};
pub use setup_payment_federations::AdmittedSetupPaymentFederation;
/// Exact SPv2 account type returned by [`FaFeeAccountProvider`].
///
/// This re-export keeps the capability boundary on `fa-client`'s exact
/// dependency identity even when the consumer workspace also contains another
/// revision of `stability-pool-common`.
pub use stability_pool_common::Account as GuardianFeeAccount;
pub use state::{
    CreationSnapshot, CreationStage, FaState, FederationSeat, FederationSnapshot,
    FedimintdVersionRange, FormationFreshness, FormationId, FormationPhase, MAX_FEDERATION_SIZE,
    MAX_FEDERATION_SIZE_EXCLUSIVE, MAX_GUARDIAN_FEE_PPM, MIN_FEDERATION_SIZE, PlanPreference,
    RecoveryCheck, SeatPhase, SeatProgress, validate_federation_name,
};
pub use unavailable::{
    UnavailableFaFeeAccountProvider, UnavailableFederationConsensusReader,
    UnavailableManifoldGuardianConnector, UnavailablePayments, UnavailableRegistry,
};

use crate::db::{FaStore, RecoveryCompletedKey};
use crate::ports::{FaClientPorts, FaIdentityExt as _};

/// Whether this opening is a restored mnemonic whose FA backup needs checking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupRecoveryHint {
    Skip,
    RestoredMnemonic,
}

/// Stateful Federation Ambassador client.
pub struct FaClient<I, P, N, F, C> {
    inner: Arc<FaClientInner<I, P, N, F, C>>,
}

struct FaClientInner<I, P, N, F, C> {
    store: FaStore,
    ports: FaClientPorts<I, P, N, F, C>,
    progress: Arc<run::Progress>,
    control: run::RunControl,
    run_guard: Arc<Mutex<()>>,
    // Connection hints only; consensus is still read and verified each time.
    peer_badge_verifier: PeerBadgeVerifier,
    setup_payment_publisher: Option<PublicKey>,
    guardian_verification_fee_account: Option<Account>,
    /// Set while a restored mnemonic still looks for its backup.
    backup_source: Option<backup_worker::BackupSource>,
    backup_tasks: TaskGroup,
}

impl<I, P, N, F, C> Drop for FaClientInner<I, P, N, F, C> {
    fn drop(&mut self) {
        self.backup_tasks.shutdown();
    }
}

impl<I, P, N, F, C> Clone for FaClient<I, P, N, F, C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
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
    /// Open FA state from a consumer-provided, already-namespaced database.
    ///
    /// The same stable identity must be supplied on every reopen. Durable
    /// state is identity-bound before it is observed.
    ///
    /// Open at most one client for a database. The client serializes all of
    /// its work in memory, and nothing guards against a second client on the
    /// same database. A RocksDB-backed database already refuses a second
    /// open of the same path.
    ///
    /// Opening storage older than schema 11 clears the complete supplied
    /// namespace and starts from Idle.
    ///
    /// The mandatory verifier authenticates every PeerBadge the selection
    /// walk examines through [`Self::preview_mg_selection`]. The resulting
    /// preview is the approval [`Self::buy_seats`] takes. The fee-account
    /// provider and Guardian Verification Fee account are explicit capabilities
    /// for the formation fee arrangement.
    #[allow(
        clippy::too_many_arguments,
        reason = "security-critical FA capability ports stay explicit at the public construction boundary"
    )]
    pub async fn open(
        database: Database,
        identity: I,
        payments: P,
        registry: N,
        mg_connector: F,
        peer_badge_verifier: PeerBadgeVerifier,
        consensus_reader: C,
        fa_fee_account_provider: impl FaFeeAccountProvider,
    ) -> FaResult<Self> {
        Self::open_inner(
            database,
            FaClientPorts {
                identity,
                payments,
                registry,
                mg_connector,
                consensus_reader,
                fa_fee_account_provider: Arc::new(fa_fee_account_provider),
            },
            peer_badge_verifier,
            None,
            None,
            None,
        )
        .await
    }

    /// Open FA state with a deployment-pinned setup-payment publisher.
    ///
    /// Paid quote selection authenticates kind-37707 publications against this
    /// key. Free selected formation through
    /// [`Self::buy_seats`] without a payer does not require setup-payment policy.
    ///
    /// The mandatory verifier authenticates every PeerBadge the selection
    /// walk examines through [`Self::preview_mg_selection`]. The resulting
    /// preview is the approval [`Self::buy_seats`] takes. The fee-account
    /// provider is an explicit local capability even though this constructor
    /// has no Guardian Verification Fee account and therefore cannot arrange
    /// fees.
    #[allow(
        clippy::too_many_arguments,
        reason = "security-critical FA capability ports stay explicit at the public construction boundary"
    )]
    pub async fn open_with_setup_payment_publisher(
        database: Database,
        identity: I,
        payments: P,
        registry: N,
        mg_connector: F,
        peer_badge_verifier: PeerBadgeVerifier,
        consensus_reader: C,
        fa_fee_account_provider: impl FaFeeAccountProvider,
        setup_payment_publisher: PublicKey,
        guardian_verification_fee_account: Option<Account>,
    ) -> FaResult<Self> {
        Self::open_inner(
            database,
            FaClientPorts {
                identity,
                payments,
                registry,
                mg_connector,
                consensus_reader,
                fa_fee_account_provider: Arc::new(fa_fee_account_provider),
            },
            peer_badge_verifier,
            Some(setup_payment_publisher),
            guardian_verification_fee_account,
            None,
        )
        .await
    }

    /// Open FA state from one canonical Manifold deployment profile.
    ///
    /// This is the production integration boundary: the same profile supplies
    /// the setup-payment publisher and the deployment-owned Guardian
    /// Verification Fee account. The separate fee-account provider resolves
    /// the formed federation consumer's own local account. That lookup may be
    /// absent until joined state exists; the dependent operation then fails
    /// closed.
    #[allow(clippy::too_many_arguments)]
    pub async fn open_with_manifold_profile(
        database: Database,
        identity: I,
        payments: P,
        registry: N,
        mg_connector: F,
        peer_badge_verifier: PeerBadgeVerifier,
        consensus_reader: C,
        fa_fee_account_provider: impl FaFeeAccountProvider,
        profile: ManifoldEnvironmentProfile,
    ) -> FaResult<Self> {
        Self::open_with_manifold_profile_and_recovery(
            database,
            identity,
            payments,
            registry,
            mg_connector,
            peer_badge_verifier,
            consensus_reader,
            fa_fee_account_provider,
            profile,
            BackupRecoveryHint::Skip,
        )
        .await
    }

    /// Open with explicit restored-mnemonic provenance.
    ///
    /// A restored mnemonic with no FA state starts in
    /// [`FaState::Restoring`] until the run loop finds its backup, or a relay
    /// quorum shows there is none.
    #[allow(clippy::too_many_arguments)]
    pub async fn open_with_manifold_profile_and_recovery(
        database: Database,
        identity: I,
        payments: P,
        registry: N,
        mg_connector: F,
        peer_badge_verifier: PeerBadgeVerifier,
        consensus_reader: C,
        fa_fee_account_provider: impl FaFeeAccountProvider,
        profile: ManifoldEnvironmentProfile,
        recovery_hint: BackupRecoveryHint,
    ) -> FaResult<Self> {
        let setup_payment_publisher = profile.setup_payment_publisher().copied();
        let guardian_verification_fee_account =
            profile.guardian_verification_fee_account().cloned();
        let relays = profile.nostr_relays().as_urls().to_vec();
        let backup_source = (recovery_hint == BackupRecoveryHint::RestoredMnemonic).then(|| {
            backup_worker::BackupSource {
                relays: relays.clone(),
                required_reads: profile.fa_backup_empty_read_quorum(),
                completed: RecoveryCompletedKey::new(profile.environment()),
            }
        });
        let client = Self::open_inner(
            database,
            FaClientPorts {
                identity,
                payments,
                registry,
                mg_connector,
                consensus_reader,
                fa_fee_account_provider: Arc::new(fa_fee_account_provider),
            },
            peer_badge_verifier,
            setup_payment_publisher,
            guardian_verification_fee_account,
            backup_source,
        )
        .await?;
        backup_worker::spawn_workers(
            &client.inner.backup_tasks,
            client.inner.store.clone(),
            client.inner.ports.identity.scoped_root(),
            relays,
        );
        Ok(client)
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_inner(
        database: Database,
        ports: FaClientPorts<I, P, N, F, C>,
        peer_badge_verifier: PeerBadgeVerifier,
        setup_payment_publisher: Option<PublicKey>,
        guardian_verification_fee_account: Option<Account>,
        backup_source: Option<backup_worker::BackupSource>,
    ) -> FaResult<Self> {
        let store = FaStore::new(database);
        let fa_id = ports.identity.public_key().map_err(FaError::Identity)?;
        store.migrate_storage().await?;
        let mut status = store.load_status(fa_id).await?;
        // Only an empty FA looks for its backup, and only once.
        let backup_source = match backup_source {
            Some(source)
                if status == FaStatus::default()
                    && !store.recovery_completed(&source.completed).await =>
            {
                status.recovery = Some(RecoveryCheck { last_error: None });
                Some(source)
            }
            _ => None,
        };
        let progress = Arc::new(run::Progress::new(status));
        let backup_tasks = TaskGroup::new();
        Ok(Self {
            inner: Arc::new(FaClientInner {
                store,
                ports,
                progress,
                control: run::RunControl::default(),
                run_guard: Arc::new(Mutex::new(())),
                peer_badge_verifier,
                setup_payment_publisher,
                guardian_verification_fee_account,
                backup_source,
                backup_tasks,
            }),
        })
    }

    /// Subscribe to the FA state.
    #[must_use]
    pub fn observe(&self) -> watch::Receiver<FaState> {
        self.inner.progress.subscribe()
    }

    /// Return the FA state.
    #[must_use]
    pub fn state(&self) -> FaState {
        self.inner.progress.state()
    }

    /// Return the internal parts of the FA state.
    pub(crate) fn status(&self) -> FaStatus {
        self.inner.progress.status()
    }
}

#[cfg(test)]
mod tests;
