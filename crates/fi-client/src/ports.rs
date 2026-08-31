//! Consumer and transport capability ports.

use std::sync::Arc;

use fedi_decentralized_domain::BitcoinNetwork;
use fedi_decentralized_service_fleet_manager::{
    FederationId, FleetManagerService, FmResult, GetAvailabilityRequest, GetAvailabilityResponse,
    GetQuoteRequest, GetQuoteResponse, InviteCode, Locator, LockedBlindedSignature, MintGeneration,
    Plan, RefundIssuance, RefundTransaction, SignatureVerified, SignedResponse,
};
use fedi_decentralized_service_liquidity_manager::PublicLiquidityApi;
use fedi_iroh_rpc::iroh::EndpointAddr;
use fedimint_core::config::ClientConfig;
use fedimint_core::task::{MaybeSend, MaybeSync};

use crate::{
    FedimintFederationId, FiError, FiResult, GuardianFeeAccount, PaymentRequirements,
    PaymentReservationId, SeatPaymentRequirement,
};

/// One verified paid quote bound to its public requirement by `fi-client`.
///
/// The fields are intentionally sealed and the type is not serializable. A
/// payment adapter cannot accidentally zip two independently ordered slices or
/// substitute an unrelated 32-byte value for the protocol-owned quote id.
pub struct ExactSeatPaymentPreflight<'a> {
    requirement: &'a SeatPaymentRequirement,
    quote: &'a SignatureVerified<GetQuoteResponse>,
}

impl ExactSeatPaymentPreflight<'_> {
    /// Consumer-facing requirement for this exact quote.
    #[must_use]
    pub fn requirement(&self) -> &SeatPaymentRequirement {
        self.requirement
    }

    /// Signature-verified quote whose locked outputs and fees must be checked.
    #[must_use]
    pub fn quote(&self) -> &SignatureVerified<GetQuoteResponse> {
        self.quote
    }

    /// Semantic quote identity used only at the wallet operation boundary.
    #[must_use]
    pub fn quote_id(&self) -> fedi_decentralized_service_fleet_manager::QuoteId {
        self.requirement.quote_id
    }
}

/// Exact aggregate funding preflight assembled by `fi-client`.
///
/// Each seat binds a requirement to the verified quote that generated it.
/// Aggregate total and cap remain available without reconstructing policy in a
/// wallet adapter. This value is read-only and non-serializable.
pub struct ExactPaymentPreflight<'a> {
    seats: Vec<ExactSeatPaymentPreflight<'a>>,
    total_msats: u64,
    max_total_msats: Option<u64>,
}

impl<'a> ExactPaymentPreflight<'a> {
    pub(crate) fn new(
        requirements: &'a PaymentRequirements,
        quotes: &'a [SignatureVerified<GetQuoteResponse>],
    ) -> FiResult<Self> {
        if requirements.seats.len() != quotes.len() {
            return Err(FiError::Storage(
                "payment requirement and verified quote counts differ".to_owned(),
            ));
        }
        let unique_quote_ids = requirements
            .seats
            .iter()
            .map(|requirement| requirement.quote_id)
            .collect::<std::collections::BTreeSet<_>>();
        if unique_quote_ids.len() != requirements.seats.len() {
            return Err(FiError::Storage(
                "payment aggregate contains duplicate semantic quote ids".to_owned(),
            ));
        }
        let seats = requirements
            .seats
            .iter()
            .zip(quotes)
            .map(|(requirement, quote)| {
                if quote.quote_id() != requirement.quote_id {
                    return Err(FiError::Storage(
                        "payment requirement is not bound to its verified quote".to_owned(),
                    ));
                }
                Ok(ExactSeatPaymentPreflight { requirement, quote })
            })
            .collect::<FiResult<Vec<_>>>()?;
        Ok(Self {
            seats,
            total_msats: requirements.total_msats,
            max_total_msats: requirements.max_total_msats,
        })
    }

    /// Exact paid seats in stable formation order.
    #[must_use]
    pub fn seats(&self) -> &[ExactSeatPaymentPreflight<'a>] {
        &self.seats
    }

