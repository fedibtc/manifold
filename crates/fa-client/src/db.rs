//! Fedimint database representation for FA recovery facts and checkpoints.

#[cfg(test)]
pub(crate) mod test_support;

use bitcoin_hashes::sha256;
use fedimint_core::db::{
    AutocommitError, Database, DatabaseError, DatabaseKeyPrefix, DatabaseTransaction,
    DatabaseValue, DecodingError, IDatabaseTransactionOpsCore as _,
    IDatabaseTransactionOpsCoreTyped as _,
};
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::registry::ModuleDecoderRegistry;
use fedimint_core::{impl_db_lookup, impl_db_record};
use futures::StreamExt as _;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

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
    PaymentAuthorizationId, PaymentRequirements, PaymentReservationId, PurchasePhase,
    ResolvedSeatPurchase, SeatPaymentRequirement, SeatPhase, SeatProgress, SeatPurchaseSnapshot,
    state::FaStatus,
};

// Persisted-format changes are governed by GATE-production-compatibility
// (../../../specs/GATE-production-compatibility.md).
const STORAGE_SCHEMA_VERSION: u16 = 12;
// Keep this fixed when advancing the schema written by current code.
const RESET_BEFORE_SCHEMA_VERSION: u16 = 11;
const FORMATION_TX_MAX_ATTEMPTS: usize = 10;

#[derive(Clone, Copy)]
enum QuoteClearDisposition {
    RefreshSameGuardian,
    DropSeat,
}

#[repr(u8)]
enum FaDbPrefix {
    Purchase = 0x00,
    PurchaseSeat = 0x01,
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

#[derive(Debug, Decodable, Encodable)]
struct RestoredBackupKey;
impl_db_record!(
    key = RestoredBackupKey,
    value = RestoredBackupState,
    db_prefix = FaDbPrefix::RestoredBackup
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

/// Legacy: an authenticated backup imported but not yet reconciled against
/// consensus. `backup_eligible` marks one that was reconciled before the
/// federation record existed.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct RestoredBackupState {
    #[serde(rename = "fi_id")]
    fa_id: FaId,
    payload: crate::backup::FaBackupPayload,
    formation_id: FormationId,
    backup_eligible: bool,
    federation_name: Option<crate::FederationName>,
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
struct DriverLeaseKey;

impl_db_record!(
    key = DriverLeaseKey,
    value = StoredDriverLease,
    db_prefix = FaDbPrefix::DriverLease,
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
    #[serde(default)]
    payment_authorization: Option<StoredPaymentAuthorization>,
    /// Deterministic wallet reservation persisted after the wallet's
    /// idempotent reserve succeeds and before the output tombstone is armed.
    /// Explicit `null` means no reservation has been checkpointed; omission
    /// is corrupt because it cannot prove whether a journal was orphaned.
    #[serde(deserialize_with = "deserialize_required_option")]
    payment_reservation_id: Option<PaymentReservationId>,
    /// Whether the driver durably committed to releasing the reservation
    /// above immediately before calling the wallet. While set, authoritative
    /// wallet absence is the expected outcome of that release rather than
    /// proof of an orphaned journal, so an interrupted abandon completes on a
    /// later run instead of retaining the formation forever. Meaningful only beside a durable
    /// reservation id; adopting a live reservation clears it. A record
    /// persisted before this field decodes to the absent commitment.
    #[serde(default)]
    payment_reservation_release_intended: bool,
    /// Monotone boundary armed immediately before the first wallet output
    /// generation future is polled. Unlike commercial authorization, this is
    /// the value-safety gate for payer switching and abandon.
    payment_outputs_started: bool,
}

#[derive(serde::Deserialize)]
struct StoredPurchaseEnvelope {
    schema_version: u16,
}

/// The formation fields of a schema 11 formation row that schema 12 moved
/// out of the purchase.
#[derive(serde::Deserialize)]
struct Schema11Formation {
    phase: Schema11Phase,
    intent: Schema11Intent,
    #[serde(default)]
    invite_code: Option<InviteCode>,
    #[serde(default)]
    formation_meta_target: Option<FormationMetaTarget>,
    #[serde(default)]
    dkg_completion_callback: Option<DkgCompletionCallback>,
}

#[derive(serde::Deserialize)]
struct Schema11Intent {
    federation_name: FederationName,
}

#[derive(PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Schema11Phase {
    Initialized,
    SeatsPartiallyCreated,
    SeatsCreated,
    DkgComplete,
    PublishingSeatBindings,
    Formed,
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer)
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

    pub(crate) fn mark_effect_authorized(
        &mut self,
        quote_id: QuoteId,
        effect: AdmissionEffect,
    ) -> FaResult<()> {
        self.authorize_effect(quote_id, effect)
    }

    pub(crate) fn validate_if_fresh(
        &self,
        expected_provenance: StoredVerifierProvenance,
        now: Timestamp,
    ) -> FaResult<()> {
        validate_fresh_admission(self, expected_provenance, now)
    }

