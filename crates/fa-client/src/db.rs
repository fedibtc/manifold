//! Fedimint database representation for FA recovery facts and checkpoints.

#[cfg(test)]
pub(crate) mod test_support;

use bitcoin_hashes::sha256;
use fedimint_core::db::{
    AutocommitError, Database, DatabaseKeyPrefix, DatabaseTransaction, DatabaseValue,
    DecodingError, IDatabaseTransactionOpsCore as _, IDatabaseTransactionOpsCoreTyped as _,
};
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::registry::ModuleDecoderRegistry;
use fedimint_core::{impl_db_lookup, impl_db_record};
use futures::StreamExt as _;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::Ordering;

use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifierProvenance;
use fedi_decentralized_service_manifold_guardian::{
    DkgCompletionCallback, FaId, FederationId, FederationName, FederationSize, FedimintdVersion,
    FormationSeatBinding, GetQuoteResponse, GuardianCode, GuardianFeeAccount, InviteCode, Locator,
    Plan, QuoteId, SeatId, SignedResponse, Timestamp,
};
use nostr_sdk::{Event, PublicKey};
use stability_pool_common::Account;

use crate::{
    FaError, FaResult, FormationFreshness, FormationId, FormationPhase, FormationSnapshot,
    PurchasePhase, ResolvedSeatPurchase, SeatPhase, SeatProgress, SeatPurchaseSnapshot,
    state::FaStatus,
};

// Persisted-format changes are governed by GATE-production-compatibility
// (../../../specs/GATE-production-compatibility.md).
const STORAGE_SCHEMA_VERSION: u16 = 12;
const FORMATION_TX_MAX_ATTEMPTS: usize = 10;

#[repr(u8)]
enum FaDbPrefix {
    Purchase = 0x00,
    PurchaseSeat = 0x01,
    /// Legacy: the database-wide driver lease. Only migration removes it.
    DriverLease = 0x02,
    SetupPaymentFederations = 0x03,
    LiquidityOperation = 0x04,
    BackupGeneration = 0x05,
    BackupRelayConfirmation = 0x06,
    /// Legacy: an imported backup that waited for reconciliation. Only
    /// migration reads it.
    RestoredBackup = 0x07,
    RecoveryCompleted = 0x08,
    Federation = 0x09,
    PoolSeat = 0x0a,
    Creation = 0x0b,
    GuardianCode = 0x0c,
}

#[derive(Debug, Decodable, Encodable)]
struct BackupGenerationKey;
impl_db_record!(
    key = BackupGenerationKey,
    value = BackupGenerationState,
    db_prefix = FaDbPrefix::BackupGeneration
);

#[derive(Debug, Decodable, Encodable)]
struct BackupRelayConfirmationKey {
    relay: String,
}
impl_db_record!(
    key = BackupRelayConfirmationKey,
    value = BackupRelayConfirmation,
    db_prefix = FaDbPrefix::BackupRelayConfirmation
);

#[derive(Debug, Decodable, Encodable)]
pub(crate) struct RecoveryCompletedKey {
    environment: u8,
}

impl RecoveryCompletedKey {
    pub(crate) fn new(environment: ManifoldEnvironment) -> Self {
        let environment = match environment {
            ManifoldEnvironment::Development => 0,
            ManifoldEnvironment::Staging => 1,
            ManifoldEnvironment::Production => 2,
        };
        Self { environment }
    }
}

impl_db_record!(
    key = RecoveryCompletedKey,
    value = (),
    db_prefix = FaDbPrefix::RecoveryCompleted
);

#[derive(Debug, Decodable, Encodable)]
struct RecoveryCompletedKeyPrefix;
impl_db_lookup!(
    key = RecoveryCompletedKey,
    query_prefix = RecoveryCompletedKeyPrefix
);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct BackupGenerationState {
    generation: u64,
    semantic_hash: sha256::Hash,
    document_hash: sha256::Hash,
    nostr_created_at: u64,
}

pub(crate) const BACKUP_REFRESH_INTERVAL_SECS: u64 = 15 * 24 * 60 * 60;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct BackupRelayConfirmation {
    pub(crate) document_hash: sha256::Hash,
    pub(crate) generation: u64,
    pub(crate) event_id: String,
    pub(crate) confirmed_at_secs: u64,
}

/// The formed federation this FA governs, whether its DKG ran here or an
/// imported backup was reconciled against consensus. Every post-formation
/// workflow reads this record; formation rows only describe how a local DKG
/// got here.
#[derive(Debug, Decodable, Encodable)]
struct FederationKey;
impl_db_record!(
    key = FederationKey,
    value = StoredFederation,
    db_prefix = FaDbPrefix::Federation
);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredFederation {
    #[serde(rename = "fi_id")]
    fa_id: FaId,
    formation_id: FormationId,
    federation_invite: InviteCode,
    federation_name: Option<crate::FederationName>,
    seats: Vec<StoredFederationSeat>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredFederationSeat {
    #[serde(rename = "fman_identity")]
    mg_identity: PublicKey,
    seat_id: SeatId,
    locator: Locator,
}

impl StoredFederationSeat {
    fn into_backup_seat(self) -> FaResult<crate::backup::FaBackupSeat> {
        Ok(crate::backup::FaBackupSeat {
            mg_identity: self.mg_identity,
            seat_id: self.seat_id,
            locator: self.locator,
        })
    }
}

impl From<crate::backup::FaBackupSeat> for StoredFederationSeat {
    fn from(seat: crate::backup::FaBackupSeat) -> Self {
        Self {
            mg_identity: seat.mg_identity,
            seat_id: seat.seat_id,
            locator: seat.locator,
        }
    }
}

#[derive(Debug, Decodable, Encodable)]
struct PurchaseKey;

impl_db_record!(
    key = PurchaseKey,
    value = StoredPurchase,
    db_prefix = FaDbPrefix::Purchase,
);

#[derive(Debug, Decodable, Encodable)]
struct LiquidityOperationKey {
    operation_id: String,
}

#[derive(Debug, Decodable, Encodable)]
struct LiquidityOperationKeyPrefix;

impl_db_record!(
    key = LiquidityOperationKey,
    value = crate::liquidity::StoredLiquidityOperation,
    db_prefix = FaDbPrefix::LiquidityOperation,
);

impl_db_lookup!(
    key = LiquidityOperationKey,
    query_prefix = LiquidityOperationKeyPrefix
);

#[derive(Debug, Decodable, Encodable)]
struct SetupPaymentFederationsKey;

impl_db_record!(
    key = SetupPaymentFederationsKey,
    value = StoredSetupPaymentFederationsEvent,
    db_prefix = FaDbPrefix::SetupPaymentFederations,
);

#[derive(Debug, Decodable, Encodable)]
struct PurchaseSeatKey {
    formation_id: String,
    index: u16,
}

#[derive(Debug, Decodable, Encodable)]
struct PurchaseSeatKeyPrefix;

impl_db_record!(
    key = PurchaseSeatKey,
    value = StoredPurchaseSeat,
    db_prefix = FaDbPrefix::PurchaseSeat,
);

impl_db_lookup!(key = PurchaseSeatKey, query_prefix = PurchaseSeatKeyPrefix);

/// One bought seat of the current creation, keyed by its seat id. It stays
/// here until its federation forms.
#[derive(Debug, Decodable, Encodable)]
struct PoolSeatKey {
    seat_id: String,
}

#[derive(Debug, Decodable, Encodable)]
struct PoolSeatKeyPrefix;

impl_db_record!(
    key = PoolSeatKey,
    value = StoredPoolSeat,
    db_prefix = FaDbPrefix::PoolSeat,
);

impl_db_lookup!(key = PoolSeatKey, query_prefix = PoolSeatKeyPrefix);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredPoolSeat {
    #[serde(rename = "fman_id")]
    mg_id: PublicKey,
    /// Verified badge holder of the MG, when the admission recorded one.
    holder: Option<PublicKey>,
    locator: Locator,
    seat_id: SeatId,
    /// MG-signed account from the seat's acceptance.
    guardian_fee_account: Account,
    /// Exact Fedimint build and federation size from the seat's quote.
    fedimintd_version: FedimintdVersion,
    federation_size: FederationSize,
}

impl StoredPoolSeat {
    fn public(&self) -> crate::PoolSeat {
        crate::PoolSeat {
            seat_id: self.seat_id.clone(),
            mg_id: self.mg_id,
            locator: self.locator.clone(),
            fedimintd_version: self.fedimintd_version.clone(),
            federation_size: self.federation_size,
        }
    }
}

/// The federation the user asked to create. It exists from the first
/// purchase until the federation forms, and it keeps the inputs every later
/// purchase and the formation need. The formation runs once the pool holds
/// every seat, and its progress lives here.
#[derive(Debug, Decodable, Encodable)]
struct CreationKey;

impl_db_record!(
    key = CreationKey,
    value = StoredCreation,
    db_prefix = FaDbPrefix::Creation,
);

/// The guardian code an MG returned for one pool seat, kept until the
/// federation forms. MGs refuse the code request once DKG starts, and a DKG
/// restart needs every code again.
// TODO: remove once MGs return the code in every state.
#[derive(Debug, Decodable, Encodable)]
struct GuardianCodeKey {
    seat_id: String,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredGuardianCode(GuardianCode);

impl_db_record!(
    key = GuardianCodeKey,
    value = StoredGuardianCode,
    db_prefix = FaDbPrefix::GuardianCode,
);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredCreation {
    fa_id: FaId,
    federation_name: FederationName,
    federation_size: FederationSize,
    fedimintd_versions: crate::FedimintdVersionRange,
    plan: crate::PlanPreference,
    /// Installation-scoped bearer callback reused verbatim for every MG,
    /// dropped with the DKG checkpoint. Its semantic type redacts `Debug`;
    /// database copies remain sensitive.
    dkg_completion_callback: Option<DkgCompletionCallback>,
    /// Stable local handle of the federation: the id of the first purchase.
    formation_id: FormationId,
    /// Federation invite, saved when DKG completes.
    invite_code: Option<InviteCode>,
    /// Exact formation metadata target, persisted before its first vote.
    formation_meta_target: Option<FormationMetaTarget>,
    /// The seat whose guardian code carries the federation name. A schema 11
    /// formation keeps the seat it named. Otherwise it is the first pool seat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    named_seat: Option<SeatId>,
    /// Why the last purchase ended without some of its seats. A new
    /// purchase clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reauthorization: Option<crate::SelectionReauthorizationReason>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredPurchase {
    schema_version: u16,
    /// `None` exists only so schema-v4 JSON can be decoded and rejected with
    /// an actionable compatibility error instead of a generic decode failure.
    #[serde(default)]
    #[serde(rename = "fi_id")]
    fa_id: Option<FaId>,
    formation_id: FormationId,
    phase: StoredPurchasePhase,
    intent: ResolvedSeatPurchase,
    seat_count: u16,
    /// Explicit consumer-selected ready payer, or `None` for the verified
    /// zero-price deployment-bootstrap path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    payment_federation_id: Option<FederationId>,
}

#[derive(serde::Deserialize)]
struct StoredPurchaseEnvelope {
    schema_version: u16,
}

/// Serializable stable projection of the verifier's canonical provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoredVerifierProvenance {
    Development { profile_revision: u32 },
    Staging { profile_revision: u32 },
    Production { profile_revision: u32 },
    ExplicitTestConfiguration,
}

impl From<PeerBadgeVerifierProvenance> for StoredVerifierProvenance {
    fn from(value: PeerBadgeVerifierProvenance) -> Self {
        match value {
            PeerBadgeVerifierProvenance::ManifoldProfile {
                environment: ManifoldEnvironment::Development,
                profile_revision,
            } => Self::Development { profile_revision },
            PeerBadgeVerifierProvenance::ManifoldProfile {
                environment: ManifoldEnvironment::Staging,
                profile_revision,
            } => Self::Staging { profile_revision },
            PeerBadgeVerifierProvenance::ManifoldProfile {
                environment: ManifoldEnvironment::Production,
                profile_revision,
            } => Self::Production { profile_revision },
            PeerBadgeVerifierProvenance::ExplicitTestConfiguration => {
                Self::ExplicitTestConfiguration
            }
        }
    }
}

/// Durable admission proof for the operator currently assigned to one row.
///
/// `fman_id` is the badge-vouched Nostr author.  The locator's service key is
/// deliberately not identity: it is rotatable commitment/dialing material.
/// The output-boundary transaction consumes every currently live proof before any
/// corresponding wallet future is polled.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct MgAdmission {
    #[serde(rename = "fman_id")]
    mg_id: PublicKey,
    /// Older saved seats did not record the verified badge holder.
    #[serde(default)]
    holder: Option<PublicKey>,
    /// The previewed plan this seat was bought at, so its quote has exactly
    /// the previewed price. Older saved seats did not record it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan: Option<Plan>,
    verifier_provenance: StoredVerifierProvenance,
    valid_until: Timestamp,
    pub(crate) state: AdmissionState,
}