    /// Checked aggregate quote face value.
    #[must_use]
    pub fn total_msats(&self) -> u64 {
        self.total_msats
    }

    /// User-approved aggregate cap, when this flow has one.
    #[must_use]
    pub fn max_total_msats(&self) -> Option<u64> {
        self.max_total_msats
    }
}

/// Sanitized failure to resolve the FI's own fee-recipient account.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct FiFeeAccountError {
    message: String,
}

impl FiFeeAccountError {
    /// Create a sanitized account-resolution error.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Consumer-owned access to the FI's own formed-federation fee account.
///
/// `fi-client` supplies the federation id parsed from its durable formed
/// invite. Implementations must resolve the already joined client for that
/// exact federation and return its own single-signature SPv2 `BtcDepositor`
/// account. This is a local capability lookup: it must not perform network or
/// value-moving work.
///
/// `fi-client` validates the returned account and owns the complete recipient
/// policy. A consumer must not select an account from caller input at this
/// boundary; development consumers that deliberately accept a test override
/// must document that weaker source explicitly.
///
/// The target-aware bounds retain `Send + Sync` on native targets and relax
/// only for the single-threaded `wasm32-unknown-unknown` consumer runtime.
pub trait FiFeeAccountProvider: MaybeSend + MaybeSync + 'static {
    /// Resolve this FI consumer's account for the exact formed federation.
    fn formed_federation_fee_account(
        &self,
        federation_id: &FedimintFederationId,
    ) -> Result<GuardianFeeAccount, FiFeeAccountError>;
}

/// Sanitized consumer payment error.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct FiPaymentError {
    kind: FiPaymentErrorKind,
    message: String,
}

/// Recover-only result for one exact aggregate wallet reservation.
///
/// `Absent` is authoritative: the wallet checked the deterministic id and
/// exact preflight without creating a journal. Any binding mismatch, storage
/// failure, or ambiguous read is an error instead.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum PaymentReservationRecovery<R> {
    /// The exact durable reservation already exists and can be resumed.
    Existing(R),
    /// The exact deterministic reservation does not exist.
    Absent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FiPaymentErrorKind {
    Other,
    DefinitiveInsufficientFundsWithoutReservation,
}

impl FiPaymentError {
    /// Create a sanitized payment error with no value-safe cleanup proof.
    ///
    /// Reservation binding mismatches, storage failures, and ambiguous or
    /// lost results after a same-id journal may have been persisted must use
    /// this constructor. The formation remains durable so the exact reserve
    /// operation can be retried or reconstructed.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            kind: FiPaymentErrorKind::Other,
            message: message.into(),
        }
    }

    /// Report definite insufficient funds before creating a reservation.
    ///
    /// This classification is the wallet adapter's proof that the reserve
    /// operation failed its balance check before it created or observed any
    /// durable journal for this reservation id. It is the only reserve error
    /// that permits selected formation to return to `Idle`. Never use it for
    /// a same-id binding mismatch, a storage error, or any response whose loss
    /// could hide a successfully persisted journal.
    #[must_use]
    pub fn insufficient_funds_without_reservation(message: impl Into<String>) -> Self {
        Self {
            kind: FiPaymentErrorKind::DefinitiveInsufficientFundsWithoutReservation,
            message: message.into(),
        }
    }

    pub(crate) fn proves_insufficient_funds_without_reservation(&self) -> bool {
        self.kind == FiPaymentErrorKind::DefinitiveInsufficientFundsWithoutReservation
    }
}

