//! Consumer and transport capability ports.

use std::sync::Arc;

use fedi_decentralized_domain::BitcoinNetwork;
use fedi_decentralized_service_liquidity_manager::PublicLiquidityApi;
use fedi_decentralized_service_manifold_guardian::{
    FaId, FaSignature, FederationId, FmResult, GetAvailabilityRequest, GetAvailabilityResponse,
    GetQuoteRequest, GetQuoteResponse, InviteCode, Locator, LockedBlindedSignature,
    ManifoldGuardianService, MintGeneration, Plan, RefundIssuance, RefundTransaction,
    SignatureVerified, SignedResponse,
};
use fedi_iroh_rpc::iroh::EndpointAddr;
use fedimint_core::config::ClientConfig;
use fedimint_core::task::{MaybeSend, MaybeSync};
use fedimint_derive_secret::{ChildId, DerivableSecret};

use crate::{FedimintFederationId, GuardianFeeAccount};

/// Consumer-owned stable root for the FA key family.
///
/// Consumers choose its place in their key hierarchy. `fa-client` derives all
/// protocol and backup keys below this boundary so those authorities cannot
/// silently disagree.
pub trait FaIdentity: Send + Sync + 'static {
    /// Return the stable, consumer-scoped FA root.
    fn scoped_root(&self) -> DerivableSecret;
}

pub(crate) trait FaIdentityExt: FaIdentity {
    fn public_key(&self) -> Result<FaId, String> {
        Ok(FaId(
            protocol_keypair(&self.scoped_root()).x_only_public_key().0,
        ))
    }

    fn sign_digest(&self, digest: [u8; 32]) -> Result<FaSignature, String> {
        let keypair = protocol_keypair(&self.scoped_root());
        Ok(FaSignature(
            secp256k1::Secp256k1::new().sign_schnorr_no_aux_rand(&digest, &keypair),
        ))
    }
}

impl<T: FaIdentity> FaIdentityExt for T {}

const FA_PROTOCOL_CHILD_ID: ChildId = ChildId(0);

pub(crate) fn protocol_keypair(root: &DerivableSecret) -> secp256k1::Keypair {
    let derived = root
        .child_key(FA_PROTOCOL_CHILD_ID)
        .to_secp_key(&fedimint_core::secp256k1::Secp256k1::new());
    let secret = secp256k1::SecretKey::from_byte_array(&derived.secret_key().secret_bytes())
        .expect("Fedimint derived a valid secp256k1 secret");
    secp256k1::Keypair::from_secret_key(&secp256k1::Secp256k1::new(), &secret)
}

/// Sanitized failure to resolve the FA's own fee-recipient account.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct FaFeeAccountError {
    message: String,
}

impl FaFeeAccountError {
    /// Create a sanitized account-resolution error.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Consumer-owned access to the FA's own formed-federation fee account.
///
/// `fa-client` supplies the federation id parsed from its durable formed
/// invite. Implementations must resolve the already joined client for that
/// exact federation and return its own single-signature SPv2 `BtcDepositor`
/// account. This is a local capability lookup: it must not perform network or
/// value-moving work.
///
/// `fa-client` validates the returned account and owns the complete recipient
/// policy. A consumer must not select an account from caller input at this
/// boundary; development consumers that deliberately accept a test override
/// must document that weaker source explicitly.
///
/// The target-aware bounds retain `Send + Sync` on native targets and relax
/// only for the single-threaded `wasm32-unknown-unknown` consumer runtime.
pub trait FaFeeAccountProvider: MaybeSend + MaybeSync + 'static {
    /// Resolve this FA consumer's account for the exact formed federation.
    fn formed_federation_fee_account(
        &self,
        federation_id: &FedimintFederationId,
    ) -> Result<GuardianFeeAccount, FaFeeAccountError>;
}

/// Sanitized consumer payment error.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct FaPaymentError {
    kind: FaPaymentErrorKind,
    message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FaPaymentErrorKind {
    Other,
    InsufficientFunds,
}