/// Exact irreversible effect authorized by one fresh selected-seat admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AdmissionEffect {
    /// Generation of quote-bound payer-wallet outputs.
    PaidOutput,
    /// Presentation of a zero-price quote to `CreateSeat`.
    FreePresentation,
}

/// Monotone durable state for one selected-seat admission.
///
/// This is deliberately private recovery state rather than a public status
/// projection.  The transition binds the exact quote and effect before that
/// effect can be polled, so later expiry or verifier-profile changes cannot
/// strand exact recovery.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AdmissionState {
    Fresh,
    EffectAuthorized {
        quote_id: QuoteId,
        effect: AdmissionEffect,
    },
}

impl MgAdmission {
    pub(crate) fn fresh_peer_badge(
        mg_id: PublicKey,
        holder: PublicKey,
        plan: Plan,
        verifier_provenance: StoredVerifierProvenance,
        valid_until: Timestamp,
    ) -> Self {
        Self {
            mg_id,
            holder: Some(holder),
            plan: Some(plan),
            verifier_provenance,
            valid_until,
            state: AdmissionState::Fresh,
        }
    }

    pub(crate) fn mg_id(&self) -> PublicKey {
        self.mg_id
    }

    pub(crate) fn holder(&self) -> Option<PublicKey> {
        self.holder
    }

    pub(crate) fn plan(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }

    pub(crate) fn requires_effect_authorization(&self) -> bool {
        self.state == AdmissionState::Fresh
    }

    pub(crate) fn validate_if_fresh(
        &self,
        expected_provenance: StoredVerifierProvenance,
        now: Timestamp,
    ) -> FaResult<()> {
        validate_fresh_admission(self, expected_provenance, now)
    }

    fn authorized_effect(&self) -> Option<(QuoteId, AdmissionEffect)> {
        match self.state {
            AdmissionState::Fresh => None,
            AdmissionState::EffectAuthorized { quote_id, effect } => Some((quote_id, effect)),
        }
    }

