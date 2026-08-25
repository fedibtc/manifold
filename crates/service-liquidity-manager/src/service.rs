//! Liquidity Manager service traits.

use fedi_iroh_rpc::service;

use crate::{
    AbandonTargetClientValueRequest, AbandonTargetClientValueResponse, ApplySetupConfigRequest,
    ApplySetupConfigResponse, AttestationInstallRequest, AttestationInstallResponse,
    AttestationListRequest, AttestationListResponse, AttestationRemoveRequest,
    AttestationRemoveResponse, BindTargetDepositRequest, BindTargetDepositResponse,
    CancelAllocationRequest, CancelAllocationResponse, CompleteReviewWithoutEvidenceRequest,
    CompleteReviewWithoutEvidenceResponse, CreateBackupRequest, CreateBackupResponse,
    CreateDepositAddressRequest, CreateDepositAddressResponse, GetAdminAllocationRequest,
    GetAdminAllocationResponse, GetAdvertisementStateRequest, GetAdvertisementStateResponse,
    GetAllocationStatusRequest, GetAllocationStatusResponse, GetFundsRequest, GetFundsResponse,
    GetHealthRequest, GetHealthResponse, GetHolderAuthorizationStateRequest,
    GetHolderAuthorizationStateResponse, GetProviderConfigRequest, GetProviderConfigResponse,
    GetProviderInfoRequest, GetProviderInfoResponse, GetSetupStateRequest, GetSetupStateResponse,
    GetVerificationSummaryRequest, GetVerificationSummaryResponse, GetWalletOperationRequest,
    GetWalletOperationResponse, InspectBackupRequest, InspectBackupResponse,
    InspectTargetClientRequest, InspectTargetClientResponse, InstallProviderIdentityRequest,
    InstallProviderIdentityResponse, ListAllocationsRequest, ListAllocationsResponse,
    ListWalletOperationsRequest, ListWalletOperationsResponse, ProbeGatewayRequest,
    ProbeGatewayResponse, RefreshHolderAuthorizationsRequest, RefreshHolderAuthorizationsResponse,
    RefreshRelaysRequest, RefreshRelaysResponse, ReleaseFederationAllocationRequest,
    ReleaseFederationAllocationResponse, ReopenFederationClientRequest,
    ReopenFederationClientResponse, RepublishAdvertisementRequest, RepublishAdvertisementResponse,
    RequestLiquidityRequest, RequestLiquidityResponse, RequestWithdrawalRequest,
    RequestWithdrawalResponse, ResolveManualReviewRequest, ResolveManualReviewResponse,
    RestoreBackupRequest, RestoreBackupResponse, RetryFundingStepRequest, RetryFundingStepResponse,
    RotateAdminTokenRequest, RotateAdminTokenResponse, ServiceResult, SetConfigSecretRequest,
    SetConfigSecretResponse, Signed, UpdateProviderConfigRequest, UpdateProviderConfigResponse,
    ValidateSetupRequest, ValidateSetupResponse, WithdrawAdvertisementRequest,
    WithdrawAdvertisementResponse,
};

/// App-facing Public Liquidity API.
///
/// Apps call this API only after discovering a provider advertisement and
/// validating the advertised provider identity, endpoint URL, and policy.
/// In MVP that advertisement is the provider's only public ready event.
/// In the MVP flow, `request_liquidity` is the acceptance boundary: a provider
/// either rejects the request or commits concrete allocation items within the
/// requested bounds and durably creates allocation work.
#[service]
pub trait PublicLiquidityApi {
    /// Optionally get provider info after discovering and validating an advertisement.
    ///
    /// This preflight repeats advertisement-derived public information while
    /// confirming that the app is talking to the expected provider, negotiating
    /// the Public Liquidity API version, and checking availability before private
    /// federation details are sent. Apps may skip this call and send
    /// `request_liquidity` directly after validating a trusted advertisement.
    async fn get_provider_info(
        &self,
        request: Signed<GetProviderInfoRequest>,
    ) -> ServiceResult<Signed<GetProviderInfoResponse>>;

    /// Request liquidity for a concrete federation.
    ///
    /// The app sends private federation details and amount bounds for the
    /// provider to verify locally. A successful response means the provider has
    /// accepted the request, committed allocation items within those bounds, and
    /// created durable allocation work that can be polled.
    async fn request_liquidity(
        &self,
        request: Signed<RequestLiquidityRequest>,
    ) -> ServiceResult<Signed<RequestLiquidityResponse>>;

