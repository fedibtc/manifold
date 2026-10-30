//! Fedimint database representation for FA recovery facts and checkpoints.

#[cfg(test)]
mod test_support;

use bitcoin_hashes::sha256;
use fedimint_core::db::{
    AutocommitError, Database, DatabaseError, DatabaseKeyPrefix, DatabaseValue, DecodingError,
    IDatabaseTransactionOpsCore as _, IDatabaseTransactionOpsCoreTyped as _,
};
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::registry::ModuleDecoderRegistry;
use fedimint_core::{impl_db_lookup, impl_db_record};
use futures::StreamExt as _;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, watch};

use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifierProvenance;
use fedi_decentralized_service_manifold_guardian::{
    DkgCompletionCallback, FaId, FederationId, FormationSeatBinding, GetQuoteResponse,
    GuardianCode, GuardianFeeAccount, InviteCode, Locator, QuoteId, SeatId, SignedResponse,
    Timestamp,
};
use nostr_sdk::{Event, PublicKey};
use stability_pool_common::Account;

use crate::{
    FaError, FaResult, FaStatus, FormationActionRequired, FormationFreshness, FormationId,
    FormationPhase, FormationSnapshot, GuardianReplacementId, GuardianReplacementRequirements,
    GuardianReplacementSeat, PaymentAuthorizationId, PaymentRequirements, PaymentReservationId,
    ResolvedFormationIntent, SeatPaymentRequirement, SeatPhase, SeatProgress,
};

// Persisted-format changes are governed by GATE-production-compatibility
// (../../../specs/GATE-production-compatibility.md).
const STORAGE_SCHEMA_VERSION: u16 = 11;
// Keep this fixed when advancing the schema written by current code.
const RESET_BEFORE_SCHEMA_VERSION: u16 = 11;
const FORMATION_TX_MAX_ATTEMPTS: usize = 10;

#[derive(Clone, Copy)]
enum QuoteClearDisposition {
    RefreshSameGuardian,
    TerminalReplacement,
}

#[repr(u8)]
enum FaDbPrefix {
    ActiveFormation = 0x00,
    Seat = 0x01,
    DriverLease = 0x02,
    SetupPaymentFederations = 0x03,
    LiquidityOperation = 0x04,
    BackupGeneration = 0x05,
    BackupRelayConfirmation = 0x06,
    FormedFederation = 0x07,
    RecoveryCompleted = 0x08,
    GuardianTransfer = 0x09,
}

#[derive(Debug, Decodable, Encodable)]
struct GuardianTransferKey;
impl_db_record!(
    key = GuardianTransferKey,
    value = crate::transfer::StoredGuardianTransfer,
    db_prefix = FaDbPrefix::GuardianTransfer
);

