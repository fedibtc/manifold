//! Fedimint database representation for FI recovery facts and checkpoints.

#[cfg(test)]
mod test_support;

use fedimint_core::db::{
    AutocommitError, Database, DatabaseError, DatabaseValue, DecodingError,
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
use fedi_decentralized_service_fleet_manager::{
    DkgCompletionCallback, FederationId, FiId, FormationSeatBinding, GetQuoteResponse,
    GuardianCode, GuardianFeeAccount, InviteCode, Locator, QuoteId, SeatId, SignedResponse,
    Timestamp,
};
use nostr_sdk::{Event, PublicKey};
use stability_pool_common::Account;

use crate::{
    FiError, FiResult, FiStatus, FormationActionRequired, FormationFreshness, FormationId,
    FormationPhase, FormationSnapshot, GuardianReplacementId, GuardianReplacementRequirements,
    GuardianReplacementSeat, PaymentAuthorizationId, PaymentRequirements, PaymentReservationId,
    ResolvedFormationIntent, SeatPaymentRequirement, SeatPhase, SeatProgress,
};

const STORAGE_SCHEMA_VERSION: u16 = 10;
const FORMATION_TX_MAX_ATTEMPTS: usize = 10;

#[derive(Clone, Copy)]
enum QuoteClearDisposition {
    RefreshSameGuardian,
    TerminalReplacement,
}

#[repr(u8)]
enum FiDbPrefix {
    ActiveFormation = 0x00,
    Seat = 0x01,
    DriverLease = 0x02,
    SetupPaymentFederations = 0x03,
    LiquidityOperation = 0x04,
}

#[derive(Debug, Decodable, Encodable)]
struct ActiveFormationKey;

impl_db_record!(
    key = ActiveFormationKey,
    value = StoredFormation,
    db_prefix = FiDbPrefix::ActiveFormation,
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
    db_prefix = FiDbPrefix::LiquidityOperation,
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
    db_prefix = FiDbPrefix::DriverLease,
);

#[derive(Debug, Decodable, Encodable)]
struct SetupPaymentFederationsKey;

impl_db_record!(
    key = SetupPaymentFederationsKey,
    value = StoredSetupPaymentFederationsEvent,
    db_prefix = FiDbPrefix::SetupPaymentFederations,
);

#[derive(Debug, Decodable, Encodable)]
struct SeatKey {
    formation_id: String,
    index: u16,
}

impl_db_record!(
    key = SeatKey,
    value = StoredSeat,
    db_prefix = FiDbPrefix::Seat,
);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredFormation {
    schema_version: u16,
    /// `None` exists only so schema-v4 JSON can be decoded and rejected with
    /// an actionable compatibility error instead of a generic decode failure.
    #[serde(default)]
    fi_id: Option<FiId>,
    formation_id: FormationId,
    phase: StoredFormationPhase,
    intent: ResolvedFormationIntent,
    seat_count: u16,
    /// Explicit creation contract. This is required in schema 9 so an
    /// explicitly absent selected-bootstrap payer remains distinguishable
    /// from legacy pinned formation.
    creation_mode: FormationCreationMode,
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
    /// Installation-scoped bearer callback reused verbatim for every FMan.
    /// Its semantic type redacts `Debug`; database copies remain sensitive.
    #[serde(default)]
    dkg_completion_callback: FieldPresence<DkgCompletionCallback>,
}

/// Distinguishes a missing legacy JSON field from explicit current-schema
/// `null`; schema 10 requires `Present`, while genuine schema 9 requires
/// `Missing`.
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

/// Durable discriminator for the two formation entry points.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FormationCreationMode {
    /// Legacy/API pinned-locator flow with a separate exact-quote action.
    Pinned,
    /// Product Pay-and-create flow authorized by one expiring preview.
    Selected {
        /// Explicit consumer-selected ready payer, or `None` for the
        /// verified zero-price deployment-bootstrap path. Selected mode owns
        /// this fact; the resolved formation intent remains product intent
        /// only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payment_federation_id: Option<FederationId>,
    },
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

impl FormationCreationMode {
    pub(crate) fn is_selected(&self) -> bool {
        matches!(self, Self::Selected { .. })
    }

    pub(crate) fn selected_payment_federation(&self) -> Option<&FederationId> {
        match self {
            Self::Pinned => None,
            Self::Selected {
                payment_federation_id,
            } => payment_federation_id.as_ref(),
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
#[serde(rename_all = "snake_case")]
pub(crate) enum FmanAdmission {
    Pinned,
    PeerBadge {
        fman_id: PublicKey,
        verifier_provenance: StoredVerifierProvenance,
        valid_until: Timestamp,
        state: AdmissionState,
    },
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

impl FmanAdmission {
    pub(crate) fn fresh_peer_badge(
        fman_id: PublicKey,
        verifier_provenance: StoredVerifierProvenance,
        valid_until: Timestamp,
    ) -> Self {
        Self::PeerBadge {
            fman_id,
            verifier_provenance,
            valid_until,
            state: AdmissionState::Fresh,
        }
    }

    pub(crate) fn fman_id(&self) -> Option<PublicKey> {
        match self {
            Self::Pinned => None,
            Self::PeerBadge { fman_id, .. } => Some(*fman_id),
        }
    }

    pub(crate) fn requires_effect_authorization(&self) -> bool {
        matches!(
            self,
            Self::PeerBadge {
                state: AdmissionState::Fresh,
                ..
            }
        )
    }

    pub(crate) fn mark_effect_authorized(
        &mut self,
        quote_id: QuoteId,
        effect: AdmissionEffect,
    ) -> FiResult<()> {
        self.authorize_effect(quote_id, effect)
    }

    pub(crate) fn validate_if_fresh(
        &self,
        expected_provenance: StoredVerifierProvenance,
        now: Timestamp,
    ) -> FiResult<()> {
        validate_fresh_admission(self, expected_provenance, now)
    }

    fn authorize_effect(&mut self, quote_id: QuoteId, effect: AdmissionEffect) -> FiResult<()> {
        match self {
            Self::Pinned => Ok(()),
            Self::PeerBadge { state, .. } => match state {
                AdmissionState::Fresh => {
                    *state = AdmissionState::EffectAuthorized { quote_id, effect };
                    Ok(())
                }
                AdmissionState::EffectAuthorized {
                    quote_id: current_quote,
                    effect: current_effect,
                } if *current_quote == quote_id && *current_effect == effect => Ok(()),
                AdmissionState::EffectAuthorized { .. } => Err(FiError::Storage(
                    "selected-seat admission is already bound to another effect".to_owned(),
                )),
            },
        }
    }

    fn authorized_effect(&self) -> Option<(QuoteId, AdmissionEffect)> {
        match self {
            Self::Pinned
            | Self::PeerBadge {
                state: AdmissionState::Fresh,
                ..
            } => None,
            Self::PeerBadge {
                state: AdmissionState::EffectAuthorized { quote_id, effect },
                ..
            } => Some((*quote_id, *effect)),
        }
    }

    fn require_authorized_effect(
        &self,
        quote_id: QuoteId,
        effect: AdmissionEffect,
    ) -> FiResult<()> {
        match self {
            Self::Pinned => Ok(()),
            Self::PeerBadge {
                state:
                    AdmissionState::EffectAuthorized {
                        quote_id: current_quote,
                        effect: current_effect,
                    },
                ..
            } if *current_quote == quote_id && *current_effect == effect => Ok(()),
            Self::PeerBadge { .. } => Err(FiError::Storage(
                "selected-seat effect crossed before exact admission authorization".to_owned(),
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredFormationPhase {
    Initialized,
    SeatsPartiallyCreated,
    SeatsCreated,
    Formed,
}

impl DatabaseValue for StoredFormation {
    fn from_bytes(data: &[u8], _modules: &ModuleDecoderRegistry) -> Result<Self, DecodingError> {
        serde_json::from_slice(data).map_err(DecodingError::other)
    }

    fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("StoredFormation serialization is infallible")
    }
}

/// Durable protocol facts for one selected seat.
///
/// This deliberately excludes observed freshness, payment evidence, raw
/// ecash, and wallet-private refund material. The exact signed quote is safe
/// to retain: it contains only FMan-authenticated public terms.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredSeat {
    index: u16,
    locator: Locator,
    admission: FmanAdmission,
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
    /// outgoing FMan after a provisional swap advances `admission`.
    #[serde(default)]
    replacement_previous_fman_id: Option<PublicKey>,
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
    /// The FMan-signed guardian-fee account for the first seat.
    GuardianFeeAccount,
    /// The guardian recovery code for the first seat.
    GuardianCode,
}

impl DatabaseValue for StoredSeat {
    fn from_bytes(data: &[u8], _modules: &ModuleDecoderRegistry) -> Result<Self, DecodingError> {
        serde_json::from_slice(data).map_err(DecodingError::other)
    }

    fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("StoredSeat serialization is infallible")
    }
}

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

impl DatabaseValue for StoredDriverLease {
    fn from_bytes(data: &[u8], _modules: &ModuleDecoderRegistry) -> Result<Self, DecodingError> {
        serde_json::from_slice(data).map_err(DecodingError::other)
    }

    fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("StoredDriverLease serialization is infallible")
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct StoredSetupPaymentFederationsEvent(Event);

impl DatabaseValue for StoredSetupPaymentFederationsEvent {
    fn from_bytes(data: &[u8], _modules: &ModuleDecoderRegistry) -> Result<Self, DecodingError> {
        serde_json::from_slice(data).map_err(DecodingError::other)
    }

    fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self)
            .expect("StoredSetupPaymentFederationsEvent serialization is infallible")
    }
}

impl DatabaseValue for crate::liquidity::StoredLiquidityOperation {
    fn from_bytes(data: &[u8], _modules: &ModuleDecoderRegistry) -> Result<Self, DecodingError> {
        serde_json::from_slice(data).map_err(DecodingError::other)
    }

    fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("StoredLiquidityOperation serialization is infallible")
    }
}

/// One seat's public projection and its private recovery-only quote facts.
pub(crate) struct SeatRecovery {
    pub(crate) progress: SeatProgress,
    pub(crate) admission: FmanAdmission,
    pub(crate) signed_quote: Option<SignedResponse<GetQuoteResponse>>,
    pub(crate) replacement_for: Option<QuoteId>,
    pub(crate) replacement_previous_locator: Option<Locator>,
    pub(crate) replacement_previous_fman_id: Option<PublicKey>,
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
    /// Entry-point discriminator and selected payer.
    pub(crate) creation_mode: FormationCreationMode,
    /// Exact formation metadata target and its readback checkpoint.
    pub(crate) formation_meta_target: Option<FormationMetaTarget>,
    /// Bearer callback handed to every FMan only at `StartDkg`.
    pub(crate) dkg_completion_callback: Option<DkgCompletionCallback>,
}

/// Complete semantic target replayed until formation metadata is confirmed.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct FormationMetaTarget {
    /// FI's canonical prediction, used only for consensus readback.
    pub(crate) seat_bindings: String,
    /// Exact structural request payload replayed until readback succeeds.
    pub(crate) binding_entries: Vec<FormationSeatBinding>,
    pub(crate) fi_fee_account: GuardianFeeAccount,
    pub(crate) fedi_fee_account: GuardianFeeAccount,
    pub(crate) send_ppm: u64,
    pub(crate) recipients: String,
    pub(crate) confirmed: bool,
}

/// One validated row supplied to atomic formation initialization.
pub(crate) struct InitialSeat {
    pub(crate) progress: SeatProgress,
    pub(crate) admission: FmanAdmission,
}

impl InitialSeat {
    /// Pair one selected locator with its admission, deriving the public
    /// progress row so its displayed FMan identity cannot disagree with the
    /// durable admission.
    pub(crate) fn new(index: usize, locator: Locator, admission: FmanAdmission) -> Self {
        Self {
            progress: SeatProgress {
                index: u16::try_from(index).expect("validated formation size fits u16"),
                fman_id: admission.fman_id(),
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

impl From<SeatProgress> for InitialSeat {
    fn from(mut progress: SeatProgress) -> Self {
        // A pinned admission carries no badge-vouched identity, so the
        // progress row must not claim one.
        progress.fman_id = None;
        Self {
            progress,
            admission: FmanAdmission::Pinned,
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

pub(crate) enum FiRecovery {
    Idle,
    Formation(Box<ActiveFormationRecovery>),
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
    store: FiStore,
    renewal_lock: Mutex<()>,
    released: AtomicBool,
}

impl DriverLease {
    /// Renew this driver's ownership before an external effect.
    pub(crate) async fn renew(&self) -> FiResult<()> {
        let _guard = self.renewal_lock.lock().await;
        self.store.renew_driver_lease(self).await
    }

    /// Atomically renew this owner while arming the wallet-output boundary.
    pub(crate) async fn arm_payment_outputs_started(
        &self,
        formation_id: &FormationId,
        verifier_provenance: StoredVerifierProvenance,
    ) -> FiResult<()> {
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
    ) -> FiResult<()> {
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
    ) -> FiResult<()> {
        let _guard = self.renewal_lock.lock().await;
        self.store
            .restore_provisional_replacements(formation_id, released_reservation, self)
            .await
    }

    /// Renew this owner while atomically wiping a still-abandonable formation.
    pub(crate) async fn abandon_formation(&self, formation_id: &FormationId) -> FiResult<()> {
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
pub(crate) struct FiStore {
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
}

impl FiStore {
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
        }
    }

    pub(crate) async fn insert_liquidity_operation(
        &self,
        operation: crate::liquidity::StoredLiquidityOperation,
    ) -> FiResult<()> {
        let key = LiquidityOperationKey {
            operation_id: operation.operation_id.0.clone(),
        };
        let mut dbtx = self.database.begin_transaction().await;
        if dbtx.get_value(&key).await.is_some() {
            return Err(FiError::Liquidity(
                "the exact liquidity operation already exists; resume it instead".to_owned(),
            ));
        }
        dbtx.insert_entry(&key, &operation).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FiError::Storage("persisting FI liquidity operation failed".to_owned()))
    }

    pub(crate) async fn load_liquidity_operation(
        &self,
        operation_id: &crate::LiquidityOperationId,
    ) -> FiResult<crate::liquidity::StoredLiquidityOperation> {
        self.database
            .begin_transaction_nc()
            .await
            .get_value(&LiquidityOperationKey {
                operation_id: operation_id.0.clone(),
            })
            .await
            .ok_or_else(|| FiError::Liquidity("liquidity operation not found".to_owned()))
    }

    pub(crate) async fn list_liquidity_operations(
        &self,
        after: Option<&crate::LiquidityOperationId>,
        limit: usize,
    ) -> FiResult<Vec<crate::liquidity::StoredLiquidityOperation>> {
        let mut dbtx = self.database.begin_transaction_nc().await;
        let mut entries = dbtx.find_by_prefix(&LiquidityOperationKeyPrefix).await;
        let mut operations = Vec::with_capacity(limit.saturating_add(1));
        while let Some((key, operation)) = entries.next().await {
            if after.is_some_and(|cursor| key.operation_id.as_str() <= cursor.0.as_str()) {
                continue;
            }
            if key.operation_id != operation.operation_id.0 {
                return Err(FiError::Storage(
                    "FI liquidity operation key does not match its stored identity".to_owned(),
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
    ) -> FiResult<()> {
        let key = LiquidityOperationKey {
            operation_id: operation_id.0.clone(),
        };
        let mut dbtx = self.database.begin_transaction().await;
        let mut operation = dbtx
            .get_value(&key)
            .await
            .ok_or_else(|| FiError::Storage("FI liquidity operation disappeared".to_owned()))?;
        if operation
            .response
            .as_ref()
            .is_some_and(|stored| stored != &response)
        {
            return Err(FiError::Liquidity(
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
            return Err(FiError::Liquidity(
                "provider returned a rejection conflicting with durable acceptance evidence \
                 for the exact liquidity request"
                    .to_owned(),
            ));
        }
        operation.response = Some(response);
        dbtx.insert_entry(&key, &operation).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FiError::Storage("persisting FI liquidity response failed".to_owned()))
    }

    pub(crate) async fn store_liquidity_status(
        &self,
        operation_id: &crate::LiquidityOperationId,
        status: fedi_decentralized_service_liquidity_manager::Signed<
            fedi_decentralized_service_liquidity_manager::GetAllocationStatusResponse,
        >,
    ) -> FiResult<()> {
        let key = LiquidityOperationKey {
            operation_id: operation_id.0.clone(),
        };
        let mut dbtx = self.database.begin_transaction().await;
        let mut operation = dbtx
            .get_value(&key)
            .await
            .ok_or_else(|| FiError::Storage("FI liquidity operation disappeared".to_owned()))?;
        if let Some(stored) = operation.status.as_ref() {
            if stored.payload.issued_at.0 > status.payload.issued_at.0 {
                return Err(FiError::Liquidity(
                    "provider returned a status older than the durable status".to_owned(),
                ));
            }
            if stored.payload.issued_at == status.payload.issued_at && stored != &status {
                return Err(FiError::Liquidity(
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
            .map_err(|_| FiError::Storage("persisting FI liquidity status failed".to_owned()))
    }

    pub(crate) async fn mark_liquidity_gateway_view_verified(
        &self,
        operation_id: &crate::LiquidityOperationId,
        gateway_api: &fedi_decentralized_domain::GatewayApiUrl,
    ) -> FiResult<()> {
        let key = LiquidityOperationKey {
            operation_id: operation_id.0.clone(),
        };
        let mut dbtx = self.database.begin_transaction().await;
        let mut operation = dbtx
            .get_value(&key)
            .await
            .ok_or_else(|| FiError::Storage("FI liquidity operation disappeared".to_owned()))?;
        if operation.completed_gateway_api()?.as_ref() != Some(gateway_api) {
            return Err(FiError::Liquidity(
                "gateway completion evidence changed before its LNv2 view was persisted".to_owned(),
            ));
        }
        operation.verified_gateway_api = Some(gateway_api.clone());
        dbtx.insert_entry(&key, &operation).await;
        dbtx.commit_tx_result()
            .await
            .map_err(|_| FiError::Storage("persisting gateway-view verification failed".to_owned()))
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
    /// Rewrite the stored schema version to exercise fail-closed loading.
    pub(crate) async fn downgrade_schema_for_test(&self, schema_version: u16) {
        let mut dbtx = self.database.begin_transaction().await;
        let mut formation = dbtx
            .get_value(&ActiveFormationKey)
            .await
            .expect("test formation exists");
        formation.schema_version = schema_version;
        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
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
    /// the legacy OR-arm in [`FiStore::load_recovery`].
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
    /// Rewind a complete formation while omitting one guardian recovery fact.
    pub(crate) async fn rewind_formed_without_guardian_for_test(&self) {
        let mut dbtx = self.database.begin_transaction().await;
        let mut formation = dbtx
            .get_value(&ActiveFormationKey)
            .await
            .expect("test formation exists");
        formation.phase = StoredFormationPhase::SeatsCreated;
        formation.invite_code = None;
        let key = SeatKey {
            formation_id: formation.formation_id.0.clone(),
            index: 0,
        };
        let mut seat = dbtx.get_value(&key).await.expect("test seat exists");
        seat.guardian_code = None;
        dbtx.insert_entry(&key, &seat).await;
        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
        dbtx.commit_tx().await;
    }

    pub(crate) async fn load_status(&self, fi_id: FiId) -> FiResult<FiStatus> {
        Ok(match self.load_recovery(fi_id).await? {
            FiRecovery::Idle => FiStatus::Idle,
            FiRecovery::Formation(recovery) => FiStatus::Formation(recovery.snapshot),
        })
    }

    pub(crate) async fn load_recovery(&self, fi_id: FiId) -> FiResult<FiRecovery> {
        self.migrate_schema_9(fi_id).await?;
        let mut dbtx = self.database.begin_transaction_nc().await;
        let Some(stored) = dbtx.get_value(&ActiveFormationKey).await else {
            return Ok(FiRecovery::Idle);
        };
        validate_schema_and_owner(&stored, fi_id)?;

        let mut seats = Vec::with_capacity(usize::from(stored.seat_count));
        for index in 0..stored.seat_count {
            let key = SeatKey {
                formation_id: stored.formation_id.0.clone(),
                index,
            };
            let seat = dbtx.get_value(&key).await.ok_or_else(|| {
                FiError::Storage(format!("missing persisted FI seat row {index}"))
            })?;
            if seat.index != index {
                return Err(FiError::Storage(format!(
                    "persisted FI seat row {index} contains index {}",
                    seat.index
                )));
            }
            seats.push(seat);
        }
        validate_formation_progress(&stored, &seats)?;

        let mut recoveries = Vec::with_capacity(seats.len());
        for seat in seats {
            let phase = if stored.phase == StoredFormationPhase::Formed {
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
                    fman_id: seat.admission.fman_id(),
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
                replacement_previous_fman_id: seat.replacement_previous_fman_id,
                replacement_approved: seat.replacement_approved,
                guardian_fee_account: seat.guardian_fee_account,
            });
        }

        validate_payment_authorization(stored.payment_authorization.as_ref(), &recoveries)?;
        if stored.payment_reservation_release_intended && stored.payment_reservation_id.is_none() {
            // No write path commits a release intent without its reservation
            // or clears the reservation while the intent stands, so this
            // combination cannot prove which cleanup it belongs to.
            return Err(FiError::Storage(
                "persisted FI release commitment names no wallet reservation".to_owned(),
            ));
        }
        let payment_requirements = payment_requirements(
            &stored.formation_id,
            &recoveries,
            &stored.intent,
            fi_id,
            None,
        )?;
        let payment_required = payment_requirements.as_ref().is_some_and(|requirements| {
            !authorization_covers(stored.payment_authorization.as_ref(), requirements)
        });
        let expose_payment_action = payment_required
            && match &stored.creation_mode {
                FormationCreationMode::Pinned => true,
                FormationCreationMode::Selected { .. } => stored.payment_outputs_started,
            };
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
        Ok(FiRecovery::Formation(Box::new(ActiveFormationRecovery {
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
            creation_mode: stored.creation_mode,
            payment_authorization: stored.payment_authorization,
            formation_meta_target: stored.formation_meta_target,
            dkg_completion_callback: stored.dkg_completion_callback.into_value(),
        })))
    }

    /// Upgrade the immediately preceding schema before any resume effect.
    ///
    /// Schema 9 contains every current monetary-safety fact and decodes the new
    /// optional callback as absent. Schemas 8 and earlier remain fail-closed.
    async fn migrate_schema_9(&self, fi_id: FiId) -> FiResult<()> {
        self.database
            .autocommit(
                move |dbtx, _| {
                    Box::pin(async move {
                        let Some(mut formation) = dbtx.get_value(&ActiveFormationKey).await else {
                            return Ok(());
                        };
                        if formation.schema_version != 9 {
                            return Ok(());
                        }
                        if formation.fi_id != Some(fi_id) {
                            return Err(FiError::Storage(
                                "persisted FI formation belongs to a different identity".to_owned(),
                            ));
                        }
                        if formation.dkg_completion_callback.is_present() {
                            return Err(FiError::Storage(
                                "schema 9 unexpectedly contains a DKG completion callback field"
                                    .to_owned(),
                            ));
                        }
                        formation.schema_version = STORAGE_SCHEMA_VERSION;
                        formation.dkg_completion_callback = FieldPresence::new(None);
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
                        Ok(())
                    })
                },
                Some(FORMATION_TX_MAX_ATTEMPTS),
            )
            .await
            .map_err(map_formation_tx_error)
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
    pub(crate) async fn store_setup_payment_federations_event(&self, event: Event) -> FiResult<()> {
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
            .map_err(|error: AutocommitError<FiError>| match error {
                AutocommitError::ClosureError { error, .. } => error,
                AutocommitError::CommitFailed { .. } => {
                    FiError::Storage("storing setup-payment policy transaction failed".to_owned())
                }
            })
    }

    pub(crate) async fn initialize<S>(
        &self,
        fi_id: FiId,
        formation_id: FormationId,
        intent: ResolvedFormationIntent,
        seats: Vec<S>,
        creation_mode: FormationCreationMode,
        dkg_completion_callback: Option<DkgCompletionCallback>,
    ) -> FiResult<()>
    where
        S: Into<InitialSeat>,
    {
        let seats = seats.into_iter().map(Into::into).collect::<Vec<_>>();
        let seat_count = u16::try_from(seats.len())
            .map_err(|_| FiError::Storage("too many FI seat rows".to_owned()))?;
        let mut dbtx = self.database.begin_transaction().await;
        if dbtx.get_value(&ActiveFormationKey).await.is_some() {
            return Err(FiError::Storage(
                "cannot initialize over an active FI formation".to_owned(),
            ));
        }
        dbtx.insert_entry(
            &ActiveFormationKey,
            &StoredFormation {
                schema_version: STORAGE_SCHEMA_VERSION,
                fi_id: Some(fi_id),
                formation_id: formation_id.clone(),
                phase: StoredFormationPhase::Initialized,
                intent,
                seat_count,
                creation_mode,
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
                    replacement_previous_fman_id: None,
                    replacement_approved: false,
                },
            )
            .await;
        }
        // Lease acquisition serializes initialization before any seat work can
        // run, so this transaction cannot encounter an expected formation RMW
        // conflict.
        dbtx.commit_tx_result().await.map_err(|_| {
            FiError::Storage("initializing FI formation transaction failed".to_owned())
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
    ) -> FiResult<()> {
        #[cfg(test)]
        {
            let mut failed_index = self.failed_seat_checkpoint.lock().expect("test lock");
            if *failed_index == Some(index) {
                *failed_index = None;
                return Err(FiError::Storage(
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
                                FiError::Storage(
                                    "cannot record a seat without an active formation".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let key = SeatKey {
                            formation_id: formation_id.0.clone(),
                            index,
                        };
                        let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                            FiError::Storage(format!("missing persisted FI seat row {index}"))
                        })?;
                        let signed_quote = seat.signed_quote.as_ref().ok_or_else(|| {
                            FiError::Storage(format!(
                                "accepted FI seat row {index} is missing its persisted signed quote"
                            ))
                        })?;
                        let quote =
                            signed_quote
                                .verify(&seat.locator.service_pubkey)
                                .map_err(|error| {
                                    FiError::Storage(format!(
                                        "invalid accepted quote for FI seat row {index}: {error}"
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
                            return Err(FiError::Storage(format!(
                                "FI seat row {index} changed its signed acceptance facts"
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
                                    FiError::Storage(format!(
                                        "missing persisted FI seat row {sibling_index}"
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
    ) -> FiResult<()> {
        let formation_id = formation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let guardian_code = guardian_code.clone();
                    Box::pin(async move {
                        let formation =
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FiError::Storage(
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
                            FiError::Storage(format!("missing persisted FI seat row {index}"))
                        })?;
                        if seat.seat_id.is_none() || seat.signed_quote.is_none() {
                            return Err(FiError::Storage(format!(
                                "FI seat row {index} lacks accepted-seat recovery facts"
                            )));
                        }
                        if let Some(existing) = &seat.guardian_code
                            && existing != &guardian_code
                        {
                            return Err(FiError::Storage(format!(
                                "FI seat row {index} changed guardian code"
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

    /// Mark formation complete without rewriting any seat row.
    pub(crate) async fn record_formed(
        &self,
        formation_id: &FormationId,
        invite_code: InviteCode,
    ) -> FiResult<()> {
        let formation_id = formation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let invite_code = invite_code.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FiError::Storage("cannot complete a missing formation".to_owned())
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
                                    FiError::Storage(format!(
                                        "missing persisted FI seat row {index}"
                                    ))
                                })?;
                            if seat.index != index {
                                return Err(FiError::Storage(format!(
                                    "persisted FI seat row {index} contains index {}",
                                    seat.index
                                )));
                            }
                            seats.push(seat);
                        }
                        formation.phase = StoredFormationPhase::Formed;
                        formation.invite_code = Some(invite_code);
                        // Every FMan now owns durable delivery retry. Drop the
                        // FI-side bearer atomically with the terminal formation
                        // checkpoint while preserving it across all pre-Formed
                        // crashes and resumes.
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
    ) -> FiResult<()> {
        let formation_id = formation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    let target = target.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FiError::Storage(
                                    "cannot record formation metadata for a missing formation"
                                        .to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
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

    /// Checkpoint exact consensus readback of the persisted formation target.
    pub(crate) async fn confirm_formation_meta_target(
        &self,
        formation_id: &FormationId,
    ) -> FiResult<()> {
        let formation_id = formation_id.clone();
        self.database
            .autocommit(
                move |dbtx, _| {
                    let formation_id = formation_id.clone();
                    Box::pin(async move {
                        let mut formation =
                            dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
                                FiError::Storage(
                                    "cannot confirm metadata for a missing formation".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        formation
                            .formation_meta_target
                            .as_mut()
                            .ok_or_else(|| {
                                FiError::Storage(
                                    "cannot confirm missing formation metadata".to_owned(),
                                )
                            })?
                            .confirmed = true;
                        dbtx.insert_entry(&ActiveFormationKey, &formation).await;
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
    ) -> FiResult<()> {
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
                                FiError::Storage(
                                    "cannot store a quote without an active formation".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let key = SeatKey {
                            formation_id: formation_id.0.clone(),
                            index,
                        };
                        let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                            FiError::Storage(format!("missing persisted FI seat row {index}"))
                        })?;
                        if seat.replacement_for.is_some() && !seat.replacement_approved {
                            return Err(FiError::Storage(format!(
                                "cannot store a quote while FI seat row {index} requires replacement"
                            )));
                        }
                        if let Some(existing) = &seat.signed_quote
                            && existing != &signed_quote
                        {
                            return Err(FiError::Storage(format!(
                                "refusing to replace persisted quote for FI seat row {index}"
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
    ) -> FiResult<()> {
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
                                FiError::Storage(
                                    "cannot refresh a quote without an active formation".to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        let key = SeatKey {
                            formation_id: formation_id.0.clone(),
                            index,
                        };
                        let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                            FiError::Storage(format!("missing persisted FI seat row {index}"))
                        })?;
                        if seat.seat_id.is_some()
                            || seat.signed_quote.as_ref() != Some(&expected_quote)
                        {
                            return Err(FiError::Storage(format!(
                                "persisted quote changed before refreshing FI seat row {index}"
                            )));
                        }
                        let old_id = expected_quote
                            .verify(&seat.locator.service_pubkey)
                            .map_err(|error| {
                                FiError::Storage(format!(
                                    "invalid old quote for seat {index}: {error}"
                                ))
                            })?
                            .quote_id();
                        let fresh_id = fresh_quote
                            .verify(&seat.locator.service_pubkey)
                            .map_err(|error| {
                                FiError::Storage(format!(
                                    "invalid fresh quote for seat {index}: {error}"
                                ))
                            })?
                            .quote_id();
                        let authorization =
                            formation.payment_authorization.as_mut().ok_or_else(|| {
                                FiError::Storage(format!(
                                    "FI seat row {index} has no payment authorization"
                                ))
                            })?;
                        let authorized = authorization
                            .quotes
                            .iter_mut()
                            .find(|authorized| {
                                authorized.index == index && authorized.quote_id == old_id
                            })
                            .ok_or_else(|| {
                                FiError::Storage(format!(
                                    "payment authorization does not cover FI seat row {index}"
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
    ) -> FiResult<()> {
        if authorizations.is_empty() {
            return Err(FiError::Storage(
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
                                FiError::Storage(
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
                                FiError::Storage(format!("missing persisted FI seat row {index}"))
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
                                    FiError::Storage(format!(
                                        "invalid persisted quote for FI seat row {index}: {error}"
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
                            return Err(FiError::Storage(
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
    ) -> FiResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let Some(current) = dbtx.get_value(&DriverLeaseKey).await else {
            return Err(FiError::Busy);
        };
        let now = (self.lease_clock)();
        let expires_at = now
            .checked_add(lease.renewal_duration.as_secs())
            .map(|expires_at| expires_at.min(lease.max_expires_at))
            .ok_or_else(|| FiError::Storage("FI driver lease renewal overflow".to_owned()))?;
        if current.owner != lease.owner || current.expires_at <= now || expires_at <= now {
            return Err(FiError::Busy);
        }

        let mut formation = dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
            FiError::Storage("cannot arm payment outputs without an active formation".to_owned())
        })?;
        validate_active_formation(&formation, formation_id)?;
        let authorization = formation.payment_authorization.as_ref().ok_or_else(|| {
            FiError::Storage(
                "cannot arm payment outputs without exact quote authorization".to_owned(),
            )
        })?;
        if authorization.quotes.is_empty() {
            return Err(FiError::Storage(
                "cannot arm payment outputs without exact quote authorization".to_owned(),
            ));
        }
        if formation.payment_reservation_id.is_none() {
            return Err(FiError::Storage(
                "cannot arm payment outputs without a durable wallet reservation".to_owned(),
            ));
        }
        for authorized in &authorization.quotes {
            let key = SeatKey {
                formation_id: formation_id.0.clone(),
                index: authorized.index,
            };
            let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                FiError::Storage(format!(
                    "missing persisted FI seat row {}",
                    authorized.index
                ))
            })?;
            let signed_quote = seat.signed_quote.as_ref().ok_or_else(|| {
                FiError::Storage(format!(
                    "authorized FI seat row {} is missing its quote",
                    authorized.index
                ))
            })?;
            let quote = signed_quote
                .verify(&seat.locator.service_pubkey)
                .map_err(|error| {
                    FiError::Storage(format!(
                        "invalid authorized quote for FI seat row {}: {error}",
                        authorized.index
                    ))
                })?;
            if quote.quote_id() != authorized.quote_id || quote.terms.payment.is_none() {
                return Err(FiError::Storage(format!(
                    "authorized FI seat row {} does not name an exact paid quote",
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
            .ok_or(FiError::Busy)
    }

    async fn record_seat_effects_authorized(
        &self,
        formation_id: &FormationId,
        effects: &[(u16, QuoteId, AdmissionEffect)],
        verifier_provenance: StoredVerifierProvenance,
        lease: &DriverLease,
    ) -> FiResult<()> {
        if effects.is_empty() {
            return Err(FiError::Storage(
                "cannot authorize an empty selected-seat effect wave".to_owned(),
            ));
        }
        let mut dbtx = self.database.begin_transaction().await;
        let Some(current) = dbtx.get_value(&DriverLeaseKey).await else {
            return Err(FiError::Busy);
        };
        let now = (self.lease_clock)();
        let expires_at = now
            .checked_add(lease.renewal_duration.as_secs())
            .map(|expires_at| expires_at.min(lease.max_expires_at))
            .ok_or_else(|| FiError::Storage("FI driver lease renewal overflow".to_owned()))?;
        if current.owner != lease.owner || current.expires_at <= now || expires_at <= now {
            return Err(FiError::Busy);
        }

        let formation = dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
            FiError::Storage(
                "cannot authorize a seat effect without an active formation".to_owned(),
            )
        })?;
        validate_active_formation(&formation, formation_id)?;
        if !formation.creation_mode.is_selected() {
            return Err(FiError::Storage(
                "pinned FI seat does not carry selected admission authority".to_owned(),
            ));
        }
        if effects.iter().any(|(_, _, effect)| {
            *effect == AdmissionEffect::PaidOutput && !formation.payment_outputs_started
        }) {
            return Err(FiError::Storage(
                "paid seat effect must be armed with the aggregate output boundary".to_owned(),
            ));
        }

        let mut seen = std::collections::BTreeSet::new();
        for &(index, quote_id, effect) in effects {
            if !seen.insert(index) {
                return Err(FiError::Storage(format!(
                    "selected-seat effect wave duplicates FI seat row {index}"
                )));
            }
            let key = SeatKey {
                formation_id: formation_id.0.clone(),
                index,
            };
            let mut seat = dbtx.get_value(&key).await.ok_or_else(|| {
                FiError::Storage(format!("missing persisted FI seat row {index}"))
            })?;
            let signed_quote = seat.signed_quote.as_ref().ok_or_else(|| {
                FiError::Storage(format!(
                    "FI seat row {index} has no quote for effect authorization"
                ))
            })?;
            let quote = signed_quote
                .verify(&seat.locator.service_pubkey)
                .map_err(|error| {
                    FiError::Storage(format!(
                        "invalid effect-authorized quote for FI seat row {index}: {error}"
                    ))
                })?;
            if quote.quote_id() != quote_id
                || match effect {
                    AdmissionEffect::PaidOutput => quote.terms.payment.is_none(),
                    AdmissionEffect::FreePresentation => quote.terms.payment.is_some(),
                }
            {
                return Err(FiError::Storage(format!(
                    "FI seat row {index} effect does not match its exact quote"
                )));
            }
            validate_fresh_admission(&seat.admission, verifier_provenance, Timestamp(now))?;
            seat.admission.authorize_effect(quote_id, effect)?;
            if seat.replacement_for.is_some() {
                if !seat.replacement_approved {
                    return Err(FiError::Storage(format!(
                        "FI seat row {index} requires a fresh guardian replacement approval"
                    )));
                }
                // The replacement safety proof remains durable until this
                // exact admission and quote are atomically bound to their
                // irreversible effect. From this point onward the new
                // guardian is pinned to exact recovery.
                seat.replacement_for = None;
                seat.replacement_previous_locator = None;
                seat.replacement_previous_fman_id = None;
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
            .ok_or(FiError::Busy)
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
    ) -> FiResult<()> {
        #[cfg(test)]
        if self.failed_restore.swap(false, Ordering::SeqCst) {
            return Err(FiError::Storage(
                "injected lease-fenced restore failure".to_owned(),
            ));
        }
        let mut dbtx = self.database.begin_transaction().await;
        let Some(current) = dbtx.get_value(&DriverLeaseKey).await else {
            return Err(FiError::Busy);
        };
        let now = (self.lease_clock)();
        let expires_at = now
            .checked_add(lease.renewal_duration.as_secs())
            .map(|expires_at| expires_at.min(lease.max_expires_at))
            .ok_or_else(|| FiError::Storage("FI driver lease renewal overflow".to_owned()))?;
        if current.owner != lease.owner || current.expires_at <= now || expires_at <= now {
            return Err(FiError::Busy);
        }

        let mut formation = dbtx.get_value(&ActiveFormationKey).await.ok_or_else(|| {
            FiError::Storage(
                "cannot restore guardian replacement without an active formation".to_owned(),
            )
        })?;
        validate_active_formation(&formation, formation_id)?;
        if !formation.creation_mode.is_selected() || !formation.payment_outputs_started {
            return Err(FiError::Storage(
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
                    return Err(FiError::Storage(
                        "wallet absence without a durable release commitment cannot clear the \
                         reservation"
                            .to_owned(),
                    ));
                }
            }
            _ => {
                return Err(FiError::Storage(
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
                FiError::Storage(format!("missing persisted FI seat row {index}"))
            })?;
            if !seat.replacement_approved {
                continue;
            }
            if seat.replacement_for.is_none()
                || seat.replacement_previous_locator.is_none()
                || seat.seat_id.is_some()
                || !seat.admission.requires_effect_authorization()
            {
                return Err(FiError::Storage(format!(
                    "provisional replacement state is inconsistent for FI seat row {index}"
                )));
            }
            seat.signed_quote = None;
            seat.replacement_approved = false;
            dbtx.insert_entry(&key, &seat).await;
            restored += 1;
        }
        if restored == 0 {
            return Err(FiError::Storage(
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
            .ok_or(FiError::Busy)
    }

    /// Persist the wallet's idempotent aggregate reservation before the
    /// output-generation tombstone. Repeating the same id is harmless; a
    /// different live id fails closed.
    pub(crate) async fn record_payment_reservation(
        &self,
        formation_id: &FormationId,
        reservation_id: &PaymentReservationId,
    ) -> FiResult<()> {
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
                                FiError::Storage(
                                    "cannot record reservation without an active formation"
                                        .to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        if formation.payment_authorization.is_none() {
                            return Err(FiError::Storage(
                                "cannot reserve an unauthorized payment aggregate".to_owned(),
                            ));
                        }
                        match &formation.payment_reservation_id {
                            Some(existing) if existing != &reservation_id => {
                                return Err(FiError::Storage(
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
    ) -> FiResult<()> {
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
                                FiError::Storage(
                                    "cannot commit to a release without an active formation"
                                        .to_owned(),
                                )
                            })?;
                        validate_active_formation(&formation, &formation_id)?;
                        if formation.payment_reservation_id.as_ref() != Some(&reservation_id) {
                            return Err(FiError::Storage(
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
    /// be `Formed`. Commercial authorization alone does not close the window.
    /// Free seats accepted server-side are forfeited, not released; the
    /// authenticated setup-payment policy retention is deliberately kept.
    async fn abandon_formation(
        &self,
        formation_id: &FormationId,
        lease: &DriverLease,
    ) -> FiResult<()> {
        #[cfg(test)]
        if self.failed_abandon.swap(false, Ordering::SeqCst) {
            return Err(FiError::Storage(
                "injected lease-fenced abandon failure".to_owned(),
            ));
        }
        let mut dbtx = self.database.begin_transaction().await;
        let Some(current) = dbtx.get_value(&DriverLeaseKey).await else {
            return Err(FiError::Busy);
        };
        let now = (self.lease_clock)();
        let expires_at = now
            .checked_add(lease.renewal_duration.as_secs())
            .map(|expires_at| expires_at.min(lease.max_expires_at))
            .ok_or_else(|| FiError::Storage("FI driver lease renewal overflow".to_owned()))?;
        if current.owner != lease.owner || current.expires_at <= now || expires_at <= now {
            return Err(FiError::Busy);
        }

        let formation = dbtx
            .get_value(&ActiveFormationKey)
            .await
            .ok_or_else(|| FiError::Storage("cannot abandon a missing formation".to_owned()))?;
        validate_active_formation(&formation, formation_id)?;
        if formation.payment_outputs_started {
            return Err(FiError::AbandonUnavailable(
                crate::AbandonUnavailableReason::PaymentOutputsStarted,
            ));
        }
        if formation.phase == StoredFormationPhase::Formed {
            return Err(FiError::AbandonUnavailable(
                crate::AbandonUnavailableReason::AlreadyFormed,
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
            .ok_or(FiError::Busy)
    }

    /// Clear one exact quote for a refresh on the same FMan.
    ///
    /// This generic transition never manufactures a guardian-replacement
    /// fact. Any authorization for the old quote is cleared atomically.
    pub(crate) async fn clear_quote(
        &self,
        formation_id: &FormationId,
        index: u16,
        expected_quote: &SignedResponse<GetQuoteResponse>,
    ) -> FiResult<()> {
        self.clear_quote_with_disposition(
            formation_id,
            index,
            expected_quote,
            QuoteClearDisposition::RefreshSameGuardian,
        )
        .await
    }

    /// Mark one exact terminally refused/rejected quote for FI-owned selected
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
    ) -> FiResult<()> {
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
    ) -> FiResult<()> {
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
    ) -> FiResult<()> {
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
    ) -> FiResult<()> {
        if expected_quotes.is_empty() {
            return Err(FiError::Storage(
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
                return Err(FiError::Storage(
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
                                FiError::Storage(
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
                                FiError::Storage(format!(
                                    "missing persisted FI seat row {index}"
                                ))
                            })?;
                            if seat.seat_id.is_some() {
                                return Err(FiError::Storage(format!(
                                    "cannot clear the quote for accepted FI seat row {index}"
                                )));
                            }
                            if seat.signed_quote.as_ref() != Some(expected_quote) {
                                return Err(FiError::Storage(format!(
                                    "persisted quote changed before clearing FI seat row {index}"
                                )));
                            }
                            seat.replacement_for = match disposition {
                                QuoteClearDisposition::RefreshSameGuardian => {
                                    if !seat.replacement_approved {
                                        seat.replacement_previous_locator = None;
                                        seat.replacement_previous_fman_id = None;
                                    }
                                    seat.replacement_for
                                        .filter(|_| seat.replacement_approved)
                                }
                                QuoteClearDisposition::TerminalReplacement => {
                                    if !formation.payment_outputs_started
                                        || !formation.creation_mode.is_selected()
                                    {
                                        return Err(FiError::Storage(format!(
                                            "cannot mark FI seat row {index} for replacement outside a selected post-output formation"
                                        )));
                                    }
                                    let previous_quote_id = expected_quote
                                            .verify(&seat.locator.service_pubkey)
                                            .map(|quote| quote.quote_id())
                                            .map_err(|error| {
                                                FiError::Storage(format!(
                                                    "invalid terminal quote for replacement row {index}: {error}"
                                                ))
                                            })?;
                                    seat.replacement_previous_locator =
                                        Some(seat.locator.clone());
                                    seat.replacement_previous_fman_id = seat.admission.fman_id();
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
        replacements: &[(Locator, FmanAdmission)],
        max_total_msats: u64,
    ) -> FiResult<()> {
        if requirements.seats.len() != replacements.len() || requirements.seats.is_empty() {
            return Err(FiError::Storage(
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
                            .ok_or_else(|| FiError::Storage("no active formation".to_owned()))?;
                        validate_active_formation(&formation, &formation_id)?;
                        if !formation.payment_outputs_started
                            || !formation.creation_mode.is_selected()
                        {
                            return Err(FiError::Storage(
                                "guardian replacement requires a selected post-output formation"
                                    .to_owned(),
                            ));
                        }
                        if formation.payment_reservation_id.is_some() {
                            return Err(FiError::Storage(
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
                            return Err(FiError::Storage(
                                "guardian replacement repeats a seat row".to_owned(),
                            ));
                        }
                        let mut service_pubkeys = BTreeMap::new();
                        for index in 0..formation.seat_count {
                            if replacement_indices.contains(&index) {
                                continue;
                            }
                            let key = SeatKey {
                                formation_id: formation_id.0.clone(),
                                index,
                            };
                            let seat = dbtx.get_value(&key).await.ok_or_else(|| {
                                FiError::Storage(format!("missing retained FI seat row {index}"))
                            })?;
                            if service_pubkeys
                                .insert(seat.locator.service_pubkey, index)
                                .is_some()
                            {
                                return Err(FiError::Storage(
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
                                FiError::Storage(format!(
                                    "missing replacement FI seat row {}",
                                    requirement.index
                                ))
                            })?;
                            let previous_locator = seat
                                .replacement_previous_locator
                                .as_ref()
                                .unwrap_or(&seat.locator);
                            let previous_fman_id = seat
                                .replacement_previous_fman_id
                                .or_else(|| seat.admission.fman_id());
                            if seat.seat_id.is_some()
                                || seat.signed_quote.is_some()
                                || seat.replacement_approved
                                || seat.replacement_for != Some(requirement.previous_quote_id)
                                || previous_locator != &requirement.previous_locator
                                || previous_fman_id != requirement.previous_fman_id
                            {
                                return Err(FiError::Storage(format!(
                                    "replacement fact changed for FI seat row {}",
                                    requirement.index
                                )));
                            }
                            seat.replacement_previous_locator =
                                Some(requirement.previous_locator.clone());
                            seat.replacement_previous_fman_id = previous_fman_id;
                            if let Some(existing_index) =
                                service_pubkeys.insert(locator.service_pubkey, requirement.index)
                            {
                                return Err(FiError::InvalidFleetManagers(format!(
                                    "replacement FI seat row {} duplicates the service signing \
                                     key of seat row {existing_index}",
                                    requirement.index
                                )));
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
    ) -> FiResult<DriverLease> {
        let now = (self.lease_clock)();
        let max_expires_at = now
            .checked_add(max_duration.as_secs())
            .ok_or_else(|| FiError::Storage("FI driver lease expiry overflow".to_owned()))?;
        let expires_at = now
            .checked_add(renewal_duration.as_secs())
            .map(|expires_at| expires_at.min(max_expires_at))
            .ok_or_else(|| FiError::Storage("FI driver lease renewal overflow".to_owned()))?;
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
            return Err(FiError::Busy);
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
    ) -> FiResult<()> {
        let now = (self.lease_clock)();
        now.checked_add(max_duration.as_secs())
            .and_then(|_| now.checked_add(renewal_duration.as_secs()))
            .map(drop)
            .ok_or_else(|| FiError::InvalidIntent("formation lease bound is too large".to_owned()))
    }

    /// Renew the lease only while the database still recognizes this owner.
    pub(crate) async fn renew_driver_lease(&self, lease: &DriverLease) -> FiResult<()> {
        let mut dbtx = self.database.begin_transaction().await;
        let Some(current) = dbtx.get_value(&DriverLeaseKey).await else {
            return Err(FiError::Busy);
        };
        let now = (self.lease_clock)();
        let expires_at = now
            .checked_add(lease.renewal_duration.as_secs())
            .map(|expires_at| expires_at.min(lease.max_expires_at))
            .ok_or_else(|| FiError::Storage("FI driver lease renewal overflow".to_owned()))?;
        let renewal_threshold = now
            .checked_add(lease.renewal_duration.as_secs() / 2)
            .ok_or_else(|| FiError::Storage("FI driver lease renewal overflow".to_owned()))?;
        if current.owner != lease.owner || current.expires_at <= now || expires_at <= now {
            return Err(FiError::Busy);
        }
        if renewal_threshold < current.expires_at || current.expires_at == expires_at {
            return (current.expires_at > (self.lease_clock)())
                .then_some(())
                .ok_or(FiError::Busy);
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
            .ok_or(FiError::Busy)
    }

    /// Release a lease only if it still belongs to this driver.
    pub(crate) async fn release_driver_lease(&self, lease: DriverLease) -> FiResult<()> {
        let result = self.release_driver_lease_owner(lease.owner).await;
        if result.is_ok() {
            lease.released.store(true, Ordering::Release);
        }
        result
    }

    async fn release_driver_lease_owner(&self, owner: [u8; 32]) -> FiResult<()> {
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

pub(crate) fn map_lease_commit_error(error: DatabaseError) -> FiError {
    match error {
        DatabaseError::WriteConflict => FiError::Busy,
        DatabaseError::TransactionConsumed
        | DatabaseError::DatabaseBackend(_)
        | DatabaseError::Other(_) => {
            FiError::Storage("FI driver lease database commit failed".to_owned())
        }
    }
}

/// Map a formation autocommit failure without exposing backend details.
pub(crate) fn map_formation_tx_error(error: AutocommitError<FiError>) -> FiError {
    match error {
        AutocommitError::ClosureError { error, .. } => error,
        AutocommitError::CommitFailed { .. } => {
            FiError::Storage("updating FI formation transaction failed".to_owned())
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
        fedimint_core::runtime::spawn("release cancelled FI driver lease", async move {
            let _ = store.release_driver_lease_owner(owner).await;
        });
    }
}

impl ActiveFormationRecovery {
    pub(crate) fn payment_requirements(
        &self,
        fi_id: FiId,
    ) -> FiResult<Option<PaymentRequirements>> {
        payment_requirements(
            &self.snapshot.formation_id,
            &self.seats,
            &self.snapshot.intent,
            fi_id,
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
        fi_id: FiId,
    ) -> FiResult<Option<PaymentRequirements>> {
        payment_requirements(
            &self.snapshot.formation_id,
            &self.seats,
            &self.snapshot.intent,
            fi_id,
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
    fi_id: FiId,
    accepted_authorization: Option<&StoredPaymentAuthorization>,
) -> FiResult<Option<PaymentRequirements>> {
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
                FiError::Storage(format!(
                    "invalid persisted quote for FI seat row {}: {error}",
                    recovery.progress.index
                ))
            })?;
        let request = &quote.terms.request;
        let plan_matches = intent.plan.matches(&request.plan);
        if request.fi_id != fi_id
            || request.fedimintd_version != intent.fedimintd_version
            || request.federation_size != intent.federation_size
            || !plan_matches
        {
            return Err(FiError::Storage(format!(
                "persisted quote for FI seat row {} does not match intent or identity",
                recovery.progress.index
            )));
        }
        quote.terms.check_coherent().map_err(|error| {
            FiError::Storage(format!(
                "persisted quote for FI seat row {} is incoherent: {error}",
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
            .ok_or_else(|| FiError::Storage("aggregate FI payment total overflow".to_owned()))?;
        seats.push(SeatPaymentRequirement {
            index: recovery.progress.index,
            fman_id: recovery.admission.fman_id(),
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
                    previous_fman_id: recovery
                        .replacement_previous_fman_id
                        .or_else(|| recovery.admission.fman_id()),
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
    admission: &FmanAdmission,
    expected_provenance: StoredVerifierProvenance,
    now: Timestamp,
) -> FiResult<()> {
    match admission {
        FmanAdmission::Pinned
        | FmanAdmission::PeerBadge {
            state: AdmissionState::EffectAuthorized { .. },
            ..
        } => Ok(()),
        FmanAdmission::PeerBadge {
            verifier_provenance,
            valid_until,
            state: AdmissionState::Fresh,
            ..
        } => {
            if *verifier_provenance != expected_provenance {
                return Err(FiError::SelectionReauthorizationRequired(
                    crate::SelectionReauthorizationReason::VerifierEnvironmentChanged,
                ));
            }
            if *valid_until <= now {
                return Err(FiError::SelectionReauthorizationRequired(
                    crate::SelectionReauthorizationReason::PreviewExpired,
                ));
            }
            Ok(())
        }
    }
}

fn validate_formation_progress(formation: &StoredFormation, seats: &[StoredSeat]) -> FiResult<()> {
    if formation.seat_count != formation.intent.federation_size.0
        || seats.len() != usize::from(formation.seat_count)
    {
        return Err(FiError::Storage(
            "persisted FI seat cardinality disagrees with formation intent".to_owned(),
        ));
    }

    let mut created = 0;
    let mut fman_ids = BTreeSet::new();
    let mut service_pubkeys = BTreeMap::new();
    for seat in seats {
        match (&formation.creation_mode, &seat.admission) {
            (FormationCreationMode::Pinned, FmanAdmission::Pinned) => {}
            (FormationCreationMode::Selected { .. }, FmanAdmission::PeerBadge { fman_id, .. }) => {
                if !fman_ids.insert(*fman_id) {
                    return Err(FiError::Storage(format!(
                        "persisted FI seat row {} duplicates an admitted FMan identity",
                        seat.index
                    )));
                }
            }
            (FormationCreationMode::Pinned, FmanAdmission::PeerBadge { .. })
            | (FormationCreationMode::Selected { .. }, FmanAdmission::Pinned) => {
                return Err(FiError::Storage(format!(
                    "persisted FI seat row {} admission disagrees with creation mode",
                    seat.index
                )));
            }
        }
        if let Some(existing_index) =
            service_pubkeys.insert(seat.locator.service_pubkey, seat.index)
        {
            return Err(FiError::Storage(format!(
                "persisted FI seat row {} duplicates the service signing key of seat row \
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
                                FiError::Storage(format!(
                                    "invalid effect-authorized quote for FI seat row {}: {error}",
                                    seat.index
                                ))
                            })?;
                    let payment_matches = match effect {
                        AdmissionEffect::PaidOutput => quote.terms.payment.is_some(),
                        AdmissionEffect::FreePresentation => quote.terms.payment.is_none(),
                    };
                    if !payment_matches {
                        return Err(FiError::Storage(format!(
                            "persisted FI seat row {} admission effect disagrees with its quote",
                            seat.index
                        )));
                    }
                    quote.quote_id()
                }
                None if seat.replacement_for == Some(authorized_quote_id) => authorized_quote_id,
                None => {
                    return Err(FiError::Storage(format!(
                        "persisted FI seat row {} authorized an effect without its exact quote",
                        seat.index
                    )));
                }
            };
            if persisted_quote_id != authorized_quote_id {
                return Err(FiError::Storage(format!(
                    "persisted FI seat row {} admission names another quote",
                    seat.index
                )));
            }
            if effect == AdmissionEffect::PaidOutput && !formation.payment_outputs_started {
                return Err(FiError::Storage(format!(
                    "persisted FI seat row {} authorized paid outputs before the aggregate boundary",
                    seat.index
                )));
            }
        } else if formation.creation_mode.is_selected() && seat.seat_id.is_some() {
            return Err(FiError::Storage(format!(
                "persisted accepted FI seat row {} lacks pre-effect admission authorization",
                seat.index
            )));
        }
        if seat.replacement_for.is_none()
            && (seat.replacement_previous_locator.is_some() || seat.replacement_approved)
        {
            return Err(FiError::Storage(format!(
                "persisted FI seat row {} has incomplete replacement recovery state",
                seat.index
            )));
        }
        if seat.replacement_approved
            && (seat.replacement_previous_locator.is_none()
                || !seat.admission.requires_effect_authorization())
        {
            return Err(FiError::Storage(format!(
                "persisted FI seat row {} has invalid provisional replacement state",
                seat.index
            )));
        }
        if seat.signed_quote.is_some()
            && seat.replacement_for.is_some()
            && !seat.replacement_approved
        {
            return Err(FiError::Storage(format!(
                "persisted FI seat row {} mixes a quote with replacement authority",
                seat.index
            )));
        }
        if seat.seat_id.is_some() && seat.replacement_for.is_some() {
            return Err(FiError::Storage(format!(
                "persisted accepted FI seat row {} retains replacement authority",
                seat.index
            )));
        }
        if seat.seat_id.is_some() {
            created += 1;
            if seat.signed_quote.is_none() {
                return Err(FiError::Storage(format!(
                    "persisted FI seat row {} has a seat without its recovery quote",
                    seat.index
                )));
            }
        }
        if seat.guardian_code.is_some() && seat.seat_id.is_none() {
            return Err(FiError::Storage(format!(
                "persisted FI seat row {} has a guardian code without an accepted seat",
                seat.index
            )));
        }
        // An accepted seat and its FMan-signed guardian-fee account are one
        // durable fact: neither may appear without the other. Formation
        // records persisted before the fee-account acceptance existed fail
        // this validation and must be reset, per the pre-launch namespace
        // policy of rejecting rather than migrating older records.
        if seat.seat_id.is_some() != seat.guardian_fee_account.is_some() {
            return Err(FiError::Storage(format!(
                "persisted FI seat row {} does not pair its accepted seat with the signed guardian-fee account",
                seat.index
            )));
        }
        if seat
            .guardian_fee_account
            .as_ref()
            .is_some_and(|account| GuardianFeeAccount::try_from(account.clone()).is_err())
        {
            return Err(FiError::Storage(format!(
                "persisted FI seat row {} has an invalid signed guardian-fee account",
                seat.index
            )));
        }
    }

    let seat_count = usize::from(formation.seat_count);
    let phase_matches = match formation.phase {
        StoredFormationPhase::Initialized => created == 0,
        StoredFormationPhase::SeatsPartiallyCreated => 0 < created && created < seat_count,
        StoredFormationPhase::SeatsCreated | StoredFormationPhase::Formed => created == seat_count,
    };
    if !phase_matches {
        return Err(FiError::Storage(
            "persisted FI formation phase disagrees with accepted-seat rows".to_owned(),
        ));
    }

    if formation.phase == StoredFormationPhase::Formed {
        if formation.invite_code.is_none() {
            return Err(FiError::Storage(
                "persisted formed FI formation lacks its invite code".to_owned(),
            ));
        }
        if let Some(seat) = seats.iter().find(|seat| seat.guardian_code.is_none()) {
            return Err(FiError::Storage(format!(
                "persisted formed FI formation lacks guardian recovery facts for seat row {}",
                seat.index
            )));
        }
    } else if formation.invite_code.is_some() {
        return Err(FiError::Storage(
            "persisted nonterminal FI formation contains an invite code".to_owned(),
        ));
    }

    Ok(())
}

fn validate_payment_authorization(
    authorization: Option<&StoredPaymentAuthorization>,
    recoveries: &[SeatRecovery],
) -> FiResult<()> {
    let Some(authorization) = authorization else {
        return Ok(());
    };
    if authorization.quotes.is_empty() {
        return Err(FiError::Storage(
            "persisted FI payment authorization is empty".to_owned(),
        ));
    }
    for authorized in &authorization.quotes {
        let recovery = recoveries
            .get(usize::from(authorized.index))
            .ok_or_else(|| {
                FiError::Storage(format!(
                    "persisted FI authorization names missing seat row {}",
                    authorized.index
                ))
            })?;
        let signed_quote = recovery.signed_quote.as_ref().ok_or_else(|| {
            FiError::Storage(format!(
                "persisted FI authorization names a missing quote for seat row {}",
                authorized.index
            ))
        })?;
        let quote = signed_quote
            .verify(&recovery.progress.locator.service_pubkey)
            .map_err(|error| {
                FiError::Storage(format!(
                    "invalid authorized quote for FI seat row {}: {error}",
                    authorized.index
                ))
            })?;
        if quote.quote_id() != authorized.quote_id || quote.terms.payment.is_none() {
            return Err(FiError::Storage(format!(
                "persisted FI authorization does not match seat row {}",
                authorized.index
            )));
        }
    }
    Ok(())
}

fn public_phase(
    stored: StoredFormationPhase,
    recoveries: &[SeatRecovery],
    payment_required: bool,
) -> FormationPhase {
    if payment_required && stored != StoredFormationPhase::Formed {
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
        StoredFormationPhase::Formed => FormationPhase::Formed,
    }
}

fn validate_schema(formation: &StoredFormation) -> FiResult<()> {
    if formation.schema_version != STORAGE_SCHEMA_VERSION {
        let guidance = match formation.schema_version {
            4 => {
                "; schema 4 predates durable FI identity binding and cannot be migrated safely; \
                 reset this unreleased FI namespace and restart formation"
            }
            5 => {
                "; schema 5 predates the post-DKG seat-binding directory and cannot be migrated \
                 safely; reset this unreleased FI namespace and restart formation"
            }
            6 => {
                "; schema 6 predates durable commercial-authorization history and cannot be \
                 migrated safely; reset this unreleased FI namespace and restart formation"
            }
            7 => {
                "; schema 7 predates the distinct wallet-output-generation tombstone and cannot \
                 be migrated safely; reset this unreleased FI namespace and restart formation"
            }
            8 => {
                "; schema 8 does not explicitly distinguish selected Pay-and-create recovery \
                 from pinned formation and cannot be migrated safely; reset this unreleased FI \
                 namespace and restart formation"
            }
            _ => "",
        };
        return Err(FiError::Storage(format!(
            "unsupported FI storage schema version {}{guidance}",
            formation.schema_version,
        )));
    }
    if formation.fi_id.is_none() {
        return Err(FiError::Storage(
            "current FI storage record is missing its owner identity".to_owned(),
        ));
    }
    match &formation.creation_mode {
        FormationCreationMode::Pinned => {}
        FormationCreationMode::Selected {
            payment_federation_id,
        } => {
            if payment_federation_id.is_some() && formation.intent.max_total_msats.is_none() {
                return Err(FiError::Storage(
                    "paid selected FI storage record is missing its spending cap".to_owned(),
                ));
            }
        }
    }
    if !formation.dkg_completion_callback.is_present() {
        return Err(FiError::Storage(
            "persisted FI schema 10 formation omits dkg_completion_callback".to_owned(),
        ));
    }
    Ok(())
}

fn validate_schema_and_owner(formation: &StoredFormation, fi_id: FiId) -> FiResult<()> {
    validate_schema(formation)?;
    if formation.fi_id != Some(fi_id) {
        return Err(FiError::Storage(
            "persisted FI formation belongs to a different identity".to_owned(),
        ));
    }
    Ok(())
}

fn validate_active_formation(
    formation: &StoredFormation,
    expected_id: &FormationId,
) -> FiResult<()> {
    validate_schema(formation)?;
    if formation.formation_id != *expected_id {
        return Err(FiError::Storage(
            "active FI formation changed during an operation".to_owned(),
        ));
    }
    Ok(())
}