    /// Get allocation status for an accepted liquidity request.
    ///
    /// The app polls this after a request has been accepted. The response
    /// reports durable allocation progress, per-item completion/failure details,
    /// and evidence hints that the app can independently verify before treating
    /// liquidity as ready.
    async fn get_allocation_status(
        &self,
        request: Signed<GetAllocationStatusRequest>,
    ) -> ServiceResult<Signed<GetAllocationStatusResponse>>;
}

/// Private Operator Admin API.
pub trait OperatorAdminApi {
    /// Get daemon and dependency health.
    async fn get_health(&self, request: GetHealthRequest) -> ServiceResult<GetHealthResponse>;

    /// Get first-run setup state.
    async fn get_setup_state(
        &self,
        request: GetSetupStateRequest,
    ) -> ServiceResult<GetSetupStateResponse>;

    /// Ask a gateway who it is, before any configuration names it.
    ///
    /// `gateway_id` is frozen at first setup and decides which gateway an
    /// accepted allocation pays, so it cannot be operator input: a typo is
    /// permanent. It is read from the gateway once, while nothing is stored.
    async fn probe_gateway(
        &self,
        request: ProbeGatewayRequest,
    ) -> ServiceResult<ProbeGatewayResponse>;

    /// Write one named secret the daemon holds for the operator.
    ///
    /// Secrets are not carried in the configuration they belong to. A config
    /// write states the whole configuration, so a secret inside one has an
    /// absent case that has to mean something — and the two secrets here used
    /// to mean opposite things: a missing gateway credential failed the write,
    /// while a missing chain-observer password deleted the stored one.
    async fn set_config_secret(
        &self,
        request: SetConfigSecretRequest,
    ) -> ServiceResult<SetConfigSecretResponse>;

    /// Apply first-run setup config.
    async fn apply_setup_config(
        &self,
        request: ApplySetupConfigRequest,
    ) -> ServiceResult<ApplySetupConfigResponse>;

    /// Validate current or candidate setup config.
    async fn validate_setup(
        &self,
        request: ValidateSetupRequest,
    ) -> ServiceResult<ValidateSetupResponse>;

    /// Get provider config.
    async fn get_provider_config(
        &self,
        request: GetProviderConfigRequest,
    ) -> ServiceResult<GetProviderConfigResponse>;

    /// Update grouped provider config.
    async fn update_provider_config(
        &self,
        request: UpdateProviderConfigRequest,
    ) -> ServiceResult<UpdateProviderConfigResponse>;

    /// Install the provider signing identity without restarting the daemon.
    async fn install_provider_identity(
        &self,
        request: InstallProviderIdentityRequest,
    ) -> ServiceResult<InstallProviderIdentityResponse>;

    /// Replace the Operator Admin API bearer token.
    async fn rotate_admin_token(
        &self,
        request: RotateAdminTokenRequest,
    ) -> ServiceResult<RotateAdminTokenResponse>;

    /// Close a target federation's Fedimint client so the next use reopens it.
    async fn reopen_federation_client(
        &self,
        request: ReopenFederationClientRequest,
    ) -> ServiceResult<ReopenFederationClientResponse>;

    /// Ingest and validate an issuer authority: the operator-configured trust
    /// policy naming an issuer this provider will accept badges from.
    async fn attestation_install(
        &self,
        request: AttestationInstallRequest,
    ) -> ServiceResult<AttestationInstallResponse>;

    /// List installed attestation payloads.
    async fn attestation_list(
        &self,
        request: AttestationListRequest,
    ) -> ServiceResult<AttestationListResponse>;

    /// Remove installed attestation payloads.
    async fn attestation_remove(
        &self,
        request: AttestationRemoveRequest,
    ) -> ServiceResult<AttestationRemoveResponse>;

    /// Read the Holder-authorization enrollment state from local state.
    async fn get_holder_authorization_state(
        &self,
        request: GetHolderAuthorizationStateRequest,
    ) -> ServiceResult<GetHolderAuthorizationStateResponse>;

    /// Reconcile enrolled Holder authorizations against the configured relays.
    async fn refresh_holder_authorizations(
        &self,
        request: RefreshHolderAuthorizationsRequest,
    ) -> ServiceResult<RefreshHolderAuthorizationsResponse>;

    /// Get advertisement publication state.
    async fn get_advertisement_state(
        &self,
        request: GetAdvertisementStateRequest,
    ) -> ServiceResult<GetAdvertisementStateResponse>;

    /// Republish the provider's public ready advertisement.
    async fn republish_advertisement(
        &self,
        request: RepublishAdvertisementRequest,
    ) -> ServiceResult<RepublishAdvertisementResponse>;