/// Consumer wallet operations required by formation.
pub trait FiPayments: Send + Sync + 'static {
    /// Consumer-owned state needed only if a paid presentation is refused.
    ///
    /// It deliberately has no serialization bound: refund secrets stay in
    /// the wallet implementation and never enter `fi-client` storage.
    type RefundContext: Send;

    /// Wallet-private ownership proof for one exact aggregate reservation.
    ///
    /// The wallet reconstructs the same capability when the deterministic
    /// reservation id is presented after a crash. `fi-client` carries it
    /// across the aggregate but starts new per-seat value movement strictly
    /// one member at a time.
    type PaymentReservation: Clone + Send + Sync;

    /// Wallet-owned proof that one exact aggregate member is terminally safe
    /// to release. Callers cannot synthesize this from ids or quote bytes.
    type TerminalReleaseProof: Send;

    /// Return admitted federation ids the consumer can currently pay from.
    ///
    /// The input is the authenticated common policy set. The wallet reports
    /// capability only; output order does not influence selection policy.
    async fn payable_federations(
        &self,
        admitted: &[FederationId],
    ) -> Result<Vec<FederationId>, FiPaymentError>;

    /// Recover one exact aggregate reservation without creating wallet state.
    ///
    /// This probe runs before selected-admission freshness checks so a wallet
    /// journal whose successful creation response was lost can still be
    /// reconstructed and released after the preview expires or the verifier
    /// environment changes. Implementations must validate an existing
    /// journal against `reservation_id` and the complete exact preflight.
    /// Return [`PaymentReservationRecovery::Absent`] only after an
    /// authoritative lookup proves that no same-id journal exists. Binding
    /// mismatch, storage failure, and ambiguous lookup use
    /// [`FiPaymentError::new`]; this method must never create, fund, reserve,
    /// or submit anything.
    async fn recover_payment_reservation(
        &self,
        reservation_id: &PaymentReservationId,
        preflight: &ExactPaymentPreflight<'_>,
    ) -> Result<PaymentReservationRecovery<Self::PaymentReservation>, FiPaymentError>;

    /// Durably reserve one exact aggregate without starting payment outputs.
    ///
    /// This value-free preflight runs after every signed quote has been
    /// verified. Implementations must check the selected wallet can cover the
    /// aggregate face value, transaction fees, and required reserve.
    /// The verified quotes expose the exact locked-output generations and
    /// issuance sets needed to calculate wallet fees; they correspond one for
    /// one, in seat order, with `requirements.seats`. Implementations must not
    /// generate outputs or spend. They journal an idempotent reservation under
    /// `reservation_id`, binding the payer, exact signed quotes/output plans,
    /// virtual value, fee-aware logical debit allocations, and required
    /// reserve. An adapter may independently dry-run each exact member to
    /// calculate those allocations, but the aggregate check must be based on
    /// total spendable value and must not require disjoint physical notes for
    /// members that `fi-client` settles sequentially. At submission it must
    /// re-check the current transaction's net cost under its global spend
    /// guard and must not commit a payment that would leave any sibling
    /// allocation or required reserve unfunded after change. Repeating the
    /// same id and exact
    /// preflight reconstructs the reservation after a crash; the same id with
    /// different terms fails closed. `fi-client` persists the id before it
    /// arms the output boundary. Return
    /// [`FiPaymentError::insufficient_funds_without_reservation`] only when a
    /// balance shortfall was proven before creating or observing a durable
    /// same-id journal. Binding mismatches, storage failures, and ambiguous or
    /// lost post-journal results use [`FiPaymentError::new`] so recovery keeps
    /// the formation and retries this exact id.
    async fn reserve_payment_requirements(
        &self,
        reservation_id: &PaymentReservationId,
        preflight: &ExactPaymentPreflight<'_>,
    ) -> Result<Self::PaymentReservation, FiPaymentError>;

    /// Release a reservation only after the wallet proves that no named
    /// payment output started and no value or recovery entitlement depends on
    /// it. A successful return is the consumer-owned wallet's authoritative
    /// proof that the exact reconstructed reservation is gone; `fi-client`
    /// may then clear only the matching durable reservation id under its
    /// driver lease. An error, timeout, cancellation, or dropped future proves
    /// nothing and must retain the id and exact recovery state. Dropping a
    /// capability is never release; ambiguity fails closed.
    async fn release_payment_reservation(
        &self,
        reservation: Self::PaymentReservation,
    ) -> Result<(), FiPaymentError>;

    /// Idempotently release one exact member after the wallet has proved its
    /// funding operation rejected and every automatically refunded payer input
    /// is spendable again, or its FMan-signed refund was settled.
    /// Other aggregate members remain consumed or reserved. This must finish
    /// before `fi-client` invalidates the old reservation id for replacement.
    async fn release_seat_payment_reservation(
        &self,
        proof: Self::TerminalReleaseProof,
    ) -> Result<(), FiPaymentError>;

    /// Form the FI's mint-generation belief and quote-request refund outputs.
    ///
    /// This runs while the formation is still value-safe — before wallet
    /// output generation is durably armed — so it **must commit no
    /// value and create no wallet state whose loss would strand funds**:
    /// generating blind nonces and reporting the mint generation is fine,
    /// but nothing may be spent, locked, or made unrecoverable-if-forgotten.
    /// [`FiClient::abandon_formation`](crate::FiClient::abandon_formation)
    /// wipes all FI formation state without consulting the wallet whenever
    /// output generation was never durably armed
    /// (`specs/ARCH-fi-client.md`); an implementation that
    /// committed value here would silently break that value-safety
    /// boundary.
    async fn prepare_quote_refund(
        &self,
        federation_id: &FederationId,
        plan: &Plan,
    ) -> Result<RefundIssuance, FiPaymentError>;

    /// Recover an existing payment attempt for an exact verified quote.
    ///
    /// This is a read/reconstruction-only operation: [`SeatPaymentRecovery::Prepared`]
    /// returns the exact payment evidence and matching refund context needed
    /// to replay `CreateSeat` when funding for this quote already exists.
    /// Wallets must durably establish this recoverability before committing
    /// any spend. `Prepared` may be returned only after consensus accepted the
    /// exact transaction and every payer-owned primary-module change output is
    /// final and spendable; recovery must await that same finality rather than
    /// submit a replacement operation. A terminally rejected funding
    /// operation must be distinguished from an operation that is still
    /// pending or recoverable. `Rejected` is terminal only after all exact
    /// inputs consumed by the rejected transaction have been restored by
    /// accepted refund transactions and every resulting payer output is final
    /// and spendable, so `fi-client` can safely replace that exact quote.
    async fn recover_seat_payment(
        &self,
        reservation_id: &PaymentReservationId,
        quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<SeatPaymentRecovery<Self::RefundContext, Self::TerminalReleaseProof>, FiPaymentError>;

    /// Start funding a verified paid quote and prepare its `CreateSeat`
    /// presentation.
    ///
    /// `fi-client` handles free quotes itself and calls this method only for a
    /// freshly journaled paid quote or after a recover-only probe proved that
    /// no funding began. Before committing any spend, the wallet must make the
    /// exact presentation and matching refund context recoverable through
    /// [`FiPayments::recover_seat_payment`]. A successful return has the same
    /// finality contract as recovered `Prepared`: the exact Fedimint
    /// transaction is accepted and its payer-owned change is already
    /// spendable, so `fi-client` may durably checkpoint this member and start
    /// the next one.
    async fn create_seat_payment(
        &self,
        reservation: &Self::PaymentReservation,
        quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<PreparedSeatPayment<Self::RefundContext>, FiPaymentError>;

    /// Settle an FMan-signed refusal into the consumer wallet.
    ///
    /// The exact same context/refund replay must be idempotent. Formation can
    /// be interrupted after the wallet settles but before `fi-client` clears
    /// its quote, and recovery will invoke this operation again. A successful
    /// return means the recovered refund value is spendable, not merely
    /// accepted by consensus.
    async fn settle_seat_refund(
        &self,
        context: Self::RefundContext,
        refund: RefundTransaction,
    ) -> Result<SettledSeatRefund<Self::TerminalReleaseProof>, FiPaymentError>;
}

/// A paid `CreateSeat` presentation plus wallet-private refund state.
pub struct PreparedSeatPayment<R> {
    /// Aggregate signatures over the quote's issuance outputs.
    pub payment_signatures: Vec<LockedBlindedSignature>,
    /// The protocol the wallet actually settled this payment under.
    ///
    /// The wallet states it so `fi-client` can compare it against the quote's
    /// signed `PaymentTerms` before presenting anything: a wallet that
    /// dispatched on something other than those terms has locked funds under
    /// the wrong protocol, and the payer catches that rather than the FMan
    /// refusing it.
    pub settled_under: MintGeneration,
    /// Private material the wallet needs if the presentation is refused.
    pub refund_context: R,
}

/// Authoritative wallet recovery outcome for one exact quote.
pub enum SeatPaymentRecovery<R, T> {
    /// The wallet can prove no funding operation began.
    NotStarted,
    /// Funding exists and its exact presentation can be replayed.
    Prepared(PreparedSeatPayment<R>),
    /// The funding operation was rejected, its original inputs were restored
    /// by accepted refund transactions, and every restored payer output is
    /// spendable, so the quote may safely be replaced.
    Rejected(T),
}

/// A completed signed refund plus wallet-owned terminal release authority.
pub struct SettledSeatRefund<T> {
    /// Value restored to the payer wallet.
    pub amount_msats: u64,
    /// Opaque proof that the exact aggregate member may now be released.
    pub release_proof: T,
}

/// Sanitized FMan connection failure.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct FleetManagerConnectorError {
    message: String,
}

