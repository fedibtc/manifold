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
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::Ordering;

use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_service_manifold_guardian::{
    DkgCompletionCallback, FaId, FederationId, FederationName, FederationSize, FedimintdDkgVersion,
    FedimintdVersion, FormationSeatBinding, GetQuoteResponse, GuardianCode, GuardianFeeAccount,
    InviteCode, Locator, Plan, SeatId, SignedResponse,
};
use nostr_sdk::{Event, PublicKey};
use stability_pool_common::Account;

use crate::{
    CreationSnapshot, CreationStage, FaError, FaResult, FormationId, FormationPhase, SeatPhase,
    SeatProgress, state::FaStatus, state::ResolvedSeatPurchase,
};

// Persisted-format changes are governed by GATE-production-compatibility
// (../../../specs/GATE-production-compatibility.md).
const STORAGE_SCHEMA_VERSION: u16 = 12;
const FORMATION_TX_MAX_ATTEMPTS: usize = 10;

#[repr(u8)]
enum FaDbPrefix {
    /// Legacy: the schema 11 formation row. Only migration reads it.
    LegacyFormation = 0x00,
    /// Legacy: the schema 11 seat rows. Only migration reads them.
    LegacySeat = 0x01,
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
    Slot = 0x0a,
    Creation = 0x0b,
    ReadInvite = 0x0d,
    Formation = 0x0e,
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

/// Guardian addresses from the last verified consensus of the federation
/// that `saved` names. They are only a connection hint, and they never enter
/// a backup.
#[derive(Debug, Decodable, Encodable)]
struct ReadInviteKey;
impl_db_record!(
    key = ReadInviteKey,
    value = StoredReadInvite,
    db_prefix = FaDbPrefix::ReadInvite
);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredReadInvite {
    saved: InviteCode,
    invite: InviteCode,
}

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

/// The federation the user asked to create. It exists from the first
/// purchase until the federation forms, and it keeps the inputs every later
/// purchase and the formation need. The formation runs once every slot holds
/// an accepted seat, and its progress lives here.
#[derive(Debug, Decodable, Encodable)]
struct CreationKey;

impl_db_record!(
    key = CreationKey,
    value = StoredCreation,
    db_prefix = FaDbPrefix::Creation,
);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredCreation {
    schema_version: u16,
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
    /// Why a slot last left the creation. A new purchase clears it.
    reauthorization: Option<crate::SelectionReauthorizationReason>,
}

#[derive(serde::Deserialize)]
struct StoredCreationEnvelope {
    schema_version: u16,
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

mod formation;
mod migration;
mod slot;

use formation::*;
pub(crate) use formation::{FormationMetaTarget, FormationSeat};
pub(crate) use slot::SlotState;
use slot::*;
json_database_value!(StoredCreation);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredSetupPaymentFederationsEvent(Event);

json_database_value!(StoredSetupPaymentFederationsEvent);
json_database_value!(crate::liquidity::StoredLiquidityOperation);
json_database_value!(BackupGenerationState);
json_database_value!(BackupRelayConfirmation);
json_database_value!(StoredFederation);
json_database_value!(StoredReadInvite);

/// The running purchase: the slots that hold no accepted seat yet.
pub(crate) struct PurchaseView {
    pub(crate) formation_id: FormationId,
    /// What every quote of the purchase must match.
    pub(crate) intent: ResolvedSeatPurchase,
    /// Explicit payer, absent for the zero-price bootstrap.
    pub(crate) payment_federation_id: Option<FederationId>,
    /// The picked or bound slots, in id order.
    pub(crate) slots: Vec<PendingSlot>,
}

/// One slot the running purchase still buys.
#[derive(Clone)]
pub(crate) struct PendingSlot {
    pub(crate) index: u16,
    pub(crate) locator: Locator,
    pub(crate) state: SlotState,
}

/// The formation of a full pool.
pub(crate) struct FormationView {
    pub(crate) formation_id: FormationId,
    pub(crate) federation_name: FederationName,
    /// Bearer callback handed to every MG only at `StartDkg`.
    pub(crate) dkg_completion_callback: Option<DkgCompletionCallback>,
    /// The pool, in slot id order. The first seat's code carries the
    /// federation name.
    pub(crate) seats: Vec<FormationSeat>,
    /// The agreed invite, once DKG completed.
    pub(crate) invite_code: Option<InviteCode>,
    /// Exact formation metadata target saved before publication.
    pub(crate) meta_target: Option<FormationMetaTarget>,
}

/// One MG a preview picked for an empty slot.
pub(crate) struct PickedSeat {
    pub(crate) locator: Locator,
    pub(crate) mg_id: PublicKey,
    pub(crate) holder: PublicKey,
    pub(crate) plan: Plan,
}

/// The inputs of a new federation creation.
pub(crate) struct NewCreation {
    pub(crate) formation_id: FormationId,
    pub(crate) federation_name: FederationName,
    pub(crate) federation_size: FederationSize,
    pub(crate) fedimintd_versions: crate::FedimintdVersionRange,
    pub(crate) plan: crate::PlanPreference,
    pub(crate) dkg_completion_callback: Option<DkgCompletionCallback>,
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
    #[cfg(test)]
    formation_commit_hook: Arc<std::sync::Mutex<Option<Arc<FormationTestHook>>>>,
    #[cfg(test)]
    seat_checkpoint_hook: SeatCheckpointTestHook,
}

impl FaStore {
    pub(crate) fn new(database: Database) -> Self {
        Self {
            database,
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
        if dbtx.get_value(&CreationKey).await.is_some()
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
    /// The guardian addresses last read for the federation `saved` names.
    pub(crate) async fn read_invite(&self, saved: &InviteCode) -> Option<InviteCode> {
        let mut dbtx = self.database.begin_transaction_nc().await;
        dbtx.get_value(&ReadInviteKey)
            .await
            .filter(|stored| &stored.saved == saved)
            .map(|stored| stored.invite)
    }

    pub(crate) async fn record_read_invite(
        &self,
        saved: InviteCode,
        invite: InviteCode,
    ) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        dbtx.insert_entry(&ReadInviteKey, &StoredReadInvite { saved, invite })
            .await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("committing the FA read invite failed".to_owned()))
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
    /// Start a purchase and, when none exists, the creation `formation_id`
    /// it belongs to.
    pub(crate) async fn initialize_for_test(
        &self,
        fa_id: FaId,
        formation_id: FormationId,
        intent: ResolvedSeatPurchase,
        seats: Vec<PickedSeat>,
        payment_federation_id: Option<FederationId>,
    ) -> FaResult<()> {
        let creation = match self.load_stored_creation(fa_id).await? {
            Some(_) => None,
            None => Some(NewCreation {
                formation_id,
                federation_name: FederationName("Test Federation".to_owned()),
                federation_size: intent.federation_size,
                fedimintd_versions: intent.fedimintd_versions.clone(),
                plan: intent.plan,
                dkg_completion_callback: None,
            }),
        };
        self.initialize(
            fa_id,
            intent.fedimintd_dkg_version,
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
        Ok(FaStatus {
            recovery: None,
            creation: self.load_snapshot(fa_id).await?,
            federation: self.load_federation(fa_id).await?,
        })
    }

    /// The validated creation of `fa_id`, if any.
    async fn load_stored_creation(&self, fa_id: FaId) -> FaResult<Option<Creation>> {
        let mut dbtx = self.database.begin_transaction_nc().await;
        let creation = read_creation(&mut dbtx.to_ref_nc(), fa_id).await?;
        if let Some(creation) = &creation {
            formation_seats(creation)?;
        }
        Ok(creation)
    }

    /// The public projection of the running creation, if any.
    pub(crate) async fn load_snapshot(&self, fa_id: FaId) -> FaResult<Option<CreationSnapshot>> {
        self.load_stored_creation(fa_id)
            .await?
            .map(|creation| creation.snapshot())
            .transpose()
    }

    /// The running purchase, if a slot holds no accepted seat.
    pub(crate) async fn load_purchase(&self, fa_id: FaId) -> FaResult<Option<PurchaseView>> {
        Ok(self
            .load_stored_creation(fa_id)
            .await?
            .and_then(|creation| creation.purchase_view()))
    }

    /// The formation, if the pool is full.
    pub(crate) async fn load_formation(&self, fa_id: FaId) -> FaResult<Option<FormationView>> {
        match self.load_stored_creation(fa_id).await? {
            Some(creation) => creation.formation_view(),
            None => Ok(None),
        }
    }

    /// End a federation creation that holds no accepted seat or formation,
    /// with its running purchase.
    pub(crate) async fn cancel_creation(&self, fa_id: FaId) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let creation = read_creation(&mut dbtx.to_ref_nc(), fa_id)
            .await?
            .ok_or(FaError::NoActiveFormation)?;
        if creation.accepted().next().is_some() {
            return Err(FaError::AbandonUnavailable(
                crate::AbandonUnavailableReason::SeatsBought,
            ));
        }
        for (index, _) in &creation.slots {
            dbtx.remove_entry(&SlotKey { index: *index }).await;
        }
        dbtx.remove_entry(&CreationKey).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("cancelling FA federation creation failed".to_owned()))
    }