    /// Withdraw the current public ready advertisement.
    async fn withdraw_advertisement(
        &self,
        request: WithdrawAdvertisementRequest,
    ) -> ServiceResult<WithdrawAdvertisementResponse>;

    /// Refresh relay connections and cursors.
    async fn refresh_relays(
        &self,
        request: RefreshRelaysRequest,
    ) -> ServiceResult<RefreshRelaysResponse>;

    /// Produce an unencrypted FLIP data-directory tarball backup archive.
    async fn create_backup(
        &self,
        request: CreateBackupRequest,
    ) -> ServiceResult<CreateBackupResponse>;

    /// Inspect an unencrypted backup archive manifest.
    async fn inspect_backup(
        &self,
        request: InspectBackupRequest,
    ) -> ServiceResult<InspectBackupResponse>;

    /// Restore from an unencrypted backup archive.
    async fn restore_backup(
        &self,
        request: RestoreBackupRequest,
    ) -> ServiceResult<RestoreBackupResponse>;

    /// Get funds, replenishment, and inventory state.
    async fn get_funds(&self, request: GetFundsRequest) -> ServiceResult<GetFundsResponse>;

    /// Create a fresh top-up address.
    async fn create_deposit_address(
        &self,
        request: CreateDepositAddressRequest,
    ) -> ServiceResult<CreateDepositAddressResponse>;

    /// Request an operator withdrawal.
    async fn request_withdrawal(
        &self,
        request: RequestWithdrawalRequest,
    ) -> ServiceResult<RequestWithdrawalResponse>;

    /// List wallet operations.
    async fn list_wallet_operations(
        &self,
        request: ListWalletOperationsRequest,
    ) -> ServiceResult<ListWalletOperationsResponse>;

    /// Read one wallet operation in full.
    ///
    /// The list shape carries no destination, chain evidence or failure detail,
    /// so a client that has to act on a single operation — an operator
    /// resolving a send held for manual review — cannot see what it is
    /// resolving from the list alone.
    async fn get_wallet_operation(
        &self,
        request: GetWalletOperationRequest,
    ) -> ServiceResult<GetWalletOperationResponse>;

    /// List allocation work.
    async fn list_allocations(
        &self,
        request: ListAllocationsRequest,
    ) -> ServiceResult<ListAllocationsResponse>;

    /// Get allocation detail.
    async fn get_allocation(
        &self,
        request: GetAdminAllocationRequest,
    ) -> ServiceResult<GetAdminAllocationResponse>;

    /// Get verification summary for a liquidity request.
    async fn get_verification_summary(
        &self,
        request: GetVerificationSummaryRequest,
    ) -> ServiceResult<GetVerificationSummaryResponse>;

    /// Retry an idempotent funding step.
    async fn retry_funding_step(
        &self,
        request: RetryFundingStepRequest,
    ) -> ServiceResult<RetryFundingStepResponse>;

    /// Inspect the target federation client behind a stability-pool allocation.
    async fn inspect_target_client(
        &self,
        request: InspectTargetClientRequest,
    ) -> ServiceResult<InspectTargetClientResponse>;

    /// Give up on a stability item whose target-client value FLIP cannot recover.
    async fn abandon_target_client_value(
        &self,
        request: AbandonTargetClientValueRequest,
    ) -> ServiceResult<AbandonTargetClientValueResponse>;

    /// Release a federation's allocation binding when it is idle but wedged, so
    /// the federation can be requested again.
    async fn release_federation_allocation(
        &self,
        request: ReleaseFederationAllocationRequest,
    ) -> ServiceResult<ReleaseFederationAllocationResponse>;

    /// Complete a reviewed wallet send FLIP could not verify against the chain,
    /// recording that no evidence existed.
    async fn complete_review_without_evidence(
        &self,
        request: CompleteReviewWithoutEvidenceRequest,
    ) -> ServiceResult<CompleteReviewWithoutEvidenceResponse>;

    /// Bind a target-client deposit to the allocation item that paid for it.
    async fn bind_target_deposit(
        &self,
        request: BindTargetDepositRequest,
    ) -> ServiceResult<BindTargetDepositResponse>;

    /// Resolve a wallet send that escalation put under manual review.
    async fn resolve_manual_review(
        &self,
        request: ResolveManualReviewRequest,
    ) -> ServiceResult<ResolveManualReviewResponse>;

    /// Cancel allocation work where protocol state allows.
    async fn cancel_allocation(
        &self,
        request: CancelAllocationRequest,
    ) -> ServiceResult<CancelAllocationResponse>;
}
