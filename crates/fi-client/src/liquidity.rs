//! Post-formation FLIP provider discovery and request intent.
//!
//! The orchestration contract is
//! `specs/SPEC-fi-post-formation-liquidity.md`.  This module owns the
//! no-private-data discovery gate: no invite code enters a provider request
//! until the provider's complete Nostr event, portable Schnorr proof, current
//! PeerBadge, product policy, and Iroh endpoint identity have all passed.

use std::collections::BTreeMap;
use std::time::Duration;

use fedi_decentralized_domain::{
    BitcoinNetwork, FMAN_SEAT_BINDINGS_META_FIELD_KEY, FmanSeatBindings,
    HolderAuthorizationEnvelope, ProtocolV1, VerifiedSeatBinding, federation_seats,
};
use fedi_decentralized_nostr::flip::{
    FLIP_PROVIDER_ADVERTISEMENT_D_TAG, FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
    FLIP_PROVIDER_ADVERTISEMENT_HASHTAG,
};
use fedi_decentralized_nostr::has_exact_d_tag;
use fedi_decentralized_nostr_clients::{
    FLIP_PROVIDER_ADVERTISEMENTS_CANDIDATE_LIMIT, FiNostrClient,
};
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerificationError;
use fedi_decentralized_service_liquidity_manager::{
    AllocationItemStatus, AllocationItemTarget, AllocationStatus, CompletionEvidence,
    FederationLiquidityDetails, FleetSeat, FleetSeatId, FmanEndorsement, GatewayApiUrl,
    GetAllocationStatusRequest, GetAllocationStatusResponse, GuardianIdentity,
    ItemAllocationStatus, LiquidityAmountBounds, LiquidityProviderAdvertisement,
    PUBLIC_LIQUIDITY_API_ALPN, PUBLIC_LIQUIDITY_PROTOCOL_VERSION, PayloadProof, PeerId,
    ProtocolVersion, Pubkey, PublicLiquidityApi, PublicRpcPayloadDomain,
    RequestLiquidityDetailsCommitmentV1, RequestLiquidityOutcome, RequestLiquidityRequest,
    RequestLiquidityResponse, Sats, ServiceErrorCode, Sha256Digest, Signed, SourceType,
    Timestamp as LiquidityTimestamp, Url, advertisement_hash, public_rpc_payload_hash,
    request_liquidity_details_hash, request_liquidity_details_hash_for_request,
};
use fedi_iroh_rpc::iroh::{EndpointAddr, EndpointId};
use fedimint_core::runtime::{Instant, timeout};
use futures::{StreamExt as _, stream::FuturesUnordered};
use nostr_sdk::{Event, EventId, Kind, PublicKey, TagKind};
use secp256k1::{XOnlyPublicKey, schnorr::Signature};
use serde::Serialize;

use crate::formation::DriverRun;
use crate::{
    FederationConsensusReader, FiClient, FiError, FiIdentity, FiPayments, FiResult,
    FleetManagerConnector, FormationFreshness, FormationId, FormationPhase, FormationRunOptions,
    LiquidityProviderConnector, Locator, SeatId,
};

use fedi_decentralized_service_fleet_manager::{
    FleetManagerService, GetFederationTrustMaterialRequest, GetFederationTrustMaterialResponse,
    Timestamp as FmanTimestamp,
};

/// End-to-end deadline for one provider enumeration and trust walk.
pub const FI_LIQUIDITY_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum accepted provider-advertisement age, independent of its expiry.
pub const FI_LIQUIDITY_MAX_ADVERTISEMENT_AGE: Duration = Duration::from_secs(4 * 60 * 60);

/// Maximum live PeerBadge envelopes examined for one provider.
pub const FI_LIQUIDITY_MAX_HOLDER_AUTHORIZATIONS: usize = 4;

/// Per-call deadline for FMan and FLIP Iroh RPCs.
pub const FI_LIQUIDITY_RPC_TIMEOUT: Duration = Duration::from_secs(60);

/// Exact request lifetime. Recovery still queries an accepted allocation
/// after expiry but never invents a new semantic request hash.
pub const FI_LIQUIDITY_REQUEST_VALIDITY: Duration = Duration::from_secs(60 * 60);

/// Largest single durable-operation page accepted by the public recovery API.
pub const FI_LIQUIDITY_OPERATION_PAGE_MAX: usize = 100;

/// Maximum accepted FMan trust-material response lifetime.
pub const FI_LIQUIDITY_TRUST_MATERIAL_VALIDITY: Duration = Duration::from_secs(4 * 60 * 60);

/// Stable semantic identifier of one exact liquidity request.
///
/// It is the lowercase hex `details_payload_hash`, so retries and restored
/// tasks cannot invent a second identity for the same intent.
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(transparent)]
pub struct LiquidityOperationId(pub String);

impl LiquidityOperationId {
    fn is_canonical(&self) -> bool {
        self.0.len() == 64
            && self
                .0
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    pub(crate) fn validate(&self) -> FiResult<()> {
        if !self.is_canonical() {
            return Err(FiError::InvalidIntent(
                "liquidity operation id must be a lowercase 32-byte hex digest".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Durable consumer-visible phase of one exact provider request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiquidityOperationPhase {
    /// Exact commitment persisted; provider acknowledgement is not yet durable.
    Prepared,
    /// Provider durably accepted the request; per-item work is authoritative.
    Accepted,
    /// Provider rejected this exact intent.
    Rejected,
}

/// Durable post-formation liquidity projection used by Fedi RPCs.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct LiquidityOperationSnapshot {
    pub operation_id: LiquidityOperationId,
    pub formation_id: FormationId,
    pub provider_pubkey: Pubkey,
    pub endpoint_hint: Url,
    pub details_payload_hash: Sha256Digest,
    pub amounts: LiquidityAmountBounds,
    pub phase: LiquidityOperationPhase,
    pub item_statuses: Vec<AllocationItemStatus>,
    pub rejection_code: Option<String>,
    /// Whether FI confirmed the completed gateway in a fresh LNv2 aggregate view.
    pub gateway_view_verified: bool,
}

impl LiquidityOperationSnapshot {
    fn is_complete(&self) -> bool {
        !self.item_statuses.is_empty()
            && self
                .item_statuses
                .iter()
                .all(|item| item.status == ItemAllocationStatus::Completed)
            && (self.amounts.gateway_min_amount.0 == 0 || self.gateway_view_verified)
    }
}

/// One bounded page of durable liquidity work for crash recovery.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct LiquidityOperationPage {
    /// Stable operation-id order; every row has passed storage validation.
    pub operations: Vec<LiquidityOperationSnapshot>,
    /// Exclusive cursor for the next page, or `None` after the final page.
    pub next_after: Option<LiquidityOperationId>,
}

/// Recovery-only persisted facts for one exact FLIP request.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct StoredLiquidityOperation {
    pub(crate) schema_version: u16,
    pub(crate) operation_id: LiquidityOperationId,
    pub(crate) formation_id: FormationId,
    pub(crate) commitment: RequestLiquidityDetailsCommitmentV1,
    pub(crate) endpoint_hint: Url,
    pub(crate) details_payload_hash: Sha256Digest,
    pub(crate) response: Option<Signed<RequestLiquidityResponse>>,
    pub(crate) status: Option<Signed<GetAllocationStatusResponse>>,
    pub(crate) verified_gateway_api: Option<GatewayApiUrl>,
}

impl StoredLiquidityOperation {
    fn allocation_status(&self) -> Option<&AllocationStatus> {
        self.status
            .as_ref()
            .map(|status| &status.payload.status)
            .or_else(
                || match self.response.as_ref().map(|value| &value.payload.outcome) {
                    Some(RequestLiquidityOutcome::Accepted(status)) => Some(status),
                    _ => None,
                },
            )
    }

    pub(crate) fn completed_gateway_api(&self) -> FiResult<Option<GatewayApiUrl>> {
        self.allocation_status()
            .map(completed_gateway_api)
            .transpose()
            .map(Option::flatten)
    }

    pub(crate) fn snapshot(&self) -> FiResult<LiquidityOperationSnapshot> {
        if self.schema_version != 3 {
            return Err(FiError::Storage(
                "unsupported FI liquidity storage schema; reset the pre-launch FI namespace"
                    .to_owned(),
            ));
        }
        if !self.operation_id.is_canonical() {
            return Err(FiError::Storage(
                "persisted FI liquidity operation id is not canonical".to_owned(),
            ));
        }
        let expected_hash = request_liquidity_details_hash(&self.commitment).map_err(|error| {
            FiError::Storage(format!(
                "hashing persisted FI liquidity commitment failed: {error}"
            ))
        })?;
        if expected_hash != self.details_payload_hash
            || self.operation_id.0 != hex::encode(expected_hash.0)
        {
            return Err(FiError::Storage(
                "persisted FI liquidity identity does not match its commitment".to_owned(),
            ));
        }
        let (phase, rejection_code) = match self.response.as_ref().map(|v| &v.payload.outcome) {
            None if self.status.is_some() => (LiquidityOperationPhase::Accepted, None),
            None => (LiquidityOperationPhase::Prepared, None),
            Some(RequestLiquidityOutcome::Accepted(_)) => (LiquidityOperationPhase::Accepted, None),
            Some(RequestLiquidityOutcome::Rejected(rejection)) => (
                LiquidityOperationPhase::Rejected,
                Some(rejection.code.to_string()),
            ),
        };
        let gateway_view_verified = match self.verified_gateway_api.as_ref() {
            None => false,
            Some(verified) => self.completed_gateway_api()?.as_ref() == Some(verified),
        };
        let item_statuses = self
            .allocation_status()
            .map(|status| status.item_statuses.clone())
            .unwrap_or_default();
        Ok(LiquidityOperationSnapshot {
            operation_id: self.operation_id.clone(),
            formation_id: self.formation_id.clone(),
            provider_pubkey: self.commitment.provider_pubkey.clone(),
            endpoint_hint: self.endpoint_hint.clone(),
            details_payload_hash: self.details_payload_hash,
            amounts: self.commitment.amounts.clone(),
            phase,
            item_statuses,
            rejection_code,
            gateway_view_verified,
        })
    }

    pub(crate) fn validate_recovery(&self) -> FiResult<()> {
        self.snapshot()?;
        if let Some(response) = &self.response {
            validate_request_response(self, response)?;
        }
        if let Some(status) = &self.status {
            validate_status_response(self, status)?;
        }
        Ok(())
    }
}

/// Consumer-owned post-formation request bounds.
///
/// Gateway-only requests are the Fedi formation-flow default.  Stability-pool
/// amounts remain available for the separately gated administrative path but
/// are never inferred from a formation.
///
/// The intent deliberately carries no provider identities: any provider
/// admitted by the selected Manifold environment's credential verification is
/// eligible, and only recovery pins a provider — to the one named by the
/// durable commitment.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiquidityRequestIntent {
    /// Exact requested source bounds.
    pub amounts: LiquidityAmountBounds,
}

impl LiquidityRequestIntent {
    /// Construct the MVP gateway-only request.
    #[must_use]
    pub fn gateway(gateway_min_sats: u64, gateway_max_sats: Option<u64>) -> Self {
        Self {
            amounts: LiquidityAmountBounds {
                gateway_min_amount: Sats(gateway_min_sats),
                gateway_max_amount: gateway_max_sats.map(Sats),
                stability_min_amount: Sats(0),
                stability_max_amount: None,
            },
        }
    }

    pub(crate) fn validate(&self) -> FiResult<()> {
        let gateway = self.amounts.gateway_min_amount.0;
        let stability = self.amounts.stability_min_amount.0;
        if gateway == 0 && stability == 0 {
            return Err(FiError::Liquidity(
                "at least one liquidity source must request a positive minimum".to_owned(),
            ));
        }
        for (minimum, maximum, source) in [
            (
                gateway,
                self.amounts.gateway_max_amount.map(|v| v.0),
                "gateway",
            ),
            (
                stability,
                self.amounts.stability_max_amount.map(|v| v.0),
                "stability_pool",
            ),
        ] {
            if minimum == 0 && maximum.is_some() {
                return Err(FiError::Liquidity(format!(
                    "{source} maximum cannot be set when that source is not requested"
                )));
            }
            if maximum.is_some_and(|maximum| maximum < minimum) {
                return Err(FiError::Liquidity(format!(
                    "{source} maximum is below its minimum"
                )));
            }
        }
        Ok(())
    }

    fn requires(&self, source: SourceType) -> bool {
        match source {
            SourceType::Gateway => self.amounts.gateway_min_amount.0 > 0,
            SourceType::StabilityPool => self.amounts.stability_min_amount.0 > 0,
        }
    }
}

/// A provider that passed every no-private-data admission check.
#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedLiquidityProvider {
    provider_pubkey: Pubkey,
    endpoint: EndpointAddr,
    endpoint_url: Url,
    advertisement_hash: Sha256Digest,
    advertisement: LiquidityProviderAdvertisement,
}

impl AdmittedLiquidityProvider {
    /// Authenticated provider identity.
    #[must_use]
    pub fn provider_pubkey(&self) -> &Pubkey {
        &self.provider_pubkey
    }