impl FleetManagerConnectorError {
    /// Create a sanitized connection error message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Sanitized local failure while calling an already-connected FMan client.
///
/// This error is deliberately outside the serialized Fleet Manager service
/// result. Only a consumer-owned connector adapter can report it, so a remote
/// service response cannot manufacture transport retry authority.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct FleetManagerCallError {
    message: String,
}

impl FleetManagerCallError {
    /// Create a sanitized local call error message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Produces clients implementing the existing FMan service contract.
pub trait FleetManagerConnector: Send + Sync + 'static {
    /// Concrete transport or mock client.
    type Client: FleetManagerService;

    /// Connect using a selected locator.
    ///
    /// The locator keeps dialing data and the commitment-verification key in
    /// one protocol-owned value, preventing transport adapters from silently
    /// dropping the identity half of the connection contract.
    async fn connect(&self, locator: &Locator) -> Result<Self::Client, FleetManagerConnectorError>;

    /// Call the value-free availability verb while retaining local transport
    /// failure outside the serialized service result.
    async fn get_availability(
        &self,
        client: &Self::Client,
        request: GetAvailabilityRequest,
    ) -> Result<FmResult<GetAvailabilityResponse>, FleetManagerCallError>;

    /// Call the value-free quote verb while retaining local transport failure
    /// outside the serialized service result.
    async fn get_quote(
        &self,
        client: &Self::Client,
        request: GetQuoteRequest,
    ) -> Result<FmResult<SignedResponse<GetQuoteResponse>>, FleetManagerCallError>;
}