impl FaPaymentError {
    /// Create a sanitized payment error.
    ///
    /// Storage failures and ambiguous or lost results use this constructor.
    /// The seat keeps its quote, and recovery asks the wallet again.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            kind: FaPaymentErrorKind::Other,
            message: message.into(),
        }
    }

    /// Report that the wallet cannot pay a seat, and proved that it started
    /// no payment for it.
    ///
    /// The purchase then ends with the seats it bought. Never use this for a
    /// storage error or for a result whose loss could hide a started payment.
    #[must_use]
    pub fn insufficient_funds(message: impl Into<String>) -> Self {
        Self {
            kind: FaPaymentErrorKind::InsufficientFunds,
            message: message.into(),
        }
    }

    pub(crate) fn proves_insufficient_funds(&self) -> bool {
        self.kind == FaPaymentErrorKind::InsufficientFunds
    }
}

/// Consumer wallet operations required by formation.
///
/// `fa-client` pays seats one at a time. Each payment is bound to one exact
/// signed quote, and the wallet recovers it by that quote alone.
pub trait FaPayments: Send + Sync + 'static {
    /// Consumer-owned state needed only if a paid presentation is refused.
    ///
    /// It deliberately has no serialization bound: refund secrets stay in
    /// the wallet implementation and never enter `fa-client` storage.
    type RefundContext: Send;

    /// Return admitted federation ids the consumer can currently pay from.
    ///
    /// The input is the authenticated common policy set. The wallet reports
    /// capability only; output order does not influence selection policy.
    async fn payable_federations(
        &self,
        admitted: &[FederationId],
    ) -> Result<Vec<FederationId>, FaPaymentError>;

    /// Form the FA's mint-generation belief and quote-request refund outputs.
    ///
    /// This runs before the seat's payment can start, so it **must commit no
    /// value and create no wallet state whose loss would strand funds**:
    /// generating blind nonces and reporting the mint generation is fine,
    /// but nothing may be spent, locked, or made unrecoverable-if-forgotten.
    async fn prepare_quote_refund(
        &self,
        federation_id: &FederationId,
        plan: &Plan,
    ) -> Result<RefundIssuance, FaPaymentError>;

    /// Recover the payment of an exact verified quote, if one started.
    ///
    /// This is a read/reconstruction-only operation: [`SeatPaymentRecovery::Prepared`]
    /// returns the exact payment evidence and matching refund context needed
    /// to replay `CreateSeat` when funding for this quote already exists.
    /// `Prepared` may be returned only after consensus accepted the exact
    /// transaction and every payer-owned primary-module change output is
    /// final and spendable; recovery must await that same finality rather
    /// than submit a replacement operation. `Rejected` is terminal only after
    /// all exact inputs consumed by the rejected transaction have been
    /// restored by accepted refund transactions and every resulting payer
    /// output is final and spendable. `NotStarted` proves that no funding
    /// operation for this quote exists.
    async fn recover_seat_payment(
        &self,
        quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<SeatPaymentRecovery<Self::RefundContext>, FaPaymentError>;

    /// Pay a verified paid quote and prepare its `CreateSeat` presentation.
    ///
    /// `fa-client` calls this only after it durably bound the quote to its
    /// seat, and only when the wallet proved that no funding began. Before
    /// committing any spend, the wallet must make the exact presentation and
    /// matching refund context recoverable through
    /// [`FaPayments::recover_seat_payment`]. A successful return has the same
    /// finality contract as recovered `Prepared`: the exact Fedimint
    /// transaction is accepted and its payer-owned change is already
    /// spendable. Return [`FaPaymentError::insufficient_funds`] only when the
    /// balance check failed before any funding operation began.
    async fn create_seat_payment(
        &self,
        quote: &SignatureVerified<GetQuoteResponse>,
    ) -> Result<PreparedSeatPayment<Self::RefundContext>, FaPaymentError>;

    /// Settle an MG-signed refusal into the consumer wallet.
    ///
    /// The exact same context/refund replay must be idempotent. Formation can
    /// be interrupted after the wallet settles but before `fa-client` drops
    /// the seat, and recovery will invoke this operation again. A successful
    /// return means the refunded value is spendable, not merely accepted by
    /// consensus.
    async fn settle_seat_refund(
        &self,
        context: Self::RefundContext,
        refund: RefundTransaction,
    ) -> Result<(), FaPaymentError>;
}