    /// The request whose preview fills the creation's empty slots. It
    /// keeps the DKG line of the bought seats, and avoids the MGs, service
    /// keys and badge holders of the bought seats.
    pub(crate) async fn missing_seats_request(
        &self,
        fa_id: FaId,
    ) -> FaResult<crate::MgSelectionRequest> {
        let creation = self
            .load_stored_creation(fa_id)
            .await?
            .ok_or(FaError::NoActiveFormation)?;
        let accepted = creation.accepted().collect::<Vec<_>>();
        let bought_count = u16::try_from(accepted.len()).expect("slots fit the federation size");
        let taken = accepted
            .iter()
            .map(|(_, seat)| (seat.slot.mg_id, &seat.slot.locator, seat.slot.holder))
            .collect::<Vec<_>>();
        let bought = crate::selection::BoughtSeats {
            fedimintd_dkg_version: accepted
                .first()
                .map(|(_, seat)| seat.fedimintd_version.dkg_version()),
            mg_ids: taken.iter().map(|(mg_id, _, _)| *mg_id).collect(),
            service_pubkeys: taken
                .iter()
                .map(|(mg_id, locator, _)| (locator.service_pubkey, *mg_id))
                .collect(),
            holders: taken.iter().filter_map(|(_, _, holder)| *holder).collect(),
        };
        let stored = &creation.stored;
        Ok(crate::MgSelectionRequest::new(
            stored.federation_size,
            stored.fedimintd_versions.clone(),
            stored.plan,
        )?
        .missing_seats(bought, bought_count))
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

    /// Start a purchase of empty slots, and the creation it belongs to when
    /// `creation` is given. Each picked MG takes a new slot id.
    pub(crate) async fn initialize(
        &self,
        fa_id: FaId,
        fedimintd_dkg_version: FedimintdDkgVersion,
        seats: Vec<PickedSeat>,
        payment_federation_id: Option<FederationId>,
        creation: Option<NewCreation>,
    ) -> FaResult<()> {
        if seats.is_empty() {
            return Err(FaError::InvalidIntent(
                "a seat purchase needs at least one seat".to_owned(),
            ));
        }
        let purchase = SlotPurchase {
            fedimintd_dkg_version,
            payment_federation_id,
        };
        let mut dbtx = self.database.begin_transaction().await;
        let existing = read_creation(&mut dbtx.to_ref_nc(), fa_id).await?;
        let mut creation = match (creation, existing) {
            (Some(creation), None) => {
                if dbtx.get_value(&FederationKey).await.is_some() {
                    return Err(FaError::InvalidIntent(
                        "this FA already creates or governs a federation".to_owned(),
                    ));
                }
                Creation {
                    stored: StoredCreation {
                        schema_version: STORAGE_SCHEMA_VERSION,
                        fa_id,
                        federation_name: creation.federation_name,
                        federation_size: creation.federation_size,
                        fedimintd_versions: creation.fedimintd_versions,
                        plan: creation.plan,
                        dkg_completion_callback: creation.dkg_completion_callback,
                        formation_id: creation.formation_id,
                        reauthorization: None,
                    },
                    slots: Vec::new(),
                    formation: None,
                }
            }
            (Some(_), Some(_)) => {
                return Err(FaError::InvalidIntent(
                    "this FA already creates or governs a federation".to_owned(),
                ));
            }
            (None, None) => {
                return Err(FaError::InvalidIntent(
                    "no federation creation needs seats".to_owned(),
                ));
            }
            (None, Some(existing)) => existing,
        };
        if creation.pending().next().is_some() {
            return Err(FaError::InvalidIntent(
                "a seat purchase is already running".to_owned(),
            ));
        }
        creation.stored.reauthorization = None;
        if creation.slots.len() + seats.len() > usize::from(creation.stored.federation_size.0) {
            return Err(FaError::InvalidIntent(
                "the purchase has more seats than empty slots".to_owned(),
            ));
        }
        // A new slot takes an id after every existing slot.
        let first = creation.slots.last().map_or(0, |(index, _)| index + 1);
        for (index, seat) in (first..).zip(seats) {
            creation.slots.push((
                index,
                StoredSlot {
                    locator: seat.locator,
                    mg_id: seat.mg_id,
                    holder: Some(seat.holder),
                    state: SlotState::Picked {
                        plan: seat.plan,
                        purchase: purchase.clone(),
                    },
                },
            ));
        }
        creation.slots.sort_by_key(|(index, _)| *index);
        validate_creation(&creation)?;
        for (index, slot) in &creation.slots {
            dbtx.insert_entry(&SlotKey { index: *index }, slot).await;
        }
        dbtx.insert_entry(&CreationKey, &creation.stored).await;
        // The run guard serializes initialization before any seat work can
        // run, so this transaction cannot encounter an expected RMW conflict.
        dbtx.commit_tx_result().await.map_err(|_| {
            FaError::Storage("initializing FA seat purchase transaction failed".to_owned())
        })?;
        Ok(())
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

/// The creation and its slots, in index order.
struct Creation {
    stored: StoredCreation,
    slots: Vec<(u16, StoredSlot)>,
    /// Present once DKG completed.
    formation: Option<StoredFormation>,
}

impl Creation {
    fn accepted(&self) -> impl Iterator<Item = (u16, AcceptedSlot<'_>)> {
        self.slots
            .iter()
            .filter_map(|(index, slot)| Some((*index, slot.accepted()?)))
    }

    fn pending(&self) -> impl Iterator<Item = (u16, &StoredSlot)> {
        self.slots
            .iter()
            .filter(|(_, slot)| slot.accepted().is_none())
            .map(|(index, slot)| (*index, slot))
    }

    fn slot_mut(&mut self, index: u16) -> FaResult<&mut StoredSlot> {
        self.slots
            .iter_mut()
            .find(|(slot, _)| *slot == index)
            .map(|(_, slot)| slot)
            .ok_or_else(|| FaError::Storage(format!("missing persisted FA slot {index}")))
    }

    /// The running purchase, while a slot holds no accepted seat.
    fn purchase_view(&self) -> Option<PurchaseView> {
        let slots = self
            .pending()
            .map(|(index, slot)| PendingSlot {
                index,
                locator: slot.locator.clone(),
                state: slot.state.clone(),
            })
            .collect::<Vec<_>>();
        // Every pending slot carries the same purchase.
        let purchase = slots.first()?.state.purchase()?.clone();
        let stored = &self.stored;
        Some(PurchaseView {
            formation_id: stored.formation_id.clone(),
            intent: ResolvedSeatPurchase {
                federation_size: stored.federation_size,
                plan: stored.plan,
                fedimintd_versions: stored.fedimintd_versions.clone(),
                fedimintd_dkg_version: purchase.fedimintd_dkg_version,
            },
            payment_federation_id: purchase.payment_federation_id,
            slots,
        })
    }

    /// The formation, once `size` slots hold accepted seats.
    fn formation_view(&self) -> FaResult<Option<FormationView>> {
        let Some(seats) = formation_seats(self)? else {
            return Ok(None);
        };
        let stored = &self.stored;
        Ok(Some(FormationView {
            formation_id: stored.formation_id.clone(),
            federation_name: stored.federation_name.clone(),
            dkg_completion_callback: stored.dkg_completion_callback.clone(),
            seats,
            invite_code: self
                .formation
                .as_ref()
                .map(|formation| formation.invite_code.clone()),
            meta_target: self
                .formation
                .as_ref()
                .and_then(|formation| formation.meta_target.clone()),
        }))
    }

    /// The public projection of the stored creation.
    fn snapshot(&self) -> FaResult<CreationSnapshot> {
        let stored = &self.stored;
        let formation = self.formation_view()?;
        // StartDkg follows once every code is saved.
        let dkg_requested = formation.as_ref().is_some_and(|formation| {
            formation
                .seats
                .iter()
                .all(|seat| seat.guardian_code.is_some())
        });
        let slots = self
            .slots
            .iter()
            .map(|(index, slot)| SeatProgress {
                index: *index,
                mg_id: slot.mg_id,
                locator: slot.locator.clone(),
                seat_id: match &slot.state {
                    SlotState::Accepted { seat_id, .. } => Some(seat_id.clone()),
                    SlotState::Picked { .. } | SlotState::Bound { .. } => None,
                },
                guardian_code: slot.state.guardian_code().cloned(),
                phase: match &slot.state {
                    SlotState::Picked { .. } => SeatPhase::Selected,
                    SlotState::Bound { .. } => SeatPhase::QuoteReady,
                    SlotState::Accepted { .. } if self.formation.is_some() => SeatPhase::Running,
                    SlotState::Accepted { .. } if dkg_requested => SeatPhase::DkgUnderway,
                    SlotState::Accepted {
                        guardian_code: Some(_),
                        ..
                    } => SeatPhase::GuardianCodeReady,
                    SlotState::Accepted { .. } => SeatPhase::Created,
                },
            })
            .collect();
        let purchase = self
            .slots
            .iter()
            .find_map(|(_, slot)| slot.state.purchase());
        let stage = if purchase.is_some() {
            CreationStage::Buying
        } else if let Some(formation) = &formation {
            CreationStage::Forming {
                phase: match (&formation.invite_code, &formation.meta_target) {
                    (Some(_), Some(_)) => FormationPhase::PublishingSeatBindings,
                    (Some(_), None) => FormationPhase::DkgComplete,
                    (None, _) if dkg_requested => FormationPhase::DkgUnderway,
                    (None, _) => FormationPhase::PreparingDkg,
                },
                invite_code: formation.invite_code.clone(),
            }
        } else {
            CreationStage::NeedsSeats {
                count: stored
                    .federation_size
                    .0
                    .saturating_sub(u16::try_from(self.slots.len()).unwrap_or(u16::MAX)),
                reauthorization: stored.reauthorization,
            }
        };
        Ok(CreationSnapshot {
            formation_id: stored.formation_id.clone(),
            federation_name: stored.federation_name.clone(),
            federation_size: stored.federation_size,
            fedimintd_dkg_version: purchase
                .map(|purchase| purchase.fedimintd_dkg_version.clone())
                .or_else(|| {
                    self.accepted()
                        .next()
                        .map(|(_, seat)| seat.fedimintd_version.dkg_version())
                }),
            slots,
            stage,
            last_error: None,
        })
    }
}

/// Read and validate the creation of `fa_id` and its slots.
async fn read_creation(
    dbtx: &mut DatabaseTransaction<'_>,
    fa_id: FaId,
) -> FaResult<Option<Creation>> {
    let Some(creation) = read_any_creation(dbtx).await? else {
        return Ok(None);
    };
    if creation.stored.fa_id != fa_id {
        return Err(FaError::Storage(
            "persisted FA creation belongs to a different identity".to_owned(),
        ));
    }
    Ok(Some(creation))
}

async fn read_any_creation(dbtx: &mut DatabaseTransaction<'_>) -> FaResult<Option<Creation>> {
    let Some(bytes) = dbtx
        .raw_get_bytes(&DatabaseKeyPrefix::to_bytes(&CreationKey))
        .await
        .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
    else {
        return Ok(None);
    };
    let envelope: StoredCreationEnvelope = serde_json::from_slice(&bytes)
        .map_err(|_| FaError::Storage("persisted FA creation header is malformed".to_owned()))?;
    if envelope.schema_version != STORAGE_SCHEMA_VERSION {
        return Err(FaError::Storage(format!(
            "unsupported FA storage schema version {}",
            envelope.schema_version
        )));
    }
    let stored = dbtx
        .get_value(&CreationKey)
        .await
        .ok_or_else(|| FaError::Storage("persisted FA creation is malformed".to_owned()))?;
    let slots = dbtx
        .find_by_prefix(&SlotKeyPrefix)
        .await
        .map(|(key, slot)| (key.index, slot))
        .collect::<Vec<_>>()
        .await;
    let formation = dbtx.get_value(&FormationKey).await;
    let mut creation = Creation {
        stored,
        slots,
        formation,
    };
    creation.slots.sort_by_key(|(index, _)| *index);
    validate_creation(&creation)?;
    Ok(Some(creation))
}

/// Reject a creation whose slots disagree with each other or with it.
fn validate_creation(creation: &Creation) -> FaResult<()> {
    let stored = &creation.stored;
    let size = stored.federation_size.0;
    let mut indexes = BTreeSet::new();
    let mut mg_ids = BTreeSet::new();
    let mut holders = BTreeSet::new();
    let mut service_pubkeys = BTreeMap::new();
    let mut dkg_versions = Vec::new();
    for (index, slot) in &creation.slots {
        if !indexes.insert(*index) {
            return Err(FaError::Storage(format!(
                "persisted FA slot {index} is duplicated"
            )));
        }
        if let Some(holder) = slot.holder
            && !holders.insert(holder)
        {
            return Err(FaError::Storage(
                "persisted FA slots contain duplicate badge holders".to_owned(),
            ));
        }
        if !mg_ids.insert(slot.mg_id) {
            return Err(FaError::Storage(format!(
                "persisted FA slot {index} duplicates an admitted MG identity"
            )));
        }
        if let Some(existing) = service_pubkeys.insert(slot.locator.service_pubkey, index) {
            return Err(FaError::Storage(format!(
                "persisted FA slot {index} duplicates the service signing key of slot \
                 {existing}"
            )));
        }
        match &slot.state {
            SlotState::Picked { .. } => {}
            SlotState::Bound { quote, .. } => {
                quote
                    .verify(&slot.locator.service_pubkey)
                    .map_err(|error| {
                        FaError::Storage(format!(
                            "invalid bound quote for FA slot {index}: {error}"
                        ))
                    })?;
            }
            SlotState::Accepted {
                guardian_fee_account,
                fedimintd_version,
                ..
            } => {
                if GuardianFeeAccount::try_from(guardian_fee_account.clone()).is_err() {
                    return Err(FaError::Storage(format!(
                        "persisted FA slot {index} has an invalid signed guardian-fee account"
                    )));
                }
                dkg_versions.push(fedimintd_version.dkg_version());
            }
        }
    }
    if creation.slots.len() > usize::from(size) {
        return Err(FaError::Storage(
            "persisted FA creation has more slots than guardians".to_owned(),
        ));
    }
    let mut purchases = creation
        .slots
        .iter()
        .filter_map(|(_, slot)| slot.state.purchase());
    if let Some(purchase) = purchases.next() {
        if purchases.any(|other| other != purchase) {
            return Err(FaError::Storage(
                "persisted FA slots belong to different purchases".to_owned(),
            ));
        }
        if !purchase.fedimintd_dkg_version.is_fedi()
            || !stored
                .fedimintd_versions
                .overlaps_dkg(&purchase.fedimintd_dkg_version)
        {
            return Err(FaError::Storage(
                "persisted FA Fedimint DKG identity is outside its version policy".to_owned(),
            ));
        }
        dkg_versions.push(purchase.fedimintd_dkg_version.clone());
    }
    if dkg_versions.windows(2).any(|pair| pair[0] != pair[1]) {
        return Err(FaError::Storage(
            "FA creation seats must be bought for one DKG line".to_owned(),
        ));
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
    federation_invite: &InviteCode,
    seats: &[FormationSeat],
) {
    dbtx.insert_entry(
        &FederationKey,
        &StoredFederation {
            fa_id: creation.fa_id,
            formation_id: creation.formation_id.clone(),
            federation_invite: federation_invite.clone(),
            federation_name: Some(creation.federation_name.clone()),
            seats: seats
                .iter()
                .map(|seat| StoredFederationSeat {
                    mg_identity: seat.mg_id,
                    seat_id: seat.seat_id.clone(),
                    locator: seat.locator.clone(),
                })
                .collect(),
        },
    )
    .await;
}

fn formed_backup_required() -> FaError {
    FaError::Storage("FA backup is available only after formed metadata is confirmed".to_owned())
}