    fn authorize_effect(&mut self, quote_id: QuoteId, effect: AdmissionEffect) -> FaResult<()> {
        match self.state {
            AdmissionState::Fresh => {
                self.state = AdmissionState::EffectAuthorized { quote_id, effect };
                Ok(())
            }
            AdmissionState::EffectAuthorized {
                quote_id: current_quote,
                effect: current_effect,
            } if current_quote == quote_id && current_effect == effect => Ok(()),
            AdmissionState::EffectAuthorized { .. } => Err(FaError::Storage(
                "selected-seat admission is already bound to another effect".to_owned(),
            )),
        }
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

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct StoredPaymentAuthorization {
    authorization_id: PaymentAuthorizationId,
    quotes: Vec<QuoteAuthorization>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredDriverLease {
    owner: [u8; 32],
    expires_at: u64,
}

json_database_value!(StoredDriverLease);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredSetupPaymentFederationsEvent(Event);

json_database_value!(StoredSetupPaymentFederationsEvent);
json_database_value!(crate::liquidity::StoredLiquidityOperation);
json_database_value!(BackupGenerationState);
json_database_value!(BackupRelayConfirmation);
json_database_value!(RestoredBackupState);
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
    payment_authorization: Option<StoredPaymentAuthorization>,
    /// Exact aggregate reservation owned by the wallet, if established.
    pub(crate) payment_reservation_id: Option<PaymentReservationId>,
    /// Whether a durable release commitment makes wallet absence of the
    /// reservation above an expected cleanup outcome.
    pub(crate) payment_reservation_release_intended: bool,
    /// Whether wallet output generation has crossed its durable boundary.
    pub(crate) payment_outputs_started: bool,
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
impl PurchaseRecovery {
    /// Expose the durable aggregate authorization to recovery-policy tests.
    pub(crate) fn test_payment_authorization(
        &self,
    ) -> Option<(&PaymentAuthorizationId, &[QuoteAuthorization])> {
        self.payment_authorization.as_ref().map(|authorization| {
            (
                &authorization.authorization_id,
                authorization.quotes.as_slice(),
            )
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct QuoteAuthorization {
    pub(crate) index: u16,
    pub(crate) quote_id: QuoteId,
}

pub(crate) struct DriverLease {
    owner: [u8; 32],
    max_expires_at: u64,
    renewal_duration: std::time::Duration,
    store: FaStore,
    renewal_lock: Mutex<()>,
    released: AtomicBool,
}

impl DriverLease {
    /// Renew this driver's ownership before an external effect.
    pub(crate) async fn renew(&self) -> FaResult<()> {
        let _guard = self.renewal_lock.lock().await;
        self.store.renew_driver_lease(self).await
    }

    /// Atomically renew this owner while arming the wallet-output boundary.
    pub(crate) async fn arm_payment_outputs_started(
        &self,
        formation_id: &FormationId,
        verifier_provenance: StoredVerifierProvenance,
    ) -> FaResult<()> {
        let _guard = self.renewal_lock.lock().await;
        self.store
            .record_payment_outputs_started(formation_id, verifier_provenance, self)
            .await
    }

    /// Atomically renew this owner and consume one exact selected effect wave.
    pub(crate) async fn authorize_seat_effects(
        &self,
        formation_id: &FormationId,
        effects: &[(u16, QuoteId, AdmissionEffect)],
        verifier_provenance: StoredVerifierProvenance,
    ) -> FaResult<()> {
        let _guard = self.renewal_lock.lock().await;
        self.store
            .record_seat_effects_authorized(formation_id, effects, verifier_provenance, self)
            .await
    }

    /// Renew this owner while atomically wiping a still-abandonable formation.
    pub(crate) async fn abandon_purchase(&self, formation_id: &FormationId) -> FaResult<()> {
        let _guard = self.renewal_lock.lock().await;
        self.store.abandon_purchase(formation_id, self).await
    }
}

#[cfg(test)]
type LeaseTestHook =
    Arc<std::sync::Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>>;

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
    lease_clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    #[cfg(test)]
    lease_acquisition_hook: LeaseTestHook,
    #[cfg(test)]
    lease_renewal_commit_hook: LeaseTestHook,
    #[cfg(test)]
    lease_release_commit_hook: LeaseTestHook,
    #[cfg(test)]
    formation_commit_hook: Arc<std::sync::Mutex<Option<Arc<FormationTestHook>>>>,
    #[cfg(test)]
    seat_checkpoint_hook: SeatCheckpointTestHook,
    #[cfg(test)]
    failed_seat_checkpoint: Arc<std::sync::Mutex<Option<u16>>>,
    #[cfg(test)]
    failed_quote_clear: Arc<std::sync::Mutex<Option<u16>>>,
    #[cfg(test)]
    failed_abandon: Arc<AtomicBool>,
}

impl FaStore {
    pub(crate) fn new(database: Database) -> Self {
        Self {
            database,
            lease_clock: Arc::new(|| fedimint_core::time::duration_since_epoch().as_secs()),
            #[cfg(test)]
            lease_acquisition_hook: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            lease_renewal_commit_hook: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            lease_release_commit_hook: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            formation_commit_hook: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            seat_checkpoint_hook: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            failed_seat_checkpoint: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            failed_quote_clear: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            failed_abandon: Arc::new(AtomicBool::new(false)),
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
    pub(crate) fn new_with_lease_clock(
        database: Database,
        lease_clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            database,
            lease_clock,
            lease_acquisition_hook: Arc::new(std::sync::Mutex::new(None)),
            lease_renewal_commit_hook: Arc::new(std::sync::Mutex::new(None)),
            lease_release_commit_hook: Arc::new(std::sync::Mutex::new(None)),
            formation_commit_hook: Arc::new(std::sync::Mutex::new(None)),
            seat_checkpoint_hook: Arc::new(std::sync::Mutex::new(None)),
            failed_seat_checkpoint: Arc::new(std::sync::Mutex::new(None)),
            failed_quote_clear: Arc::new(std::sync::Mutex::new(None)),
            failed_abandon: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(test)]
    /// Fail the next accepted-seat checkpoint for `index`.
    pub(crate) fn fail_seat_checkpoint(&self, index: u16) {
        *self.failed_seat_checkpoint.lock().expect("test lock") = Some(index);
    }

    #[cfg(test)]
    /// Fail the next exact-quote clear for `index`.
    pub(crate) fn fail_quote_clear(&self, index: u16) {
        *self.failed_quote_clear.lock().expect("test lock") = Some(index);
    }

    #[cfg(test)]
    /// Fail the next lease-fenced abandon before its durable transaction.
    pub(crate) fn fail_abandon_once(&self) {
        self.failed_abandon.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn set_lease_acquisition_hook(
        &self,
        reached: Arc<tokio::sync::Notify>,
        proceed: Arc<tokio::sync::Notify>,
    ) {
        *self
            .lease_acquisition_hook
            .lock()
            .expect("test lease hook lock") = Some((reached, proceed));
    }

    #[cfg(test)]
    pub(crate) fn set_lease_renewal_commit_hook(
        &self,
        reached: Arc<tokio::sync::Notify>,
        proceed: Arc<tokio::sync::Notify>,
    ) {
        *self
            .lease_renewal_commit_hook
            .lock()
            .expect("test lease hook lock") = Some((reached, proceed));
    }

    #[cfg(test)]
    pub(crate) fn set_lease_release_commit_hook(
        &self,
        reached: Arc<tokio::sync::Notify>,
        proceed: Arc<tokio::sync::Notify>,
    ) {
        *self
            .lease_release_commit_hook
            .lock()
            .expect("test lease hook lock") = Some((reached, proceed));
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
    /// Return whether the current schema fails closed when one required JSON
    /// recovery discriminator is removed.
    pub(crate) async fn rejects_missing_recovery_field_for_test(&self, field: &str) -> bool {
        let mut dbtx = self.database.begin_transaction_nc().await;
        let formation = dbtx
            .get_value(&PurchaseKey)
            .await
            .expect("test formation exists");
        let mut value = serde_json::to_value(formation).expect("stored formation serializes");
        value
            .as_object_mut()
            .expect("stored formation is an object")
            .remove(field);
        serde_json::from_value::<StoredPurchase>(value).is_err()
    }

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

    /// Bring storage written by older releases to the current schema.
    pub(crate) async fn migrate_storage(&self) -> FaResult<()> {
        self.split_schema_11_formation().await?;
        let mut dbtx = self.database.begin_transaction().await;
        let Some(restored) = dbtx.get_value(&RestoredBackupKey).await else {
            return Ok(());
        };
        dbtx.remove_entry(&RestoredBackupKey).await;
        // A reconciled legacy import becomes the record. An unreconciled one
        // is dropped, and the next restored open looks it up again.
        if restored.backup_eligible {
            adopt_backup(
                &mut dbtx.to_ref_nc(),
                restored.fa_id,
                restored.formation_id,
                restored.payload,
                restored.federation_name,
            )
            .await?;
        } else {
            dbtx.remove_by_prefix(&RecoveryCompletedKeyPrefix).await;
        }
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("migrating restored FA backup failed".to_owned()))
    }

    /// Schema 11 kept a formation and its seat purchase in one row. A
    /// purchase that has not created every seat stays a purchase. From
    /// `seats_created` on, the seats move into the pool and the formation
    /// progress into the creation record.
    async fn split_schema_11_formation(&self) -> FaResult<()> {
        let malformed =
            || FaError::Storage("persisted schema 11 FA formation is malformed".to_owned());
        let mut dbtx = self.database.begin_transaction().await;
        let Some(bytes) = dbtx
            .raw_get_bytes(&DatabaseKeyPrefix::to_bytes(&PurchaseKey))
            .await
            .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
        else {
            return Ok(());
        };
        let mut header: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| malformed())?;
        if header["schema_version"] != 11 {
            return Ok(());
        }
        let legacy: Schema11Formation =
            serde_json::from_value(header.clone()).map_err(|_| malformed())?;
        let object = header.as_object_mut().ok_or_else(malformed)?;
        for field in [
            "invite_code",
            "formation_meta_target",
            "dkg_completion_callback",
        ] {
            object.remove(field);
        }
        object.insert("schema_version".to_owned(), STORAGE_SCHEMA_VERSION.into());
        // Schema 12 keeps only the selected payer. Pinned formations were a
        // development path with no authenticated MG identities.
        let payment_federation_id = match object.remove("creation_mode") {
            Some(serde_json::Value::Object(mut mode)) => mode
                .remove("selected")
                .ok_or_else(|| {
                    FaError::Storage(
                        "a schema 11 pinned FA formation cannot be migrated; reset this FA \
                         namespace"
                            .to_owned(),
                    )
                })?
                .get_mut("payment_federation_id")
                .map(serde_json::Value::take),
            Some(serde_json::Value::String(mode)) if mode == "pinned" => {
                return Err(FaError::Storage(
                    "a schema 11 pinned FA formation cannot be migrated; reset this FA namespace"
                        .to_owned(),
                ));
            }
            _ => return Err(malformed()),
        };
        if let Some(payment_federation_id) = payment_federation_id {
            object.insert("payment_federation_id".to_owned(), payment_federation_id);
        }
        let in_purchase = matches!(
            legacy.phase,
            Schema11Phase::Initialized | Schema11Phase::SeatsPartiallyCreated
        );
        if !in_purchase {
            object.insert("phase".to_owned(), "seats_created".into());
        }
        let mut purchase: StoredPurchase =
            serde_json::from_value(header).map_err(|_| malformed())?;
        let mut seats = Vec::with_capacity(usize::from(purchase.seat_count));
        for index in 0..purchase.seat_count {
            let key = PurchaseSeatKey {
                formation_id: purchase.formation_id.0.clone(),
                index,
            };
            let bytes = dbtx
                .raw_get_bytes(&DatabaseKeyPrefix::to_bytes(&key))
                .await
                .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
                .ok_or_else(|| {
                    FaError::Storage(format!("missing persisted FA seat row {index}"))
                })?;
            let mut seat: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| malformed())?;
            let seat_object = seat.as_object_mut().ok_or_else(malformed)?;
            let admission = seat_object
                .get_mut("admission")
                .and_then(|admission| admission.get_mut("peer_badge"))
                .ok_or_else(malformed)?
                .take();
            seat_object.insert("admission".to_owned(), admission);
            let guardian_code: Option<GuardianCode> = seat_object
                .remove("guardian_code")
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| malformed())?
                .flatten();
            // Schema 12 has no guardian replacement. A seat that waited for
            // one ended after payment, so it leaves the purchase. An applied
            // replacement is an ordinary seat of its new MG.
            let ended = !seat_object
                .get("replacement_for")
                .unwrap_or(&serde_json::Value::Null)
                .is_null()
                && seat_object.get("replacement_approved") != Some(&serde_json::Value::Bool(true));
            let seat: StoredPurchaseSeat = serde_json::from_value(seat).map_err(|_| malformed())?;
            if ended {
                dbtx.raw_remove_entry(&DatabaseKeyPrefix::to_bytes(&key))
                    .await
                    .map_err(|_| FaError::Storage("removing a schema 11 seat failed".to_owned()))?;
                continue;
            }
            seats.push((key, seat, guardian_code));
        }
        // Schema 12 keeps the user's creation inputs and formation progress
        // in their own record until the federation forms.
        let creation = StoredCreation {
            fa_id: purchase.fa_id.ok_or_else(malformed)?,
            federation_name: legacy.intent.federation_name,
            federation_size: purchase.intent.federation_size,
            fedimintd_versions: purchase.intent.fedimintd_versions.clone(),
            plan: purchase.intent.plan,
            dkg_completion_callback: legacy.dkg_completion_callback,
            formation_id: purchase.formation_id.clone(),
            invite_code: legacy.invite_code,
            formation_meta_target: legacy.formation_meta_target,
        };
        if in_purchase {
            purchase.seat_count =
                u16::try_from(seats.len()).expect("kept seats fit the stored count");
            purchase.phase = purchase_phase(
                &seats
                    .iter()
                    .map(|(_, seat, _)| seat.clone())
                    .collect::<Vec<_>>(),
            );
            // The schema 11 rows no longer decode, so they are replaced
            // without decoding.
            dbtx.raw_remove_entry(&DatabaseKeyPrefix::to_bytes(&PurchaseKey))
                .await
                .map_err(|_| FaError::Storage("removing a schema 11 purchase failed".to_owned()))?;
            dbtx.insert_new_entry(&PurchaseKey, &purchase).await;
            for (key, seat, _) in seats {
                dbtx.raw_remove_entry(&DatabaseKeyPrefix::to_bytes(&key))
                    .await
                    .map_err(|_| FaError::Storage("removing a schema 11 seat failed".to_owned()))?;
                dbtx.insert_new_entry(&key, &seat).await;
            }
            dbtx.insert_new_entry(&CreationKey, &creation).await;
        } else {
            let mut formation_seats = Vec::with_capacity(seats.len());
            for (key, seat, guardian_code) in seats {
                dbtx.raw_remove_entry(&DatabaseKeyPrefix::to_bytes(&key))
                    .await
                    .map_err(|_| FaError::Storage("removing a schema 11 seat failed".to_owned()))?;
                formation_seats.push((pool_seat(seat)?, guardian_code));
            }
            // The schema 11 row may hold a phase the purchase type cannot
            // decode, so it is removed without decoding.
            dbtx.raw_remove_entry(&DatabaseKeyPrefix::to_bytes(&PurchaseKey))
                .await
                .map_err(|_| {
                    FaError::Storage("removing a schema 11 formation failed".to_owned())
                })?;
            validate_formation(&creation, &formation_seats)?;
            if legacy.phase == Schema11Phase::Formed {
                // A formed federation keeps only its federation record.
                if dbtx.get_value(&FederationKey).await.is_none() {
                    insert_formed_federation(&mut dbtx.to_ref_nc(), &creation, &formation_seats)
                        .await?;
                }
            } else {
                for (seat, guardian_code) in formation_seats {
                    let seat_id = seat.seat_id.to_string();
                    if let Some(guardian_code) = guardian_code {
                        dbtx.insert_new_entry(
                            &GuardianCodeKey {
                                seat_id: seat_id.clone(),
                            },
                            &StoredGuardianCode(guardian_code),
                        )
                        .await;
                    }
                    dbtx.insert_new_entry(&PoolSeatKey { seat_id }, &seat).await;
                }
                dbtx.insert_new_entry(&CreationKey, &creation).await;
            }
        }
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("migrating schema 11 FA formation failed".to_owned()))
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
        Ok(FaStatus {
            recovery: None,
            creation: self.load_creation(fa_id).await?,
            reauthorization: None,
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

    /// End a federation creation that holds no purchase, seat, or formation.
    pub(crate) async fn cancel_creation(&self) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        if dbtx.get_value(&CreationKey).await.is_none() {
            return Err(FaError::NoActiveFormation);
        }
        if dbtx.get_value(&PurchaseKey).await.is_some()
            || dbtx
                .find_by_prefix(&PoolSeatKeyPrefix)
                .await
                .next()
                .await
                .is_some()
        {
            return Err(FaError::AbandonUnavailable(
                crate::AbandonUnavailableReason::SeatsBought,
            ));
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
            if envelope.schema_version < RESET_BEFORE_SCHEMA_VERSION {
                dbtx.raw_remove_by_prefix(&[]).await.map_err(|_| {
                    FaError::Storage("resetting legacy FA storage failed".to_owned())
                })?;
                dbtx.commit_tx_result().await.map_err(|_| {
                    FaError::Storage("committing legacy FA reset failed".to_owned())
                })?;
                return Ok(None);
            }
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

        validate_payment_authorization(stored.payment_authorization.as_ref(), &recoveries)?;
        if stored.payment_reservation_release_intended && stored.payment_reservation_id.is_none() {
            // No write path commits a release intent without its reservation
            // or clears the reservation while the intent stands, so this
            // combination cannot prove which cleanup it belongs to.
            return Err(FaError::Storage(
                "persisted FA release commitment names no wallet reservation".to_owned(),
            ));
        }
        // Reject a persisted quote that does not match the intent or owner.
        payment_requirements(
            &stored.formation_id,
            &recoveries,
            &stored.intent,
            fa_id,
            None,
        )?;
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
            payment_outputs_started: stored.payment_outputs_started,
            last_error: None,
        };
        Ok(Some(PurchaseRecovery {
            snapshot,
            seats: recoveries,
            payment_reservation_id: stored.payment_reservation_id,
            payment_reservation_release_intended: stored.payment_reservation_release_intended,
            payment_outputs_started: stored.payment_outputs_started,
            payment_federation_id: stored.payment_federation_id,
            payment_authorization: stored.payment_authorization,
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
                    },
                )
                .await;
            }
            None => {
                if dbtx.get_value(&CreationKey).await.is_none() {
                    return Err(FaError::InvalidIntent(
                        "no federation creation needs seats".to_owned(),
                    ));
                }
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
                payment_authorization: None,
                payment_reservation_id: None,
                payment_reservation_release_intended: false,
                payment_outputs_started: false,
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
        // Lease acquisition serializes initialization before any seat work can
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
        #[cfg(test)]
        {
            let mut failed_index = self.failed_seat_checkpoint.lock().expect("test lock");
            if *failed_index == Some(index) {
                *failed_index = None;
                return Err(FaError::Storage(
                    "injected accepted-seat checkpoint failure".to_owned(),
                ));
            }
        }
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

    /// Persist an exact verified quote before any payment or `CreateSeat`
    /// side effect.
    pub(crate) async fn store_quote(
        &self,
        formation_id: &FormationId,
        index: u16,
        signed_quote: SignedResponse<GetQuoteResponse>,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        let store = self.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let signed_quote = signed_quote.clone();
                    let store = store.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot store a quote without an active formation".to_owned(),
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
                        if let Some(existing) = &seat.signed_quote
                            && existing != &signed_quote
                        {
                            return Err(FaError::Storage(format!(
                                "refusing to replace persisted quote for FA seat row {index}"
                            )));
                        }
                        seat.signed_quote = Some(signed_quote);
                        // Any changed membership invalidates the one aggregate authorization.
                        formation.payment_authorization = None;
                        dbtx.insert_entry(&key, &seat).await;
                        dbtx.insert_entry(&PurchaseKey, &formation).await;
                        store.wait_before_hooked_formation_commit().await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Atomically refresh one authorized quote before its payment starts.
    ///
    /// The consumer authorization remains the permission to pay the same
    /// commercial terms; only the quote identity used for wallet recovery is
    /// advanced to the freshly persisted quote.
    pub(crate) async fn refresh_authorized_quote(
        &self,
        formation_id: &FormationId,
        index: u16,
        expected_quote: &SignedResponse<GetQuoteResponse>,
        fresh_quote: SignedResponse<GetQuoteResponse>,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        let expected_quote = expected_quote.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let expected_quote = expected_quote.clone();
                    let fresh_quote = fresh_quote.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot refresh a quote without an active formation".to_owned(),
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
                        if seat.seat_id.is_some()
                            || seat.signed_quote.as_ref() != Some(&expected_quote)
                        {
                            return Err(FaError::Storage(format!(
                                "persisted quote changed before refreshing FA seat row {index}"
                            )));
                        }
                        let old_id = expected_quote
                            .verify(&seat.locator.service_pubkey)
                            .map_err(|error| {
                                FaError::Storage(format!(
                                    "invalid old quote for seat {index}: {error}"
                                ))
                            })?
                            .quote_id();
                        let fresh_id = fresh_quote
                            .verify(&seat.locator.service_pubkey)
                            .map_err(|error| {
                                FaError::Storage(format!(
                                    "invalid fresh quote for seat {index}: {error}"
                                ))
                            })?
                            .quote_id();
                        let authorization =
                            formation.payment_authorization.as_mut().ok_or_else(|| {
                                FaError::Storage(format!(
                                    "FA seat row {index} has no payment authorization"
                                ))
                            })?;
                        let authorized = authorization
                            .quotes
                            .iter_mut()
                            .find(|authorized| {
                                authorized.index == index && authorized.quote_id == old_id
                            })
                            .ok_or_else(|| {
                                FaError::Storage(format!(
                                    "payment authorization does not cover FA seat row {index}"
                                ))
                            })?;
                        authorized.quote_id = fresh_id;
                        seat.signed_quote = Some(fresh_quote);
                        dbtx.insert_entry(&key, &seat).await;
                        dbtx.insert_entry(&PurchaseKey, &formation).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Atomically authorize the currently journaled paid quotes.
    pub(crate) async fn authorize_payments(
        &self,
        formation_id: &FormationId,
        authorization_id: &PaymentAuthorizationId,
        authorizations: &[QuoteAuthorization],
    ) -> FaResult<()> {
        if authorizations.is_empty() {
            return Err(FaError::Storage(
                "cannot persist an empty payment authorization".to_owned(),
            ));
        }
        let formation_id = formation_id.clone();
        let authorization_id = authorization_id.clone();
        let authorizations = authorizations.to_vec();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let authorization_id = authorization_id.clone();
                    let authorizations = authorizations.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot authorize payment without an active formation"
                                        .to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let mut current = Vec::new();
                        for (PurchaseSeatKey { index, .. }, seat) in
                            purchase_seat_rows(dbtx, &formation_id).await?
                        {
                            if seat.seat_id.is_some() {
                                continue;
                            }
                            let Some(signed_quote) = seat.signed_quote.as_ref() else {
                                continue;
                            };
                            let verified = signed_quote
                                .verify(&seat.locator.service_pubkey)
                                .map_err(|error| {
                                    FaError::Storage(format!(
                                        "invalid persisted quote for FA seat row {index}: {error}"
                                    ))
                                })?;
                            if verified.terms.payment.is_some() {
                                current.push(QuoteAuthorization {
                                    index,
                                    quote_id: verified.quote_id(),
                                });
                            }
                        }
                        if current != authorizations {
                            return Err(FaError::Storage(
                                "payment quote set changed before authorization".to_owned(),
                            ));
                        }
                        formation.payment_authorization = Some(StoredPaymentAuthorization {
                            authorization_id,
                            quotes: current,
                        });
                        dbtx.insert_entry(&PurchaseKey, &formation).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Arm the irreversible wallet-output boundary before polling the first
    /// value-moving payment call.
    async fn record_payment_outputs_started(
        &self,
        formation_id: &FormationId,
        verifier_provenance: StoredVerifierProvenance,
        lease: &DriverLease,
    ) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let Some(current) = dbtx.get_value(&DriverLeaseKey).await else {
            return Err(FaError::Busy);
        };
        let now = (self.lease_clock)();
        let expires_at = now
            .checked_add(lease.renewal_duration.as_secs())
            .map(|expires_at| expires_at.min(lease.max_expires_at))
            .ok_or_else(|| FaError::Storage("FA driver lease renewal overflow".to_owned()))?;
        if current.owner != lease.owner || current.expires_at <= now || expires_at <= now {
            return Err(FaError::Busy);
        }

        let mut formation = dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
            FaError::Storage("cannot arm payment outputs without an active formation".to_owned())
        })?;
        validate_active_formation(&formation, formation_id)?;
        let authorization = formation.payment_authorization.as_ref().ok_or_else(|| {
            FaError::Storage(
                "cannot arm payment outputs without exact quote authorization".to_owned(),
            )
        })?;
        if authorization.quotes.is_empty() {
            return Err(FaError::Storage(
                "cannot arm payment outputs without exact quote authorization".to_owned(),
            ));
        }
        if formation.payment_reservation_id.is_none() {
            return Err(FaError::Storage(
                "cannot arm payment outputs without a durable wallet reservation".to_owned(),
            ));
        }
        for authorized in &authorization.quotes {
            let key = PurchaseSeatKey {
                formation_id: formation_id.0.clone(),
                index: authorized.index,
            };
            let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                FaError::Storage(format!(
                    "missing persisted FA seat row {}",
                    authorized.index
                ))
            })?;
            let signed_quote = seat.signed_quote.as_ref().ok_or_else(|| {
                FaError::Storage(format!(
                    "authorized FA seat row {} is missing its quote",
                    authorized.index
                ))
            })?;
            let quote = signed_quote
                .verify(&seat.locator.service_pubkey)
                .map_err(|error| {
                    FaError::Storage(format!(
                        "invalid authorized quote for FA seat row {}: {error}",
                        authorized.index
                    ))
                })?;
            if quote.quote_id() != authorized.quote_id || quote.terms.payment.is_none() {
                return Err(FaError::Storage(format!(
                    "authorized FA seat row {} does not name an exact paid quote",
                    authorized.index
                )));
            }
            validate_fresh_admission(&seat.admission, verifier_provenance, Timestamp(now))?;
            seat.admission
                .authorize_effect(authorized.quote_id, AdmissionEffect::PaidOutput)?;
            dbtx.insert_entry(&key, &seat).await;
        }
        formation.payment_outputs_started = true;
        dbtx.insert_entry(
            &DriverLeaseKey,
            &StoredDriverLease {
                owner: lease.owner,
                expires_at,
            },
        )
        .await;
        dbtx.insert_entry(&PurchaseKey, &formation).await;
        dbtx.commit_tx_result()
            .await
            .map_err(map_lease_commit_error)?;
        (expires_at > (self.lease_clock)())
            .then_some(())
            .ok_or(FaError::Busy)
    }

    async fn record_seat_effects_authorized(
        &self,
        formation_id: &FormationId,
        effects: &[(u16, QuoteId, AdmissionEffect)],
        verifier_provenance: StoredVerifierProvenance,
        lease: &DriverLease,
    ) -> FaResult<()> {
        if effects.is_empty() {
            return Err(FaError::Storage(
                "cannot authorize an empty selected-seat effect wave".to_owned(),
            ));
        }
        let mut dbtx = self.database.begin_transaction().await;
        let Some(current) = dbtx.get_value(&DriverLeaseKey).await else {
            return Err(FaError::Busy);
        };
        let now = (self.lease_clock)();
        let expires_at = now
            .checked_add(lease.renewal_duration.as_secs())
            .map(|expires_at| expires_at.min(lease.max_expires_at))
            .ok_or_else(|| FaError::Storage("FA driver lease renewal overflow".to_owned()))?;
        if current.owner != lease.owner || current.expires_at <= now || expires_at <= now {
            return Err(FaError::Busy);
        }

        let formation = dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
            FaError::Storage(
                "cannot authorize a seat effect without an active formation".to_owned(),
            )
        })?;
        validate_active_formation(&formation, formation_id)?;
        if effects.iter().any(|(_, _, effect)| {
            *effect == AdmissionEffect::PaidOutput && !formation.payment_outputs_started
        }) {
            return Err(FaError::Storage(
                "paid seat effect must be armed with the aggregate output boundary".to_owned(),
            ));
        }

        let mut seen = std::collections::BTreeSet::new();
        for &(index, quote_id, effect) in effects {
            if !seen.insert(index) {
                return Err(FaError::Storage(format!(
                    "selected-seat effect wave duplicates FA seat row {index}"
                )));
            }
            let key = PurchaseSeatKey {
                formation_id: formation_id.0.clone(),
                index,
            };
            let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                FaError::Storage(format!("missing persisted FA seat row {index}"))
            })?;
            let signed_quote = seat.signed_quote.as_ref().ok_or_else(|| {
                FaError::Storage(format!(
                    "FA seat row {index} has no quote for effect authorization"
                ))
            })?;
            let quote = signed_quote
                .verify(&seat.locator.service_pubkey)
                .map_err(|error| {
                    FaError::Storage(format!(
                        "invalid effect-authorized quote for FA seat row {index}: {error}"
                    ))
                })?;
            if quote.quote_id() != quote_id
                || match effect {
                    AdmissionEffect::PaidOutput => quote.terms.payment.is_none(),
                    AdmissionEffect::FreePresentation => quote.terms.payment.is_some(),
                }
            {
                return Err(FaError::Storage(format!(
                    "FA seat row {index} effect does not match its exact quote"
                )));
            }
            validate_fresh_admission(&seat.admission, verifier_provenance, Timestamp(now))?;
            seat.admission.authorize_effect(quote_id, effect)?;
            dbtx.insert_entry(&key, &seat).await;
        }
        dbtx.insert_entry(
            &DriverLeaseKey,
            &StoredDriverLease {
                owner: lease.owner,
                expires_at,
            },
        )
        .await;
        dbtx.commit_tx_result()
            .await
            .map_err(map_lease_commit_error)?;
        (expires_at > (self.lease_clock)())
            .then_some(())
            .ok_or(FaError::Busy)
    }

    /// Persist the wallet's idempotent aggregate reservation before the
    /// output-generation tombstone. Repeating the same id is harmless; a
    /// different live id fails closed.
    pub(crate) async fn record_payment_reservation(
        &self,
        formation_id: &FormationId,
        reservation_id: &PaymentReservationId,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        let reservation_id = reservation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let reservation_id = reservation_id.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot record reservation without an active formation"
                                        .to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        if formation.payment_authorization.is_none() {
                            return Err(FaError::Storage(
                                "cannot reserve an unauthorized payment aggregate".to_owned(),
                            ));
                        }
                        match &formation.payment_reservation_id {
                            Some(existing) if existing != &reservation_id => {
                                return Err(FaError::Storage(
                                    "a different wallet reservation is already active".to_owned(),
                                ));
                            }
                            Some(_) if !formation.payment_reservation_release_intended => {
                                return Ok(());
                            }
                            _ => {}
                        }
                        // Adopting a live reservation supersedes any earlier
                        // release commitment: the wallet journal provably
                        // exists again, so absence stops being expected.
                        formation.payment_reservation_id = Some(reservation_id);
                        formation.payment_reservation_release_intended = false;
                        dbtx.insert_entry(&PurchaseKey, &formation).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Durably commit to releasing the exact current wallet reservation
    /// before the wallet call is made.
    ///
    /// After this commits, authoritative wallet absence of that reservation
    /// is an expected release outcome instead of proof of an orphaned
    /// journal, so a run interrupted between the wallet release and the
    /// local wipe or restore can complete that cleanup later. Repeating an
    /// already-committed intent is harmless; naming any reservation other
    /// than the durable one fails closed.
    pub(crate) async fn record_reservation_release_intent(
        &self,
        formation_id: &FormationId,
        reservation_id: &PaymentReservationId,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        let reservation_id = reservation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let reservation_id = reservation_id.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot commit to a release without an active formation"
                                        .to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        if formation.payment_reservation_id.as_ref() != Some(&reservation_id) {
                            return Err(FaError::Storage(
                                "cannot commit to releasing a reservation that is not durably \
                                 active"
                                    .to_owned(),
                            ));
                        }
                        if formation.payment_reservation_release_intended {
                            return Ok(());
                        }
                        formation.payment_reservation_release_intended = true;
                        dbtx.insert_entry(&PurchaseKey, &formation).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Wipe the active formation back to `Idle` while it is value-safe.
    ///
    /// Value safety is re-validated inside the transaction: wallet output
    /// generation must not have been durably armed and the formation must not
    /// have completed DKG. Commercial authorization alone does not close the window.
    /// Free seats accepted server-side are forfeited, not released; the
    /// authenticated setup-payment policy retention is deliberately kept.
    async fn abandon_purchase(
        &self,
        formation_id: &FormationId,
        lease: &DriverLease,
    ) -> FaResult<()> {
        #[cfg(test)]
        if self.failed_abandon.swap(false, Ordering::SeqCst) {
            return Err(FaError::Storage(
                "injected lease-fenced abandon failure".to_owned(),
            ));
        }
        let mut dbtx = self.database.begin_transaction().await;
        let Some(current) = dbtx.get_value(&DriverLeaseKey).await else {
            return Err(FaError::Busy);
        };
        let now = (self.lease_clock)();
        let expires_at = now
            .checked_add(lease.renewal_duration.as_secs())
            .map(|expires_at| expires_at.min(lease.max_expires_at))
            .ok_or_else(|| FaError::Storage("FA driver lease renewal overflow".to_owned()))?;
        if current.owner != lease.owner || current.expires_at <= now || expires_at <= now {
            return Err(FaError::Busy);
        }

        let formation = dbtx
            .get_value(&PurchaseKey)
            .await
            .ok_or_else(|| FaError::Storage("cannot abandon a missing formation".to_owned()))?;
        validate_active_formation(&formation, formation_id)?;
        if formation.payment_outputs_started {
            return Err(FaError::AbandonUnavailable(
                crate::AbandonUnavailableReason::PaymentOutputsStarted,
            ));
        }
        let rows = purchase_seat_rows(&mut dbtx.to_ref_nc(), formation_id).await?;
        for (key, _) in rows {
            dbtx.remove_entry(&key).await;
        }
        dbtx.remove_entry(&PurchaseKey).await;
        dbtx.insert_entry(
            &DriverLeaseKey,
            &StoredDriverLease {
                owner: lease.owner,
                expires_at,
            },
        )
        .await;
        dbtx.commit_tx_result()
            .await
            .map_err(map_lease_commit_error)?;
        (expires_at > (self.lease_clock)())
            .then_some(())
            .ok_or(FaError::Busy)
    }

    /// Clear one exact quote for a refresh on the same MG.
    ///
    /// Any authorization for the old quote is cleared atomically.
    pub(crate) async fn clear_quote(
        &self,
        formation_id: &FormationId,
        index: u16,
        expected_quote: &SignedResponse<GetQuoteResponse>,
    ) -> FaResult<()> {
        self.clear_quote_with_disposition(
            formation_id,
            index,
            expected_quote,
            QuoteClearDisposition::RefreshSameGuardian,
        )
        .await
    }

    async fn clear_quote_with_disposition(
        &self,
        formation_id: &FormationId,
        index: u16,
        expected_quote: &SignedResponse<GetQuoteResponse>,
        disposition: QuoteClearDisposition,
    ) -> FaResult<()> {
        self.clear_quotes_with_disposition(
            formation_id,
            &[(index, expected_quote.clone())],
            disposition,
        )
        .await
    }

    /// Clear one terminal recovery wave as one durable transition.
    ///
    /// Wallet releases happen before this call and are idempotent. Until every
    /// release succeeds, all old quote and aggregate facts therefore remain
    /// recoverable. Once they have all succeeded, this transaction clears the
    /// complete terminal subset before invalidating its aggregate reservation.
    /// After payment outputs started, the terminal seats leave the purchase:
    /// a new purchase buys any seats still missing.
    pub(crate) async fn clear_terminal_quotes(
        &self,
        formation_id: &FormationId,
        expected_quotes: &[(u16, SignedResponse<GetQuoteResponse>)],
        drop_seats: bool,
    ) -> FaResult<()> {
        let disposition = if drop_seats {
            QuoteClearDisposition::DropSeat
        } else {
            QuoteClearDisposition::RefreshSameGuardian
        };
        self.clear_quotes_with_disposition(formation_id, expected_quotes, disposition)
            .await
    }

    async fn clear_quotes_with_disposition(
        &self,
        formation_id: &FormationId,
        expected_quotes: &[(u16, SignedResponse<GetQuoteResponse>)],
        disposition: QuoteClearDisposition,
    ) -> FaResult<()> {
        if expected_quotes.is_empty() {
            return Err(FaError::Storage(
                "cannot clear an empty terminal quote set".to_owned(),
            ));
        }
        #[cfg(test)]
        {
            let mut failed_index = self.failed_quote_clear.lock().expect("test lock");
            if expected_quotes
                .iter()
                .any(|(index, _)| *failed_index == Some(*index))
            {
                *failed_index = None;
                return Err(FaError::Storage(
                    "injected quote-clear checkpoint failure".to_owned(),
                ));
            }
        }
        let formation_id = formation_id.clone();
        let expected_quotes = expected_quotes.to_vec();
        let store = self.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let expected_quotes = expected_quotes.clone();
                    let store = store.clone();
                    let disposition = disposition;
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&PurchaseKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot clear a quote without an active formation".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        for (index, expected_quote) in &expected_quotes {
                            let key = PurchaseSeatKey {
                                formation_id: formation_id.0.clone(),
                                index: *index,
                            };
                            let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                                FaError::Storage(format!(
                                    "missing persisted FA seat row {index}"
                                ))
                            })?;
                            if seat.seat_id.is_some() {
                                return Err(FaError::Storage(format!(
                                    "cannot clear the quote for accepted FA seat row {index}"
                                )));
                            }
                            if seat.signed_quote.as_ref() != Some(expected_quote) {
                                return Err(FaError::Storage(format!(
                                    "persisted quote changed before clearing FA seat row {index}"
                                )));
                            }
                            match disposition {
                                QuoteClearDisposition::RefreshSameGuardian => {
                                    seat.signed_quote = None;
                                    dbtx.insert_entry(&key, &seat).await;
                                }
                                QuoteClearDisposition::DropSeat => {
                                    if !formation.payment_outputs_started {
                                        return Err(FaError::Storage(format!(
                                            "cannot drop FA seat row {index} before payment outputs started"
                                        )));
                                    }
                                    dbtx.remove_entry(&key).await;
                                }
                            }
                        }
                        if matches!(disposition, QuoteClearDisposition::DropSeat) {
                            let seats = purchase_seat_rows(dbtx, &formation_id)
                                .await?
                                .into_iter()
                                .map(|(_, seat)| seat)
                                .collect::<Vec<_>>();
                            formation.seat_count = u16::try_from(seats.len())
                                .expect("remaining seats fit the stored count");
                            formation.phase = purchase_phase(&seats);
                        }
                        // Invalidate the aggregate only after every exact
                        // terminal member is durably advanced in this tx.
                        formation.payment_authorization = None;
                        formation.payment_reservation_id = None;
                        formation.payment_reservation_release_intended = false;
                        dbtx.insert_entry(&PurchaseKey, &formation).await;
                        store.wait_before_hooked_formation_commit().await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Acquire the database-wide mutation lease for one bounded driver run.
    ///
    /// Graceful completion releases the lease. Cancellation schedules
    /// best-effort release, while a crash leaves the current bounded renewal
    /// interval in place; no renewal can exceed the run's maximum deadline.
    pub(crate) async fn acquire_driver_lease(
        &self,
        max_duration: std::time::Duration,
        renewal_duration: std::time::Duration,
    ) -> FaResult<DriverLease> {
        let now = (self.lease_clock)();
        let max_expires_at = now
            .checked_add(max_duration.as_secs())
            .ok_or_else(|| FaError::Storage("FA driver lease expiry overflow".to_owned()))?;
        let expires_at = now
            .checked_add(renewal_duration.as_secs())
            .map(|expires_at| expires_at.min(max_expires_at))
            .ok_or_else(|| FaError::Storage("FA driver lease renewal overflow".to_owned()))?;
        let owner = fedimint_core::secp256k1::rand::random();
        #[cfg(test)]
        let acquisition_hook = self
            .lease_acquisition_hook
            .lock()
            .expect("test lease hook lock")
            .take();
        #[cfg(test)]
        if let Some((reached, proceed)) = acquisition_hook {
            reached.notify_one();
            proceed.notified().await;
        }
        let mut dbtx = self.database.begin_transaction().await;
        if dbtx
            .get_value(&DriverLeaseKey)
            .await
            .is_some_and(|lease| lease.expires_at > now)
        {
            return Err(FaError::Busy);
        }
        dbtx.insert_entry(&DriverLeaseKey, &StoredDriverLease { owner, expires_at })
            .await;
        dbtx.commit_tx_result()
            .await
            .map_err(map_lease_commit_error)?;
        Ok(DriverLease {
            owner,
            max_expires_at,
            renewal_duration,
            store: self.clone(),
            renewal_lock: Mutex::new(()),
            released: AtomicBool::new(false),
        })
    }

    pub(crate) fn validate_driver_lease_durations(
        &self,
        max_duration: std::time::Duration,
        renewal_duration: std::time::Duration,
    ) -> FaResult<()> {
        let now = (self.lease_clock)();
        now.checked_add(max_duration.as_secs())
            .and_then(|_| now.checked_add(renewal_duration.as_secs()))
            .map(drop)
            .ok_or_else(|| FaError::InvalidIntent("formation lease bound is too large".to_owned()))
    }

    /// Renew the lease only while the database still recognizes this owner.
    pub(crate) async fn renew_driver_lease(&self, lease: &DriverLease) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let Some(current) = dbtx.get_value(&DriverLeaseKey).await else {
            return Err(FaError::Busy);
        };
        let now = (self.lease_clock)();
        let expires_at = now
            .checked_add(lease.renewal_duration.as_secs())
            .map(|expires_at| expires_at.min(lease.max_expires_at))
            .ok_or_else(|| FaError::Storage("FA driver lease renewal overflow".to_owned()))?;
        let renewal_threshold = now
            .checked_add(lease.renewal_duration.as_secs() / 2)
            .ok_or_else(|| FaError::Storage("FA driver lease renewal overflow".to_owned()))?;
        if current.owner != lease.owner || current.expires_at <= now || expires_at <= now {
            return Err(FaError::Busy);
        }
        if renewal_threshold < current.expires_at || current.expires_at == expires_at {
            return (current.expires_at > (self.lease_clock)())
                .then_some(())
                .ok_or(FaError::Busy);
        }
        #[cfg(test)]
        let commit_hook = {
            self.lease_renewal_commit_hook
                .lock()
                .expect("test lease hook lock")
                .clone()
        };
        #[cfg(test)]
        if let Some((reached, proceed)) = commit_hook {
            reached.notify_one();
            proceed.notified().await;
        }
        dbtx.insert_entry(
            &DriverLeaseKey,
            &StoredDriverLease {
                owner: lease.owner,
                expires_at,
            },
        )
        .await;
        dbtx.commit_tx_result()
            .await
            .map_err(map_lease_commit_error)?;
        (expires_at > (self.lease_clock)())
            .then_some(())
            .ok_or(FaError::Busy)
    }

    /// Release a lease only if it still belongs to this driver.
    pub(crate) async fn release_driver_lease(&self, lease: DriverLease) -> FaResult<()> {
        let result = self.release_driver_lease_owner(lease.owner).await;
        if result.is_ok() {
            lease.released.store(true, Ordering::Release);
        }
        result
    }

    async fn release_driver_lease_owner(&self, owner: [u8; 32]) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        if dbtx
            .get_value(&DriverLeaseKey)
            .await
            .is_some_and(|stored| stored.owner == owner)
        {
            #[cfg(test)]
            let release_hook = self
                .lease_release_commit_hook
                .lock()
                .expect("test lease hook lock")
                .take();
            #[cfg(test)]
            if let Some((reached, proceed)) = release_hook {
                reached.notify_one();
                proceed.notified().await;
            }
            dbtx.remove_entry(&DriverLeaseKey).await;
            dbtx.commit_tx_result()
                .await
                .map_err(map_lease_commit_error)?;
        }
        Ok(())
    }
}

pub(crate) fn map_lease_commit_error(error: DatabaseError) -> FaError {
    match error {
        DatabaseError::WriteConflict | DatabaseError::SnapshotTooOld(_) => FaError::Busy,
        DatabaseError::TransactionConsumed
        | DatabaseError::DatabaseBackend(_)
        | DatabaseError::Other(_) => {
            FaError::Storage("FA driver lease database commit failed".to_owned())
        }
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

impl Drop for DriverLease {
    fn drop(&mut self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        let store = self.store.clone();
        let owner = self.owner;
        fedimint_core::runtime::spawn("release cancelled FA driver lease", async move {
            let _ = store.release_driver_lease_owner(owner).await;
        });
    }
}

impl PurchaseRecovery {
    pub(crate) fn payment_requirements(
        &self,
        fa_id: FaId,
    ) -> FaResult<Option<PaymentRequirements>> {
        payment_requirements(
            &self.snapshot.purchase_id,
            &self.seats,
            &self.snapshot.request,
            fa_id,
            None,
        )
    }

    /// Reconstruct the original aggregate behind a durable wallet reservation.
    ///
    /// Accepted seats remain members of that aggregate: their wallet journals
    /// prove consumption, but do not change the reservation identity used to
    /// recover the still-unconsumed siblings after a crash.
    pub(crate) fn reserved_payment_requirements(
        &self,
        fa_id: FaId,
    ) -> FaResult<Option<PaymentRequirements>> {
        payment_requirements(
            &self.snapshot.purchase_id,
            &self.seats,
            &self.snapshot.request,
            fa_id,
            self.payment_authorization.as_ref(),
        )
    }

    pub(crate) fn payments_authorized(&self, requirements: &PaymentRequirements) -> bool {
        authorization_covers(self.payment_authorization.as_ref(), requirements)
    }

    pub(crate) fn quote_is_authorized(&self, index: u16, quote_id: QuoteId) -> bool {
        self.payment_authorization
            .as_ref()
            .is_some_and(|authorization| {
                authorization
                    .quotes
                    .contains(&QuoteAuthorization { index, quote_id })
            })
    }

    pub(crate) fn invalidate_payment_authorization(&mut self) {
        self.payment_authorization = None;
    }

    /// Mirror a just-persisted aggregate authorization into this in-memory
    /// recovery so the driver can continue without a durable reload.
    pub(crate) fn apply_payment_authorization(
        &mut self,
        authorization_id: PaymentAuthorizationId,
        quotes: Vec<QuoteAuthorization>,
    ) {
        self.payment_authorization = Some(StoredPaymentAuthorization {
            authorization_id,
            quotes,
        });
    }
}

fn payment_requirements(
    formation_id: &FormationId,
    recoveries: &[PurchaseSeatRecovery],
    intent: &ResolvedSeatPurchase,
    fa_id: FaId,
    accepted_authorization: Option<&StoredPaymentAuthorization>,
) -> FaResult<Option<PaymentRequirements>> {
    if recoveries
        .iter()
        .any(|recovery| recovery.progress.seat_id.is_none() && recovery.signed_quote.is_none())
    {
        return Ok(None);
    }
    let mut seats = Vec::new();
    let mut total_msats = 0u64;
    for recovery in recoveries {
        if recovery.progress.seat_id.is_some() && accepted_authorization.is_none() {
            continue;
        }
        let Some(signed_quote) = &recovery.signed_quote else {
            continue;
        };
        let quote = signed_quote
            .verify(&recovery.progress.locator.service_pubkey)
            .map_err(|error| {
                FaError::Storage(format!(
                    "invalid persisted quote for FA seat row {}: {error}",
                    recovery.progress.index
                ))
            })?;
        let request = &quote.terms.request;
        let plan_matches = intent.plan.matches(&request.plan);
        if request.fa_id != fa_id
            || request.fedimintd_version.dkg_version() != intent.fedimintd_dkg_version
            || !intent
                .fedimintd_versions
                .contains(&request.fedimintd_version)
            || request.federation_size != intent.federation_size
            || !plan_matches
        {
            return Err(FaError::Storage(format!(
                "persisted quote for FA seat row {} does not match intent or identity",
                recovery.progress.index
            )));
        }
        quote.terms.check_coherent().map_err(|error| {
            FaError::Storage(format!(
                "persisted quote for FA seat row {} is incoherent: {error}",
                recovery.progress.index
            ))
        })?;
        let Some(payment) = &quote.terms.payment else {
            continue;
        };
        if recovery.progress.seat_id.is_some()
            && !accepted_authorization.is_some_and(|authorization| {
                authorization.quotes.contains(&QuoteAuthorization {
                    index: recovery.progress.index,
                    quote_id: quote.quote_id(),
                })
            })
        {
            continue;
        }
        total_msats = total_msats
            .checked_add(quote.terms.price_msats)
            .ok_or_else(|| FaError::Storage("aggregate FA payment total overflow".to_owned()))?;
        seats.push(SeatPaymentRequirement {
            index: recovery.progress.index,
            mg_id: recovery.admission.mg_id(),
            quote_id: quote.quote_id(),
            payment_federation_id: payment.federation_id().clone(),
            amount_msats: quote.terms.price_msats,
        });
    }
    Ok((!seats.is_empty()).then(|| PaymentRequirements {
        authorization_id: payment_authorization_id(formation_id, total_msats, &seats),
        total_msats,
        seats,
    }))
}

fn payment_authorization_id(
    formation_id: &FormationId,
    total_msats: u64,
    seats: &[SeatPaymentRequirement],
) -> PaymentAuthorizationId {
    use fedimint_core::bitcoin::hashes::{Hash as _, sha256};

    let mut preimage = Vec::new();
    preimage.extend_from_slice(b"fedi:fi-client:payment-authorization:v1\0");
    preimage.extend_from_slice(&(formation_id.0.len() as u64).to_be_bytes());
    preimage.extend_from_slice(formation_id.0.as_bytes());
    preimage.extend_from_slice(&total_msats.to_be_bytes());
    preimage.extend_from_slice(&(seats.len() as u64).to_be_bytes());
    for seat in seats {
        preimage.extend_from_slice(&seat.index.to_be_bytes());
        preimage.extend_from_slice(&seat.quote_id.0);
        preimage.extend_from_slice(&(seat.payment_federation_id.0.len() as u64).to_be_bytes());
        preimage.extend_from_slice(seat.payment_federation_id.0.as_bytes());
        preimage.extend_from_slice(&seat.amount_msats.to_be_bytes());
    }
    PaymentAuthorizationId::from_digest(sha256::Hash::hash(&preimage).to_byte_array())
}

pub(crate) fn payment_reservation_id(
    formation_id: &FormationId,
    requirements: &PaymentRequirements,
) -> PaymentReservationId {
    use fedimint_core::bitcoin::hashes::{Hash as _, sha256};

    let mut preimage = Vec::new();
    preimage.extend_from_slice(b"fedi:fi-client:payment-reservation:v1\0");
    preimage.extend_from_slice(&(formation_id.0.len() as u64).to_be_bytes());
    preimage.extend_from_slice(formation_id.0.as_bytes());
    preimage
        .extend_from_slice(&(requirements.authorization_id.as_str().len() as u64).to_be_bytes());
    preimage.extend_from_slice(requirements.authorization_id.as_str().as_bytes());
    PaymentReservationId::from_digest(sha256::Hash::hash(&preimage).to_byte_array())
}

fn authorization_covers(
    authorization: Option<&StoredPaymentAuthorization>,
    requirements: &PaymentRequirements,
) -> bool {
    let Some(authorization) = authorization else {
        return false;
    };
    requirements.seats.iter().all(|requirement| {
        authorization.quotes.contains(&QuoteAuthorization {
            index: requirement.index,
            quote_id: requirement.quote_id,
        })
    })
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
        if let Some((authorized_quote_id, effect)) = seat.admission.authorized_effect() {
            let persisted_quote_id = match seat.signed_quote.as_ref() {
                Some(signed_quote) => {
                    let quote =
                        signed_quote
                            .verify(&seat.locator.service_pubkey)
                            .map_err(|error| {
                                FaError::Storage(format!(
                                    "invalid effect-authorized quote for FA seat row {}: {error}",
                                    seat.index
                                ))
                            })?;
                    let payment_matches = match effect {
                        AdmissionEffect::PaidOutput => quote.terms.payment.is_some(),
                        AdmissionEffect::FreePresentation => quote.terms.payment.is_none(),
                    };
                    if !payment_matches {
                        return Err(FaError::Storage(format!(
                            "persisted FA seat row {} admission effect disagrees with its quote",
                            seat.index
                        )));
                    }
                    quote.quote_id()
                }
                None => {
                    return Err(FaError::Storage(format!(
                        "persisted FA seat row {} authorized an effect without its exact quote",
                        seat.index
                    )));
                }
            };
            if persisted_quote_id != authorized_quote_id {
                return Err(FaError::Storage(format!(
                    "persisted FA seat row {} admission names another quote",
                    seat.index
                )));
            }
            if effect == AdmissionEffect::PaidOutput && !formation.payment_outputs_started {
                return Err(FaError::Storage(format!(
                    "persisted FA seat row {} authorized paid outputs before the aggregate boundary",
                    seat.index
                )));
            }
        } else if seat.seat_id.is_some() {
            return Err(FaError::Storage(format!(
                "persisted accepted FA seat row {} lacks pre-effect admission authorization",
                seat.index
            )));
        }
        if seat.seat_id.is_some() && seat.signed_quote.is_none() {
            return Err(FaError::Storage(format!(
                "persisted FA seat row {} has a seat without its recovery quote",
                seat.index
            )));
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
/// guardian codes. The formation runs once the pool holds every seat.
async fn formation_seats(
    dbtx: &mut DatabaseTransaction<'_>,
    creation: &StoredCreation,
) -> FaResult<Option<Vec<FormationSeat>>> {
    let pool: Vec<StoredPoolSeat> = dbtx
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

fn validate_payment_authorization(
    authorization: Option<&StoredPaymentAuthorization>,
    recoveries: &[PurchaseSeatRecovery],
) -> FaResult<()> {
    let Some(authorization) = authorization else {
        return Ok(());
    };
    if authorization.quotes.is_empty() {
        return Err(FaError::Storage(
            "persisted FA payment authorization is empty".to_owned(),
        ));
    }
    for authorized in &authorization.quotes {
        let recovery = recoveries
            .get(usize::from(authorized.index))
            .ok_or_else(|| {
                FaError::Storage(format!(
                    "persisted FA authorization names missing seat row {}",
                    authorized.index
                ))
            })?;
        let signed_quote = recovery.signed_quote.as_ref().ok_or_else(|| {
            FaError::Storage(format!(
                "persisted FA authorization names a missing quote for seat row {}",
                authorized.index
            ))
        })?;
        let quote = signed_quote
            .verify(&recovery.progress.locator.service_pubkey)
            .map_err(|error| {
                FaError::Storage(format!(
                    "invalid authorized quote for FA seat row {}: {error}",
                    authorized.index
                ))
            })?;
        if quote.quote_id() != authorized.quote_id || quote.terms.payment.is_none() {
            return Err(FaError::Storage(format!(
                "persisted FA authorization does not match seat row {}",
                authorized.index
            )));
        }
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
        let guidance = match formation.schema_version {
            4 => {
                "; schema 4 predates durable FA identity binding and cannot be migrated safely; \
                 reset this unreleased FA namespace and restart formation"
            }
            5 => {
                "; schema 5 predates the post-DKG seat-binding directory and cannot be migrated \
                 safely; reset this unreleased FA namespace and restart formation"
            }
            6 => {
                "; schema 6 predates durable commercial-authorization history and cannot be \
                 migrated safely; reset this unreleased FA namespace and restart formation"
            }
            7 => {
                "; schema 7 predates the distinct wallet-output-generation tombstone and cannot \
                 be migrated safely; reset this unreleased FA namespace and restart formation"
            }
            8 => {
                "; schema 8 does not explicitly distinguish selected Pay-and-create recovery \
                 from pinned formation and cannot be migrated safely; reset this unreleased FA \
                 namespace and restart formation"
            }
            9 | 10 => {
                "; this pre-production schema predates durable Fedimint compatibility ranges and \
                 DKG cohort selection; reset this unreleased FA namespace and restart formation"
            }
            _ => "",
        };
        return Err(FaError::Storage(format!(
            "unsupported FA storage schema version {}{guidance}",
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