    fn require_authorized_effect(
        &self,
        quote_id: QuoteId,
        effect: AdmissionEffect,
    ) -> FaResult<()> {
        if self.authorized_effect() == Some((quote_id, effect)) {
            Ok(())
        } else {
            Err(FaError::Storage(
                "selected-seat effect crossed before exact admission authorization".to_owned(),
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredPurchasePhase {
    Initialized,
    SeatsPartiallyCreated,
    /// Every seat exists; the next step moves them into the pool.
    SeatsCreated,
}

macro_rules! json_database_value {
    ($type:ty) => {
        impl DatabaseValue for $type {
            fn from_bytes(
                data: &[u8],
                _modules: &ModuleDecoderRegistry,
            ) -> Result<Self, DecodingError> {
                serde_json::from_slice(data).map_err(DecodingError::other)
            }

            fn to_bytes(&self) -> Vec<u8> {
                serde_json::to_vec(self)
                    .expect(concat!(stringify!($type), " serialization is infallible"))
            }
        }
    };
}

mod migration;
json_database_value!(StoredPurchase);

/// Durable protocol facts for one selected seat.
///
/// This deliberately excludes observed freshness, payment evidence, raw
/// ecash, and wallet-private refund material. The exact signed quote is safe
/// to retain: it contains only MG-authenticated public terms.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredPurchaseSeat {
    index: u16,
    locator: Locator,
    admission: MgAdmission,
    signed_quote: Option<SignedResponse<GetQuoteResponse>>,
    seat_id: Option<SeatId>,
    #[serde(default)]
    guardian_fee_account: Option<Account>,
}

json_database_value!(StoredPurchaseSeat);
json_database_value!(StoredCreation);
json_database_value!(StoredGuardianCode);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredSetupPaymentFederationsEvent(Event);

json_database_value!(StoredSetupPaymentFederationsEvent);
json_database_value!(crate::liquidity::StoredLiquidityOperation);
json_database_value!(BackupGenerationState);
json_database_value!(BackupRelayConfirmation);
json_database_value!(StoredFederation);
json_database_value!(StoredPoolSeat);

/// One seat's public projection and its private recovery-only quote facts.
pub(crate) struct PurchaseSeatRecovery {
    pub(crate) progress: SeatProgress,
    pub(crate) admission: MgAdmission,
    pub(crate) signed_quote: Option<SignedResponse<GetQuoteResponse>>,
    pub(crate) guardian_fee_account: Option<Account>,
}

pub(crate) struct PurchaseRecovery {
    pub(crate) snapshot: SeatPurchaseSnapshot,
    pub(crate) seats: Vec<PurchaseSeatRecovery>,
    /// Explicit payer, absent for the zero-price bootstrap.
    pub(crate) payment_federation_id: Option<FederationId>,
}

/// One formation seat's public projection and its signed acceptance facts.
pub(crate) struct FormationSeatRecovery {
    pub(crate) progress: SeatProgress,
    pub(crate) guardian_fee_account: Account,
}

pub(crate) struct FormationRecovery {
    pub(crate) snapshot: FormationSnapshot,
    pub(crate) seats: Vec<FormationSeatRecovery>,
    /// Exact formation metadata target saved before publication.
    pub(crate) formation_meta_target: Option<FormationMetaTarget>,
    /// Bearer callback handed to every MG only at `StartDkg`.
    pub(crate) dkg_completion_callback: Option<DkgCompletionCallback>,
}

/// Complete semantic target replayed until formation metadata is confirmed.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FormationMetaTarget {
    /// FA's canonical prediction, used only for consensus readback.
    pub(crate) seat_bindings: String,
    /// Exact structural request payload replayed until readback succeeds.
    pub(crate) binding_entries: Vec<FormationSeatBinding>,
    #[serde(rename = "fi_fee_account")]
    pub(crate) fa_fee_account: GuardianFeeAccount,
    pub(crate) guardian_verification_fee_account: GuardianFeeAccount,
    pub(crate) send_ppm: u64,
    pub(crate) recipients: String,
}

/// One validated row supplied to atomic formation initialization.
pub(crate) struct InitialSeat {
    pub(crate) progress: SeatProgress,
    pub(crate) admission: MgAdmission,
}

impl InitialSeat {
    /// Pair one selected locator with its admission, deriving the public
    /// progress row so its displayed MG identity cannot disagree with the
    /// durable admission.
    pub(crate) fn new(index: usize, locator: Locator, admission: MgAdmission) -> Self {
        Self {
            progress: SeatProgress {
                index: u16::try_from(index).expect("validated formation size fits u16"),
                mg_id: admission.mg_id(),
                locator,
                seat_id: None,
                guardian_code: None,
                phase: SeatPhase::Selected,
                freshness: FormationFreshness::Fresh,
            },
            admission,
        }
    }
}

/// The inputs of a new federation creation.
pub(crate) struct NewCreation {
    pub(crate) federation_name: FederationName,
    pub(crate) federation_size: FederationSize,
    pub(crate) fedimintd_versions: crate::FedimintdVersionRange,
    pub(crate) plan: crate::PlanPreference,
    pub(crate) dkg_completion_callback: Option<DkgCompletionCallback>,
}

/// The stored inputs of the running federation creation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CreationIntent {
    pub(crate) federation_name: FederationName,
    pub(crate) federation_size: FederationSize,
    pub(crate) fedimintd_versions: crate::FedimintdVersionRange,
    pub(crate) plan: crate::PlanPreference,
}

#[cfg(test)]
type SeatCheckpointTestHook =
    Arc<std::sync::Mutex<Option<Arc<dyn Fn(u16) + Send + Sync + 'static>>>>;

#[cfg(test)]
struct FormationTestHook {
    barrier: Arc<tokio::sync::Barrier>,
    remaining: std::sync::atomic::AtomicUsize,
}

#[derive(Clone)]
pub(crate) struct FaStore {
    database: Database,
    /// Wall-clock seconds for admission freshness. Tests inject their own.
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    #[cfg(test)]
    formation_commit_hook: Arc<std::sync::Mutex<Option<Arc<FormationTestHook>>>>,
    #[cfg(test)]
    seat_checkpoint_hook: SeatCheckpointTestHook,
}

impl FaStore {
    pub(crate) fn new(database: Database) -> Self {
        Self {
            database,
            clock: Arc::new(|| fedimint_core::time::duration_since_epoch().as_secs()),
            #[cfg(test)]
            formation_commit_hook: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            seat_checkpoint_hook: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Prepare one eligible backup, allocating its generation and Nostr
    /// timestamp in the same transaction. Unchanged facts reuse both until the
    /// next refresh boundary; changed facts advance both immediately.
    pub(crate) async fn backup_payload(&self) -> FaResult<crate::backup::PreparedFaBackup> {
        let mut dbtx = self.database.begin_transaction().await;
        let federation = dbtx
            .get_value(&FederationKey)
            .await
            .ok_or_else(formed_backup_required)?;
        let federation_invite = federation.federation_invite;
        let seats = federation
            .seats
            .into_iter()
            .map(StoredFederationSeat::into_backup_seat)
            .collect::<FaResult<Vec<_>>>()?;
        let mut entries = dbtx.find_by_prefix(&LiquidityOperationKeyPrefix).await;
        let mut liquidity = None;
        while let Some((_key, operation)) = entries.next().await {
            if !operation.is_terminal_for_backup()?
                && liquidity.replace(operation.commitment).is_some()
            {
                return Err(FaError::Storage(
                    "more than one live FA liquidity request".to_owned(),
                ));
            }
        }
        drop(entries);
        let mut payload = crate::backup::FaBackupPayload {
            schema_version: crate::backup::BACKUP_SCHEMA_VERSION,
            snapshot_generation: 0,
            federation_invite,
            seats,
            liquidity,
        };
        let semantic_hash = crate::backup::semantic_hash(&payload)?;
        let prior = dbtx.get_value(&BackupGenerationKey).await;
        let generation = match prior.as_ref() {
            Some(prior) if prior.semantic_hash == semantic_hash => prior.generation,
            Some(prior) => prior.generation.checked_add(1).ok_or_else(|| {
                FaError::Storage("FA backup snapshot generation exhausted".to_owned())
            })?,
            None => 1,
        };
        payload.snapshot_generation = generation;
        let document_hash = crate::backup::document_hash(&payload)?;
        let now = fedimint_core::time::duration_since_epoch().as_secs();
        let created_at = match prior.as_ref() {
            Some(prior) if prior.semantic_hash == semantic_hash => {
                let elapsed = now.saturating_sub(prior.nostr_created_at);
                prior.nostr_created_at.saturating_add(
                    elapsed / BACKUP_REFRESH_INTERVAL_SECS * BACKUP_REFRESH_INTERVAL_SECS,
                )
            }
            Some(prior) => now.max(prior.nostr_created_at.saturating_add(1)),
            None => now,
        };
        dbtx.insert_entry(
            &BackupGenerationKey,
            &BackupGenerationState {
                generation,
                semantic_hash,
                document_hash,
                nostr_created_at: created_at,
            },
        )
        .await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("persisting FA backup generation failed".to_owned()))?;
        Ok(crate::backup::PreparedFaBackup {
            payload,
            document_hash,
            created_at,
        })
    }

    pub(crate) async fn recovery_completed(&self, key: &RecoveryCompletedKey) -> bool {
        self.database
            .begin_transaction_nc()
            .await
            .get_value(key)
            .await
            .is_some()
    }

    pub(crate) async fn complete_empty_recovery(&self, key: &RecoveryCompletedKey) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        dbtx.insert_entry(key, &()).await;
        dbtx.commit_tx_result().await.map_err(|_| {
            FaError::Storage("committing completed FA backup lookup failed".to_owned())
        })
    }

    /// Import a backup that consensus confirmed as this FA's federation, and
    /// mark the backup lookup done.
    pub(crate) async fn import_backup(
        &self,
        fa_id: FaId,
        payload: crate::backup::FaBackupPayload,
        federation_name: Option<crate::FederationName>,
        completed: &RecoveryCompletedKey,
    ) -> FaResult<()> {
        if let Some(commitment) = &payload.liquidity {
            if commitment.requester_pubkey.0 != fa_id.0.to_string() {
                return Err(FaError::Storage(
                    "authenticated FA backup liquidity belongs to another requester".to_owned(),
                ));
            }
            fedi_decentralized_service_liquidity_manager::request_liquidity_details_hash(
                commitment,
            )
            .map_err(|_| {
                FaError::Storage(
                    "authenticated FA backup has an invalid liquidity commitment".to_owned(),
                )
            })?;
        }
        let formation_id =
            crate::formation::restored_formation_id(fa_id, &payload.federation_invite)?;
        let mut dbtx = self.database.begin_transaction().await;
        if dbtx.get_value(&PurchaseKey).await.is_some()
            || dbtx.get_value(&CreationKey).await.is_some()
            || dbtx.get_value(&FederationKey).await.is_some()
            || dbtx
                .find_by_prefix(&LiquidityOperationKeyPrefix)
                .await
                .next()
                .await
                .is_some()
        {
            return Err(FaError::Storage(
                "FA restore requires an empty database namespace".to_owned(),
            ));
        }
        adopt_backup(
            &mut dbtx.to_ref_nc(),
            fa_id,
            formation_id,
            payload,
            federation_name,
        )
        .await?;
        dbtx.insert_entry(completed, &()).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("committing authenticated FA restore failed".to_owned()))
    }

    pub(crate) async fn backup_confirmation(&self, relay: &str) -> Option<BackupRelayConfirmation> {
        self.database
            .begin_transaction_nc()
            .await
            .get_value(&BackupRelayConfirmationKey {
                relay: relay.to_owned(),
            })
            .await
    }

    /// Refresh the federation name from a consensus recheck.
    pub(crate) async fn record_federation_name(
        &self,
        fa_id: FaId,
        federation_name: Option<crate::FederationName>,
    ) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let mut federation = dbtx
            .get_value(&FederationKey)
            .await
            .ok_or(FaError::NoActiveFormation)?;
        if federation.fa_id != fa_id {
            return Err(FaError::Storage(
                "FA federation record belongs to another scoped root".to_owned(),
            ));
        }
        federation.federation_name = federation_name;
        dbtx.insert_entry(&FederationKey, &federation).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("committing FA federation recheck failed".to_owned()))
    }

    pub(crate) async fn record_backup_confirmation(
        &self,
        relay: &str,
        confirmation: BackupRelayConfirmation,
    ) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        if dbtx
            .get_value(&BackupRelayConfirmationKey {
                relay: relay.to_owned(),
            })
            .await
            .is_some_and(|current| {
                current.generation > confirmation.generation
                    || (current.generation == confirmation.generation
                        && current.confirmed_at_secs > confirmation.confirmed_at_secs)
            })
        {
            return Ok(());
        }
        dbtx.insert_entry(
            &BackupRelayConfirmationKey {
                relay: relay.to_owned(),
            },
            &confirmation,
        )
        .await;
        dbtx.commit_tx_result().await.map_err(|_| {
            FaError::Storage("persisting FA relay backup confirmation failed".to_owned())
        })
    }

    pub(crate) async fn insert_liquidity_operation(
        &self,
        operation: crate::liquidity::StoredLiquidityOperation,
    ) -> FaResult<()> {
        let key = LiquidityOperationKey {
            operation_id: operation.operation_id.0.clone(),
        };
        let mut dbtx = self.database.begin_transaction().await;
        if dbtx.get_value(&key).await.is_some() {
            return Err(FaError::Liquidity(
                "the exact liquidity operation already exists; resume it instead".to_owned(),
            ));
        }
        dbtx.insert_entry(&key, &operation).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("persisting FA liquidity operation failed".to_owned()))
    }

    pub(crate) async fn load_liquidity_operation(
        &self,
        operation_id: &crate::LiquidityOperationId,
    ) -> FaResult<crate::liquidity::StoredLiquidityOperation> {
        self.database
            .begin_transaction_nc()
            .await
            .get_value(&LiquidityOperationKey {
                operation_id: operation_id.0.clone(),
            })
            .await
            .ok_or_else(|| FaError::Liquidity("liquidity operation not found".to_owned()))
    }

    pub(crate) async fn list_liquidity_operations(
        &self,
        after: Option<&crate::LiquidityOperationId>,
        limit: usize,
    ) -> FaResult<Vec<crate::liquidity::StoredLiquidityOperation>> {
        let mut dbtx = self.database.begin_transaction_nc().await;
        let mut entries = dbtx.find_by_prefix(&LiquidityOperationKeyPrefix).await;
        let mut operations = Vec::with_capacity(limit.saturating_add(1));
        while let Some((key, operation)) = entries.next().await {
            if after.is_some_and(|cursor| key.operation_id.as_str() <= cursor.0.as_str()) {
                continue;
            }
            if key.operation_id != operation.operation_id.0 {
                return Err(FaError::Storage(
                    "FA liquidity operation key does not match its stored identity".to_owned(),
                ));
            }
            operations.push(operation);
            if operations.len() > limit {
                break;
            }
        }
        Ok(operations)
    }

    pub(crate) async fn store_liquidity_response(
        &self,
        operation_id: &crate::LiquidityOperationId,
        response: fedi_decentralized_service_liquidity_manager::Signed<
            fedi_decentralized_service_liquidity_manager::RequestLiquidityResponse,
        >,
    ) -> FaResult<()> {
        let key = LiquidityOperationKey {
            operation_id: operation_id.0.clone(),
        };
        let mut dbtx = self.database.begin_transaction().await;
        let mut operation = dbtx
            .get_value(&key)
            .await
            .ok_or_else(|| FaError::Storage("FA liquidity operation disappeared".to_owned()))?;
        if operation
            .response
            .as_ref()
            .is_some_and(|stored| stored != &response)
        {
            return Err(FaError::Liquidity(
                "provider returned a conflicting response for the exact liquidity request"
                    .to_owned(),
            ));
        }
        let durably_accepted = operation.status.is_some()
            || operation.response.as_ref().is_some_and(|stored| {
                matches!(
                    stored.payload.outcome,
                    fedi_decentralized_service_liquidity_manager::RequestLiquidityOutcome::Accepted(
                        _
                    )
                )
            });
        if durably_accepted
            && matches!(
                response.payload.outcome,
                fedi_decentralized_service_liquidity_manager::RequestLiquidityOutcome::Rejected(_)
            )
        {
            return Err(FaError::Liquidity(
                "provider returned a rejection conflicting with durable acceptance evidence \
                 for the exact liquidity request"
                    .to_owned(),
            ));
        }
        operation.response = Some(response);
        dbtx.insert_entry(&key, &operation).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("persisting FA liquidity response failed".to_owned()))
    }

    pub(crate) async fn store_liquidity_status(
        &self,
        operation_id: &crate::LiquidityOperationId,
        status: fedi_decentralized_service_liquidity_manager::Signed<
            fedi_decentralized_service_liquidity_manager::GetAllocationStatusResponse,
        >,
    ) -> FaResult<()> {
        let key = LiquidityOperationKey {
            operation_id: operation_id.0.clone(),
        };
        let mut dbtx = self.database.begin_transaction().await;
        let mut operation = dbtx
            .get_value(&key)
            .await
            .ok_or_else(|| FaError::Storage("FA liquidity operation disappeared".to_owned()))?;
        if let Some(stored) = operation.status.as_ref() {
            if stored.payload.issued_at.0 > status.payload.issued_at.0 {
                return Err(FaError::Liquidity(
                    "provider returned a status older than the durable status".to_owned(),
                ));
            }
            if stored.payload.issued_at == status.payload.issued_at && stored != &status {
                return Err(FaError::Liquidity(
                    "provider returned conflicting statuses at one timestamp".to_owned(),
                ));
            }
        }
        operation.status = Some(status);
        if operation.verified_gateway_api.as_ref() != operation.completed_gateway_api()?.as_ref() {
            operation.verified_gateway_api = None;
        }
        dbtx.insert_entry(&key, &operation).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("persisting FA liquidity status failed".to_owned()))
    }

    pub(crate) async fn mark_liquidity_gateway_view_verified(
        &self,
        operation_id: &crate::LiquidityOperationId,
        gateway_api: &fedi_decentralized_domain::GatewayApiUrl,
    ) -> FaResult<()> {
        let key = LiquidityOperationKey {
            operation_id: operation_id.0.clone(),
        };
        let mut dbtx = self.database.begin_transaction().await;
        let mut operation = dbtx
            .get_value(&key)
            .await
            .ok_or_else(|| FaError::Storage("FA liquidity operation disappeared".to_owned()))?;
        if operation.completed_gateway_api()?.as_ref() != Some(gateway_api) {
            return Err(FaError::Liquidity(
                "gateway completion evidence changed before its LNv2 view was persisted".to_owned(),
            ));
        }
        operation.verified_gateway_api = Some(gateway_api.clone());
        dbtx.insert_entry(&key, &operation).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("persisting gateway-view verification failed".to_owned()))
    }

    #[cfg(test)]
    pub(crate) fn new_with_clock(
        database: Database,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            database,
            clock,
            formation_commit_hook: Arc::new(std::sync::Mutex::new(None)),
            seat_checkpoint_hook: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    #[cfg(test)]
    /// Pause the next hooked quote or seat transactions immediately before commit.
    ///
    /// Only the first `participants` calls from `store_quote`, `clear_quote`, or
    /// `record_seat_accepted` wait; an autocommit retry bypasses the barrier.
    pub(crate) fn set_next_formation_commit_barrier(&self, participants: usize) {
        *self
            .formation_commit_hook
            .lock()
            .expect("test formation hook lock") = Some(Arc::new(FormationTestHook {
            barrier: Arc::new(tokio::sync::Barrier::new(participants)),
            remaining: std::sync::atomic::AtomicUsize::new(participants),
        }));
    }

    #[cfg(test)]
    pub(crate) fn set_seat_checkpoint_hook(&self, hook: Arc<dyn Fn(u16) + Send + Sync + 'static>) {
        *self
            .seat_checkpoint_hook
            .lock()
            .expect("test seat checkpoint hook lock") = Some(hook);
    }

    #[cfg(test)]
    async fn wait_before_hooked_formation_commit(&self) {
        let hook = self
            .formation_commit_hook
            .lock()
            .expect("test formation hook lock")
            .clone();
        let Some(hook) = hook else {
            return;
        };
        if hook
            .remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            hook.barrier.wait().await;
        }
    }

    #[cfg(not(test))]
    async fn wait_before_hooked_formation_commit(&self) {}

    #[cfg(test)]
    /// Start a purchase and, when none exists, the creation it belongs to.
    pub(crate) async fn initialize_for_test(
        &self,
        fa_id: FaId,
        formation_id: FormationId,
        intent: ResolvedSeatPurchase,
        seats: Vec<InitialSeat>,
        payment_federation_id: Option<FederationId>,
    ) -> FaResult<()> {
        let creation = match self.load_creation(fa_id).await? {
            Some(_) => None,
            None => Some(NewCreation {
                federation_name: FederationName("Test Federation".to_owned()),
                federation_size: intent.federation_size,
                fedimintd_versions: intent.fedimintd_versions.clone(),
                plan: intent.plan,
                dkg_completion_callback: None,
            }),
        };
        self.initialize(
            fa_id,
            formation_id,
            intent,
            seats,
            payment_federation_id,
            creation,
        )
        .await
    }

    /// The formed federation's invite and seats, for post-formation work.
    pub(crate) async fn post_formed_authority(
        &self,
        fa_id: FaId,
    ) -> FaResult<crate::formation::PostFormedAuthority> {
        let federation = self
            .database
            .begin_transaction_nc()
            .await
            .get_value(&FederationKey)
            .await
            .ok_or(FaError::NoActiveFormation)?;
        if federation.fa_id != fa_id {
            return Err(FaError::Storage(
                "FA federation record belongs to another scoped root".to_owned(),
            ));
        }
        let seats = federation
            .seats
            .into_iter()
            .enumerate()
            .map(|(index, seat)| {
                Ok(crate::formation::PostFormedSeat {
                    index: u16::try_from(index)
                        .map_err(|_| FaError::Storage("FA seat index overflow".to_owned()))?,
                    mg_id: seat.mg_identity,
                    locator: seat.locator,
                    seat_id: seat.seat_id,
                })
            })
            .collect::<FaResult<Vec<_>>>()?;
        Ok(crate::formation::PostFormedAuthority {
            formation_id: federation.formation_id,
            invite_code: federation.federation_invite,
            seats,
        })
    }

    pub(crate) async fn load_status(&self, fa_id: FaId) -> FaResult<FaStatus> {
        let formation = self.load_formation(fa_id).await?;
        let reauthorization = self
            .database
            .begin_transaction_nc()
            .await
            .get_value(&CreationKey)
            .await
            .and_then(|creation| creation.reauthorization);
        Ok(FaStatus {
            recovery: None,
            creation: self.load_creation(fa_id).await?,
            reauthorization,
            purchase: self
                .load_purchase(fa_id)
                .await?
                .map(|purchase| purchase.snapshot),
            pool: self.pool_seats().await,
            formation: formation.map(|formation| formation.snapshot),
            federation: self.load_federation(fa_id).await?,
        })
    }

    /// The running federation creation, if any.
    pub(crate) async fn load_creation(&self, fa_id: FaId) -> FaResult<Option<CreationIntent>> {
        let Some(creation) = self
            .database
            .begin_transaction_nc()
            .await
            .get_value(&CreationKey)
            .await
        else {
            return Ok(None);
        };
        if creation.fa_id != fa_id {
            return Err(FaError::Storage(
                "FA federation creation belongs to another scoped root".to_owned(),
            ));
        }
        Ok(Some(CreationIntent {
            federation_name: creation.federation_name,
            federation_size: creation.federation_size,
            fedimintd_versions: creation.fedimintd_versions,
            plan: creation.plan,
        }))
    }

    /// End a federation creation that holds no bought seat or formation,
    /// with its running purchase.
    pub(crate) async fn cancel_creation(&self) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        if dbtx.get_value(&CreationKey).await.is_none() {
            return Err(FaError::NoActiveFormation);
        }
        let seats_bought =
            || FaError::AbandonUnavailable(crate::AbandonUnavailableReason::SeatsBought);
        if dbtx
            .find_by_prefix(&PoolSeatKeyPrefix)
            .await
            .next()
            .await
            .is_some()
        {
            return Err(seats_bought());
        }
        if let Some(purchase) = dbtx.get_value(&PurchaseKey).await {
            let rows = purchase_seat_rows(&mut dbtx.to_ref_nc(), &purchase.formation_id).await?;
            for (key, seat) in rows {
                if seat.seat_id.is_some() {
                    return Err(seats_bought());
                }
                dbtx.remove_entry(&key).await;
            }
            dbtx.remove_entry(&PurchaseKey).await;
        }
        dbtx.remove_entry(&CreationKey).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("cancelling FA federation creation failed".to_owned()))
    }

    /// What a preview of the missing seats must avoid and keep.
    pub(crate) async fn bought_seats(&self) -> crate::selection::BoughtSeats {
        let seats: Vec<StoredPoolSeat> = self
            .database
            .begin_transaction_nc()
            .await
            .find_by_prefix(&PoolSeatKeyPrefix)
            .await
            .map(|(_, seat)| seat)
            .collect()
            .await;
        crate::selection::BoughtSeats {
            fedimintd_dkg_version: seats
                .first()
                .map(|seat| seat.fedimintd_version.dkg_version()),
            mg_ids: seats.iter().map(|seat| seat.mg_id).collect(),
            service_pubkeys: seats
                .iter()
                .map(|seat| (seat.locator.service_pubkey, seat.mg_id))
                .collect(),
            holders: seats.iter().filter_map(|seat| seat.holder).collect(),
        }
    }

    /// Bought seats that host no guardian yet.
    pub(crate) async fn pool_seats(&self) -> Vec<crate::PoolSeat> {
        self.database
            .begin_transaction_nc()
            .await
            .find_by_prefix(&PoolSeatKeyPrefix)
            .await
            .map(|(_, seat)| seat.public())
            .collect()
            .await
    }

    /// The federation this FA governs, as stored.
    pub(crate) async fn load_federation(
        &self,
        fa_id: FaId,
    ) -> FaResult<Option<crate::FederationSnapshot>> {
        let mut dbtx = self.database.begin_transaction_nc().await;
        let Some(federation) = dbtx.get_value(&FederationKey).await else {
            return Ok(None);
        };
        if federation.fa_id != fa_id {
            return Err(FaError::Storage(
                "FA federation record belongs to another scoped root".to_owned(),
            ));
        }
        Ok(Some(crate::FederationSnapshot {
            formation_id: federation.formation_id,
            federation_invite: federation.federation_invite,
            federation_name: federation.federation_name,
            seats: federation
                .seats
                .into_iter()
                .map(|seat| crate::FederationSeat {
                    mg_id: seat.mg_identity,
                    seat_id: seat.seat_id,
                    locator: seat.locator,
                })
                .collect(),
            freshness: FormationFreshness::Unsynced,
            last_error: None,
        }))
    }

    /// The running formation, with the name and callback of its creation.
    pub(crate) async fn load_formation(&self, fa_id: FaId) -> FaResult<Option<FormationRecovery>> {
        let mut dbtx = self.database.begin_transaction_nc().await;
        let Some(creation) = dbtx.get_value(&CreationKey).await else {
            return Ok(None);
        };
        if creation.fa_id != fa_id {
            return Err(FaError::Storage(
                "persisted FA creation belongs to a different identity".to_owned(),
            ));
        }
        let Some(seats) = formation_seats(&mut dbtx.to_ref_nc(), &creation).await? else {
            return Ok(None);
        };
        let fedimintd_dkg_version = seats[0].0.fedimintd_version.dkg_version();
        let dkg_complete = creation.invite_code.is_some();
        let seats = seats
            .into_iter()
            .enumerate()
            .map(|(position, (seat, guardian_code))| FormationSeatRecovery {
                progress: SeatProgress {
                    index: u16::try_from(position).expect("validated formation size fits u16"),
                    mg_id: seat.mg_id,
                    locator: seat.locator,
                    seat_id: Some(seat.seat_id),
                    phase: if dkg_complete {
                        SeatPhase::Running
                    } else if guardian_code.is_some() {
                        SeatPhase::GuardianCodeReady
                    } else {
                        SeatPhase::Created
                    },
                    guardian_code,
                    freshness: FormationFreshness::Unsynced,
                },
                guardian_fee_account: seat.guardian_fee_account,
            })
            .collect::<Vec<_>>();
        let phase = if creation.formation_meta_target.is_some() {
            FormationPhase::PublishingSeatBindings
        } else if dkg_complete {
            FormationPhase::DkgComplete
        } else {
            FormationPhase::PreparingDkg
        };
        Ok(Some(FormationRecovery {
            snapshot: FormationSnapshot {
                formation_id: creation.formation_id,
                federation_name: creation.federation_name,
                fedimintd_dkg_version,
                phase,
                seats: seats.iter().map(|seat| seat.progress.clone()).collect(),
                freshness: FormationFreshness::Unsynced,
                invite_code: creation.invite_code,
                last_error: None,
            },
            seats,
            formation_meta_target: creation.formation_meta_target,
            dkg_completion_callback: creation.dkg_completion_callback,
        }))
    }

    pub(crate) async fn load_purchase(&self, fa_id: FaId) -> FaResult<Option<PurchaseRecovery>> {
        let mut dbtx = self.database.begin_transaction().await;
        if let Some(bytes) = dbtx
            .raw_get_bytes(&DatabaseKeyPrefix::to_bytes(&PurchaseKey))
            .await
            .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
        {
            let envelope: StoredPurchaseEnvelope =
                serde_json::from_slice(&bytes).map_err(|_| {
                    FaError::Storage("persisted FA purchase header is malformed".to_owned())
                })?;
            if envelope.schema_version != STORAGE_SCHEMA_VERSION {
                return Err(FaError::Storage(format!(
                    "unsupported FA storage schema version {}",
                    envelope.schema_version
                )));
            }
        }
        let Some(stored) = dbtx.get_value(&PurchaseKey).await else {
            return Ok(None);
        };
        validate_schema_and_owner(&stored, fa_id)?;

        let seats = purchase_seat_rows(&mut dbtx.to_ref_nc(), &stored.formation_id)
            .await?
            .into_iter()
            .map(|(_, seat)| seat)
            .collect::<Vec<_>>();
        validate_formation_progress(&stored, &seats)?;

        let mut recoveries = Vec::with_capacity(seats.len());
        for seat in seats {
            let phase = if seat.seat_id.is_some() {
                SeatPhase::Created
            } else if seat.signed_quote.is_some() {
                SeatPhase::QuoteReady
            } else {
                SeatPhase::Selected
            };
            recoveries.push(PurchaseSeatRecovery {
                progress: SeatProgress {
                    index: seat.index,
                    mg_id: seat.admission.mg_id(),
                    locator: seat.locator,
                    seat_id: seat.seat_id,
                    guardian_code: None,
                    phase,
                    freshness: FormationFreshness::Unsynced,
                },
                admission: seat.admission,
                signed_quote: seat.signed_quote,
                guardian_fee_account: seat.guardian_fee_account,
            });
        }

        let phase = public_phase(stored.phase, &recoveries);
        let snapshot = SeatPurchaseSnapshot {
            purchase_id: stored.formation_id,
            request: stored.intent,
            phase,
            seats: recoveries
                .iter()
                .map(|recovery| recovery.progress.clone())
                .collect(),
            freshness: FormationFreshness::Unsynced,
            last_error: None,
        };
        Ok(Some(PurchaseRecovery {
            snapshot,
            seats: recoveries,
            payment_federation_id: stored.payment_federation_id,
        }))
    }

    /// Load the complete last durably admitted setup-payment publication.
    pub(crate) async fn load_setup_payment_federations_event(&self) -> Option<Event> {
        self.database
            .begin_transaction_nc()
            .await
            .get_value(&SetupPaymentFederationsKey)
            .await
            .map(|stored| stored.0)
    }

    /// Atomically retain one complete admitted setup-payment publication if it is highest.
    pub(crate) async fn store_setup_payment_federations_event(&self, event: Event) -> FaResult<()> {
        self.database
            .autocommit(
                move |dbtx, _| {
                    let event = event.clone();
                    Box::pin(async move {
                        let current = dbtx.get_value(&SetupPaymentFederationsKey).await;
                        if current.as_ref().is_some_and(|current| {
                            current.0.id == event.id
                                || !setup_payment_event_is_newer(&event, &current.0)
                        }) {
                            return Ok(());
                        }
                        dbtx.insert_entry(
                            &SetupPaymentFederationsKey,
                            &StoredSetupPaymentFederationsEvent(event),
                        )
                        .await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(|error: AutocommitError<FaError>| match error {
                AutocommitError::ClosureError { error, .. } => error,
                AutocommitError::CommitFailed { .. } => {
                    FaError::Storage("storing setup-payment policy transaction failed".to_owned())
                }
            })
    }

    pub(crate) async fn initialize(
        &self,
        fa_id: FaId,
        formation_id: FormationId,
        intent: ResolvedSeatPurchase,
        seats: Vec<InitialSeat>,
        payment_federation_id: Option<FederationId>,
        creation: Option<NewCreation>,
    ) -> FaResult<()> {
        let seat_count = u16::try_from(seats.len())
            .map_err(|_| FaError::Storage("too many FA seat rows".to_owned()))?;
        let mut dbtx = self.database.begin_transaction().await;
        if dbtx.get_value(&PurchaseKey).await.is_some() {
            return Err(FaError::InvalidIntent(
                "a seat purchase is already running".to_owned(),
            ));
        }
        match creation {
            Some(creation) => {
                if dbtx.get_value(&CreationKey).await.is_some()
                    || dbtx.get_value(&FederationKey).await.is_some()
                {
                    return Err(FaError::InvalidIntent(
                        "this FA already creates or governs a federation".to_owned(),
                    ));
                }
                dbtx.insert_new_entry(
                    &CreationKey,
                    &StoredCreation {
                        fa_id,
                        federation_name: creation.federation_name,
                        federation_size: creation.federation_size,
                        fedimintd_versions: creation.fedimintd_versions,
                        plan: creation.plan,
                        dkg_completion_callback: creation.dkg_completion_callback,
                        formation_id: formation_id.clone(),
                        invite_code: None,
                        formation_meta_target: None,
                        named_seat: None,
                        reauthorization: None,
                    },
                )
                .await;
            }
            None => {
                let Some(mut existing) = dbtx.get_value(&CreationKey).await else {
                    return Err(FaError::InvalidIntent(
                        "no federation creation needs seats".to_owned(),
                    ));
                };
                existing.reauthorization = None;
                dbtx.insert_entry(&CreationKey, &existing).await;
            }
        }
        dbtx.insert_entry(
            &PurchaseKey,
            &StoredPurchase {
                schema_version: STORAGE_SCHEMA_VERSION,
                fa_id: Some(fa_id),
                formation_id: formation_id.clone(),
                phase: StoredPurchasePhase::Initialized,
                intent,
                seat_count,
                payment_federation_id,
            },
        )
        .await;
        for seat in seats {
            let InitialSeat {
                progress,
                admission,
            } = seat;
            dbtx.insert_entry(
                &PurchaseSeatKey {
                    formation_id: formation_id.0.clone(),
                    index: progress.index,
                },
                &StoredPurchaseSeat {
                    index: progress.index,
                    locator: progress.locator,
                    admission,
                    signed_quote: None,
                    seat_id: None,
                    guardian_fee_account: None,
                },
            )
            .await;
        }
        // The run guard serializes initialization before any seat work can
        // run, so this transaction cannot encounter an expected formation RMW
        // conflict.
        dbtx.commit_tx_result().await.map_err(|_| {
            FaError::Storage("initializing FA formation transaction failed".to_owned())
        })?;
        Ok(())
    }

    /// Record one accepted seat and derive completion without rewriting sibling rows.
    pub(crate) async fn record_seat_accepted(
        &self,
        formation_id: &FormationId,
        index: u16,
        seat_id: SeatId,
        guardian_fee_account: Account,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        let store = self.clone();
        let result = self
            .database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let seat_id = seat_id.clone();
                    let guardian_fee_account = guardian_fee_account.clone();
                    let store = store.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot record a seat without an active formation".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let key = PurchaseSeatKey {
                            formation_id: formation_id.0.clone(),
                            index,
                        };
                        let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                            FaError::Storage(format!("missing persisted FA seat row {index}"))
                        })?;
                        let signed_quote = seat.signed_quote.as_ref().ok_or_else(|| {
                            FaError::Storage(format!(
                                "accepted FA seat row {index} is missing its persisted signed quote"
                            ))
                        })?;
                        let quote =
                            signed_quote
                                .verify(&seat.locator.service_pubkey)
                                .map_err(|error| {
                                    FaError::Storage(format!(
                                        "invalid accepted quote for FA seat row {index}: {error}"
                                    ))
                                })?;
                        let effect = if quote.terms.payment.is_some() {
                            AdmissionEffect::PaidOutput
                        } else {
                            AdmissionEffect::FreePresentation
                        };
                        seat.admission
                            .require_authorized_effect(quote.quote_id(), effect)?;
                        if let Some(existing) = &seat.seat_id
                            && (existing != &seat_id
                                || seat.guardian_fee_account.as_ref()
                                    != Some(&guardian_fee_account))
                        {
                            return Err(FaError::Storage(format!(
                                "FA seat row {index} changed its signed acceptance facts"
                            )));
                        }
                        seat.seat_id = Some(seat_id);
                        seat.guardian_fee_account = Some(guardian_fee_account);
                        dbtx.insert_entry(&key, &seat).await;

                        formation.phase = purchase_phase(
                            &purchase_seat_rows(dbtx, &formation_id)
                                .await?
                                .into_iter()
                                .map(|(_, seat)| seat)
                                .collect::<Vec<_>>(),
                        );
                        dbtx.insert_entry(&PurchaseKey, &formation).await;
                        store.wait_before_hooked_formation_commit().await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error);
        #[cfg(test)]
        if result.is_ok()
            && let Some(hook) = self
                .seat_checkpoint_hook
                .lock()
                .expect("test seat checkpoint hook lock")
                .clone()
        {
            hook(index);
        }
        result
    }

    /// Move a finished purchase's seats into the pool and drop the purchase.
    pub(crate) async fn complete_purchase(&self, purchase_id: &FormationId) -> FaResult<()> {
        let purchase_id = purchase_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let purchase_id = purchase_id.clone();
                    Box::pin(async move {
                        let purchase = dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
                            FaError::Storage("cannot complete a missing purchase".to_owned())
                        })?;
                        validate_active_formation(&purchase, &purchase_id)?;
                        if purchase.phase != StoredPurchasePhase::SeatsCreated {
                            return Err(FaError::Storage(
                                "cannot complete a purchase before every seat exists".to_owned(),
                            ));
                        }
                        for (key, seat) in purchase_seat_rows(dbtx, &purchase_id).await? {
                            let pool_seat = pool_seat(seat)?;
                            dbtx.insert_entry(
                                &PoolSeatKey {
                                    seat_id: pool_seat.seat_id.to_string(),
                                },
                                &pool_seat,
                            )
                            .await;
                            dbtx.remove_entry(&key).await;
                        }
                        dbtx.remove_entry(&PurchaseKey).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Change the formation progress in one transaction.
    async fn update_formation(
        &self,
        formation_id: &FormationId,
        update: impl Fn(&mut StoredCreation) + Send + Sync,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        let update = &update;
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    Box::pin(async move {
                        let (mut creation, _) = active_formation(dbtx, &formation_id).await?;
                        update(&mut creation);
                        active_formation_seats(dbtx, &creation).await?;
                        dbtx.insert_entry(&CreationKey, &creation).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Record one guardian code.
    pub(crate) async fn record_guardian_code(
        &self,
        formation_id: &FormationId,
        index: u16,
        guardian_code: GuardianCode,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let guardian_code = guardian_code.clone();
                    Box::pin(async move {
                        let (_, seats) = active_formation(dbtx, &formation_id).await?;
                        let (seat, existing) = seats.get(usize::from(index)).ok_or_else(|| {
                            FaError::Storage(format!("missing FA formation seat {index}"))
                        })?;
                        if existing
                            .as_ref()
                            .is_some_and(|existing| existing != &guardian_code)
                        {
                            return Err(FaError::Storage(format!(
                                "FA formation seat {index} changed guardian code"
                            )));
                        }
                        dbtx.insert_entry(
                            &GuardianCodeKey {
                                seat_id: seat.seat_id.to_string(),
                            },
                            &StoredGuardianCode(guardian_code),
                        )
                        .await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Remove pool seats whose MG ended them before DKG started. DKG starts
    /// only after every seat cached its guardian code, so a seat without a
    /// cached code proves that no seat received `StartDkg`. The remaining
    /// cached codes are dropped too: a new named seat needs a new code.
    pub(crate) async fn drop_ended_pool_seats(
        &self,
        formation_id: &FormationId,
        ended: Vec<SeatId>,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        let ended = &ended;
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    Box::pin(async move {
                        let (mut creation, seats) = active_formation(dbtx, &formation_id).await?;
                        if seats.iter().all(|(_, code)| code.is_some()) {
                            return Err(FaError::Storage(
                                "cannot drop FA pool seats after DKG may have started".to_owned(),
                            ));
                        }
                        for (seat, _) in &seats {
                            let seat_id = seat.seat_id.to_string();
                            dbtx.remove_entry(&GuardianCodeKey {
                                seat_id: seat_id.clone(),
                            })
                            .await;
                            if ended.contains(&seat.seat_id) {
                                dbtx.remove_entry(&PoolSeatKey { seat_id }).await;
                            }
                        }
                        if creation
                            .named_seat
                            .as_ref()
                            .is_some_and(|named| ended.contains(named))
                        {
                            creation.named_seat = None;
                        }
                        creation.reauthorization.get_or_insert(
                            crate::SelectionReauthorizationReason::SelectedMgUnavailable,
                        );
                        dbtx.insert_entry(&CreationKey, &creation).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Save completed DKG and its invite.
    pub(crate) async fn record_dkg_complete(
        &self,
        formation_id: &FormationId,
        invite_code: InviteCode,
    ) -> FaResult<()> {
        self.update_formation(formation_id, |creation| {
            creation.invite_code = Some(invite_code.clone());
            // Every MG now owns durable delivery retry. Drop the FA-side
            // bearer with the DKG checkpoint, retaining it through every
            // earlier crash and restart.
            creation.dkg_completion_callback = None;
        })
        .await
    }

    /// Persist the complete formation metadata target before it is submitted.
    pub(crate) async fn record_formation_meta_target(
        &self,
        formation_id: &FormationId,
        target: FormationMetaTarget,
    ) -> FaResult<()> {
        self.update_formation(formation_id, |creation| {
            creation.formation_meta_target = Some(target.clone());
        })
        .await
    }

    /// Checkpoint exact consensus readback of the persisted formation target:
    /// record the federation it formed, and end the creation and its pool.
    pub(crate) async fn finish_formation(&self, formation_id: &FormationId) -> FaResult<()> {
        let formation_id = formation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    Box::pin(async move {
                        let (creation, seats) = active_formation(dbtx, &formation_id).await?;
                        if creation.formation_meta_target.is_none() {
                            return Err(FaError::Storage(
                                "cannot complete formation before publishing metadata".to_owned(),
                            ));
                        }
                        insert_formed_federation(dbtx, &creation, &seats).await?;
                        for (seat, _) in &seats {
                            let seat_id = seat.seat_id.to_string();
                            dbtx.remove_entry(&GuardianCodeKey {
                                seat_id: seat_id.clone(),
                            })
                            .await;
                            dbtx.remove_entry(&PoolSeatKey { seat_id }).await;
                        }
                        dbtx.remove_entry(&CreationKey).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Bind one seat to its quote before its payment or presentation can
    /// start. The first seat of a purchase must still hold its preview
    /// approval. A paid seat
    /// moves to a new quote only after the wallet proved that no payment of
    /// its bound quote started.
    pub(crate) async fn bind_quote(
        &self,
        formation_id: &FormationId,
        index: u16,
        bound_quote: Option<&SignedResponse<GetQuoteResponse>>,
        signed_quote: SignedResponse<GetQuoteResponse>,
        verifier_provenance: StoredVerifierProvenance,
    ) -> FaResult<()> {
        let now = Timestamp((self.clock)());
        let formation_id = formation_id.clone();
        let bound_quote = bound_quote.cloned();
        let store = self.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let bound_quote = bound_quote.clone();
                    let signed_quote = signed_quote.clone();
                    let store = store.clone();
                    Box::pin(async move {
                        let formation = dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
                            FaError::Storage(
                                "cannot bind a quote without an active purchase".to_owned(),
                            )
                        })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let key = PurchaseSeatKey {
                            formation_id: formation_id.0.clone(),
                            index,
                        };
                        let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                            FaError::Storage(format!("missing persisted FA seat row {index}"))
                        })?;
                        if seat.seat_id.is_some() || seat.signed_quote != bound_quote {
                            return Err(FaError::Storage(format!(
                                "FA seat row {index} changed before its quote was bound"
                            )));
                        }
                        let quote =
                            signed_quote
                                .verify(&seat.locator.service_pubkey)
                                .map_err(|error| {
                                    FaError::Storage(format!(
                                        "invalid quote for FA seat row {index}: {error}"
                                    ))
                                })?;
                        let effect = if quote.terms.payment.is_some() {
                            AdmissionEffect::PaidOutput
                        } else {
                            AdmissionEffect::FreePresentation
                        };
                        // The preview approval must be fresh until the
                        // purchase binds its first seat.
                        let purchase_started = purchase_seat_rows(dbtx, &formation_id)
                            .await?
                            .iter()
                            .any(|(_, seat)| seat.signed_quote.is_some());
                        match seat.admission.authorized_effect() {
                            None if purchase_started => {}
                            None => {
                                validate_fresh_admission(&seat.admission, verifier_provenance, now)?
                            }
                            Some((_, AdmissionEffect::PaidOutput))
                                if effect == AdmissionEffect::PaidOutput => {}
                            Some(_) => {
                                return Err(FaError::Storage(format!(
                                    "FA seat row {index} cannot move its bound quote"
                                )));
                            }
                        }
                        seat.admission.state = AdmissionState::EffectAuthorized {
                            quote_id: quote.quote_id(),
                            effect,
                        };
                        seat.signed_quote = Some(signed_quote);
                        dbtx.insert_entry(&key, &seat).await;
                        store.wait_before_hooked_formation_commit().await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Remove a seat that cannot be bought from its purchase. The caller
    /// proved that no payment of its bound quote is outstanding.
    pub(crate) async fn drop_seat(
        &self,
        formation_id: &FormationId,
        index: u16,
        bound_quote: Option<&SignedResponse<GetQuoteResponse>>,
        reason: crate::SelectionReauthorizationReason,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        let bound_quote = bound_quote.cloned();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let bound_quote = bound_quote.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot drop a seat without an active purchase".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let key = PurchaseSeatKey {
                            formation_id: formation_id.0.clone(),
                            index,
                        };
                        let seat = dbtx.get_value(&key).await.ok_or_else(|| {
                            FaError::Storage(format!("missing persisted FA seat row {index}"))
                        })?;
                        if seat.seat_id.is_some() || seat.signed_quote != bound_quote {
                            return Err(FaError::Storage(format!(
                                "FA seat row {index} changed before it left the purchase"
                            )));
                        }
                        dbtx.remove_entry(&key).await;
                        let seats = purchase_seat_rows(dbtx, &formation_id)
                            .await?
                            .into_iter()
                            .map(|(_, seat)| seat)
                            .collect::<Vec<_>>();
                        formation.seat_count = u16::try_from(seats.len())
                            .expect("remaining seats fit the stored count");
                        formation.phase = purchase_phase(&seats);
                        dbtx.insert_entry(&PurchaseKey, &formation).await;
                        record_reauthorization(dbtx, reason).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }
}

/// Map a formation autocommit failure without exposing backend details.
pub(crate) fn map_formation_tx_error(error: AutocommitError<FaError>) -> FaError {
    match error {
        AutocommitError::ClosureError { error, .. } => error,
        AutocommitError::CommitFailed { .. } => {
            FaError::Storage("updating FA formation transaction failed".to_owned())
        }
    }
}

fn setup_payment_event_is_newer(candidate: &Event, current: &Event) -> bool {
    current.created_at < candidate.created_at
        || (candidate.created_at == current.created_at && candidate.id < current.id)
}

fn validate_fresh_admission(
    admission: &MgAdmission,
    expected_provenance: StoredVerifierProvenance,
    now: Timestamp,
) -> FaResult<()> {
    if !admission.requires_effect_authorization() {
        return Ok(());
    }
    if admission.verifier_provenance != expected_provenance {
        return Err(FaError::SelectionReauthorizationRequired {
            reason: crate::SelectionReauthorizationReason::VerifierEnvironmentChanged,
            failure: None,
        });
    }
    if admission.valid_until <= now {
        return Err(FaError::SelectionReauthorizationRequired {
            reason: crate::SelectionReauthorizationReason::PreviewExpired,
            failure: None,
        });
    }
    Ok(())
}

/// The seat rows of one purchase, in index order. A seat that ended after
/// payment outputs started leaves its row, so indexes can have gaps.
async fn purchase_seat_rows(
    dbtx: &mut DatabaseTransaction<'_>,
    formation_id: &FormationId,
) -> FaResult<Vec<(PurchaseSeatKey, StoredPurchaseSeat)>> {
    let mut rows = dbtx
        .find_by_prefix(&PurchaseSeatKeyPrefix)
        .await
        .filter(|(key, _)| std::future::ready(key.formation_id == formation_id.0))
        .collect::<Vec<_>>()
        .await;
    rows.sort_by_key(|(key, _)| key.index);
    for (key, seat) in &rows {
        if seat.index != key.index {
            return Err(FaError::Storage(format!(
                "persisted FA seat row {} contains index {}",
                key.index, seat.index
            )));
        }
    }
    Ok(rows)
}

/// The purchase phase that its seat rows imply.
fn purchase_phase(seats: &[StoredPurchaseSeat]) -> StoredPurchasePhase {
    let created = seats.iter().filter(|seat| seat.seat_id.is_some()).count();
    if created == seats.len() {
        StoredPurchasePhase::SeatsCreated
    } else if created == 0 {
        StoredPurchasePhase::Initialized
    } else {
        StoredPurchasePhase::SeatsPartiallyCreated
    }
}

fn validate_formation_progress(
    formation: &StoredPurchase,
    seats: &[StoredPurchaseSeat],
) -> FaResult<()> {
    if formation.seat_count > formation.intent.federation_size.0
        || seats.len() != usize::from(formation.seat_count)
    {
        return Err(FaError::Storage(
            "persisted FA seat cardinality disagrees with formation intent".to_owned(),
        ));
    }

    let mut mg_ids = BTreeSet::new();
    let mut holders = BTreeSet::new();
    let mut service_pubkeys = BTreeMap::new();
    for seat in seats {
        if let Some(holder) = seat.admission.holder()
            && !holders.insert(holder)
        {
            return Err(FaError::Storage(
                "persisted FA seats contain duplicate badge holders".to_owned(),
            ));
        }
        if !mg_ids.insert(seat.admission.mg_id()) {
            return Err(FaError::Storage(format!(
                "persisted FA seat row {} duplicates an admitted MG identity",
                seat.index
            )));
        }
        if let Some(existing_index) =
            service_pubkeys.insert(seat.locator.service_pubkey, seat.index)
        {
            return Err(FaError::Storage(format!(
                "persisted FA seat row {} duplicates the service signing key of seat row \
                 {existing_index}",
                seat.index
            )));
        }
        // A seat holds a quote exactly while it is bound to it, and an
        // accepted seat is always bound.
        match (
            seat.admission.authorized_effect(),
            seat.signed_quote.as_ref(),
        ) {
            (None, None) if seat.seat_id.is_none() => {}
            (Some((quote_id, effect)), Some(signed_quote)) => {
                let quote = signed_quote
                    .verify(&seat.locator.service_pubkey)
                    .map_err(|error| {
                        FaError::Storage(format!(
                            "invalid bound quote for FA seat row {}: {error}",
                            seat.index
                        ))
                    })?;
                let payment_matches = match effect {
                    AdmissionEffect::PaidOutput => quote.terms.payment.is_some(),
                    AdmissionEffect::FreePresentation => quote.terms.payment.is_none(),
                };
                if !payment_matches || quote.quote_id() != quote_id {
                    return Err(FaError::Storage(format!(
                        "persisted FA seat row {} is bound to another quote",
                        seat.index
                    )));
                }
            }
            _ => {
                return Err(FaError::Storage(format!(
                    "persisted FA seat row {} does not hold exactly its bound quote",
                    seat.index
                )));
            }
        }
        // An accepted seat and its MG-signed guardian-fee account are one
        // durable fact: neither may appear without the other. Formation
        // records persisted before the fee-account acceptance existed fail
        // this validation and must be reset, per the pre-launch namespace
        // policy of rejecting rather than migrating older records.
        if seat.seat_id.is_some() != seat.guardian_fee_account.is_some() {
            return Err(FaError::Storage(format!(
                "persisted FA seat row {} does not pair its accepted seat with the signed guardian-fee account",
                seat.index
            )));
        }
        if seat
            .guardian_fee_account
            .as_ref()
            .is_some_and(|account| GuardianFeeAccount::try_from(account.clone()).is_err())
        {
            return Err(FaError::Storage(format!(
                "persisted FA seat row {} has an invalid signed guardian-fee account",
                seat.index
            )));
        }
    }

    if formation.phase != purchase_phase(seats) {
        return Err(FaError::Storage(
            "persisted FA purchase phase disagrees with accepted-seat rows".to_owned(),
        ));
    }
    Ok(())
}

/// A pool seat and its cached guardian code.
type FormationSeat = (StoredPoolSeat, Option<GuardianCode>);

/// The seats of the creation's formation, in pool order with their cached
/// guardian codes and the named seat first. The formation runs once the pool
/// holds every seat.
async fn formation_seats(
    dbtx: &mut DatabaseTransaction<'_>,
    creation: &StoredCreation,
) -> FaResult<Option<Vec<FormationSeat>>> {
    let mut pool: Vec<StoredPoolSeat> = dbtx
        .find_by_prefix(&PoolSeatKeyPrefix)
        .await
        .map(|(_, seat)| seat)
        .collect()
        .await;
    if pool.len() < usize::from(creation.federation_size.0) {
        if creation.invite_code.is_some() {
            return Err(FaError::Storage(
                "persisted FA formation lacks pool seats".to_owned(),
            ));
        }
        return Ok(None);
    }
    if let Some(named) = creation
        .named_seat
        .as_ref()
        .and_then(|named| pool.iter().position(|seat| &seat.seat_id == named))
    {
        pool[..=named].rotate_right(1);
    }
    let mut seats = Vec::with_capacity(pool.len());
    for seat in pool {
        let guardian_code = dbtx
            .get_value(&GuardianCodeKey {
                seat_id: seat.seat_id.to_string(),
            })
            .await
            .map(|code| code.0);
        seats.push((seat, guardian_code));
    }
    validate_formation(creation, &seats)?;
    Ok(Some(seats))
}

/// Keep the first reason a seat left since the last purchase started.
async fn record_reauthorization(
    dbtx: &mut DatabaseTransaction<'_>,
    reason: crate::SelectionReauthorizationReason,
) {
    if let Some(mut creation) = dbtx.get_value(&CreationKey).await
        && creation.reauthorization.is_none()
    {
        creation.reauthorization = Some(reason);
        dbtx.insert_entry(&CreationKey, &creation).await;
    }
}

/// The seats of the running formation.
async fn active_formation_seats(
    dbtx: &mut DatabaseTransaction<'_>,
    creation: &StoredCreation,
) -> FaResult<Vec<FormationSeat>> {
    formation_seats(dbtx, creation)
        .await?
        .ok_or_else(|| FaError::Storage("the FA formation is missing".to_owned()))
}

/// The creation whose formation `formation_id` names, and its seats.
async fn active_formation(
    dbtx: &mut DatabaseTransaction<'_>,
    formation_id: &FormationId,
) -> FaResult<(StoredCreation, Vec<FormationSeat>)> {
    let creation = dbtx
        .get_value(&CreationKey)
        .await
        .filter(|creation| &creation.formation_id == formation_id)
        .ok_or_else(|| {
            FaError::Storage("active FA formation changed during an operation".to_owned())
        })?;
    let seats = active_formation_seats(dbtx, &creation).await?;
    Ok((creation, seats))
}

/// Reject formation progress that disagrees with its seats.
fn validate_formation(creation: &StoredCreation, seats: &[FormationSeat]) -> FaResult<()> {
    let size = creation.federation_size.0;
    if usize::from(size) != seats.len()
        || !(crate::MIN_FEDERATION_SIZE..crate::MAX_FEDERATION_SIZE_EXCLUSIVE).contains(&size)
    {
        return Err(FaError::Storage(
            "persisted FA formation has an unsupported seat count".to_owned(),
        ));
    }
    let dkg_version = seats[0].0.fedimintd_version.dkg_version();
    if seats.iter().any(|(seat, _)| {
        seat.federation_size.0 != size || seat.fedimintd_version.dkg_version() != dkg_version
    }) {
        return Err(FaError::Storage(
            "FA formation seats must be bought for its size and DKG line".to_owned(),
        ));
    }
    if creation.invite_code.is_none() {
        if creation.formation_meta_target.is_some() {
            return Err(FaError::Storage(
                "persisted FA formation has a metadata target before its invite".to_owned(),
            ));
        }
    } else if let Some(position) = seats.iter().position(|(_, code)| code.is_none()) {
        return Err(FaError::Storage(format!(
            "persisted formed FA formation lacks guardian recovery facts for seat row \
             {position}"
        )));
    }
    Ok(())
}

/// Record the federation of a verified backup. The backup becomes the last
/// published generation, so unchanged facts reuse its generation and changed
/// facts advance it.
async fn adopt_backup(
    dbtx: &mut DatabaseTransaction<'_>,
    fa_id: FaId,
    formation_id: FormationId,
    payload: crate::backup::FaBackupPayload,
    federation_name: Option<crate::FederationName>,
) -> FaResult<()> {
    if dbtx.get_value(&BackupGenerationKey).await.is_none() {
        dbtx.insert_entry(
            &BackupGenerationKey,
            &BackupGenerationState {
                generation: payload.snapshot_generation,
                semantic_hash: crate::backup::semantic_hash(&payload)?,
                document_hash: crate::backup::document_hash(&payload)?,
                nostr_created_at: fedimint_core::time::duration_since_epoch().as_secs(),
            },
        )
        .await;
    }
    if let Some(commitment) = payload.liquidity.clone() {
        let details_payload_hash =
            fedi_decentralized_service_liquidity_manager::request_liquidity_details_hash(
                &commitment,
            )
            .map_err(|error| {
                FaError::Storage(format!(
                    "hashing restored liquidity commitment failed: {error}"
                ))
            })?;
        let operation_id = crate::LiquidityOperationId(hex::encode(details_payload_hash.0));
        let key = LiquidityOperationKey {
            operation_id: operation_id.0.clone(),
        };
        if dbtx.get_value(&key).await.is_none() {
            dbtx.insert_entry(
                &key,
                &crate::liquidity::StoredLiquidityOperation {
                    schema_version: 3,
                    operation_id,
                    formation_id: formation_id.clone(),
                    commitment,
                    endpoint_hint: None,
                    details_payload_hash,
                    response: None,
                    status: None,
                    verified_gateway_api: None,
                },
            )
            .await;
        }
    }
    dbtx.insert_entry(
        &FederationKey,
        &StoredFederation {
            fa_id,
            formation_id,
            federation_invite: payload.federation_invite,
            federation_name,
            seats: payload.seats.into_iter().map(Into::into).collect(),
        },
    )
    .await;
    Ok(())
}

/// Record the federation a local formation formed, in the transaction that
/// confirms its metadata.
async fn insert_formed_federation(
    dbtx: &mut DatabaseTransaction<'_>,
    creation: &StoredCreation,
    seats: &[FormationSeat],
) -> FaResult<()> {
    let federation_invite = creation
        .invite_code
        .clone()
        .ok_or_else(formed_backup_required)?;
    dbtx.insert_entry(
        &FederationKey,
        &StoredFederation {
            fa_id: creation.fa_id,
            formation_id: creation.formation_id.clone(),
            federation_invite,
            federation_name: Some(creation.federation_name.clone()),
            seats: seats
                .iter()
                .map(|(seat, _)| StoredFederationSeat {
                    mg_identity: seat.mg_id,
                    seat_id: seat.seat_id.clone(),
                    locator: seat.locator.clone(),
                })
                .collect(),
        },
    )
    .await;
    Ok(())
}

/// The pool entry for one created purchase seat.
fn pool_seat(seat: StoredPurchaseSeat) -> FaResult<StoredPoolSeat> {
    let index = seat.index;
    let missing =
        |fact: &str| FaError::Storage(format!("created FA seat row {index} has no {fact}"));
    let quote = seat
        .signed_quote
        .as_ref()
        .ok_or_else(|| missing("quote"))?
        .verify(&seat.locator.service_pubkey)
        .map_err(|error| {
            FaError::Storage(format!(
                "invalid quote for created FA seat row {index}: {error}"
            ))
        })?;
    Ok(StoredPoolSeat {
        mg_id: seat.admission.mg_id(),
        holder: seat.admission.holder(),
        fedimintd_version: quote.terms.request.fedimintd_version.clone(),
        federation_size: quote.terms.request.federation_size,
        locator: seat.locator,
        seat_id: seat.seat_id.ok_or_else(|| missing("seat id"))?,
        guardian_fee_account: seat
            .guardian_fee_account
            .ok_or_else(|| missing("guardian-fee account"))?,
    })
}

fn formed_backup_required() -> FaError {
    FaError::Storage("FA backup is available only after formed metadata is confirmed".to_owned())
}

fn public_phase(stored: StoredPurchasePhase, recoveries: &[PurchaseSeatRecovery]) -> PurchasePhase {
    match stored {
        StoredPurchasePhase::Initialized
            if !recoveries
                .iter()
                .all(|recovery| recovery.signed_quote.is_some()) =>
        {
            PurchasePhase::Preparing
        }
        StoredPurchasePhase::Initialized
        | StoredPurchasePhase::SeatsPartiallyCreated
        | StoredPurchasePhase::SeatsCreated => PurchasePhase::AcquiringSeats,
    }
}

fn validate_schema(formation: &StoredPurchase) -> FaResult<()> {
    if formation.schema_version != STORAGE_SCHEMA_VERSION {
        return Err(FaError::Storage(format!(
            "unsupported FA storage schema version {}",
            formation.schema_version,
        )));
    }
    if formation.fa_id.is_none() {
        return Err(FaError::Storage(
            "current FA storage record is missing its owner identity".to_owned(),
        ));
    }
    if !formation.intent.fedimintd_dkg_version.is_fedi()
        || !formation
            .intent
            .fedimintd_versions
            .overlaps_dkg(&formation.intent.fedimintd_dkg_version)
    {
        return Err(FaError::Storage(
            "persisted FA Fedimint DKG identity is outside its version policy".to_owned(),
        ));
    }
    Ok(())
}

fn validate_schema_and_owner(formation: &StoredPurchase, fa_id: FaId) -> FaResult<()> {
    validate_schema(formation)?;
    if formation.fa_id != Some(fa_id) {
        return Err(FaError::Storage(
            "persisted FA formation belongs to a different identity".to_owned(),
        ));
    }
    Ok(())
}

fn validate_active_formation(
    formation: &StoredPurchase,
    expected_id: &FormationId,
) -> FaResult<()> {
    validate_schema(formation)?;
    if formation.formation_id != *expected_id {
        return Err(FaError::Storage(
            "active FA formation changed during an operation".to_owned(),
        ));
    }
    Ok(())
}