/// A paid `CreateSeat` presentation plus wallet-private refund state.
pub struct PreparedSeatPayment<R> {
    /// Aggregate signatures over the quote's issuance outputs.
    pub payment_signatures: Vec<LockedBlindedSignature>,
    /// The protocol the wallet actually settled this payment under.
    ///
    /// The wallet states it so `fa-client` can compare it against the quote's
    /// signed `PaymentTerms` before presenting anything: a wallet that
    /// dispatched on something other than those terms has locked funds under
    /// the wrong protocol, and the payer catches that rather than the MG
    /// refusing it.
    pub settled_under: MintGeneration,
    /// Private material the wallet needs if the presentation is refused.
    pub refund_context: R,
}

/// Authoritative wallet recovery outcome for one exact quote.
pub enum SeatPaymentRecovery<R> {
    /// The wallet can prove no funding operation began.
    NotStarted,
    /// Funding exists and its exact presentation can be replayed.
    Prepared(PreparedSeatPayment<R>),
    /// The funding operation was rejected, and its original inputs were
    /// restored and are spendable.
    Rejected,
}

/// Sanitized MG connection failure.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct ManifoldGuardianConnectorError {
    message: String,
}

impl ManifoldGuardianConnectorError {
    /// Create a sanitized connection error message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Sanitized local failure while calling an already-connected MG client.
///
/// This error is deliberately outside the serialized Manifold Guardian service
/// result. Only a consumer-owned connector adapter can report it, so a remote
/// service response cannot manufacture transport retry authority.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct ManifoldGuardianCallError {
    message: String,
}

impl ManifoldGuardianCallError {
    /// Create a sanitized local call error message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Produces clients implementing the existing MG service contract.
pub trait ManifoldGuardianConnector: Send + Sync + 'static {
    /// Concrete transport or mock client.
    type Client: ManifoldGuardianService;

    /// Connect using a selected locator.
    ///
    /// The locator keeps dialing data and the commitment-verification key in
    /// one protocol-owned value, preventing transport adapters from silently
    /// dropping the identity half of the connection contract.
    async fn connect(
        &self,
        locator: &Locator,
    ) -> Result<Self::Client, ManifoldGuardianConnectorError>;

    /// Call the value-free availability verb while retaining local transport
    /// failure outside the serialized service result.
    async fn get_availability(
        &self,
        client: &Self::Client,
        request: GetAvailabilityRequest,
    ) -> Result<FmResult<GetAvailabilityResponse>, ManifoldGuardianCallError>;

    /// Call the value-free quote verb while retaining local transport failure
    /// outside the serialized service result.
    async fn get_quote(
        &self,
        client: &Self::Client,
        request: GetQuoteRequest,
    ) -> Result<FmResult<SignedResponse<GetQuoteResponse>>, ManifoldGuardianCallError>;
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
/// adapter.  `fa-client` independently verifies every provider response.
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
/// rather than anything derived from them. `fa-client` performs every
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
    /// not a trust verdict; `fa-client` still validates it against the
    /// selected provider policy before disclosing the invite.
    pub network: BitcoinNetwork,
}

/// Consumer-owned read of a federation's consensus state.
///
/// `fa-client` depends on `fedimint-core` alone and stays
/// `wasm32-unknown-unknown`-compatible, so it cannot dial guardians itself.
/// The consumer performs invite-code reads. [`Self::read_consensus`] returns
/// raw config and metadata material; `fa-client` derives the peer set, verifies
/// every attestation signature, and compares against what it wrote.
/// [`Self::read_lnv2_gateways`] performs the upstream LNv2 threshold strategy
/// and returns only individually valid public gateway endpoints. Target
/// membership remains an `fa-client` decision.
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
/// `fa-client` cannot verify that the query happened. Fedimint's
/// `get_consensus` derives its guarantee from the client performing threshold
/// agreement across peers, and the response carries no signatures, so nothing
/// in the returned bytes distinguishes a genuine read from a value echoed back
/// from the write that preceded it. An implementation that fabricates or
/// caches a snapshot defeats the post-DKG readback in
/// [`SPEC-federation-trust-directory`](../../domain/specs/SPEC-federation-trust-directory.md)
/// silently and in the dangerous direction: the FA would believe the directory
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

pub(crate) struct FaClientPorts<I, P, N, F, C> {
    pub(crate) identity: I,
    pub(crate) payments: P,
    pub(crate) registry: N,
    pub(crate) mg_connector: F,
    pub(crate) consensus_reader: C,
    pub(crate) fa_fee_account_provider: Arc<dyn FaFeeAccountProvider>,
}