/// Sanitized FLIP connection failure.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct LiquidityProviderConnectorError {
    message: String,
}

impl LiquidityProviderConnectorError {
    /// Create a sanitized transport error.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Consumer-owned transport for the provider-signed Iroh endpoint.
///
/// The endpoint address carries the node identity authenticated by Iroh; the
/// generated client is fixed to `PUBLIC_LIQUIDITY_API_ALPN` by the consumer's
/// adapter.  `fi-client` independently verifies every provider response.
pub trait LiquidityProviderConnector: Send + Sync {
    /// Concrete generated transport client or test double.
    type Client: PublicLiquidityApi;

    /// Dial the exact endpoint admitted from the fresh signed advertisement.
    async fn connect(
        &self,
        endpoint: &EndpointAddr,
    ) -> Result<Self::Client, LiquidityProviderConnectorError>;
}

/// Sanitized consensus-read failure.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct FederationConsensusError {
    message: String,
}

impl FederationConsensusError {
    /// Create a sanitized consensus-read error message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Undigested result of one federation consensus read.
///
/// Deliberately carries the downloaded config and the raw metadata object
/// rather than anything derived from them. `fi-client` performs every
/// derivation and every signature check itself, so no trust conclusion crosses
/// the capability boundary.
#[derive(Clone, Debug)]
pub struct FederationConsensusSnapshot {
    /// Final client config, as downloaded and threshold-agreed.
    pub config: ClientConfig,