    /// Iroh endpoint whose node identity was signed by the provider.
    #[must_use]
    pub fn endpoint(&self) -> &EndpointAddr {
        &self.endpoint
    }

    /// Exact signed endpoint URL, retained as a recovery hint.
    #[must_use]
    pub fn endpoint_url(&self) -> &Url {
        &self.endpoint_url
    }

    /// Domain-separated hash of the signed advertisement payload.
    #[must_use]
    pub fn advertisement_hash(&self) -> Sha256Digest {
        self.advertisement_hash
    }

    /// Authenticated provider policy and display payload.
    #[must_use]
    pub fn advertisement(&self) -> &LiquidityProviderAdvertisement {
        &self.advertisement
    }
}

/// Typed local reason one relay candidate was refused.
#[derive(Debug)]
#[non_exhaustive]
pub enum LiquidityProviderRejection {
    WrongEventRole,
    InvalidEvent,
    InvalidDocument,
    InvalidProviderKey,
    AuthorMismatch,
    InvalidProviderProof,
    Superseded,
    IssuedInFuture,
    Expired,
    Stale,
    UnsupportedVersion,
    UnsupportedNetwork,
    UnsupportedSource,
    /// Recovery re-discovery admits only the provider named by the durable
    /// commitment; every other candidate is filtered, not distrusted.
    NotCommittedProvider,
    InvalidEndpoint,
    MissingPeerBadge,
    PeerBadgeRejected(PeerBadgeVerificationError),
    PeerBadgeSubjectMismatch,
    DeadlineExpired,
}

impl LiquidityProviderRejection {
    /// Stable code for RPC projection without leaking credential diagnostics.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::WrongEventRole => "wrong_event_role",
            Self::InvalidEvent => "invalid_event",
            Self::InvalidDocument => "invalid_document",
            Self::InvalidProviderKey => "invalid_provider_key",
            Self::AuthorMismatch => "author_mismatch",
            Self::InvalidProviderProof => "invalid_provider_proof",
            Self::Superseded => "superseded",
            Self::IssuedInFuture => "issued_in_future",
            Self::Expired => "expired",
            Self::Stale => "stale",
            Self::UnsupportedVersion => "unsupported_version",
            Self::UnsupportedNetwork => "unsupported_network",
            Self::UnsupportedSource => "unsupported_source",
            Self::NotCommittedProvider => "not_committed_provider",
            Self::InvalidEndpoint => "invalid_endpoint",
            Self::MissingPeerBadge => "missing_peer_badge",
            Self::PeerBadgeRejected(_) => "peer_badge_rejected",
            Self::PeerBadgeSubjectMismatch => "peer_badge_subject_mismatch",
            Self::DeadlineExpired => "deadline_expired",
        }
    }
}

/// One bounded no-private-data provider enumeration.
#[derive(Debug, Default)]
pub struct LiquidityDiscovery {
    /// Fully admitted providers, deterministically ordered by provider key.
    pub providers: Vec<AdmittedLiquidityProvider>,
    /// Stable provider identity when parseable, plus refusal reason.
    pub rejected: Vec<(Option<Pubkey>, LiquidityProviderRejection)>,
}

trait LiquidityBadgeVerifier {
    async fn verify_subject(
        &self,
        envelope: &HolderAuthorizationEnvelope,
    ) -> Result<PublicKey, PeerBadgeVerificationError>;
}

impl LiquidityBadgeVerifier for fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier {
    async fn verify_subject(
        &self,
        envelope: &HolderAuthorizationEnvelope,
    ) -> Result<PublicKey, PeerBadgeVerificationError> {
        self.verify(envelope).await.map(|badge| badge.subject().0)
    }
}

#[cfg(test)]
pub(crate) trait TestLiquidityBadgeVerifier {
    async fn verify_subject_for_test(
        &self,
        envelope: &HolderAuthorizationEnvelope,
    ) -> Result<PublicKey, PeerBadgeVerificationError>;
}