/// One lease-fenced change to the open guardian transfer.
pub(crate) enum GuardianTransferWrite {
    /// Record a new transfer; refused while one is open.
    Begin(crate::transfer::StoredGuardianTransfer),
    /// Replace the open transfer with the same transfer id.
    Update(crate::transfer::StoredGuardianTransfer),
    /// Put the destination seat into the federation's seats in place of the
    /// source seat, after consensus adopted the new binding.
    Rebind(crate::transfer::StoredGuardianTransfer),
    /// Close the open transfer without a move: its purchase ended.
    Close,
    /// Close the open transfer after the move completed.
    Complete(crate::transfer::StoredGuardianTransfer),
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
struct FormedFederationKey;
impl_db_record!(
    key = FormedFederationKey,
    value = FormedFederationRecord,
    db_prefix = FaDbPrefix::FormedFederation
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

/// The one record of a formed federation: its seats as they are now. A
/// formation writes it once consensus confirmed the formation, and a restore
/// writes it from a backup. The formation record keeps only the setup.
///
/// The key and the JSON stay those of the earlier restored-backup record, so
/// an older build reads a formed federation as a restored one.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(from = "UncheckedFormedFederationRecord")]
struct FormedFederationRecord {
    #[serde(rename = "fi_id")]
    fa_id: FaId,
    payload: crate::backup::FaBackupPayload,
    formation_id: FormationId,
    backup_eligible: bool,
    federation_name: Option<crate::FederationName>,
    /// Verified badge holders this device knows, by seat id. A formation on
    /// this device records its seats' holders. A backup carries none, so an
    /// imported record starts empty.
    seat_holders: Vec<(SeatId, PublicKey)>,
    origin: crate::FormedOrigin,
}

/// A stored [`FormedFederationRecord`], which older builds wrote without
/// `origin` or `seat_holders`.
#[derive(serde::Deserialize)]
struct UncheckedFormedFederationRecord {
    #[serde(rename = "fi_id")]
    fa_id: FaId,
    payload: crate::backup::FaBackupPayload,
    formation_id: FormationId,
    backup_eligible: bool,
    federation_name: Option<crate::FederationName>,
    #[serde(default)]
    seat_holders: Vec<(SeatId, PublicKey)>,
    origin: Option<crate::FormedOrigin>,
}

impl From<UncheckedFormedFederationRecord> for FormedFederationRecord {
    fn from(record: UncheckedFormedFederationRecord) -> Self {
        // A record without an origin: a backup carries no badge holders, and
        // a formation on this device records the holders of its seats.
        let origin = record.origin.unwrap_or(if record.seat_holders.is_empty() {
            crate::FormedOrigin::Backup
        } else {
            crate::FormedOrigin::ThisDevice
        });
        Self {
            fa_id: record.fa_id,
            payload: record.payload,
            formation_id: record.formation_id,
            backup_eligible: record.backup_eligible,
            federation_name: record.federation_name,
            seat_holders: record.seat_holders,
            origin,
        }
    }
}

#[derive(Debug, Decodable, Encodable)]
struct ActiveFormationKey;

impl_db_record!(
    key = ActiveFormationKey,
    value = StoredFormation,
    db_prefix = FaDbPrefix::ActiveFormation,
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
struct SeatKey {
    formation_id: String,
    index: u16,
}

impl_db_record!(
    key = SeatKey,
    value = StoredSeat,
    db_prefix = FaDbPrefix::Seat,
);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredFormation {
    schema_version: u16,
    /// `None` exists only so schema-v4 JSON can be decoded and rejected with
    /// an actionable compatibility error instead of a generic decode failure.
    #[serde(default)]
    #[serde(rename = "fi_id")]
    fa_id: Option<FaId>,
    formation_id: FormationId,
    phase: StoredFormationPhase,
    intent: ResolvedFormationIntent,
    seat_count: u16,
    /// Explicit consumer-selected ready payer, or `None` for the verified
    /// zero-price deployment-bootstrap path.
    #[serde(rename = "creation_mode", with = "selected_creation_mode")]
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
    /// proof of an orphaned journal, so an interrupted abandon or
    /// provisional-replacement restore completes on a later run instead of
    /// retaining the formation forever. Meaningful only beside a durable
    /// reservation id; adopting a live reservation clears it. Like the
    /// spending cap, a record persisted before this field decodes to the
    /// absent commitment.
    #[serde(default)]
    payment_reservation_release_intended: bool,
    /// Whether an aggregate payment authorization was ever durably recorded.
    ///
    /// `payment_authorization` is cleared whenever the quote set changes, but
    /// the initial spending cap must never silently authorize a replacement
    /// aggregate, so this commercial-history tombstone never clears. It is
    /// independent of the wallet-output value-safety boundary below.
    payment_authorization_recorded: bool,
    /// Monotone boundary armed immediately before the first wallet output
    /// generation future is polled. Unlike commercial authorization, this is
    /// the value-safety gate for payer switching and abandon.
    payment_outputs_started: bool,
    invite_code: Option<InviteCode>,
    /// Exact formation metadata target, persisted before its first vote.
    formation_meta_target: Option<FormationMetaTarget>,
    /// Installation-scoped bearer callback reused verbatim for every MG.
    /// Its semantic type redacts `Debug`; database copies remain sensitive.
    #[serde(default)]
    dkg_completion_callback: FieldPresence<DkgCompletionCallback>,
}

#[derive(serde::Deserialize)]
struct StoredFormationEnvelope {
    schema_version: u16,
    /// Read before the full record so a removed pinned formation fails with
    /// reset guidance instead of a decode failure.
    #[serde(default)]
    creation_mode: Option<serde_json::Value>,
}

/// Schema 11 stores the payer under a `selected` creation mode. The other
/// mode, pinned formation, was removed; `load_recovery` rejects it.
mod selected_creation_mode {
    use super::FederationId;

    #[derive(serde::Deserialize, serde::Serialize)]
    #[serde(rename_all = "snake_case")]
    enum CreationMode {
        Selected {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            payment_federation_id: Option<FederationId>,
        },
    }

    pub(super) fn serialize<S: serde::Serializer>(
        payment_federation_id: &Option<FederationId>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(
            &CreationMode::Selected {
                payment_federation_id: payment_federation_id.clone(),
            },
            serializer,
        )
    }

    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<FederationId>, D::Error> {
        let CreationMode::Selected {
            payment_federation_id,
        } = serde::Deserialize::deserialize(deserializer)?;
        Ok(payment_federation_id)
    }
}

/// Schema 11 seat rows tag the admission `peer_badge`, the one admission
/// kind left after pinned formation was removed.
mod peer_badge_admission {
    use super::MgAdmission;

    #[derive(serde::Deserialize, serde::Serialize)]
    #[serde(rename_all = "snake_case")]
    enum Tagged<T> {
        PeerBadge(T),
    }

    pub(super) fn serialize<S: serde::Serializer>(
        admission: &MgAdmission,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&Tagged::PeerBadge(admission), serializer)
    }

    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<MgAdmission, D::Error> {
        let Tagged::PeerBadge(admission) = serde::Deserialize::deserialize(deserializer)?;
        Ok(admission)
    }
}

/// Distinguishes a missing legacy JSON field from explicit current-schema
/// `null`; current storage requires `Present`, while older schemas may
/// decode it as `Missing` before being rejected.
#[derive(Clone, Debug, Default)]
enum FieldPresence<T> {
    #[default]
    Missing,
    Present(Option<T>),
}

impl<T> FieldPresence<T> {
    fn new(value: Option<T>) -> Self {
        Self::Present(value)
    }

    fn is_present(&self) -> bool {
        matches!(self, Self::Present(_))
    }

    fn into_value(self) -> Option<T> {
        match self {
            Self::Missing => None,
            Self::Present(value) => value,
        }
    }

    fn clear(&mut self) {
        *self = Self::Present(None);
    }
}

impl<T: serde::Serialize> serde::Serialize for FieldPresence<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Missing => Err(serde::ser::Error::custom(
                "missing legacy callback field cannot be serialized",
            )),
            Self::Present(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for FieldPresence<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self::new(Option::<T>::deserialize(deserializer)?))
    }
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
/// A fresh replacement writes a fresh unconsumed proof for only its row; the
/// output-boundary transaction consumes every currently live proof before any
/// corresponding wallet future is polled.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct MgAdmission {
    #[serde(rename = "fman_id")]
    mg_id: PublicKey,
    /// Older saved seats did not record the verified badge holder.
    #[serde(default)]
    holder: Option<PublicKey>,
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
        verifier_provenance: StoredVerifierProvenance,
        valid_until: Timestamp,
    ) -> Self {
        Self {
            mg_id,
            holder: Some(holder),
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
enum StoredFormationPhase {
    Initialized,
    SeatsPartiallyCreated,
    SeatsCreated,
    DkgComplete,
    PublishingSeatBindings,
    /// Written only by older builds, which kept a formed federation in its
    /// formation record. Loading converts it to the formed-federation record.
    Formed,
}

impl StoredFormationPhase {
    fn dkg_complete(self) -> bool {
        matches!(
            self,
            Self::DkgComplete | Self::PublishingSeatBindings | Self::Formed
        )
    }
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
json_database_value!(StoredFormation);

/// Durable protocol facts for one selected seat.
///
/// This deliberately excludes observed freshness, payment evidence, raw
/// ecash, and wallet-private refund material. The exact signed quote is safe
/// to retain: it contains only MG-authenticated public terms.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredSeat {
    index: u16,
    locator: Locator,
    #[serde(with = "peer_badge_admission")]
    admission: MgAdmission,
    signed_quote: Option<SignedResponse<GetQuoteResponse>>,
    seat_id: Option<SeatId>,
    #[serde(default)]
    guardian_fee_account: Option<Account>,
    guardian_code: Option<GuardianCode>,
    /// Exact terminal quote proving this row, and only this row, is safe to
    /// replace after payment outputs started.
    #[serde(default)]
    replacement_for: Option<QuoteId>,
    /// Locator that produced `replacement_for`. Once a fresh replacement is
    /// applied, `locator` advances to that provisional guardian while this
    /// field keeps the terminal safety proof stable across another preview.
    #[serde(default)]
    replacement_previous_locator: Option<Locator>,
    /// Badge-vouched identity that produced `replacement_for`, kept beside
    /// the previous locator so the exposed replacement row keeps naming the
    /// outgoing MG after a provisional swap advances `admission`.
    #[serde(default)]
    #[serde(rename = "replacement_previous_fman_id")]
    replacement_previous_mg_id: Option<PublicKey>,
    /// Whether a fresh replacement approval has been applied but its exact
    /// paid-output or free-presentation effect is not authorized yet.
    #[serde(default)]
    replacement_approved: bool,
}

/// One terminal recovery fact corrupted by a storage-corruption test.
#[derive(Clone, Copy, Debug)]
#[cfg(test)]
pub(crate) enum FormedFact {
    /// The aggregate seat count.
    SeatCount,
    /// The common federation invite.
    InviteCode,
    /// The exact signed quote for the first seat.
    SignedQuote,
    /// The accepted identifier for the first seat.
    SeatId,
    /// The MG-signed guardian-fee account for the first seat.
    GuardianFeeAccount,
    /// The guardian recovery code for the first seat.
    GuardianCode,
}

json_database_value!(StoredSeat);

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
json_database_value!(FormedFederationRecord);
json_database_value!(crate::transfer::StoredGuardianTransfer);

/// One seat's public projection and its private recovery-only quote facts.
pub(crate) struct SeatRecovery {
    pub(crate) progress: SeatProgress,
    pub(crate) admission: MgAdmission,
    pub(crate) signed_quote: Option<SignedResponse<GetQuoteResponse>>,
    pub(crate) replacement_for: Option<QuoteId>,
    pub(crate) replacement_previous_locator: Option<Locator>,
    pub(crate) replacement_previous_mg_id: Option<PublicKey>,
    pub(crate) replacement_approved: bool,
    pub(crate) guardian_fee_account: Option<Account>,
}

pub(crate) struct ActiveFormationRecovery {
    pub(crate) snapshot: FormationSnapshot,
    pub(crate) seats: Vec<SeatRecovery>,
    payment_authorization: Option<StoredPaymentAuthorization>,
    /// Whether an aggregate payment authorization was ever durably recorded.
    pub(crate) payment_authorization_recorded: bool,
    /// Exact aggregate reservation owned by the wallet, if established.
    pub(crate) payment_reservation_id: Option<PaymentReservationId>,
    /// Whether a durable release commitment makes wallet absence of the
    /// reservation above an expected cleanup outcome.
    pub(crate) payment_reservation_release_intended: bool,
    /// Whether wallet output generation has crossed its durable boundary.
    pub(crate) payment_outputs_started: bool,
    /// Explicit payer, absent for the zero-price bootstrap.
    pub(crate) payment_federation_id: Option<FederationId>,
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

#[cfg(test)]
impl ActiveFormationRecovery {
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

pub(crate) enum FaRecovery {
    Idle,
    Formation(Box<ActiveFormationRecovery>),
    Formed(crate::FormedFederationSnapshot),
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

    /// Renew this owner while returning an unapplied, unreserved replacement
    /// wave to its durable fresh-preview boundary.
    pub(crate) async fn restore_provisional_replacements(
        &self,
        formation_id: &FormationId,
        released_reservation: Option<&crate::formation::ReleasedReplacementReservation>,
    ) -> FaResult<()> {
        let _guard = self.renewal_lock.lock().await;
        self.store
            .restore_provisional_replacements(formation_id, released_reservation, self)
            .await
    }

    /// Renew this owner and apply one guardian-transfer change in the same
    /// transaction, so a fenced-out driver cannot move the transfer.
    pub(crate) async fn write_guardian_transfer(
        &self,
        fa_id: FaId,
        write: GuardianTransferWrite,
    ) -> FaResult<()> {
        let _guard = self.renewal_lock.lock().await;
        self.store.write_guardian_transfer(fa_id, write, self).await
    }

    /// Renew this owner while atomically wiping a still-abandonable formation.
    pub(crate) async fn abandon_formation(&self, formation_id: &FormationId) -> FaResult<()> {
        let _guard = self.renewal_lock.lock().await;
        self.store.abandon_formation(formation_id, self).await
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
    #[cfg(test)]
    failed_restore: Arc<AtomicBool>,
    /// Count of guardian transfer writes left before a simulated crash: the
    /// last one commits and then fails, so its run stops right there.
    #[cfg(test)]
    stopped_transfer_write: Arc<std::sync::Mutex<Option<usize>>>,
    /// The open transfer as last written, or the one that last completed.
    guardian_transfer: Arc<watch::Sender<Option<crate::transfer::GuardianTransfer>>>,
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
            #[cfg(test)]
            failed_restore: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            stopped_transfer_write: Arc::new(std::sync::Mutex::new(None)),
            guardian_transfer: Arc::new(watch::channel(None).0),
        }
    }

    /// Load the open transfer into the transfer watch. A client calls this
    /// once when it opens; every later transfer write updates the watch.
    pub(crate) async fn load_guardian_transfer_watch(&self) {
        self.guardian_transfer.send_replace(
            self.load_guardian_transfer()
                .await
                .map(|transfer| transfer.view(transfer.public_phase())),
        );
    }

    pub(crate) fn observe_guardian_transfer(
        &self,
    ) -> watch::Receiver<Option<crate::transfer::GuardianTransfer>> {
        self.guardian_transfer.subscribe()
    }

    /// Prepare one eligible backup, allocating its generation and Nostr
    /// timestamp in the same transaction. Unchanged facts reuse both until the
    /// next refresh boundary; changed facts advance both immediately.
    pub(crate) async fn backup_payload(&self) -> FaResult<crate::backup::PreparedFaBackup> {
        let mut dbtx = self.database.begin_transaction().await;
        let formed = dbtx
            .get_value(&FormedFederationKey)
            .await
            .filter(|formed| formed.backup_eligible)
            .ok_or_else(formed_backup_required)?;
        let baseline = (
            formed.payload.snapshot_generation,
            crate::backup::semantic_hash(&formed.payload)?,
        );
        let federation_invite = formed.payload.federation_invite;
        let seats = formed.payload.seats;
        let past_hosts = formed.payload.past_hosts;
        let mut entries = dbtx.find_by_prefix(&LiquidityOperationKeyPrefix).await;
        let mut liquidity = None;
        while let Some((_key, operation)) = entries.next().await {
            if !operation.is_terminal_for_backup()? {
                if liquidity.replace(operation.commitment).is_some() {
                    return Err(FaError::Storage(
                        "more than one live FA liquidity request".to_owned(),
                    ));
                }
            }
        }
        drop(entries);
        let mut payload = crate::backup::FaBackupPayload {
            schema_version: crate::backup::BACKUP_SCHEMA_VERSION,
            snapshot_generation: 0,
            federation_invite,
            seats,
            liquidity,
            past_hosts,
        };
        let semantic_hash = crate::backup::semantic_hash(&payload)?;
        let prior = dbtx.get_value(&BackupGenerationKey).await;
        let generation = match prior.as_ref() {
            Some(prior) if prior.semantic_hash == semantic_hash => prior.generation,
            Some(prior) => prior.generation.checked_add(1).ok_or_else(|| {
                FaError::Storage("FA backup snapshot generation exhausted".to_owned())
            })?,
            None => match baseline {
                (generation, formed_hash) if formed_hash == semantic_hash => generation,
                (generation, _) => generation.checked_add(1).ok_or_else(|| {
                    FaError::Storage("FA backup snapshot generation exhausted".to_owned())
                })?,
            },
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

    pub(crate) async fn restore_backup_payload(
        &self,
        fa_id: FaId,
        payload: crate::backup::FaBackupPayload,
    ) -> FaResult<FaStatus> {
        self.restore_backup_payload_with_completion(fa_id, payload, None)
            .await
    }

    pub(crate) async fn restore_backup_payload_with_completion(
        &self,
        fa_id: FaId,
        payload: crate::backup::FaBackupPayload,
        recovery: Option<&RecoveryCompletedKey>,
    ) -> FaResult<FaStatus> {
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
        let lease = dbtx.get_value(&DriverLeaseKey).await;
        if dbtx.get_value(&ActiveFormationKey).await.is_some()
            || dbtx.get_value(&FormedFederationKey).await.is_some()
            || lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > (self.lease_clock)())
            || dbtx
                .find_by_prefix(&LiquidityOperationKeyPrefix)
                .await
                .next()
                .await
                .is_some()
            || dbtx.get_value(&GuardianTransferKey).await.is_some()
        {
            return Err(FaError::Storage(
                "FA restore requires an empty database namespace".to_owned(),
            ));
        }
        // An expired driver lease is not a formation. Keep independently
        // authenticated setup-payment policy; its publisher is revalidated on use.
        if lease.is_some() {
            dbtx.remove_entry(&DriverLeaseKey).await;
        }
        dbtx.insert_entry(
            &FormedFederationKey,
            &FormedFederationRecord {
                fa_id,
                payload: payload.clone(),
                formation_id,
                backup_eligible: false,
                federation_name: None,
                seat_holders: Vec::new(),
                origin: crate::FormedOrigin::Backup,
            },
        )
        .await;
        if let Some(recovery) = recovery {
            dbtx.insert_entry(recovery, &()).await;
        }
        dbtx.commit_tx_result().await.map_err(|_| {
            FaError::Storage("committing authenticated FA restore failed".to_owned())
        })?;
        self.load_status(fa_id).await
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

    pub(crate) async fn reconcile_formed_federation(
        &self,
        fa_id: FaId,
        federation_name: Option<crate::FederationName>,
        invite: crate::InviteCode,
    ) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let mut formed = dbtx.get_value(&FormedFederationKey).await.ok_or_else(|| {
            FaError::Storage("formed FA federation disappeared during reconciliation".to_owned())
        })?;
        if formed.fa_id != fa_id {
            return Err(FaError::Storage(
                "formed FA federation belongs to another scoped root".to_owned(),
            ));
        }
        formed.backup_eligible = true;
        // Consensus has the current name. A formation on this device saved
        // its intent's name, which stays when consensus names none.
        formed.federation_name = federation_name.or(formed.federation_name);
        // The verified config's endpoints, so later reads and backups dial
        // each guardian where it is now. A restored record has no backup
        // generation of its own yet, so a changed invite advances the imported
        // one: a restore must prefer this backup over the one it came from.
        if formed.payload.federation_invite != invite {
            if dbtx.get_value(&BackupGenerationKey).await.is_none() {
                formed.payload.snapshot_generation = formed
                    .payload
                    .snapshot_generation
                    .checked_add(1)
                    .ok_or_else(|| {
                        FaError::Storage("FA backup snapshot generation exhausted".to_owned())
                    })?;
            }
            formed.payload.federation_invite = invite;
        }
        if let Some(commitment) = formed.payload.liquidity.clone() {
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
                        formation_id: formed.formation_id.clone(),
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
        dbtx.insert_entry(&FormedFederationKey, &formed).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("committing formed FA reconciliation failed".to_owned()))
    }

    pub(crate) async fn formed_federation_payload(
        &self,
        fa_id: FaId,
    ) -> FaResult<crate::backup::FaBackupPayload> {
        let formed = self
            .database
            .begin_transaction_nc()
            .await
            .get_value(&FormedFederationKey)
            .await
            .ok_or(FaError::NoActiveFormation)?;
        if formed.fa_id != fa_id {
            return Err(FaError::Storage(
                "formed FA federation belongs to another scoped root".to_owned(),
            ));
        }
        Ok(formed.payload)
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
            failed_restore: Arc::new(AtomicBool::new(false)),
            stopped_transfer_write: Arc::new(std::sync::Mutex::new(None)),
            guardian_transfer: Arc::new(watch::channel(None).0),
        }
    }

    #[cfg(test)]
    /// Commit `count` more guardian transfer writes, then fail the last one
    /// after its commit, as a crash right after it would.
    pub(crate) fn stop_after_transfer_writes(&self, count: usize) {
        *self.stopped_transfer_write.lock().expect("test lock") = Some(count);
    }

    #[cfg(test)]
    /// Whether the stop set by [`Self::stop_after_transfer_writes`] has not
    /// happened yet.
    pub(crate) fn transfer_write_stop_pending(&self) -> bool {
        self.stopped_transfer_write
            .lock()
            .expect("test lock")
            .is_some()
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
    /// Fail the next lease-fenced provisional-replacement restore before its
    /// durable transaction.
    pub(crate) fn fail_restore_once(&self) {
        self.failed_restore.store(true, Ordering::SeqCst);
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
    /// Mark the formation formed in place, as builds before the formed
    /// record did after the metadata readback.
    pub(crate) async fn mark_formation_formed_for_test(&self) {
        let mut dbtx = self.database.begin_transaction().await;
        let mut formation = dbtx
            .get_value(&ActiveFormationKey)
            .await
            .expect("test formation exists");
        assert_eq!(
            formation.phase,
            StoredFormationPhase::PublishingSeatBindings,
            "only a formation that published its metadata can be formed"
        );
        formation.phase = StoredFormationPhase::Formed;
        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
        dbtx.commit_tx().await;
    }

    #[cfg(test)]
    pub(crate) async fn has_formation_record_for_test(&self) -> bool {
        let mut dbtx = self.database.begin_transaction_nc().await;
        dbtx.get_value(&ActiveFormationKey).await.is_some()
    }

    #[cfg(test)]
    /// Corrupt one required terminal fact to exercise fail-closed loading.
    pub(crate) async fn remove_formed_fact_for_test(&self, fact: FormedFact) {
        let mut dbtx = self.database.begin_transaction().await;
        let mut formation = dbtx
            .get_value(&ActiveFormationKey)
            .await
            .expect("test formation exists");
        if matches!(fact, FormedFact::SeatCount | FormedFact::InviteCode) {
            match fact {
                FormedFact::SeatCount => formation.seat_count -= 1,
                FormedFact::InviteCode => formation.invite_code = None,
                _ => unreachable!("matched aggregate fact"),
            }
            dbtx.insert_entry(&ActiveFormationKey, &formation).await;
            dbtx.commit_tx().await;
            return;
        }
        let key = SeatKey {
            formation_id: formation.formation_id.0,
            index: 0,
        };
        let mut seat = dbtx.get_value(&key).await.expect("test seat exists");
        match fact {
            FormedFact::SeatCount | FormedFact::InviteCode => unreachable!("handled above"),
            FormedFact::SignedQuote => seat.signed_quote = None,
            FormedFact::SeatId => seat.seat_id = None,
            FormedFact::GuardianFeeAccount => seat.guardian_fee_account = None,
            FormedFact::GuardianCode => seat.guardian_code = None,
        }
        dbtx.insert_entry(&key, &seat).await;
        dbtx.commit_tx().await;
    }

    #[cfg(test)]
    /// Corrupt one seat into the forbidden quote-plus-replacement state.
    pub(crate) async fn mix_quote_with_replacement_for_test(&self, index: u16) {
        let mut dbtx = self.database.begin_transaction().await;
        let formation = dbtx
            .get_value(&ActiveFormationKey)
            .await
            .expect("test formation exists");
        let key = SeatKey {
            formation_id: formation.formation_id.0,
            index,
        };
        let mut seat = dbtx.get_value(&key).await.expect("test seat exists");
        let quote_id = seat
            .signed_quote
            .as_ref()
            .expect("test seat has quote")
            .verify(&seat.locator.service_pubkey)
            .expect("test quote verifies")
            .quote_id();
        seat.replacement_for = Some(quote_id);
        dbtx.insert_entry(&key, &seat).await;
        dbtx.commit_tx().await;
    }

    #[cfg(test)]
    /// Return whether the current schema fails closed when one required JSON
    /// recovery discriminator is removed.
    pub(crate) async fn rejects_missing_recovery_field_for_test(&self, field: &str) -> bool {
        let mut dbtx = self.database.begin_transaction_nc().await;
        let formation = dbtx
            .get_value(&ActiveFormationKey)
            .await
            .expect("test formation exists");
        let mut value = serde_json::to_value(formation).expect("stored formation serializes");
        value
            .as_object_mut()
            .expect("stored formation is an object")
            .remove(field);
        serde_json::from_value::<StoredFormation>(value).is_err()
    }

    #[cfg(test)]
    /// Clear only the tombstone flag, simulating a record where an aggregate
    /// authorization is present without the durable tombstone, to exercise
    /// the legacy OR-arm in [`FaStore::load_recovery`].
    pub(crate) async fn clear_authorization_tombstone_for_test(&self) {
        let mut dbtx = self.database.begin_transaction().await;
        let mut formation = dbtx
            .get_value(&ActiveFormationKey)
            .await
            .expect("test formation exists");
        formation.payment_authorization_recorded = false;
        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
        dbtx.commit_tx().await;
    }

    #[cfg(test)]
    /// Drop the first seat's guardian recovery fact.
    pub(crate) async fn remove_first_guardian_code_for_test(&self) {
        let mut dbtx = self.database.begin_transaction().await;
        let formation = dbtx
            .get_value(&ActiveFormationKey)
            .await
            .expect("test formation exists");
        let key = SeatKey {
            formation_id: formation.formation_id.0.clone(),
            index: 0,
        };
        let mut seat = dbtx.get_value(&key).await.expect("test seat exists");
        seat.guardian_code = None;
        dbtx.insert_entry(&key, &seat).await;
        dbtx.commit_tx().await;
    }

    pub(crate) async fn load_status(&self, fa_id: FaId) -> FaResult<FaStatus> {
        Ok(match self.load_recovery(fa_id).await? {
            FaRecovery::Idle => FaStatus::Idle,
            FaRecovery::Formation(recovery) => FaStatus::Formation(recovery.snapshot),
            FaRecovery::Formed(snapshot) => FaStatus::Formed(snapshot),
        })
    }

    pub(crate) async fn load_recovery(&self, fa_id: FaId) -> FaResult<FaRecovery> {
        let mut dbtx = self.database.begin_transaction().await;
        if let Some(bytes) = dbtx
            .raw_get_bytes(&DatabaseKeyPrefix::to_bytes(&ActiveFormationKey))
            .await
            .map_err(|_| FaError::Storage("reading FA storage failed".to_owned()))?
        {
            let envelope: StoredFormationEnvelope =
                serde_json::from_slice(&bytes).map_err(|_| {
                    FaError::Storage("persisted FA formation header is malformed".to_owned())
                })?;
            if envelope.schema_version < RESET_BEFORE_SCHEMA_VERSION {
                dbtx.raw_remove_by_prefix(&[]).await.map_err(|_| {
                    FaError::Storage("resetting legacy FA storage failed".to_owned())
                })?;
                dbtx.commit_tx_result().await.map_err(|_| {
                    FaError::Storage("committing legacy FA reset failed".to_owned())
                })?;
                return Ok(FaRecovery::Idle);
            }
            if envelope.schema_version > STORAGE_SCHEMA_VERSION {
                return Err(FaError::Storage(format!(
                    "unsupported FA storage schema version {}",
                    envelope.schema_version
                )));
            }
            if envelope
                .creation_mode
                .is_some_and(|creation_mode| creation_mode == "pinned")
            {
                return Err(FaError::Storage(
                    "this FA formation was created from pinned MG locators, which are no longer \
                     supported; reset this unreleased FA namespace and restart formation"
                        .to_owned(),
                ));
            }
        }
        let Some(stored) = dbtx.get_value(&ActiveFormationKey).await else {
            let Some(formed) = dbtx.get_value(&FormedFederationKey).await else {
                return Ok(FaRecovery::Idle);
            };
            return formed_recovery(formed, fa_id);
        };
        validate_schema_and_owner(&stored, fa_id)?;
        let mut seats = Vec::with_capacity(usize::from(stored.seat_count));
        for index in 0..stored.seat_count {
            let key = SeatKey {
                formation_id: stored.formation_id.0.clone(),
                index,
            };
            let seat = dbtx.get_value(&key).await.ok_or_else(|| {
                FaError::Storage(format!("missing persisted FA seat row {index}"))
            })?;
            if seat.index != index {
                return Err(FaError::Storage(format!(
                    "persisted FA seat row {index} contains index {}",
                    seat.index
                )));
            }
            seats.push(seat);
        }
        validate_formation_progress(&stored, &seats)?;
        if stored.phase == StoredFormationPhase::Formed {
            // An older build kept a formed federation in its formation record.
            record_formed_federation(&mut dbtx.to_ref_nc(), stored).await?;
            let formed = dbtx
                .get_value(&FormedFederationKey)
                .await
                .expect("the formed record was written in this transaction");
            dbtx.commit_tx_result().await.map_err(|_| {
                FaError::Storage("committing the formed FA federation record failed".to_owned())
            })?;
            return formed_recovery(formed, fa_id);
        }

        let mut recoveries = Vec::with_capacity(seats.len());
        for seat in seats {
            let phase = if stored.phase.dkg_complete() {
                SeatPhase::Running
            } else if seat.guardian_code.is_some() {
                SeatPhase::GuardianCodeReady
            } else if seat.seat_id.is_some() {
                SeatPhase::Created
            } else if seat.replacement_for.is_some() && !seat.replacement_approved {
                SeatPhase::ReplacementRequired
            } else if seat.signed_quote.is_some() {
                SeatPhase::QuoteReady
            } else {
                SeatPhase::Selected
            };
            recoveries.push(SeatRecovery {
                progress: SeatProgress {
                    index: seat.index,
                    mg_id: seat.admission.mg_id(),
                    locator: seat.locator,
                    seat_id: seat.seat_id,
                    guardian_code: seat.guardian_code,
                    phase,
                    freshness: FormationFreshness::Unsynced,
                },
                admission: seat.admission,
                signed_quote: seat.signed_quote,
                replacement_for: seat.replacement_for,
                replacement_previous_locator: seat.replacement_previous_locator,
                replacement_previous_mg_id: seat.replacement_previous_mg_id,
                replacement_approved: seat.replacement_approved,
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
        let payment_requirements = payment_requirements(
            &stored.formation_id,
            &recoveries,
            &stored.intent,
            fa_id,
            None,
        )?;
        let payment_required = payment_requirements.as_ref().is_some_and(|requirements| {
            !authorization_covers(stored.payment_authorization.as_ref(), requirements)
        });
        let expose_payment_action = payment_required && stored.payment_outputs_started;
        let phase = public_phase(stored.phase, &recoveries, expose_payment_action);
        let replacements = replacement_requirements(&stored.formation_id, &recoveries);
        let snapshot = FormationSnapshot {
            formation_id: stored.formation_id,
            intent: stored.intent,
            phase,
            seats: recoveries
                .iter()
                .map(|recovery| recovery.progress.clone())
                .collect(),
            freshness: FormationFreshness::Unsynced,
            action_required: replacements
                .map(FormationActionRequired::ReplaceGuardians)
                .or_else(|| {
                    expose_payment_action.then(|| {
                        FormationActionRequired::AuthorizePayments(
                            payment_requirements.expect("payment readiness implies requirements"),
                        )
                    })
                }),
            payment_outputs_started: stored.payment_outputs_started,
            invite_code: stored.invite_code,
            last_error: None,
        };
        Ok(FaRecovery::Formation(Box::new(ActiveFormationRecovery {
            snapshot,
            seats: recoveries,
            // Preserve one-shot cap/commercial history even if an in-memory
            // record somehow carries authorization without its tombstone.
            // Only `payment_outputs_started` gates abandon.
            payment_authorization_recorded: stored.payment_authorization_recorded
                || stored.payment_authorization.is_some(),
            payment_reservation_id: stored.payment_reservation_id,
            payment_reservation_release_intended: stored.payment_reservation_release_intended,
            payment_outputs_started: stored.payment_outputs_started,
            payment_federation_id: stored.payment_federation_id,
            payment_authorization: stored.payment_authorization,
            formation_meta_target: stored.formation_meta_target,
            dkg_completion_callback: stored.dkg_completion_callback.into_value(),
        })))
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
        intent: ResolvedFormationIntent,
        seats: Vec<InitialSeat>,
        payment_federation_id: Option<FederationId>,
        dkg_completion_callback: Option<DkgCompletionCallback>,
    ) -> FaResult<()> {
        let seat_count = u16::try_from(seats.len())
            .map_err(|_| FaError::Storage("too many FA seat rows".to_owned()))?;
        let mut dbtx = self.database.begin_transaction().await;
        if dbtx.get_value(&ActiveFormationKey).await.is_some()
            || dbtx.get_value(&FormedFederationKey).await.is_some()
        {
            return Err(FaError::Storage(
                "cannot initialize over an active FA formation".to_owned(),
            ));
        }
        dbtx.insert_entry(
            &ActiveFormationKey,
            &StoredFormation {
                schema_version: STORAGE_SCHEMA_VERSION,
                fa_id: Some(fa_id),
                formation_id: formation_id.clone(),
                phase: StoredFormationPhase::Initialized,
                intent,
                seat_count,
                payment_federation_id,
                payment_authorization: None,
                payment_reservation_id: None,
                payment_reservation_release_intended: false,
                payment_authorization_recorded: false,
                payment_outputs_started: false,
                invite_code: None,
                formation_meta_target: None,
                dkg_completion_callback: FieldPresence::new(dkg_completion_callback),
            },
        )
        .await;
        for seat in seats {
            let InitialSeat {
                progress,
                admission,
            } = seat;
            dbtx.insert_entry(
                &SeatKey {
                    formation_id: formation_id.0.clone(),
                    index: progress.index,
                },
                &StoredSeat {
                    index: progress.index,
                    locator: progress.locator,
                    admission,
                    signed_quote: None,
                    seat_id: None,
                    guardian_fee_account: None,
                    guardian_code: None,
                    replacement_for: None,
                    replacement_previous_locator: None,
                    replacement_previous_mg_id: None,
                    replacement_approved: false,
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
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot record a seat without an active formation".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let key = SeatKey {
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

                        let mut all_created = true;
                        for sibling_index in 0..formation.seat_count {
                            if sibling_index == index {
                                continue;
                            }
                            let sibling = dbtx
                                .get_value(&SeatKey {
                                    formation_id: formation_id.0.clone(),
                                    index: sibling_index,
                                })
                                .await
                                .ok_or_else(|| {
                                    FaError::Storage(format!(
                                        "missing persisted FA seat row {sibling_index}"
                                    ))
                                })?;
                            all_created &= sibling.seat_id.is_some();
                        }
                        formation.phase = if all_created {
                            StoredFormationPhase::SeatsCreated
                        } else {
                            StoredFormationPhase::SeatsPartiallyCreated
                        };
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
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

    /// Record one guardian code without rewriting any quote or sibling row.
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
                        let formation =
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot record a guardian code without an active formation"
                                        .to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let key = SeatKey {
                            formation_id: formation_id.0.clone(),
                            index,
                        };
                        let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                            FaError::Storage(format!("missing persisted FA seat row {index}"))
                        })?;
                        if seat.seat_id.is_none() || seat.signed_quote.is_none() {
                            return Err(FaError::Storage(format!(
                                "FA seat row {index} lacks accepted-seat recovery facts"
                            )));
                        }
                        if let Some(existing) = &seat.guardian_code
                            && existing != &guardian_code
                        {
                            return Err(FaError::Storage(format!(
                                "FA seat row {index} changed guardian code"
                            )));
                        }
                        seat.guardian_code = Some(guardian_code);
                        dbtx.insert_entry(&key, &seat).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Save completed DKG and its invite without rewriting any seat row.
    pub(crate) async fn record_dkg_complete(
        &self,
        formation_id: &FormationId,
        invite_code: InviteCode,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let invite_code = invite_code.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FaError::Storage("cannot complete a missing formation".to_owned())
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let mut seats = Vec::with_capacity(usize::from(formation.seat_count));
                        for index in 0..formation.seat_count {
                            let seat = dbtx
                                .get_value(&SeatKey {
                                    formation_id: formation_id.0.clone(),
                                    index,
                                })
                                .await
                                .ok_or_else(|| {
                                    FaError::Storage(format!(
                                        "missing persisted FA seat row {index}"
                                    ))
                                })?;
                            if seat.index != index {
                                return Err(FaError::Storage(format!(
                                    "persisted FA seat row {index} contains index {}",
                                    seat.index
                                )));
                            }
                            seats.push(seat);
                        }
                        formation.phase = StoredFormationPhase::DkgComplete;
                        formation.invite_code = Some(invite_code);
                        // Every MG now owns durable delivery retry. Drop the
                        // FA-side bearer with the DKG checkpoint, retaining it
                        // through every earlier crash and resume.
                        formation.dkg_completion_callback.clear();
                        validate_formation_progress(&formation, &seats)?;
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Persist the complete formation metadata target before it is submitted.
    pub(crate) async fn record_formation_meta_target(
        &self,
        formation_id: &FormationId,
        target: FormationMetaTarget,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let target = target.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot record formation metadata for a missing formation"
                                        .to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        formation.phase = StoredFormationPhase::PublishingSeatBindings;
                        formation.formation_meta_target = Some(target);
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Checkpoint exact consensus readback of the persisted formation target:
    /// the federation is formed, and its formed record replaces the formation
    /// record.
    pub(crate) async fn confirm_formation_meta_target(
        &self,
        formation_id: &FormationId,
    ) -> FaResult<()> {
        let formation_id = formation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    Box::pin(async move {
                        let formation =
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot confirm metadata for a missing formation".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        if formation.phase != StoredFormationPhase::PublishingSeatBindings {
                            return Err(FaError::Storage(
                                "cannot complete formation before publishing metadata".to_owned(),
                            ));
                        }
                        record_formed_federation(dbtx, formation).await
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
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot store a quote without an active formation".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let key = SeatKey {
                            formation_id: formation_id.0.clone(),
                            index,
                        };
                        let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                            FaError::Storage(format!("missing persisted FA seat row {index}"))
                        })?;
                        if seat.replacement_for.is_some() && !seat.replacement_approved {
                            return Err(FaError::Storage(format!(
                                "cannot store a quote while FA seat row {index} requires replacement"
                            )));
                        }
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
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
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
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot refresh a quote without an active formation".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let key = SeatKey {
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
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
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
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot authorize payment without an active formation"
                                        .to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let mut current = Vec::new();
                        for index in 0..formation.seat_count {
                            let key = SeatKey {
                                formation_id: formation_id.0.clone(),
                                index,
                            };
                            let seat = dbtx.get_value(&key).await.ok_or_else(|| {
                                FaError::Storage(format!("missing persisted FA seat row {index}"))
                            })?;
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
                        // Never cleared: one-shot cap authorization must not
                        // silently cover a later replacement aggregate.
                        formation.payment_authorization_recorded = true;
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
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

        let mut formation = dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
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
            let key = SeatKey {
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
        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
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

        let formation = dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
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
            let key = SeatKey {
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
            if seat.replacement_for.is_some() {
                if !seat.replacement_approved {
                    return Err(FaError::Storage(format!(
                        "FA seat row {index} requires a fresh guardian replacement approval"
                    )));
                }
                // The replacement safety proof remains durable until this
                // exact admission and quote are atomically bound to their
                // irreversible effect. From this point onward the new
                // guardian is pinned to exact recovery.
                seat.replacement_for = None;
                seat.replacement_previous_locator = None;
                seat.replacement_previous_mg_id = None;
                seat.replacement_approved = false;
            }
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

    /// Restore every still-unconsumed replacement approval as one atomic
    /// fresh-preview requirement. A durable wallet reservation is cleared
    /// only with the matching witness created after authoritative wallet
    /// release.
    async fn restore_provisional_replacements(
        &self,
        formation_id: &FormationId,
        released_reservation: Option<&crate::formation::ReleasedReplacementReservation>,
        lease: &DriverLease,
    ) -> FaResult<()> {
        #[cfg(test)]
        if self.failed_restore.swap(false, Ordering::SeqCst) {
            return Err(FaError::Storage(
                "injected lease-fenced restore failure".to_owned(),
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

        let mut formation = dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
            FaError::Storage(
                "cannot restore guardian replacement without an active formation".to_owned(),
            )
        })?;
        validate_active_formation(&formation, formation_id)?;
        if !formation.payment_outputs_started {
            return Err(FaError::Storage(
                "provisional guardian restoration requires a selected post-output formation"
                    .to_owned(),
            ));
        }
        match (
            formation.payment_reservation_id.as_ref(),
            released_reservation,
        ) {
            (None, None) => {}
            (Some(stored), Some(released)) if stored == released.reservation_id() => {
                // An absence-based witness proves only that no journal
                // exists now; it clears the reservation solely under the
                // durable commitment written before that release began.
                if released.witnesses_absence() && !formation.payment_reservation_release_intended {
                    return Err(FaError::Storage(
                        "wallet absence without a durable release commitment cannot clear the \
                         reservation"
                            .to_owned(),
                    ));
                }
            }
            _ => {
                return Err(FaError::Storage(
                    "replacement wallet release does not match the durable reservation".to_owned(),
                ));
            }
        }

        let mut restored = 0usize;
        for index in 0..formation.seat_count {
            let key = SeatKey {
                formation_id: formation_id.0.clone(),
                index,
            };
            let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                FaError::Storage(format!("missing persisted FA seat row {index}"))
            })?;
            if !seat.replacement_approved {
                continue;
            }
            if seat.replacement_for.is_none()
                || seat.replacement_previous_locator.is_none()
                || seat.seat_id.is_some()
                || !seat.admission.requires_effect_authorization()
            {
                return Err(FaError::Storage(format!(
                    "provisional replacement state is inconsistent for FA seat row {index}"
                )));
            }
            seat.signed_quote = None;
            seat.replacement_approved = false;
            dbtx.insert_entry(&key, &seat).await;
            restored += 1;
        }
        if restored == 0 {
            return Err(FaError::Storage(
                "no provisional guardian replacement remains recoverable".to_owned(),
            ));
        }
        formation.payment_authorization = None;
        formation.payment_reservation_id = None;
        formation.payment_reservation_release_intended = false;
        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
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
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
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
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
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
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
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
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
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
    async fn abandon_formation(
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
            .get_value(&ActiveFormationKey)
            .await
            .ok_or_else(|| FaError::Storage("cannot abandon a missing formation".to_owned()))?;
        validate_active_formation(&formation, formation_id)?;
        if formation.payment_outputs_started {
            return Err(FaError::AbandonUnavailable(
                crate::AbandonUnavailableReason::PaymentOutputsStarted,
            ));
        }
        if formation.phase.dkg_complete() {
            return Err(FaError::AbandonUnavailable(
                crate::AbandonUnavailableReason::DkgComplete,
            ));
        }
        for index in 0..formation.seat_count {
            dbtx.remove_entry(&SeatKey {
                formation_id: formation_id.0.clone(),
                index,
            })
            .await;
        }
        dbtx.remove_entry(&ActiveFormationKey).await;
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
    /// This generic transition never manufactures a guardian-replacement
    /// fact. Any authorization for the old quote is cleared atomically.
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

    /// Mark one exact terminally refused/rejected quote for FA-owned selected
    /// guardian replacement.
    ///
    /// Callers may enter this transition only after the wallet's terminal
    /// release proof has been accepted. The store additionally requires the
    /// selected flow's payment-output tombstone, preventing generic quote
    /// refresh from being projected as replacement authority.
    #[cfg(test)]
    pub(crate) async fn mark_replacement_required(
        &self,
        formation_id: &FormationId,
        index: u16,
        expected_quote: &SignedResponse<GetQuoteResponse>,
    ) -> FaResult<()> {
        self.clear_quote_with_disposition(
            formation_id,
            index,
            expected_quote,
            QuoteClearDisposition::TerminalReplacement,
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
    pub(crate) async fn clear_terminal_quotes(
        &self,
        formation_id: &FormationId,
        expected_quotes: &[(u16, SignedResponse<GetQuoteResponse>)],
        mark_replacements: bool,
    ) -> FaResult<()> {
        let disposition = if mark_replacements {
            QuoteClearDisposition::TerminalReplacement
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
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FaError::Storage(
                                    "cannot clear a quote without an active formation".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        for (index, expected_quote) in &expected_quotes {
                            let key = SeatKey {
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
                            seat.replacement_for = match disposition {
                                QuoteClearDisposition::RefreshSameGuardian => {
                                    if !seat.replacement_approved {
                                        seat.replacement_previous_locator = None;
                                        seat.replacement_previous_mg_id = None;
                                    }
                                    seat.replacement_for
                                        .filter(|_| seat.replacement_approved)
                                }
                                QuoteClearDisposition::TerminalReplacement => {
                                    if !formation.payment_outputs_started {
                                        return Err(FaError::Storage(format!(
                                            "cannot mark FA seat row {index} for replacement outside a selected post-output formation"
                                        )));
                                    }
                                    let previous_quote_id = expected_quote
                                            .verify(&seat.locator.service_pubkey)
                                            .map(|quote| quote.quote_id())
                                            .map_err(|error| {
                                                FaError::Storage(format!(
                                                    "invalid terminal quote for replacement row {index}: {error}"
                                                ))
                                            })?;
                                    seat.replacement_previous_locator =
                                        Some(seat.locator.clone());
                                    seat.replacement_previous_mg_id = Some(seat.admission.mg_id());
                                    seat.replacement_approved = false;
                                    Some(previous_quote_id)
                                }
                            };
                            seat.signed_quote = None;
                            dbtx.insert_entry(&key, &seat).await;
                        }
                        // Invalidate the aggregate only after every exact
                        // terminal member is durably advanced in this tx.
                        formation.payment_authorization = None;
                        formation.payment_reservation_id = None;
                        formation.payment_reservation_release_intended = false;
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
                        store.wait_before_hooked_formation_commit().await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
    }

    /// Atomically replace only rows carrying the exact proven-safe
    /// replacement facts. Accepted and every other sibling are immutable.
    pub(crate) async fn replace_guardians(
        &self,
        formation_id: &FormationId,
        requirements: &GuardianReplacementRequirements,
        replacements: &[(Locator, MgAdmission)],
        max_total_msats: u64,
    ) -> FaResult<()> {
        if requirements.seats.len() != replacements.len() || requirements.seats.is_empty() {
            return Err(FaError::Storage(
                "replacement rows and verified locators differ".to_owned(),
            ));
        }
        let formation_id = formation_id.clone();
        let requirements = requirements.clone();
        let replacements = replacements.to_vec();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let requirements = requirements.clone();
                    let replacements = replacements.clone();
                    Box::pin(async move {
                        let mut formation = dbtx
                            .get_value(&ActiveFormationKey)
                            .await
                            .ok_or_else(|| FaError::Storage("no active formation".to_owned()))?;
                        validate_active_formation(&formation, &formation_id)?;
                        if !formation.payment_outputs_started {
                            return Err(FaError::Storage(
                                "guardian replacement requires a selected post-output formation"
                                    .to_owned(),
                            ));
                        }
                        if formation.payment_reservation_id.is_some() {
                            return Err(FaError::Storage(
                                "guardian replacement cannot discard a live wallet reservation"
                                    .to_owned(),
                            ));
                        }
                        let replacement_indices = requirements
                            .seats
                            .iter()
                            .map(|requirement| requirement.index)
                            .collect::<BTreeSet<_>>();
                        if replacement_indices.len() != requirements.seats.len() {
                            return Err(FaError::Storage(
                                "guardian replacement repeats a seat row".to_owned(),
                            ));
                        }
                        let mut service_pubkeys = BTreeMap::new();
                        let mut holders = BTreeSet::new();
                        for index in 0..formation.seat_count {
                            if replacement_indices.contains(&index) {
                                continue;
                            }
                            let key = SeatKey {
                                formation_id: formation_id.0.clone(),
                                index,
                            };
                            let seat = dbtx.get_value(&key).await.ok_or_else(|| {
                                FaError::Storage(format!("missing retained FA seat row {index}"))
                            })?;
                            holders.extend(seat.admission.holder());
                            if service_pubkeys
                                .insert(seat.locator.service_pubkey, index)
                                .is_some()
                            {
                                return Err(FaError::Storage(
                                    "selected formation contains duplicate retained service \
                                     signing keys"
                                        .to_owned(),
                                ));
                            }
                        }
                        for (requirement, (locator, admission)) in
                            requirements.seats.iter().zip(&replacements)
                        {
                            let key = SeatKey {
                                formation_id: formation_id.0.clone(),
                                index: requirement.index,
                            };
                            let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                                FaError::Storage(format!(
                                    "missing replacement FA seat row {}",
                                    requirement.index
                                ))
                            })?;
                            let previous_locator = seat
                                .replacement_previous_locator
                                .as_ref()
                                .unwrap_or(&seat.locator);
                            let previous_mg_id = seat
                                .replacement_previous_mg_id
                                .unwrap_or_else(|| seat.admission.mg_id());
                            if seat.seat_id.is_some()
                                || seat.signed_quote.is_some()
                                || seat.replacement_approved
                                || seat.replacement_for != Some(requirement.previous_quote_id)
                                || previous_locator != &requirement.previous_locator
                                || previous_mg_id != requirement.previous_mg_id
                            {
                                return Err(FaError::Storage(format!(
                                    "replacement fact changed for FA seat row {}",
                                    requirement.index
                                )));
                            }
                            seat.replacement_previous_locator =
                                Some(requirement.previous_locator.clone());
                            seat.replacement_previous_mg_id = Some(previous_mg_id);
                            if let Some(existing_index) =
                                service_pubkeys.insert(locator.service_pubkey, requirement.index)
                            {
                                return Err(FaError::InvalidManifoldGuardians(format!(
                                    "replacement FA seat row {} duplicates the service signing \
                                     key of seat row {existing_index}",
                                    requirement.index
                                )));
                            }
                            if let Some(holder) = admission.holder()
                                && !holders.insert(holder)
                            {
                                return Err(FaError::InvalidManifoldGuardians(
                                    "replacement guardian duplicates a retained or replacement badge holder".to_owned(),
                                ));
                            }
                            seat.locator = locator.clone();
                            seat.admission = admission.clone();
                            seat.replacement_approved = true;
                            dbtx.insert_entry(&key, &seat).await;
                        }
                        formation.payment_authorization = None;
                        formation.payment_reservation_id = None;
                        formation.payment_reservation_release_intended = false;
                        // The fresh sealed replacement approval supplies one
                        // new cap-bound authorization opportunity for exactly
                        // the newly quoted subset.
                        formation.payment_authorization_recorded = false;
                        formation.intent.max_total_msats = Some(max_total_msats);
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
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

    /// Who may hold a key share of the formed federation, besides its
    /// current MGs: the known badge holders of its seats and of its past
    /// hosts, and the past hosts' MGs.
    pub(crate) async fn formed_custody(&self) -> (BTreeSet<PublicKey>, BTreeSet<PublicKey>) {
        let Some(formed) = self
            .database
            .begin_transaction_nc()
            .await
            .get_value(&FormedFederationKey)
            .await
        else {
            return Default::default();
        };
        let mut holders = formed
            .seat_holders
            .into_iter()
            .map(|(_, holder)| holder)
            .collect::<BTreeSet<_>>();
        let mut past_mgs = BTreeSet::new();
        for host in formed.payload.past_hosts {
            past_mgs.insert(host.mg_identity);
            holders.extend(host.holder);
        }
        (holders, past_mgs)
    }

    pub(crate) async fn load_guardian_transfer(
        &self,
    ) -> Option<crate::transfer::StoredGuardianTransfer> {
        self.database
            .begin_transaction_nc()
            .await
            .get_value(&GuardianTransferKey)
            .await
    }

    async fn write_guardian_transfer(
        &self,
        fa_id: FaId,
        write: GuardianTransferWrite,
        lease: &DriverLease,
    ) -> FaResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let Some(current) = dbtx.get_value(&DriverLeaseKey).await else {
            return Err(FaError::Busy);
        };
        let now = (self.lease_clock)();
        if current.owner != lease.owner || current.expires_at <= now {
            return Err(FaError::Busy);
        }
        let open = dbtx.get_value(&GuardianTransferKey).await;
        let same = |transfer: &crate::transfer::StoredGuardianTransfer| {
            open.as_ref()
                .is_some_and(|open| open.transfer_id == transfer.transfer_id)
        };
        let published = match &write {
            GuardianTransferWrite::Begin(transfer)
            | GuardianTransferWrite::Update(transfer)
            | GuardianTransferWrite::Rebind(transfer) => {
                Some(transfer.view(transfer.public_phase()))
            }
            GuardianTransferWrite::Close => None,
            GuardianTransferWrite::Complete(transfer) => {
                Some(transfer.view(crate::transfer::GuardianTransferPhase::Completed))
            }
        };
        match write {
            GuardianTransferWrite::Begin(transfer) => {
                if open.is_some() {
                    return Err(FaError::InvalidIntent(
                        "a guardian transfer is already open; resume it instead".to_owned(),
                    ));
                }
                dbtx.insert_new_entry(&GuardianTransferKey, &transfer).await;
            }
            GuardianTransferWrite::Update(transfer) => {
                if !same(&transfer) {
                    return Err(FaError::Storage(
                        "the open guardian transfer changed under its driver".to_owned(),
                    ));
                }
                dbtx.insert_entry(&GuardianTransferKey, &transfer).await;
            }
            GuardianTransferWrite::Rebind(transfer) => {
                if !same(&transfer) {
                    return Err(FaError::Storage(
                        "the open guardian transfer changed under its driver".to_owned(),
                    ));
                }
                let crate::transfer::TransferStage::Moving(moving) = &transfer.stage else {
                    return Err(FaError::Storage(
                        "a guardian transfer rebinds before its seat is bought".to_owned(),
                    ));
                };
                rebind_federation_seat(
                    &mut dbtx,
                    fa_id,
                    transfer.seat_index,
                    &transfer.source_seat_id,
                    &moving.destination,
                )
                .await?;
                dbtx.insert_entry(&GuardianTransferKey, &transfer).await;
            }
            GuardianTransferWrite::Close | GuardianTransferWrite::Complete(_) => {
                dbtx.remove_entry(&GuardianTransferKey).await;
            }
        }
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FaError::Storage("persisting the guardian transfer failed".to_owned()))?;
        self.guardian_transfer.send_replace(published);
        #[cfg(test)]
        {
            let mut left = self.stopped_transfer_write.lock().expect("test lock");
            match *left {
                Some(1) => {
                    *left = None;
                    return Err(FaError::Storage(
                        "injected stop after a guardian transfer write".to_owned(),
                    ));
                }
                Some(count) => *left = Some(count - 1),
                None => {}
            }
        }
        Ok(())
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

impl ActiveFormationRecovery {
    pub(crate) fn payment_requirements(
        &self,
        fa_id: FaId,
    ) -> FaResult<Option<PaymentRequirements>> {
        payment_requirements(
            &self.snapshot.formation_id,
            &self.seats,
            &self.snapshot.intent,
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
            &self.snapshot.formation_id,
            &self.seats,
            &self.snapshot.intent,
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
        self.payment_authorization_recorded = true;
    }
}

fn payment_requirements(
    formation_id: &FormationId,
    recoveries: &[SeatRecovery],
    intent: &ResolvedFormationIntent,
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
        max_total_msats: intent.max_total_msats,
        seats,
    }))
}

pub(crate) fn replacement_requirements(
    formation_id: &FormationId,
    recoveries: &[SeatRecovery],
) -> Option<GuardianReplacementRequirements> {
    use fedimint_core::bitcoin::hashes::{Hash as _, sha256};

    let seats = recoveries
        .iter()
        .filter_map(|recovery| {
            if recovery.replacement_approved {
                return None;
            }
            recovery
                .replacement_for
                .map(|previous_quote_id| GuardianReplacementSeat {
                    index: recovery.progress.index,
                    // The durable field keeps the outgoing identity stable
                    // after a provisional swap advances the row's admission;
                    // the fallback covers rows persisted before it existed.
                    previous_mg_id: recovery
                        .replacement_previous_mg_id
                        .unwrap_or_else(|| recovery.admission.mg_id()),
                    previous_quote_id,
                    previous_locator: recovery
                        .replacement_previous_locator
                        .clone()
                        // Backward compatibility for the first persisted
                        // replacement rows, written before the locator was a
                        // distinct durable field.
                        .unwrap_or_else(|| recovery.progress.locator.clone()),
                })
        })
        .collect::<Vec<_>>();
    if seats.is_empty() {
        return None;
    }
    let mut preimage = Vec::new();
    preimage.extend_from_slice(b"fedi:fi-client:guardian-replacement:v1\0");
    preimage.extend_from_slice(&(formation_id.0.len() as u64).to_be_bytes());
    preimage.extend_from_slice(formation_id.0.as_bytes());
    for seat in &seats {
        preimage.extend_from_slice(&seat.index.to_be_bytes());
        preimage.extend_from_slice(&seat.previous_quote_id.0);
        preimage.extend_from_slice(&seat.previous_locator.service_pubkey.serialize());
    }
    Some(GuardianReplacementRequirements {
        replacement_id: GuardianReplacementId::from_digest(
            sha256::Hash::hash(&preimage).to_byte_array(),
        ),
        seats,
    })
}

pub(crate) fn payment_authorization_id(
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

/// Replace the transferred seat in the formed federation's seats. Repeating
/// it changes nothing.
async fn rebind_federation_seat(
    dbtx: &mut fedimint_core::db::DatabaseTransaction<'_, fedimint_core::db::Committable>,
    fa_id: FaId,
    seat_index: u16,
    source_seat_id: &SeatId,
    destination: &crate::seat_purchase::StoredDestinationSeat,
) -> FaResult<()> {
    let mut formed = dbtx
        .get_value(&FormedFederationKey)
        .await
        .ok_or(FaError::NoActiveFormation)?;
    if formed.fa_id != fa_id {
        return Err(FaError::Storage(
            "formed FA federation belongs to another scoped root".to_owned(),
        ));
    }
    let seat = formed
        .payload
        .seats
        .get_mut(usize::from(seat_index))
        .ok_or_else(|| FaError::Storage("the transferred seat index is out of range".to_owned()))?;
    if seat.seat_id != destination.seat_id {
        if &seat.seat_id != source_seat_id {
            return Err(FaError::Storage(
                "the transferred seat is no longer in the federation".to_owned(),
            ));
        }
        let source = std::mem::replace(
            seat,
            crate::backup::FaBackupSeat {
                mg_identity: destination.mg_id,
                seat_id: destination.seat_id.clone(),
                locator: destination.locator.clone(),
            },
        );
        let holder = formed
            .seat_holders
            .iter()
            .find(|(seat_id, _)| seat_id == source_seat_id)
            .map(|(_, holder)| *holder);
        formed
            .payload
            .past_hosts
            .push(crate::backup::FaBackupPastHost {
                mg_identity: source.mg_identity,
                holder,
            });
        formed
            .seat_holders
            .retain(|(seat_id, _)| seat_id != source_seat_id);
        if let Some(holder) = destination.holder {
            formed
                .seat_holders
                .push((destination.seat_id.clone(), holder));
        }
    }
    dbtx.insert_entry(&FormedFederationKey, &formed).await;
    Ok(())
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

fn validate_formation_progress(formation: &StoredFormation, seats: &[StoredSeat]) -> FaResult<()> {
    if formation.seat_count != formation.intent.federation_size.0
        || seats.len() != usize::from(formation.seat_count)
    {
        return Err(FaError::Storage(
            "persisted FA seat cardinality disagrees with formation intent".to_owned(),
        ));
    }

    let mut created = 0;
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
                None if seat.replacement_for == Some(authorized_quote_id) => authorized_quote_id,
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
        if seat.replacement_for.is_none()
            && (seat.replacement_previous_locator.is_some() || seat.replacement_approved)
        {
            return Err(FaError::Storage(format!(
                "persisted FA seat row {} has incomplete replacement recovery state",
                seat.index
            )));
        }
        if seat.replacement_approved
            && (seat.replacement_previous_locator.is_none()
                || !seat.admission.requires_effect_authorization())
        {
            return Err(FaError::Storage(format!(
                "persisted FA seat row {} has invalid provisional replacement state",
                seat.index
            )));
        }
        if seat.signed_quote.is_some()
            && seat.replacement_for.is_some()
            && !seat.replacement_approved
        {
            return Err(FaError::Storage(format!(
                "persisted FA seat row {} mixes a quote with replacement authority",
                seat.index
            )));
        }
        if seat.seat_id.is_some() && seat.replacement_for.is_some() {
            return Err(FaError::Storage(format!(
                "persisted accepted FA seat row {} retains replacement authority",
                seat.index
            )));
        }
        if seat.seat_id.is_some() {
            created += 1;
            if seat.signed_quote.is_none() {
                return Err(FaError::Storage(format!(
                    "persisted FA seat row {} has a seat without its recovery quote",
                    seat.index
                )));
            }
        }
        if seat.guardian_code.is_some() && seat.seat_id.is_none() {
            return Err(FaError::Storage(format!(
                "persisted FA seat row {} has a guardian code without an accepted seat",
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

    let seat_count = usize::from(formation.seat_count);
    let phase_matches = match formation.phase {
        StoredFormationPhase::Initialized => created == 0,
        StoredFormationPhase::SeatsPartiallyCreated => 0 < created && created < seat_count,
        StoredFormationPhase::SeatsCreated
        | StoredFormationPhase::DkgComplete
        | StoredFormationPhase::PublishingSeatBindings
        | StoredFormationPhase::Formed => created == seat_count,
    };
    if !phase_matches {
        return Err(FaError::Storage(
            "persisted FA formation phase disagrees with accepted-seat rows".to_owned(),
        ));
    }

    let needs_target = matches!(
        formation.phase,
        StoredFormationPhase::PublishingSeatBindings | StoredFormationPhase::Formed
    );
    if needs_target != formation.formation_meta_target.is_some() {
        return Err(FaError::Storage(
            "persisted formation phase disagrees with its metadata target".to_owned(),
        ));
    }
    if formation.phase.dkg_complete() {
        if formation.invite_code.is_none() {
            return Err(FaError::Storage(
                "persisted formed FA formation lacks its invite code".to_owned(),
            ));
        }
        if let Some(seat) = seats.iter().find(|seat| seat.guardian_code.is_none()) {
            return Err(FaError::Storage(format!(
                "persisted formed FA formation lacks guardian recovery facts for seat row {}",
                seat.index
            )));
        }
    } else if formation.invite_code.is_some() {
        return Err(FaError::Storage(
            "persisted nonterminal FA formation contains an invite code".to_owned(),
        ));
    }

    Ok(())
}

fn validate_payment_authorization(
    authorization: Option<&StoredPaymentAuthorization>,
    recoveries: &[SeatRecovery],
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

fn formed_backup_required() -> FaError {
    FaError::Storage("FA backup is available only after formed metadata is confirmed".to_owned())
}

fn public_phase(
    stored: StoredFormationPhase,
    recoveries: &[SeatRecovery],
    payment_required: bool,
) -> FormationPhase {
    if payment_required && !stored.dkg_complete() {
        return FormationPhase::AwaitingPaymentReadiness;
    }
    match stored {
        StoredFormationPhase::Initialized => {
            if recoveries
                .iter()
                .all(|recovery| recovery.signed_quote.is_some())
            {
                FormationPhase::AcquiringSeats
            } else {
                FormationPhase::Preparing
            }
        }
        StoredFormationPhase::SeatsPartiallyCreated => FormationPhase::AcquiringSeats,
        StoredFormationPhase::SeatsCreated => FormationPhase::PreparingDkg,
        StoredFormationPhase::DkgComplete => FormationPhase::DkgComplete,
        StoredFormationPhase::PublishingSeatBindings => FormationPhase::PublishingSeatBindings,
        StoredFormationPhase::Formed => {
            unreachable!("loading a formed formation record converts it first")
        }
    }
}

fn validate_schema(formation: &StoredFormation) -> FaResult<()> {
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
    if formation.payment_federation_id.is_some() && formation.intent.max_total_msats.is_none() {
        return Err(FaError::Storage(
            "paid FA storage record is missing its spending cap".to_owned(),
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
    if !formation.dkg_completion_callback.is_present() {
        return Err(FaError::Storage(
            "persisted FA schema 11 formation omits dkg_completion_callback".to_owned(),
        ));
    }
    Ok(())
}

fn formed_recovery(formed: FormedFederationRecord, fa_id: FaId) -> FaResult<FaRecovery> {
    if formed.fa_id != fa_id {
        return Err(FaError::Storage(
            "formed FA federation belongs to another scoped root".to_owned(),
        ));
    }
    Ok(FaRecovery::Formed(crate::FormedFederationSnapshot {
        formation_id: formed.formation_id,
        federation_invite: formed.payload.federation_invite,
        federation_name: formed.federation_name,
        seats: formed
            .payload
            .seats
            .into_iter()
            .map(|seat| crate::FormedSeat {
                mg_identity: seat.mg_identity,
                seat_id: seat.seat_id,
                locator: seat.locator,
            })
            .collect(),
        freshness: FormationFreshness::Unsynced,
        backup_eligible: formed.backup_eligible,
        origin: formed.origin,
    }))
}

/// Replace a formed formation record with the formed-federation record.
///
/// The formation record holds the setup: intent, quotes, payment state and
/// guardian codes. A formed federation needs none of them, because its
/// payment outputs are spent and its DKG is done.
async fn record_formed_federation(
    dbtx: &mut fedimint_core::db::DatabaseTransaction<'_>,
    formation: StoredFormation,
) -> FaResult<()> {
    let fa_id = formation
        .fa_id
        .ok_or_else(|| FaError::Storage("persisted FA formation has no owner".to_owned()))?;
    let mut seats = Vec::with_capacity(usize::from(formation.seat_count));
    let mut seat_holders = Vec::new();
    for index in 0..formation.seat_count {
        let key = SeatKey {
            formation_id: formation.formation_id.0.clone(),
            index,
        };
        let seat = dbtx
            .get_value(&key)
            .await
            .ok_or_else(|| FaError::Storage("formed FA record is missing a seat".to_owned()))?;
        dbtx.remove_entry(&key).await;
        let seat_id = seat
            .seat_id
            .ok_or_else(|| FaError::Storage("formed FA seat has no seat id".to_owned()))?;
        if let Some(holder) = seat.admission.holder() {
            seat_holders.push((seat_id.clone(), holder));
        }
        seats.push(crate::backup::FaBackupSeat {
            mg_identity: seat.admission.mg_id(),
            seat_id,
            locator: seat.locator,
        });
    }
    dbtx.remove_entry(&ActiveFormationKey).await;
    if dbtx.get_value(&FormedFederationKey).await.is_some() {
        return Err(FaError::Storage(
            "a formed FA federation record already exists".to_owned(),
        ));
    }
    // No backup carries generation 0. A formation without a prepared backup
    // starts at 1, so an older build that reads this record as a restored
    // backup publishes a valid generation.
    let snapshot_generation = dbtx
        .get_value(&BackupGenerationKey)
        .await
        .map_or(1, |state| state.generation);
    dbtx.insert_entry(
        &FormedFederationKey,
        &FormedFederationRecord {
            fa_id,
            payload: crate::backup::FaBackupPayload {
                schema_version: crate::backup::BACKUP_SCHEMA_VERSION,
                snapshot_generation,
                federation_invite: formation
                    .invite_code
                    .ok_or_else(|| FaError::Storage("formed FA record has no invite".to_owned()))?,
                seats,
                liquidity: None,
                past_hosts: Vec::new(),
            },
            formation_id: formation.formation_id,
            backup_eligible: true,
            federation_name: Some(formation.intent.federation_name),
            seat_holders,
            origin: crate::FormedOrigin::ThisDevice,
        },
    )
    .await;
    Ok(())
}

fn validate_schema_and_owner(formation: &StoredFormation, fa_id: FaId) -> FaResult<()> {
    validate_schema(formation)?;
    if formation.fa_id != Some(fa_id) {
        return Err(FaError::Storage(
            "persisted FA formation belongs to a different identity".to_owned(),
        ));
    }
    Ok(())
}

fn validate_active_formation(
    formation: &StoredFormation,
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

#[cfg(test)]
mod formed_record_tests {
    use super::*;

    fn record(seat_holders: Vec<(SeatId, PublicKey)>) -> serde_json::Value {
        let key = secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[7; 32]).unwrap();
        let mut value = serde_json::to_value(FormedFederationRecord {
            fa_id: FaId(key.x_only_public_key().0),
            payload: crate::backup::FaBackupPayload {
                schema_version: crate::backup::BACKUP_SCHEMA_VERSION,
                snapshot_generation: 1,
                federation_invite: crate::InviteCode("fed11test".to_owned()),
                seats: Vec::new(),
                liquidity: None,
                past_hosts: Vec::new(),
            },
            formation_id: FormationId("formation".to_owned()),
            backup_eligible: true,
            federation_name: None,
            seat_holders,
            origin: crate::FormedOrigin::ThisDevice,
        })
        .unwrap();
        value.as_object_mut().unwrap().remove("origin");
        value
    }

    #[test]
    fn a_record_from_before_origin_gets_it_from_its_badge_holders() {
        let decode = |value| serde_json::from_value::<FormedFederationRecord>(value).unwrap();
        // Only a formation on this device records badge holders.
        assert_eq!(
            decode(record(Vec::new())).origin,
            crate::FormedOrigin::Backup
        );
        let holder = PublicKey::from_slice(&[2; 32]).unwrap();
        assert_eq!(
            decode(record(vec![(
                SeatId::new("ab".repeat(32)).unwrap(),
                holder
            )]))
            .origin,
            crate::FormedOrigin::ThisDevice
        );
        // A stored origin wins over the inference.
        let mut stored = record(Vec::new());
        stored["origin"] = serde_json::json!("this_device");
        assert_eq!(decode(stored).origin, crate::FormedOrigin::ThisDevice);
    }
}