    /// Raw `meta` module consensus value: the whole JSON object of meta
    /// fields, not any single field within it. `None` when the federation has
    /// no meta module or no value set.
    pub meta_value: Option<Vec<u8>>,

    /// The meta module's monotone consensus revision for `meta_value`,
    /// reported by the same `get_consensus` read that produced it — never a
    /// separate query, which could pair a stale revision with fresh bytes.
    /// `Some` exactly when `meta_value` is `Some`; the driver refuses an
    /// unpaired combination. The revision is what distinguishes two
    /// byte-identical occurrences of a board state, so metadata mutations
    /// rebased on this snapshot cannot re-match an earlier occurrence's
    /// admissions.
    pub meta_revision: Option<u64>,

    /// Bitcoin network decoded from the final wallet module config by the
    /// consumer's real consensus reader.  This is raw config-derived input,
    /// not a trust verdict; `fi-client` still validates it against the
    /// selected provider policy before disclosing the invite.
    pub network: BitcoinNetwork,
}

/// Consumer-owned read of a federation's consensus state.
///
/// `fi-client` depends on `fedimint-core` alone and stays
/// `wasm32-unknown-unknown`-compatible, so it cannot dial guardians itself.
/// The consumer performs invite-code reads. [`Self::read_consensus`] returns
/// raw config and metadata material; `fi-client` derives the peer set, verifies
/// every attestation signature, and compares against what it wrote.
/// [`Self::read_lnv2_gateways`] performs the upstream LNv2 threshold strategy
/// and returns only individually valid public gateway endpoints. Target
/// membership remains an `fi-client` decision.
///
/// # Implementation obligation
///
/// The implementation **must** perform real, uncached queries: download the
/// config through the invite code, read the `meta` module's consensus value
/// across peers, and aggregate the LNv2 gateway view from a threshold of peer
/// responses. An unrelated LNv2 value outside
/// [`GatewayApiUrl`](fedi_decentralized_domain::GatewayApiUrl) policy is
/// omitted individually rather than turning target absence into a transport
/// failure. `fedi-decentralized-federation-preview` provides these operations.
///
/// `fi-client` cannot verify that the query happened. Fedimint's
/// `get_consensus` derives its guarantee from the client performing threshold
/// agreement across peers, and the response carries no signatures, so nothing
/// in the returned bytes distinguishes a genuine read from a value echoed back
/// from the write that preceded it. An implementation that fabricates or
/// caches a snapshot defeats the post-DKG readback in
/// [`SPEC-federation-trust-directory`](../../domain/specs/SPEC-federation-trust-directory.md)
/// silently and in the dangerous direction: the FI would believe the directory
/// reached consensus when it had not.
pub trait FederationConsensusReader: Send + Sync + 'static {
    /// Read the federation's current consensus state through its invite code.
    async fn read_consensus(
        &self,
        invite_code: &InviteCode,
    ) -> Result<FederationConsensusSnapshot, FederationConsensusError>;

    /// Read a fresh upstream threshold-aggregated LNv2 gateway view.
    ///
    /// Implementations omit unrelated entries outside the shared public
    /// [`GatewayApiUrl`](fedi_decentralized_domain::GatewayApiUrl) policy. They
    /// must not fabricate or cache target membership.
    ///
    /// # Errors
    ///
    /// Returns [`FederationConsensusError`] when the invite or final config is
    /// invalid, the federation has no LNv2 module, or a threshold view cannot
    /// be obtained. A single unrelated entry outside `GatewayApiUrl` policy is
    /// omitted from an otherwise successful view rather than returned as an
    /// error.
    async fn read_lnv2_gateways(
        &self,
        invite_code: &InviteCode,
    ) -> Result<Vec<fedi_decentralized_domain::GatewayApiUrl>, FederationConsensusError>;
}

pub(crate) struct FiClientPorts<P, N, F, C> {
    pub(crate) identity: crate::identity::FiKeys,
    pub(crate) payments: P,
    pub(crate) registry: N,
    pub(crate) fman_connector: F,
    pub(crate) consensus_reader: C,
    pub(crate) fi_fee_account_provider: Arc<dyn FiFeeAccountProvider>,
}