#[cfg(test)]
impl<T> LiquidityBadgeVerifier for T
where
    T: TestLiquidityBadgeVerifier,
{
    async fn verify_subject(
        &self,
        envelope: &HolderAuthorizationEnvelope,
    ) -> Result<PublicKey, PeerBadgeVerificationError> {
        self.verify_subject_for_test(envelope).await
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
    /// Enumerate and fully verify currently eligible FLIP providers.
    ///
    /// This is deliberately read-only and uncached.  Re-entering the Fedi flow
    /// performs a new relay query and fresh PeerBadge revocation checks.
    pub async fn discover_liquidity_providers(
        &self,
        intent: &LiquidityRequestIntent,
        network: BitcoinNetwork,
    ) -> FiResult<LiquidityDiscovery> {
        intent.validate()?;
        let deadline = Instant::now() + FI_LIQUIDITY_DISCOVERY_TIMEOUT;
        let events = self
            .inner
            .ports
            .registry
            .fetch_liquidity_provider_advertisements(
                deadline.saturating_duration_since(Instant::now()),
            )
            .await
            .map_err(|error| FiError::Registry(error.to_string()))?;
        discover_with(
            events,
            intent,
            network,
            None,
            &self.inner.peer_badge_verifier,
            deadline,
            fedimint_core::time::duration_since_epoch().as_secs(),
        )
        .await
    }

    /// Start one exact post-formation liquidity request.
    ///
    /// The private invite is disclosed only after a fresh provider admission.
    /// The semantic commitment is persisted before the first mutating provider
    /// call, so cancellation or process death can only lead to an exact replay.
    /// A federation carries at most one non-terminal operation: while one is
    /// [`LiquidityOperationPhase::Prepared`] or
    /// [`LiquidityOperationPhase::Accepted`], starting again returns
    /// [`FiError::LiquidityOperationExists`] naming it, and the consumer must
    /// resume that operation instead —
    /// [`Self::current_liquidity_operation`] returns it. Both sources are
    /// funded through one combined gateway-plus-stability intent, never
    /// through a second request.
    pub async fn start_liquidity<L>(
        &self,
        formation_id: &FormationId,
        provider_pubkey: &Pubkey,
        intent: LiquidityRequestIntent,
        connector: &L,
    ) -> FiResult<LiquidityOperationSnapshot>
    where
        L: LiquidityProviderConnector,
    {
        self.start_liquidity_with_verifier(
            formation_id,
            provider_pubkey,
            intent,
            connector,
            &self.inner.peer_badge_verifier,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn start_liquidity_for_test<L, V>(
        &self,
        formation_id: &FormationId,
        provider_pubkey: &Pubkey,
        intent: LiquidityRequestIntent,
        connector: &L,
        verifier: &V,
    ) -> FiResult<LiquidityOperationSnapshot>
    where
        L: LiquidityProviderConnector,
        V: TestLiquidityBadgeVerifier,
    {
        self.start_liquidity_with_verifier(
            formation_id,
            provider_pubkey,
            intent,
            connector,
            verifier,
        )
        .await
    }

    async fn start_liquidity_with_verifier<L, V>(
        &self,
        formation_id: &FormationId,
        provider_pubkey: &Pubkey,
        intent: LiquidityRequestIntent,
        connector: &L,
        verifier: &V,
    ) -> FiResult<LiquidityOperationSnapshot>
    where
        L: LiquidityProviderConnector,
        V: LiquidityBadgeVerifier,
    {
        let _run = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        let options = FormationRunOptions::default();
        options.validate_for_start(&self.inner.store)?;
        let (deadline, lease) =
            crate::formation::start_driver_run(&self.inner.store, options).await?;
        let result = async {
            intent.validate()?;
            lease.renew().await?;
            let context = self.formed_liquidity_context(formation_id).await?;
            self.ensure_no_live_liquidity_operation(context.federation.federation_id())
                .await?;
            lease.renew().await?;
            let provider = self
                .refresh_provider_with_verifier(
                    provider_pubkey,
                    &intent,
                    context.network,
                    None,
                    verifier,
                )
                .await?;
            let requester = self.requester_pubkey()?;
            let expires_at = now_secs()?
                .checked_add(FI_LIQUIDITY_REQUEST_VALIDITY.as_secs())
                .ok_or_else(|| {
                    FiError::Liquidity("liquidity request expiry overflow".to_owned())
                })?;
            let commitment = RequestLiquidityDetailsCommitmentV1 {
                version: PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
                requester_pubkey: requester,
                provider_pubkey: provider.provider_pubkey.clone(),
                network: context.network,
                amounts: intent.amounts,
                federation_details: context.federation_details()?,
                expires_at: LiquidityTimestamp(expires_at),
            };
            let details_payload_hash =
                request_liquidity_details_hash(&commitment).map_err(|error| {
                    FiError::Liquidity(format!("hashing liquidity intent: {error}"))
                })?;
            let operation_id = LiquidityOperationId(hex::encode(details_payload_hash.0));
            let operation = StoredLiquidityOperation {
                schema_version: 3,
                operation_id,
                formation_id: formation_id.clone(),
                commitment,
                endpoint_hint: provider.endpoint_url.clone(),
                details_payload_hash,
                response: None,
                status: None,
                verified_gateway_api: None,
            };

            lease.renew().await?;
            self.inner
                .store
                .insert_liquidity_operation(operation.clone())
                .await?;
            let snapshot = self
                .submit_prepared_liquidity(
                    operation, context, provider, connector, verifier, &lease,
                )
                .await?;
            self.attach_completed_gateway(
                &snapshot.operation_id,
                DriverRun::new(options, deadline, &lease),
            )
            .await
        }
        .await;
        crate::formation::finish_driver_run(
            result,
            self.inner.store.release_driver_lease(lease).await,
        )
    }

    /// Resume one exact request by querying status before any exact replay.
    ///
    /// A replay after provider `NotFound` is permitted only while the durable
    /// journal holds no signed acceptance evidence; a disclaimed but
    /// durably-evidenced allocation fails closed and leaves the operation
    /// unchanged.
    pub async fn resume_liquidity<L>(
        &self,
        operation_id: &LiquidityOperationId,
        connector: &L,
    ) -> FiResult<LiquidityOperationSnapshot>
    where
        L: LiquidityProviderConnector,
    {
        self.resume_liquidity_with_verifier(
            operation_id,
            connector,
            &self.inner.peer_badge_verifier,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn resume_liquidity_for_test<L, V>(
        &self,
        operation_id: &LiquidityOperationId,
        connector: &L,
        verifier: &V,
    ) -> FiResult<LiquidityOperationSnapshot>
    where
        L: LiquidityProviderConnector,
        V: TestLiquidityBadgeVerifier,
    {
        self.resume_liquidity_with_verifier(operation_id, connector, verifier)
            .await
    }

    async fn resume_liquidity_with_verifier<L, V>(
        &self,
        operation_id: &LiquidityOperationId,
        connector: &L,
        verifier: &V,
    ) -> FiResult<LiquidityOperationSnapshot>
    where
        L: LiquidityProviderConnector,
        V: LiquidityBadgeVerifier,
    {
        operation_id.validate()?;
        let _run = self.inner.run_guard.try_lock().map_err(|_| FiError::Busy)?;
        let operation = self
            .inner
            .store
            .load_liquidity_operation(operation_id)
            .await?;
        if operation.response.as_ref().is_some_and(|response| {
            matches!(
                response.payload.outcome,
                RequestLiquidityOutcome::Rejected(_)
            )
        }) {
            return operation.snapshot();
        }
        let options = FormationRunOptions::default();
        options.validate_for_start(&self.inner.store)?;
        let (deadline, lease) =
            crate::formation::start_driver_run(&self.inner.store, options).await?;
        let result = async {
            lease.renew().await?;
            let context = self
                .formed_liquidity_context(&operation.formation_id)
                .await?;
            context.matches_commitment(&operation.commitment)?;
            let durable_snapshot = self
                .attach_completed_gateway(operation_id, DriverRun::new(options, deadline, &lease))
                .await?;
            if durable_snapshot.is_complete() {
                return Ok(durable_snapshot);
            }
            let intent = LiquidityRequestIntent {
                amounts: operation.commitment.amounts.clone(),
            };
            lease.renew().await?;
            let provider = self
                .refresh_provider_with_verifier(
                    &operation.commitment.provider_pubkey,
                    &intent,
                    operation.commitment.network,
                    Some(&operation.commitment.provider_pubkey),
                    verifier,
                )
                .await?;
            lease.renew().await?;
            let client = timeout(
                FI_LIQUIDITY_RPC_TIMEOUT,
                connector.connect(provider.endpoint()),
            )
            .await
            .map_err(|_| FiError::Timeout("connecting to liquidity provider".to_owned()))?
            .map_err(|error| FiError::Liquidity(format!("connecting to provider: {error}")))?;
            let status_request = GetAllocationStatusRequest {
                version: PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
                requester_pubkey: operation.commitment.requester_pubkey.clone(),
                details_payload_hash: operation.details_payload_hash,
                provider_pubkey: operation.commitment.provider_pubkey.clone(),
                issued_at: LiquidityTimestamp(now_secs()?),
            };
            let status_request = self.sign_public_rpc(
                PublicRpcPayloadDomain::GetAllocationStatusRequest,
                status_request,
            )?;
            lease.renew().await?;
            let snapshot = match timeout(
                FI_LIQUIDITY_RPC_TIMEOUT,
                client.get_allocation_status(status_request),
            )
            .await
            .map_err(|_| FiError::Timeout("querying liquidity allocation".to_owned()))?
            {
                Ok(response) => {
                    self.verify_status_response(&operation, &response)?;
                    lease.renew().await?;
                    self.inner
                        .store
                        .store_liquidity_status(operation_id, response)
                        .await?;
                    self.inner
                        .store
                        .load_liquidity_operation(operation_id)
                        .await?
                        .snapshot()
                }
                Err(error) if error.code() == ServiceErrorCode::NotFound => {
                    // NotFound only authorizes an exact replay while the FI
                    // holds no durable acceptance evidence. Once a signed
                    // acceptance (response or status) is journaled, a provider
                    // that lost its data must not get a replay it could turn
                    // into a stored terminal rejection of an accepted
                    // allocation.
                    if operation.response.is_some() || operation.status.is_some() {
                        return Err(FiError::Liquidity(
                            "provider disclaims a liquidity allocation the durable journal \
                             holds signed acceptance evidence for; refusing the replay"
                                .to_owned(),
                        ));
                    }
                    self.submit_prepared_liquidity(
                        operation, context, provider, connector, verifier, &lease,
                    )
                    .await
                }
                Err(error) => Err(FiError::Liquidity(format!(
                    "provider status query failed ({}): {}",
                    error.code(),
                    error.message()
                ))),
            }?;
            self.attach_completed_gateway(
                &snapshot.operation_id,
                DriverRun::new(options, deadline, &lease),
            )
            .await
        }
        .await;
        crate::formation::finish_driver_run(
            result,
            self.inner.store.release_driver_lease(lease).await,
        )
    }

    /// Read the latest durable projection without performing network work.
    pub async fn liquidity_status(
        &self,
        operation_id: &LiquidityOperationId,
    ) -> FiResult<LiquidityOperationSnapshot> {
        operation_id.validate()?;
        self.inner
            .store
            .load_liquidity_operation(operation_id)
            .await?
            .snapshot()
    }

    async fn attach_completed_gateway(
        &self,
        operation_id: &LiquidityOperationId,
        run: DriverRun<'_>,
    ) -> FiResult<LiquidityOperationSnapshot> {
        let operation = self
            .inner
            .store
            .load_liquidity_operation(operation_id)
            .await?;
        let gateway_api = operation.completed_gateway_api()?;
        if operation.verified_gateway_api.as_ref() == gateway_api.as_ref() && gateway_api.is_some()
        {
            return operation.snapshot();
        }
        let Some(gateway_api) = gateway_api else {
            return operation.snapshot();
        };
        self.register_gateway_pinned(gateway_api.clone(), self.fi_id()?, run)
            .await?;
        self.inner
            .store
            .mark_liquidity_gateway_view_verified(operation_id, &gateway_api)
            .await?;
        self.inner
            .store
            .load_liquidity_operation(operation_id)
            .await?
            .snapshot()
    }

    /// Enumerate durable liquidity work for launch-time recovery.
    ///
    /// Pages are ordered by semantic operation id. `after` is exclusive and
    /// must be the `next_after` cursor returned by the previous page. Listing
    /// is read-only and performs no provider or formation network work.
    pub async fn list_liquidity_operations(
        &self,
        after: Option<&LiquidityOperationId>,
        limit: usize,
    ) -> FiResult<LiquidityOperationPage> {
        if !(1..=FI_LIQUIDITY_OPERATION_PAGE_MAX).contains(&limit) {
            return Err(FiError::InvalidIntent(format!(
                "liquidity operation page size must be between 1 and {FI_LIQUIDITY_OPERATION_PAGE_MAX}"
            )));
        }
        if let Some(after) = after {
            after.validate()?;
        }
        let mut stored = self
            .inner
            .store
            .list_liquidity_operations(after, limit)
            .await?;
        let has_more = stored.len() > limit;
        if has_more {
            stored.pop();
        }
        let operations = stored
            .into_iter()
            .map(|operation| operation.snapshot())
            .collect::<FiResult<Vec<_>>>()?;
        let next_after = if has_more {
            operations
                .last()
                .map(|snapshot| snapshot.operation_id.clone())
        } else {
            None
        };
        Ok(LiquidityOperationPage {
            operations,
            next_after,
        })
    }

    /// Return the active formation's single non-terminal liquidity operation.
    ///
    /// The provider protocol holds at most one allocation per federation and
    /// [`Self::start_liquidity`] refuses a second live operation, so "which
    /// operation is THE operation" has one canonical answer; consumers resume
    /// this snapshot instead of paging [`Self::list_liquidity_operations`]
    /// and filtering. Returns `Ok(None)` when no formation is active or the
    /// active formation has no [`LiquidityOperationPhase::Prepared`] or
    /// [`LiquidityOperationPhase::Accepted`] operation. Read-only: no
    /// provider or formation network work is performed.
    pub async fn current_liquidity_operation(
        &self,
    ) -> FiResult<Option<LiquidityOperationSnapshot>> {
        let formation_id = match self.status() {
            crate::FiStatus::Formation(snapshot) => snapshot.formation_id,
            crate::FiStatus::Idle => return Ok(None),
        };
        self.find_live_liquidity_operation(|operation| operation.formation_id == formation_id)
            .await
    }

    /// Refuse to mint a second live request identity for one federation.
    ///
    /// Every start embeds a fresh expiry, so a consumer retry after an
    /// ambiguous failure would otherwise create a second semantic operation —
    /// and against a different provider that means double-accepted liquidity,
    /// because providers hold at most one allocation per federation. Only a
    /// durable terminal rejection frees the federation for a fresh start.
    async fn ensure_no_live_liquidity_operation(
        &self,
        federation_id: &fedi_decentralized_domain::FederationId,
    ) -> FiResult<()> {
        match self
            .find_live_liquidity_operation(|operation| {
                &operation.commitment.federation_details.federation_id == federation_id
            })
            .await?
        {
            Some(snapshot) => Err(FiError::LiquidityOperationExists {
                operation_id: snapshot.operation_id,
            }),
            None => Ok(()),
        }
    }

    /// Page the durable journal for the first matching non-terminal
    /// operation.
    ///
    /// The one-live-operation-per-federation invariant makes "first" also
    /// "only" for the filters used here.
    async fn find_live_liquidity_operation<M>(
        &self,
        matches: M,
    ) -> FiResult<Option<LiquidityOperationSnapshot>>
    where
        M: Fn(&StoredLiquidityOperation) -> bool,
    {
        let mut after: Option<LiquidityOperationId> = None;
        loop {
            let mut stored = self
                .inner
                .store
                .list_liquidity_operations(after.as_ref(), FI_LIQUIDITY_OPERATION_PAGE_MAX)
                .await?;
            let has_more = stored.len() > FI_LIQUIDITY_OPERATION_PAGE_MAX;
            if has_more {
                stored.pop();
            }
            for operation in &stored {
                if !matches(operation) {
                    continue;
                }
                let snapshot = operation.snapshot()?;
                if snapshot.phase != LiquidityOperationPhase::Rejected {
                    return Ok(Some(snapshot));
                }
            }
            if !has_more {
                return Ok(None);
            }
            after = stored
                .last()
                .map(|operation| operation.operation_id.clone());
        }
    }

    /// Freshly re-admit one provider immediately before private disclosure.
    ///
    /// `pinned_provider` is recovery's internal mechanism, not consumer
    /// policy: resume passes the provider named by the durable commitment so
    /// re-discovery admits no other identity, while start passes `None` and
    /// relies on credential admission alone.
    async fn refresh_provider_with_verifier<V>(
        &self,
        provider_pubkey: &Pubkey,
        intent: &LiquidityRequestIntent,
        network: BitcoinNetwork,
        pinned_provider: Option<&Pubkey>,
        verifier: &V,
    ) -> FiResult<AdmittedLiquidityProvider>
    where
        V: LiquidityBadgeVerifier,
    {
        intent.validate()?;
        let deadline = Instant::now() + FI_LIQUIDITY_DISCOVERY_TIMEOUT;
        let events = self
            .inner
            .ports
            .registry
            .fetch_liquidity_provider_advertisements(
                deadline.saturating_duration_since(Instant::now()),
            )
            .await
            .map_err(|error| FiError::Registry(error.to_string()))?;
        discover_with(
            events,
            intent,
            network,
            pinned_provider,
            verifier,
            deadline,
            fedimint_core::time::duration_since_epoch().as_secs(),
        )
        .await?
        .providers
        .into_iter()
        .find(|provider| provider.provider_pubkey() == provider_pubkey)
        .ok_or_else(|| {
            FiError::Liquidity(
                "selected liquidity provider did not pass a fresh admission".to_owned(),
            )
        })
    }

    async fn formed_liquidity_context(
        &self,
        expected_formation_id: &FormationId,
    ) -> FiResult<FormedLiquidityContext> {
        match self.status() {
            crate::FiStatus::Formation(snapshot)
                if &snapshot.formation_id == expected_formation_id
                    && snapshot.phase == FormationPhase::Formed
                    && snapshot.freshness == FormationFreshness::Fresh => {}
            _ => {
                return Err(FiError::Liquidity(
                    "formation must be freshly reconciled and formed before requesting liquidity"
                        .to_owned(),
                ));
            }
        }
        let fi_id = self
            .inner
            .ports
            .identity
            .public_key()
            .map_err(FiError::Identity)?;
        let recovery = match self.inner.store.load_recovery(fi_id).await? {
            crate::db::FiRecovery::Idle => return Err(FiError::NoActiveFormation),
            crate::db::FiRecovery::Formation(recovery) => *recovery,
        };
        if &recovery.snapshot.formation_id != expected_formation_id {
            return Err(FiError::Liquidity(
                "liquidity operation does not belong to the active formation".to_owned(),
            ));
        }
        if recovery.snapshot.phase != FormationPhase::Formed {
            return Err(FiError::Liquidity(
                "durable formation is not formed".to_owned(),
            ));
        }
        let invite_code = recovery.snapshot.invite_code.clone().ok_or_else(|| {
            FiError::Storage("formed FI record has no federation invite".to_owned())
        })?;
        let consensus = timeout(
            FI_LIQUIDITY_RPC_TIMEOUT,
            self.inner
                .ports
                .consensus_reader
                .read_consensus(&invite_code),
        )
        .await
        .map_err(|_| FiError::Timeout("reading liquidity federation consensus".to_owned()))?
        .map_err(|error| FiError::Liquidity(format!("reading federation consensus: {error}")))?;
        let federation = federation_seats(&consensus.config).map_err(|error| {
            FiError::Liquidity(format!("deriving final federation seats: {error}"))
        })?;
        let directory_value =
            liquidity_seat_bindings_field(&consensus.meta_value)?.ok_or_else(|| {
                FiError::Liquidity("federation has no FMan seat directory".to_owned())
            })?;
        let bindings = FmanSeatBindings::parse_canonical(&directory_value)
            .and_then(|bindings| bindings.verify_for_federation(&federation))
            .map_err(|error| {
                FiError::Liquidity(format!("verifying FMan seat directory: {error}"))
            })?;
        let seats = recovery
            .seats
            .into_iter()
            .map(|seat| {
                let fman_id = seat.admission.fman_id().ok_or_else(|| {
                    FiError::Liquidity(
                        "post-formation liquidity requires badge-vouched FMan identities"
                            .to_owned(),
                    )
                })?;
                let seat_id = seat.progress.seat_id.ok_or_else(|| {
                    FiError::Storage("formed FI seat has no durable seat id".to_owned())
                })?;
                Ok(FormedFmanSeat {
                    fman_id,
                    locator: seat.progress.locator,
                    seat_id,
                })
            })
            .collect::<FiResult<Vec<_>>>()?;
        Ok(FormedLiquidityContext {
            federation_name: recovery.snapshot.intent.federation_name,
            invite_code,
            network: consensus.network,
            federation,
            bindings,
            seats,
        })
    }

    async fn submit_prepared_liquidity<L, V>(
        &self,
        operation: StoredLiquidityOperation,
        context: FormedLiquidityContext,
        provider: AdmittedLiquidityProvider,
        connector: &L,
        verifier: &V,
        lease: &crate::db::DriverLease,
    ) -> FiResult<LiquidityOperationSnapshot>
    where
        L: LiquidityProviderConnector,
        V: LiquidityBadgeVerifier,
    {
        context.matches_commitment(&operation.commitment)?;
        lease.renew().await?;
        let (endorsement, trust_material) = self.collect_fman_trust(&context, verifier).await?;
        let request = RequestLiquidityRequest {
            version: operation.commitment.version,
            requester_pubkey: operation.commitment.requester_pubkey.clone(),
            provider_pubkey: operation.commitment.provider_pubkey.clone(),
            issued_at: LiquidityTimestamp(now_secs()?),
            network: operation.commitment.network,
            amounts: operation.commitment.amounts.clone(),
            details_payload_hash: operation.details_payload_hash,
            federation_details: operation.commitment.federation_details.clone(),
            fman_endorsement: Some(endorsement),
            fman_trust_material: Some(trust_material),
            expires_at: operation.commitment.expires_at,
        };
        let recomputed = request_liquidity_details_hash_for_request(&request)
            .map_err(|error| FiError::Liquidity(format!("hashing liquidity request: {error}")))?;
        if recomputed != operation.details_payload_hash {
            return Err(FiError::Storage(
                "persisted liquidity commitment does not match its operation id".to_owned(),
            ));
        }
        let request =
            self.sign_public_rpc(PublicRpcPayloadDomain::RequestLiquidityRequest, request)?;
        lease.renew().await?;
        let client = timeout(
            FI_LIQUIDITY_RPC_TIMEOUT,
            connector.connect(provider.endpoint()),
        )
        .await
        .map_err(|_| FiError::Timeout("connecting to liquidity provider".to_owned()))?
        .map_err(|error| FiError::Liquidity(format!("connecting to provider: {error}")))?;
        lease.renew().await?;
        let response = timeout(FI_LIQUIDITY_RPC_TIMEOUT, client.request_liquidity(request))
            .await
            .map_err(|_| FiError::Timeout("requesting federation liquidity".to_owned()))?
            .map_err(|error| {
                FiError::Liquidity(format!(
                    "provider liquidity request failed ({}): {}",
                    error.code(),
                    error.message()
                ))
            })?;
        self.verify_request_response(&operation, &response)?;
        lease.renew().await?;
        self.inner
            .store
            .store_liquidity_response(&operation.operation_id, response)
            .await?;
        self.inner
            .store
            .load_liquidity_operation(&operation.operation_id)
            .await?
            .snapshot()
    }

    async fn collect_fman_trust<V>(
        &self,
        context: &FormedLiquidityContext,
        verifier: &V,
    ) -> FiResult<(FmanEndorsement, Vec<GetFederationTrustMaterialResponse>)>
    where
        V: LiquidityBadgeVerifier,
    {
        let mut peer_ids_by_fman = BTreeMap::<Pubkey, Vec<PeerId>>::new();
        for binding in &context.bindings {
            peer_ids_by_fman
                .entry(binding.fman_pubkey.clone())
                .or_default()
                .push(PeerId(binding.peer_id.0.clone()));
        }
        let request_base = GetFederationTrustMaterialRequest {
            version: ProtocolV1,
            federation_id: context.federation.federation_id().clone(),
            federation_config_hash: context.federation.federation_config_hash().clone(),
            peer_ids: Vec::new(),
        };
        let mut pending = FuturesUnordered::new();
        for (fman_pubkey, peer_ids) in peer_ids_by_fman {
            let locator = context
                .seats
                .iter()
                .find(|seat| seat.fman_id.to_string() == fman_pubkey.0)
                .map(|seat| &seat.locator)
                .ok_or_else(|| {
                    FiError::Liquidity(
                        "consensus FMan directory names an identity absent from formation recovery"
                            .to_owned(),
                    )
                })?
                .clone();
            let request = GetFederationTrustMaterialRequest {
                peer_ids,
                ..request_base.clone()
            };
            pending.push(async move {
                let client = self
                    .inner
                    .ports
                    .fman_connector
                    .connect(&locator)
                    .await
                    .map_err(|error| {
                        FiError::Liquidity(format!("connecting to federation FMan: {error}"))
                    })?;
                let response = client
                    .get_federation_trust_material(request.clone())
                    .await
                    .map_err(|error| {
                        FiError::Liquidity(format!("fetching FMan trust material: {error}"))
                    })?;
                let material = response
                    .verify_for_request(
                        &request,
                        FmanTimestamp(now_secs()?),
                        FI_LIQUIDITY_TRUST_MATERIAL_VALIDITY.as_secs(),
                    )
                    .map_err(|error| {
                        FiError::Liquidity(format!("verifying FMan trust material: {error}"))
                    })?;
                if material.fman_pubkey != fman_pubkey {
                    return Err(FiError::Liquidity(
                        "FMan trust response identity does not match consensus directory"
                            .to_owned(),
                    ));
                }
                let trust = first_verified_badge(
                    &fman_pubkey,
                    &material.holder_authorizations,
                    verifier,
                    Instant::now() + FI_LIQUIDITY_RPC_TIMEOUT,
                )
                .await
                .map_err(|reason| {
                    FiError::Liquidity(format!(
                        "FMan PeerBadge admission failed: {}",
                        reason.code()
                    ))
                })?;
                Ok::<_, FiError>((fman_pubkey, response, material, trust))
            });
        }

        let collected = timeout(FI_LIQUIDITY_RPC_TIMEOUT, async move {
            let mut all = BTreeMap::new();
            while let Some(result) = pending.next().await {
                let (pubkey, response, material, trust) = result?;
                all.insert(pubkey, (response, material, trust));
            }
            Ok::<_, FiError>(all)
        })
        .await
        .map_err(|_| FiError::Timeout("collecting federation FMan trust material".to_owned()))??;

        let mut endorsement = None;
        let mut all = Vec::with_capacity(collected.len());
        for (_pubkey, (response, material, trust)) in collected {
            if endorsement.is_none() {
                let attestation = material.peer_attestations.first().cloned().ok_or_else(|| {
                    FiError::Liquidity(
                        "FMan trust response contains no peer attestation".to_owned(),
                    )
                })?;
                endorsement = Some(FmanEndorsement { attestation, trust });
            }
            all.push(response);
        }
        Ok((
            endorsement.ok_or_else(|| {
                FiError::Liquidity("no FMan endorsement was available".to_owned())
            })?,
            all,
        ))
    }

    fn requester_pubkey(&self) -> FiResult<Pubkey> {
        self.inner
            .ports
            .identity
            .public_key()
            .map(|id| Pubkey(id.0.to_string()))
            .map_err(FiError::Identity)
    }

    fn sign_public_rpc<T>(&self, domain: PublicRpcPayloadDomain, payload: T) -> FiResult<Signed<T>>
    where
        T: Serialize,
    {
        let hash = public_rpc_payload_hash(domain, &payload)
            .map_err(|error| FiError::Liquidity(format!("hashing provider RPC: {error}")))?;
        let signature = self
            .inner
            .ports
            .identity
            .sign_digest(hash.0)
            .map_err(FiError::Identity)?;
        Ok(Signed {
            payload,
            proof: PayloadProof {
                signature: fedi_decentralized_service_liquidity_manager::Signature(
                    signature.0.to_byte_array().to_vec(),
                ),
            },
        })
    }

    fn verify_request_response(
        &self,
        operation: &StoredLiquidityOperation,
        response: &Signed<RequestLiquidityResponse>,
    ) -> FiResult<()> {
        validate_request_response(operation, response)?;
        verify_provider_freshness(response.payload.issued_at)
    }

    fn verify_status_response(
        &self,
        operation: &StoredLiquidityOperation,
        response: &Signed<GetAllocationStatusResponse>,
    ) -> FiResult<()> {
        validate_status_response(operation, response)?;
        verify_provider_freshness(response.payload.issued_at)
    }
}

fn validate_request_response(
    operation: &StoredLiquidityOperation,
    response: &Signed<RequestLiquidityResponse>,
) -> FiResult<()> {
    verify_provider_rpc(
        PublicRpcPayloadDomain::RequestLiquidityResponse,
        response,
        &operation.commitment.provider_pubkey,
    )?;
    if response.payload.version != PUBLIC_LIQUIDITY_PROTOCOL_VERSION
        || response.payload.provider_pubkey != operation.commitment.provider_pubkey
        || response.payload.details_payload_hash != operation.details_payload_hash
    {
        return Err(FiError::Liquidity(
            "provider response does not match the exact persisted request".to_owned(),
        ));
    }
    if let RequestLiquidityOutcome::Accepted(status) = &response.payload.outcome {
        verify_allocation_status(operation, status)?;
    }
    Ok(())
}

fn validate_status_response(
    operation: &StoredLiquidityOperation,
    response: &Signed<GetAllocationStatusResponse>,
) -> FiResult<()> {
    verify_provider_rpc(
        PublicRpcPayloadDomain::GetAllocationStatusResponse,
        response,
        &operation.commitment.provider_pubkey,
    )?;
    if response.payload.version != PUBLIC_LIQUIDITY_PROTOCOL_VERSION
        || response.payload.provider_pubkey != operation.commitment.provider_pubkey
    {
        return Err(FiError::Liquidity(
            "provider status does not match the exact persisted request".to_owned(),
        ));
    }
    verify_allocation_status(operation, &response.payload.status)
}

struct FormedFmanSeat {
    fman_id: PublicKey,
    locator: Locator,
    seat_id: SeatId,
}

struct FormedLiquidityContext {
    federation_name: crate::FederationName,
    invite_code: crate::InviteCode,
    network: BitcoinNetwork,
    federation: fedi_decentralized_domain::FederationSeats,
    bindings: Vec<VerifiedSeatBinding>,
    seats: Vec<FormedFmanSeat>,
}

impl FormedLiquidityContext {
    /// Build the provider-visible details, pairing each verified consensus
    /// binding with the recovered formation seat holding the same FMan
    /// identity.
    ///
    /// Bindings arrive in peer-id order and recovery seats in formation
    /// order, so index pairing would bake wrong seat ids into the semantic
    /// `details_payload_hash`. Contradictory material — a binding whose FMan
    /// identity matches no recovered seat or more than one, or one seat
    /// claimed by two bindings — fails closed instead of fabricating a hint.
    fn federation_details(&self) -> FiResult<FederationLiquidityDetails> {
        let mut claimed = vec![false; self.seats.len()];
        let fleet_seat_hints = self
            .bindings
            .iter()
            .map(|binding| {
                let mut matches = self
                    .seats
                    .iter()
                    .enumerate()
                    .filter(|(_, seat)| seat.fman_id.to_string() == binding.fman_pubkey.0);
                let (index, seat) = matches.next().ok_or_else(|| {
                    FiError::Liquidity(
                        "consensus seat binding names an FMan identity absent from formation \
                         recovery"
                            .to_owned(),
                    )
                })?;
                if matches.next().is_some() {
                    return Err(FiError::Liquidity(
                        "formation recovery holds multiple seats for one consensus FMan \
                         identity"
                            .to_owned(),
                    ));
                }
                if std::mem::replace(&mut claimed[index], true) {
                    return Err(FiError::Liquidity(
                        "two consensus seat bindings resolve to one recovered formation seat"
                            .to_owned(),
                    ));
                }
                Ok(FleetSeat {
                    seat_id: FleetSeatId(seat.seat_id.to_string()),
                    peer_id: PeerId(binding.peer_id.0.clone()),
                    guardian_identity: GuardianIdentity(binding.guardian_identity.0.clone()),
                    fleet_manager_pubkey: binding.fman_pubkey.clone(),
                    role_metadata: Vec::new(),
                })
            })
            .collect::<FiResult<Vec<_>>>()?;
        Ok(FederationLiquidityDetails {
            invite_code: self.invite_code.clone(),
            federation_id: self.federation.federation_id().clone(),
            federation_name: self.federation_name.clone(),
            federation_config_hash: self.federation.federation_config_hash().clone(),
            fleet_seat_hints,
            revocation_locations: Vec::new(),
        })
    }

    fn matches_commitment(&self, commitment: &RequestLiquidityDetailsCommitmentV1) -> FiResult<()> {
        let current = self.federation_details()?;
        // Display metadata is intentionally mutable after formation. Recovery
        // binds to final federation identity/config and replays the committed
        // name rather than making a later metadata edit strand an allocation.
        if self.network != commitment.network
            || current.invite_code != commitment.federation_details.invite_code
            || current.federation_id != commitment.federation_details.federation_id
            || current.federation_config_hash
                != commitment.federation_details.federation_config_hash
            || current.fleet_seat_hints != commitment.federation_details.fleet_seat_hints
        {
            return Err(FiError::Liquidity(
                "current federation consensus no longer matches the persisted liquidity request"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

fn liquidity_seat_bindings_field(meta_value: &Option<Vec<u8>>) -> FiResult<Option<String>> {
    let Some(meta_value) = meta_value else {
        return Ok(None);
    };
    let fields: BTreeMap<String, serde_json::Value> = serde_json::from_slice(meta_value)
        .map_err(|error| FiError::Liquidity(format!("invalid federation metadata: {error}")))?;
    match fields.get(FMAN_SEAT_BINDINGS_META_FIELD_KEY) {
        None => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(FiError::Liquidity(format!(
            "{FMAN_SEAT_BINDINGS_META_FIELD_KEY} is not a string"
        ))),
    }
}

fn verify_provider_rpc<T>(
    domain: PublicRpcPayloadDomain,
    signed: &Signed<T>,
    provider: &Pubkey,
) -> FiResult<()>
where
    T: Serialize,
{
    let public_key = provider
        .0
        .parse::<XOnlyPublicKey>()
        .map_err(|_| FiError::Liquidity("provider returned a malformed identity".to_owned()))?;
    let signature = Signature::from_slice(&signed.proof.signature.0)
        .map_err(|_| FiError::Liquidity("provider returned a malformed signature".to_owned()))?;
    let hash = public_rpc_payload_hash(domain, &signed.payload)
        .map_err(|error| FiError::Liquidity(format!("hashing provider response: {error}")))?;
    secp256k1::SECP256K1
        .verify_schnorr(&signature, &hash.0, &public_key)
        .map_err(|_| FiError::Liquidity("provider response signature is invalid".to_owned()))
}

fn verify_provider_freshness(issued_at: LiquidityTimestamp) -> FiResult<()> {
    if issued_at.0.abs_diff(now_secs()?) > FI_LIQUIDITY_REQUEST_VALIDITY.as_secs() {
        return Err(FiError::Liquidity(
            "provider response is outside the accepted freshness window".to_owned(),
        ));
    }
    Ok(())
}

fn verify_allocation_status(
    operation: &StoredLiquidityOperation,
    status: &AllocationStatus,
) -> FiResult<()> {
    if status.provider_pubkey != operation.commitment.provider_pubkey
        || status.details_payload_hash != operation.details_payload_hash
    {
        return Err(FiError::Liquidity(
            "provider allocation status is bound to another request".to_owned(),
        ));
    }
    let gateway_items = status
        .item_statuses
        .iter()
        .filter(|item| matches!(&item.target, AllocationItemTarget::Gateway { .. }))
        .count();
    let stability_items = status
        .item_statuses
        .iter()
        .filter(|item| matches!(&item.target, AllocationItemTarget::StabilityPool { .. }))
        .count();
    let expected_gateway = usize::from(operation.commitment.amounts.gateway_min_amount.0 > 0);
    let expected_stability = usize::from(operation.commitment.amounts.stability_min_amount.0 > 0);
    if gateway_items != expected_gateway || stability_items != expected_stability {
        return Err(FiError::Liquidity(
            "provider allocation items do not match the requested liquidity sources".to_owned(),
        ));
    }
    for item in &status.item_statuses {
        let (amount, minimum, maximum, source) = match &item.target {
            AllocationItemTarget::Gateway { amount, .. } => (
                amount.0,
                operation.commitment.amounts.gateway_min_amount.0,
                operation
                    .commitment
                    .amounts
                    .gateway_max_amount
                    .map(|maximum| maximum.0),
                "gateway",
            ),
            AllocationItemTarget::StabilityPool { amount, .. } => (
                amount.0,
                operation.commitment.amounts.stability_min_amount.0,
                operation
                    .commitment
                    .amounts
                    .stability_max_amount
                    .map(|maximum| maximum.0),
                "stability_pool",
            ),
        };
        if amount < minimum || maximum.is_some_and(|maximum| amount > maximum) {
            return Err(FiError::Liquidity(format!(
                "provider {source} allocation amount is outside the persisted request bounds"
            )));
        }
        match item.status {
            ItemAllocationStatus::Completed => match (&item.target, &item.completion_evidence) {
                (
                    AllocationItemTarget::Gateway {
                        gateway_id, amount, ..
                    },
                    Some(CompletionEvidence::Gateway(evidence)),
                ) if item.fulfilled_amount == Some(*amount)
                    && evidence.gateway_id == *gateway_id
                    && evidence.fulfilled_amount == *amount => {}
                (
                    AllocationItemTarget::StabilityPool { amount, .. },
                    Some(CompletionEvidence::StabilityPool(evidence)),
                ) if item.fulfilled_amount == Some(*amount)
                    && evidence.fulfilled_amount == *amount => {}
                _ => {
                    return Err(FiError::Liquidity(
                        "provider completion evidence does not match its allocation item"
                            .to_owned(),
                    ));
                }
            },
            _ if item.completion_evidence.is_some() => {
                return Err(FiError::Liquidity(
                    "provider attached completion evidence to a non-completed item".to_owned(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn completed_gateway_api(status: &AllocationStatus) -> FiResult<Option<GatewayApiUrl>> {
    status
        .item_statuses
        .iter()
        .find_map(|item| {
            if item.status != ItemAllocationStatus::Completed {
                return None;
            }
            match (&item.target, &item.completion_evidence) {
                (
                    AllocationItemTarget::Gateway { .. },
                    Some(CompletionEvidence::Gateway(evidence)),
                ) => Some(Ok(evidence.gateway_api.clone())),
                (AllocationItemTarget::Gateway { .. }, _) => Some(Err(FiError::Liquidity(
                    "completed gateway item has no gateway completion evidence".to_owned(),
                ))),
                _ => None,
            }
        })
        .transpose()
}

fn now_secs() -> FiResult<u64> {
    Ok(fedimint_core::time::duration_since_epoch().as_secs())
}

struct StaticProvider {
    event_id: EventId,
    created_at: u64,
    signed: Signed<LiquidityProviderAdvertisement>,
}

async fn discover_with<V>(
    events: Vec<Event>,
    intent: &LiquidityRequestIntent,
    network: BitcoinNetwork,
    pinned_provider: Option<&Pubkey>,
    verifier: &V,
    deadline: Instant,
    now: u64,
) -> FiResult<LiquidityDiscovery>
where
    V: LiquidityBadgeVerifier,
{
    let mut discovery = LiquidityDiscovery::default();
    let mut newest = BTreeMap::<PublicKey, StaticProvider>::new();
    for event in events
        .into_iter()
        .take(usize::from(FLIP_PROVIDER_ADVERTISEMENTS_CANDIDATE_LIMIT))
    {
        let author = event.pubkey;
        match static_admit(&event) {
            Ok(candidate) => match newest.entry(author) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(candidate);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let is_newer = candidate.created_at > entry.get().created_at
                        || (candidate.created_at == entry.get().created_at
                            && candidate.event_id.as_bytes() < entry.get().event_id.as_bytes());
                    if is_newer {
                        entry.insert(candidate);
                    }
                    discovery.rejected.push((
                        Some(Pubkey(author.to_string())),
                        LiquidityProviderRejection::Superseded,
                    ));
                }
            },
            Err(reason) => discovery
                .rejected
                .push((Some(Pubkey(author.to_string())), reason)),
        }
    }

    for (author, candidate) in newest {
        if Instant::now() >= deadline {
            discovery.rejected.push((
                Some(Pubkey(author.to_string())),
                LiquidityProviderRejection::DeadlineExpired,
            ));
            continue;
        }
        match admit_provider(
            candidate.signed,
            intent,
            network,
            pinned_provider,
            verifier,
            deadline,
            now,
        )
        .await
        {
            Ok(provider) => discovery.providers.push(provider),
            Err(reason) => discovery
                .rejected
                .push((Some(Pubkey(author.to_string())), reason)),
        }
    }
    discovery
        .providers
        .sort_by(|left, right| left.provider_pubkey.0.cmp(&right.provider_pubkey.0));
    Ok(discovery)
}

fn static_admit(event: &Event) -> Result<StaticProvider, LiquidityProviderRejection> {
    if event.kind != Kind::Custom(FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND)
        || !has_exact_d_tag(event, FLIP_PROVIDER_ADVERTISEMENT_D_TAG)
        || !has_exact_hashtag(event, FLIP_PROVIDER_ADVERTISEMENT_HASHTAG)
    {
        return Err(LiquidityProviderRejection::WrongEventRole);
    }
    event
        .verify()
        .map_err(|_| LiquidityProviderRejection::InvalidEvent)?;
    let signed: Signed<LiquidityProviderAdvertisement> = serde_json::from_str(&event.content)
        .map_err(|_| LiquidityProviderRejection::InvalidDocument)?;
    if signed.payload.provider_pubkey.0 != event.pubkey.to_string() {
        return Err(LiquidityProviderRejection::AuthorMismatch);
    }
    verify_advertisement_proof(&signed)?;
    Ok(StaticProvider {
        event_id: event.id,
        created_at: event.created_at.as_secs(),
        signed,
    })
}

fn has_exact_hashtag(event: &Event, expected: &str) -> bool {
    let mut tags = event.tags.iter().filter(|tag| tag.kind() == TagKind::t());
    let Some(tag) = tags.next() else {
        return false;
    };
    let tag = tag.as_slice();
    tag.len() == 2 && tag[0] == "t" && tag[1] == expected && tags.next().is_none()
}

fn verify_advertisement_proof(
    signed: &Signed<LiquidityProviderAdvertisement>,
) -> Result<(), LiquidityProviderRejection> {
    let pubkey = signed
        .payload
        .provider_pubkey
        .0
        .parse::<XOnlyPublicKey>()
        .map_err(|_| LiquidityProviderRejection::InvalidProviderKey)?;
    let signature = Signature::from_slice(&signed.proof.signature.0)
        .map_err(|_| LiquidityProviderRejection::InvalidProviderProof)?;
    let hash = advertisement_hash(&signed.payload)
        .map_err(|_| LiquidityProviderRejection::InvalidProviderProof)?;
    secp256k1::SECP256K1
        .verify_schnorr(&signature, &hash.0, &pubkey)
        .map_err(|_| LiquidityProviderRejection::InvalidProviderProof)
}

async fn admit_provider<V>(
    signed: Signed<LiquidityProviderAdvertisement>,
    intent: &LiquidityRequestIntent,
    network: BitcoinNetwork,
    pinned_provider: Option<&Pubkey>,
    verifier: &V,
    deadline: Instant,
    now: u64,
) -> Result<AdmittedLiquidityProvider, LiquidityProviderRejection>
where
    V: LiquidityBadgeVerifier,
{
    let ad = signed.payload;
    if ad.version != PUBLIC_LIQUIDITY_PROTOCOL_VERSION
        || !ad.api_versions.contains(&ProtocolVersion(1))
    {
        return Err(LiquidityProviderRejection::UnsupportedVersion);
    }
    if ad.issued_at.0 > now {
        return Err(LiquidityProviderRejection::IssuedInFuture);
    }
    if ad.expires_at.0 <= now {
        return Err(LiquidityProviderRejection::Expired);
    }
    if now - ad.issued_at.0 > FI_LIQUIDITY_MAX_ADVERTISEMENT_AGE.as_secs() {
        return Err(LiquidityProviderRejection::Stale);
    }
    if !ad.policy.supported_networks.contains(&network) {
        return Err(LiquidityProviderRejection::UnsupportedNetwork);
    }
    for source in [SourceType::Gateway, SourceType::StabilityPool] {
        if intent.requires(source) && !ad.supported_sources.contains(&source) {
            return Err(LiquidityProviderRejection::UnsupportedSource);
        }
    }
    if pinned_provider.is_some_and(|pinned| pinned != &ad.provider_pubkey) {
        return Err(LiquidityProviderRejection::NotCommittedProvider);
    }
    let (endpoint_url, endpoint) = admit_endpoint(&ad.api_endpoints)?;
    verify_provider_badge(
        &ad.provider_pubkey,
        &ad.holder_authorizations,
        verifier,
        deadline,
    )
    .await?;
    let hash =
        advertisement_hash(&ad).map_err(|_| LiquidityProviderRejection::InvalidProviderProof)?;
    Ok(AdmittedLiquidityProvider {
        provider_pubkey: ad.provider_pubkey.clone(),
        endpoint,
        endpoint_url,
        advertisement_hash: hash,
        advertisement: ad,
    })
}

pub(crate) fn admit_endpoint(
    urls: &[Url],
) -> Result<(Url, EndpointAddr), LiquidityProviderRejection> {
    let expected_alpn = String::from_utf8_lossy(PUBLIC_LIQUIDITY_API_ALPN).replace('/', "%2F");
    let expected_alpn_pair = format!("alpn={expected_alpn}");
    urls.iter()
        .find_map(|url| {
            let endpoint = url.0.strip_prefix("iroh://")?;
            let (node, query) = endpoint.split_once('?')?;
            if node.is_empty() || node.contains(['/', '#']) || query.contains('#') {
                return None;
            }
            if !query.split('&').any(|pair| pair == expected_alpn_pair) {
                return None;
            }
            let id = node.parse::<EndpointId>().ok()?;
            Some((url.clone(), EndpointAddr::new(id)))
        })
        .ok_or(LiquidityProviderRejection::InvalidEndpoint)
}

async fn verify_provider_badge<V>(
    provider: &Pubkey,
    envelopes: &[HolderAuthorizationEnvelope],
    verifier: &V,
    deadline: Instant,
) -> Result<(), LiquidityProviderRejection>
where
    V: LiquidityBadgeVerifier,
{
    first_verified_badge(provider, envelopes, verifier, deadline)
        .await
        .map(drop)
}

async fn first_verified_badge<V>(
    provider: &Pubkey,
    envelopes: &[HolderAuthorizationEnvelope],
    verifier: &V,
    deadline: Instant,
) -> Result<HolderAuthorizationEnvelope, LiquidityProviderRejection>
where
    V: LiquidityBadgeVerifier,
{
    if envelopes.is_empty() {
        return Err(LiquidityProviderRejection::MissingPeerBadge);
    }
    let mut last_error = None;
    let mut subject_mismatch = false;
    for envelope in envelopes
        .iter()
        .take(FI_LIQUIDITY_MAX_HOLDER_AUTHORIZATIONS)
    {
        if Instant::now() >= deadline {
            return Err(LiquidityProviderRejection::DeadlineExpired);
        }
        match verifier.verify_subject(envelope).await {
            Ok(subject) if subject.to_string() == provider.0 => {
                return Ok(envelope.clone());
            }
            Ok(_) => subject_mismatch = true,
            Err(error) => last_error = Some(error),
        }
    }
    if subject_mismatch {
        return Err(LiquidityProviderRejection::PeerBadgeSubjectMismatch);
    }
    Err(last_error.map_or(
        LiquidityProviderRejection::MissingPeerBadge,
        LiquidityProviderRejection::PeerBadgeRejected,
    ))
}

#[cfg(test)]
mod tests {
    use fedi_decentralized_domain::{FederationId, FederationName, HashBytes, InviteCode};
    use secp256k1::{Keypair, SecretKey};

    use super::*;

    fn commitment(provider: Pubkey) -> RequestLiquidityDetailsCommitmentV1 {
        RequestLiquidityDetailsCommitmentV1 {
            version: PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
            requester_pubkey: Pubkey("requester".to_owned()),
            provider_pubkey: provider,
            network: BitcoinNetwork::Regtest,
            amounts: LiquidityRequestIntent::gateway(10_000, Some(20_000)).amounts,
            federation_details: FederationLiquidityDetails {
                invite_code: InviteCode("invite".to_owned()),
                federation_id: FederationId("federation".to_owned()),
                federation_name: FederationName("name".to_owned()),
                federation_config_hash: HashBytes(vec![7; 32]),
                fleet_seat_hints: Vec::new(),
                revocation_locations: Vec::new(),
            },
            expires_at: LiquidityTimestamp(now_secs().expect("clock") + 3600),
        }
    }

    #[test]
    fn liquidity_intent_requires_a_source_and_ordered_bounds() {
        let empty = LiquidityRequestIntent {
            amounts: LiquidityAmountBounds {
                gateway_min_amount: Sats(0),
                gateway_max_amount: None,
                stability_min_amount: Sats(0),
                stability_max_amount: None,
            },
        };
        assert!(empty.validate().is_err());

        let inverted = LiquidityRequestIntent::gateway(20, Some(19));
        assert!(inverted.validate().is_err());
        assert!(
            LiquidityRequestIntent::gateway(20, Some(20))
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn metadata_directory_field_is_typed_and_fail_closed() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            FMAN_SEAT_BINDINGS_META_FIELD_KEY: "canonical"
        }))
        .expect("json");
        assert_eq!(
            liquidity_seat_bindings_field(&Some(bytes)).expect("metadata"),
            Some("canonical".to_owned())
        );
        let wrong = serde_json::to_vec(&serde_json::json!({
            FMAN_SEAT_BINDINGS_META_FIELD_KEY: 1
        }))
        .expect("json");
        assert!(liquidity_seat_bindings_field(&Some(wrong)).is_err());
    }

    fn fman_identity(marker: u8) -> (PublicKey, Pubkey) {
        let secret = SecretKey::from_byte_array(&[marker; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(secp256k1::SECP256K1, &secret);
        let hex = XOnlyPublicKey::from_keypair(&keypair).0.to_string();
        (
            PublicKey::parse(&hex).expect("x-only key is a valid nostr key"),
            Pubkey(hex),
        )
    }

    fn formed_seat(fman_id: PublicKey, marker: u8) -> FormedFmanSeat {
        let secret = SecretKey::from_byte_array(&[marker; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(secp256k1::SECP256K1, &secret);
        FormedFmanSeat {
            fman_id,
            locator: Locator::new(
                EndpointAddr::new(
                    fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[marker; 32]).public(),
                ),
                XOnlyPublicKey::from_keypair(&keypair).0,
            ),
            seat_id: SeatId::from(crate::QuoteId([marker; 32])),
        }
    }

    fn verified_binding(peer: &str, fman_pubkey: Pubkey) -> VerifiedSeatBinding {
        let scalar = peer
            .bytes()
            .fold(1u8, |acc, byte| acc.wrapping_add(byte).max(1));
        VerifiedSeatBinding {
            peer_id: fedi_decentralized_domain::PeerId(peer.to_owned()),
            guardian_identity: fedi_decentralized_domain::GuardianIdentity(format!(
                "guardian-{peer}"
            )),
            fman_pubkey,
            guardian_fee_account: stability_pool_common::Account::single(
                bitcoin::secp256k1::PublicKey::from_secret_key(
                    bitcoin::secp256k1::SECP256K1,
                    &bitcoin::secp256k1::SecretKey::from_slice(&[scalar; 32])
                        .expect("fixed test scalar is valid"),
                ),
                stability_pool_common::AccountType::BtcDepositor,
            ),
        }
    }

    fn formed_context(
        seats: Vec<FormedFmanSeat>,
        bindings: Vec<VerifiedSeatBinding>,
    ) -> FormedLiquidityContext {
        FormedLiquidityContext {
            federation_name: FederationName("name".to_owned()),
            invite_code: InviteCode("invite".to_owned()),
            network: BitcoinNetwork::Regtest,
            federation: fedi_decentralized_domain::FederationSeats::from_parts(
                FederationId("federation".to_owned()),
                HashBytes(vec![7; 32]),
                3,
                Vec::new(),
            ),
            bindings,
            seats,
        }
    }

    #[test]
    fn seat_id_hints_pair_bindings_with_seats_by_fman_identity() {
        let (first_id, first_pubkey) = fman_identity(51);
        let (second_id, second_pubkey) = fman_identity(52);
        // Recovery seats arrive in formation order and bindings in peer-id
        // order; the pairing must follow FMan identity, not position.
        let context = formed_context(
            vec![formed_seat(first_id, 1), formed_seat(second_id, 2)],
            vec![
                verified_binding("0", second_pubkey.clone()),
                verified_binding("1", first_pubkey.clone()),
            ],
        );
        let details = context
            .federation_details()
            .expect("aligned identities pair");
        assert_eq!(details.fleet_seat_hints.len(), 2);
        assert_eq!(
            details.fleet_seat_hints[0].fleet_manager_pubkey,
            second_pubkey
        );
        assert_eq!(
            details.fleet_seat_hints[0].seat_id,
            FleetSeatId(SeatId::from(crate::QuoteId([2; 32])).to_string()),
        );
        assert_eq!(
            details.fleet_seat_hints[1].fleet_manager_pubkey,
            first_pubkey
        );
        assert_eq!(
            details.fleet_seat_hints[1].seat_id,
            FleetSeatId(SeatId::from(crate::QuoteId([1; 32])).to_string()),
        );
    }

    #[test]
    fn seat_id_hints_fail_closed_on_contradictory_material() {
        // A binding naming an identity absent from formation recovery.
        let (first_id, _) = fman_identity(53);
        let (_, unknown_pubkey) = fman_identity(54);
        let context = formed_context(
            vec![formed_seat(first_id, 1)],
            vec![verified_binding("0", unknown_pubkey)],
        );
        assert!(
            context.federation_details().is_err(),
            "an unmatched binding must not fabricate a seat id"
        );

        // Two bindings resolving to the one recovered seat.
        let (second_id, second_pubkey) = fman_identity(55);
        let context = formed_context(
            vec![formed_seat(second_id, 2)],
            vec![
                verified_binding("0", second_pubkey.clone()),
                verified_binding("1", second_pubkey),
            ],
        );
        assert!(
            context.federation_details().is_err(),
            "one seat cannot back two bindings"
        );

        // One binding matching two recovered seats is ambiguous.
        let (third_id, third_pubkey) = fman_identity(56);
        let context = formed_context(
            vec![formed_seat(third_id, 3), formed_seat(third_id, 4)],
            vec![verified_binding("0", third_pubkey)],
        );
        assert!(
            context.federation_details().is_err(),
            "an ambiguous identity match must not guess a seat id"
        );
    }

    #[test]
    fn recovered_provider_status_proves_acceptance_without_lost_ack() {
        let provider = Pubkey("provider".to_owned());
        let commitment = commitment(provider.clone());
        let hash = request_liquidity_details_hash(&commitment).expect("hash");
        let stored = StoredLiquidityOperation {
            schema_version: 3,
            operation_id: LiquidityOperationId(hex::encode(hash.0)),
            formation_id: FormationId("formation".to_owned()),
            commitment,
            endpoint_hint: Url("iroh://hint".to_owned()),
            details_payload_hash: hash,
            response: None,
            status: Some(Signed {
                payload: GetAllocationStatusResponse {
                    version: PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
                    provider_pubkey: provider.clone(),
                    issued_at: LiquidityTimestamp(now_secs().expect("clock")),
                    status: AllocationStatus {
                        details_payload_hash: hash,
                        provider_pubkey: provider,
                        item_statuses: Vec::new(),
                    },
                },
                proof: PayloadProof {
                    signature: fedi_decentralized_service_liquidity_manager::Signature(vec![0; 64]),
                },
            }),
            verified_gateway_api: None,
        };
        assert_eq!(
            stored.snapshot().expect("snapshot").phase,
            LiquidityOperationPhase::Accepted
        );
    }

    #[test]
    fn provider_rpc_signature_is_domain_separated_and_payload_bound() {
        let secret = SecretKey::from_byte_array(&[42; 32]).expect("secret");
        let keypair = Keypair::from_secret_key(secp256k1::SECP256K1, &secret);
        let (public_key, _) = XOnlyPublicKey::from_keypair(&keypair);
        let provider = Pubkey(public_key.to_string());
        let payload = GetAllocationStatusResponse {
            version: PUBLIC_LIQUIDITY_PROTOCOL_VERSION,
            provider_pubkey: provider.clone(),
            issued_at: LiquidityTimestamp(now_secs().expect("clock")),
            status: AllocationStatus {
                details_payload_hash: Sha256Digest([3; 32]),
                provider_pubkey: provider.clone(),
                item_statuses: Vec::new(),
            },
        };
        let hash = public_rpc_payload_hash(
            PublicRpcPayloadDomain::GetAllocationStatusResponse,
            &payload,
        )
        .expect("hash");
        let proof = secp256k1::SECP256K1
            .sign_schnorr_no_aux_rand(&hash.0, &keypair)
            .to_byte_array()
            .to_vec();
        let mut signed = Signed {
            payload,
            proof: PayloadProof {
                signature: fedi_decentralized_service_liquidity_manager::Signature(proof),
            },
        };
        assert!(
            verify_provider_rpc(
                PublicRpcPayloadDomain::GetAllocationStatusResponse,
                &signed,
                &provider,
            )
            .is_ok()
        );
        signed.payload.status.details_payload_hash = Sha256Digest([4; 32]);
        assert!(
            verify_provider_rpc(
                PublicRpcPayloadDomain::GetAllocationStatusResponse,
                &signed,
                &provider,
            )
            .is_err()
        );
    }
}
