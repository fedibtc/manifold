//! Operator Admin API: the verb implementations and the HTTP surface serving
//! them.
//!
//! One private HTTP/JSON endpoint behind a bearer token, split into the live
//! route set and the smaller one restore-only mode serves. Each verb implements
//! `OperatorAdminApi` for `DaemonContext` and delegates to the store or worker
//! that owns the effect. See
//! [SPEC-flip-admin-api](../specs/SPEC-flip-admin-api.md).

use anyhow::Context;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use constant_time_eq::constant_time_eq;
use fedi_decentralized_service_liquidity_manager::{
    AbandonTargetClientValueRequest, AbandonTargetClientValueResponse, ApplySetupConfigRequest,
    ApplySetupConfigResponse, AttestationInstallRequest, AttestationInstallResponse,
    AttestationListRequest, AttestationListResponse, AttestationRemoveRequest,
    AttestationRemoveResponse, BindTargetDepositRequest, BindTargetDepositResponse,
    CancelAllocationRequest, CancelAllocationResponse, CompleteReviewWithoutEvidenceRequest,
    CompleteReviewWithoutEvidenceResponse, ComponentHealth, CreateBackupRequest,
    CreateBackupResponse, CreateDepositAddressRequest, CreateDepositAddressResponse,
    GetAdminAllocationRequest, GetAdminAllocationResponse, GetAdvertisementStateRequest,
    GetAdvertisementStateResponse, GetFundsRequest, GetFundsResponse, GetHealthRequest,
    GetHealthResponse, GetHolderAuthorizationStateRequest, GetHolderAuthorizationStateResponse,
    GetProviderConfigRequest, GetProviderConfigResponse, GetSetupStateRequest,
    GetSetupStateResponse, GetVerificationSummaryRequest, GetVerificationSummaryResponse,
    GetWalletOperationRequest, GetWalletOperationResponse, HealthComponent, HealthMode,
    HealthStatus, HolderAuthorizationStatus, InspectBackupRequest, InspectBackupResponse,
    InspectTargetClientRequest, InspectTargetClientResponse, InstallProviderIdentityRequest,
    InstallProviderIdentityResponse, ListAllocationsRequest, ListAllocationsResponse,
    ListWalletOperationsRequest, ListWalletOperationsResponse, OperatorAdminApi,
    ProbeGatewayRequest, ProbeGatewayResponse, RefreshHolderAuthorizationsRequest,
    RefreshHolderAuthorizationsResponse, RefreshRelaysRequest, RefreshRelaysResponse,
    RelayFetchFailure, ReleaseFederationAllocationRequest, ReleaseFederationAllocationResponse,
    ReopenFederationClientRequest, ReopenFederationClientResponse, RepublishAdvertisementRequest,
    RepublishAdvertisementResponse, RequestWithdrawalRequest, RequestWithdrawalResponse,
    ResolveManualReviewRequest, ResolveManualReviewResponse, RestoreBackupRequest,
    RestoreBackupResponse, RetryFundingStepRequest, RetryFundingStepResponse,
    RotateAdminTokenRequest, RotateAdminTokenResponse, ServiceError, ServiceErrorCode,
    ServiceResult, SetConfigSecretRequest, SetConfigSecretResponse, Timestamp,
    UpdateProviderConfigRequest, UpdateProviderConfigResponse, ValidateSetupRequest,
    ValidateSetupResponse, WithdrawAdvertisementRequest, WithdrawAdvertisementResponse,
};
use serde::Serialize;
use tokio::net::TcpListener;

use crate::DaemonContext;
use crate::admin_token;
use crate::advertisement;
use crate::allocation_store;
use crate::attestation_store;
use crate::backup;
use crate::config::{DaemonArgs, DaemonPaths};
use crate::daemon::DaemonShell;
use crate::funds_admin;
use crate::manual_ops;
use crate::setup_store;
use crate::target_recovery;
use crate::wallet;

impl OperatorAdminApi for DaemonContext {
    async fn get_health(&self, _request: GetHealthRequest) -> ServiceResult<GetHealthResponse> {
        Ok(self.health_response().await)
    }

    async fn get_setup_state(
        &self,
        _request: GetSetupStateRequest,
    ) -> ServiceResult<GetSetupStateResponse> {
        setup_store::get_setup_state(&self.database).await
    }

    async fn probe_gateway(
        &self,
        request: ProbeGatewayRequest,
    ) -> ServiceResult<ProbeGatewayResponse> {
        setup_store::probe_gateway(&self.database, &self.secret_store, request).await
    }

    async fn set_config_secret(
        &self,
        request: SetConfigSecretRequest,
    ) -> ServiceResult<SetConfigSecretResponse> {
        let response =
            setup_store::set_config_secret(&self.database, &self.secret_store, request).await?;
        // A secret change can make the deployment fit to advertise, or unfit:
        // the chain observer and the gateway wallet both authenticate with one.
        // Same reconcile every config verb runs, for the same reason.
        advertisement::reconcile_after_config_change(self).await?;
        Ok(response)
    }

    async fn apply_setup_config(
        &self,
        request: ApplySetupConfigRequest,
    ) -> ServiceResult<ApplySetupConfigResponse> {
        let response = setup_store::apply_setup_config(
            &self.database,
            &self.secret_store,
            self.args.trust_fixtures_dir.is_some(),
            self.local_iroh_node_id().await.as_deref(),
            request,
        )
        .await?;
        advertisement::reconcile_after_config_change(self).await?;
        Ok(response)
    }

    async fn validate_setup(
        &self,
        request: ValidateSetupRequest,
    ) -> ServiceResult<ValidateSetupResponse> {
        setup_store::validate_setup(
            &self.database,
            &self.secret_store,
            self.args.trust_fixtures_dir.is_some(),
            request,
        )
        .await
    }

    async fn get_provider_config(
        &self,
        _request: GetProviderConfigRequest,
    ) -> ServiceResult<GetProviderConfigResponse> {
        setup_store::get_provider_config(&self.database).await
    }

    async fn update_provider_config(
        &self,
        request: UpdateProviderConfigRequest,
    ) -> ServiceResult<UpdateProviderConfigResponse> {
        let response = setup_store::update_provider_config(
            &self.database,
            &self.secret_store,
            self.local_iroh_node_id().await.as_deref(),
            request,
        )
        .await?;
        advertisement::reconcile_after_config_change(self).await?;
        Ok(response)
    }

    async fn install_provider_identity(
        &self,
        request: InstallProviderIdentityRequest,
    ) -> ServiceResult<InstallProviderIdentityResponse> {
        let (provider_pubkey, installed) = self
            .install_provider_signing_identity(request.nostr_secret_key.0.trim())
            .await?;
        // Installing the key can be the last thing standing between the
        // deployment and a published advertisement, so settle readiness here
        // instead of waiting for the next reconcile tick.
        advertisement::reconcile_after_config_change(self).await?;
        let readiness = advertisement::public_readiness(self).await?;
        Ok(InstallProviderIdentityResponse {
            provider_pubkey,
            installed,
            public_ready: readiness.ready,
            not_ready_reason: readiness.reason,
        })
    }

    async fn rotate_admin_token(
        &self,
        request: RotateAdminTokenRequest,
    ) -> ServiceResult<RotateAdminTokenResponse> {
        admin_token::rotate(&self.database, &self.secret_store, &request.new_token.0).await?;
        Ok(RotateAdminTokenResponse {
            bootstrap_token_accepted: false,
        })
    }

    async fn reopen_federation_client(
        &self,
        request: ReopenFederationClientRequest,
    ) -> ServiceResult<ReopenFederationClientResponse> {
        Ok(ReopenFederationClientResponse {
            closed: self
                .target_fedimint_clients
                .evict(&request.federation_id.0)
                .await,
        })
    }

    async fn attestation_install(
        &self,
        request: AttestationInstallRequest,
    ) -> ServiceResult<AttestationInstallResponse> {
        // Installing trust policy needs no provider identity: the only
        // installable document describes an issuer, not this provider, so an
        // operator can configure who to trust before minting a key.
        let response = attestation_store::install(&self.database, request).await?;
        advertisement::reconcile_after_config_change(self).await?;
        Ok(response)
    }

    async fn attestation_list(
        &self,
        _request: AttestationListRequest,
    ) -> ServiceResult<AttestationListResponse> {
        attestation_store::list(&self.database).await
    }

    async fn attestation_remove(
        &self,
        request: AttestationRemoveRequest,
    ) -> ServiceResult<AttestationRemoveResponse> {
        let response = attestation_store::remove(&self.database, request).await?;
        advertisement::reconcile_after_config_change(self).await?;
        Ok(response)
    }

    async fn get_holder_authorization_state(
        &self,
        _request: GetHolderAuthorizationStateRequest,
    ) -> ServiceResult<GetHolderAuthorizationStateResponse> {
        // Absent before an operator installs the provider identity. That is a
        // stage of the setup flow rather than a fault, so it reports an empty
        // identity instead of failing the route the console polls.
        let Some(provider_pubkey) = crate::identity::find_provider_identity(&self.database).await?
        else {
            return Ok(GetHolderAuthorizationStateResponse {
                provider_pubkey: None,
                status: HolderAuthorizationStatus::Checking,
            });
        };
        let last_read = self.holder_authorization_read.read().await.clone();
        Ok(GetHolderAuthorizationStateResponse {
            status: crate::holder_authorization::status(
                &self.database,
                &provider_pubkey,
                &last_read,
            )
            .await?,
            provider_pubkey: Some(provider_pubkey),
        })
    }

    async fn refresh_holder_authorizations(
        &self,
        _request: RefreshHolderAuthorizationsRequest,
    ) -> ServiceResult<RefreshHolderAuthorizationsResponse> {
        let outcome = crate::holder_authorization::reconcile_now(self).await?;
        let last_read = self.holder_authorization_read.read().await.clone();
        let provider_pubkey = crate::identity::load_provider_identity(&self.database).await?;

        Ok(RefreshHolderAuthorizationsResponse {
            relays_answered: u32::try_from(outcome.relays_answered).unwrap_or(u32::MAX),
            relays_failed: outcome
                .relays_failed
                .into_iter()
                .map(|(relay_url, reason)| RelayFetchFailure { relay_url, reason })
                .collect(),
            candidates_seen: u32::try_from(outcome.candidates_seen).unwrap_or(u32::MAX),
            candidates_verified: u32::try_from(outcome.candidates_verified).unwrap_or(u32::MAX),
            status: crate::holder_authorization::status(
                &self.database,
                &provider_pubkey,
                &last_read,
            )
            .await?,
        })
    }

    async fn get_advertisement_state(
        &self,
        _request: GetAdvertisementStateRequest,
    ) -> ServiceResult<GetAdvertisementStateResponse> {
        advertisement::get_state(self).await
    }

    async fn republish_advertisement(
        &self,
        request: RepublishAdvertisementRequest,
    ) -> ServiceResult<RepublishAdvertisementResponse> {
        let record = advertisement::republish(self, request.force).await?;
        Ok(RepublishAdvertisementResponse {
            publication_status: record.status,
            relay_states: record.relay_states,
        })
    }

    async fn withdraw_advertisement(
        &self,
        request: WithdrawAdvertisementRequest,
    ) -> ServiceResult<WithdrawAdvertisementResponse> {
        let record = advertisement::withdraw(self, request.reason).await?;
        Ok(WithdrawAdvertisementResponse {
            publication_status: record.status,
            relay_states: record.relay_states,
        })
    }

    async fn refresh_relays(
        &self,
        _request: RefreshRelaysRequest,
    ) -> ServiceResult<RefreshRelaysResponse> {
        Ok(RefreshRelaysResponse {
            relay_states: advertisement::refresh_relays(self).await?,
        })
    }

    async fn create_backup(
        &self,
        _request: CreateBackupRequest,
    ) -> ServiceResult<CreateBackupResponse> {
        backup::create_backup(self).await
    }

    async fn inspect_backup(
        &self,
        request: InspectBackupRequest,
    ) -> ServiceResult<InspectBackupResponse> {
        backup::inspect_backup(request)
    }

    async fn restore_backup(
        &self,
        _request: RestoreBackupRequest,
    ) -> ServiceResult<RestoreBackupResponse> {
        Err(backup::restore_requires_restore_mode())
    }

    async fn get_funds(&self, request: GetFundsRequest) -> ServiceResult<GetFundsResponse> {
        funds_admin::get_funds(self, request).await
    }

    async fn create_deposit_address(
        &self,
        request: CreateDepositAddressRequest,
    ) -> ServiceResult<CreateDepositAddressResponse> {
        funds_admin::create_deposit_address(self, request).await
    }

    async fn request_withdrawal(
        &self,
        request: RequestWithdrawalRequest,
    ) -> ServiceResult<RequestWithdrawalResponse> {
        funds_admin::request_withdrawal(self, request).await
    }

    async fn list_wallet_operations(
        &self,
        request: ListWalletOperationsRequest,
    ) -> ServiceResult<ListWalletOperationsResponse> {
        funds_admin::list_operations(self, request).await
    }

    async fn get_wallet_operation(
        &self,
        request: GetWalletOperationRequest,
    ) -> ServiceResult<GetWalletOperationResponse> {
        Ok(GetWalletOperationResponse {
            operation: wallet::get_wallet_operation(&self.database, &request.operation_id).await?,
        })
    }

    async fn list_allocations(
        &self,
        request: ListAllocationsRequest,
    ) -> ServiceResult<ListAllocationsResponse> {
        let allocations = allocation_store::list_allocations(
            &self.database,
            allocation_store::ListAllocationsStoreRequest {
                page: request.page,
                time_range: request.time_range,
            },
        )
        .await?;
        Ok(ListAllocationsResponse { allocations })
    }

    async fn get_allocation(
        &self,
        request: GetAdminAllocationRequest,
    ) -> ServiceResult<GetAdminAllocationResponse> {
        allocation_store::get_admin_allocation(&self.database, &request.federation_id).await
    }

    async fn get_verification_summary(
        &self,
        request: GetVerificationSummaryRequest,
    ) -> ServiceResult<GetVerificationSummaryResponse> {
        allocation_store::get_verification_summary(&self.database, &request.federation_id).await
    }

    async fn retry_funding_step(
        &self,
        request: RetryFundingStepRequest,
    ) -> ServiceResult<RetryFundingStepResponse> {
        manual_ops::retry_funding_step(self, request).await
    }

    async fn inspect_target_client(
        &self,
        request: InspectTargetClientRequest,
    ) -> ServiceResult<InspectTargetClientResponse> {
        let backend = stability_pool_backend(self).await?;
        target_recovery::inspect_target_client(&self.database, &backend, request).await
    }

    async fn release_federation_allocation(
        &self,
        request: ReleaseFederationAllocationRequest,
    ) -> ServiceResult<ReleaseFederationAllocationResponse> {
        // Reaches no gateway and no target client: it decides who may request a
        // federation, so it stays available when the funding path is not.
        manual_ops::release_federation_allocation(self, request).await
    }

    async fn abandon_target_client_value(
        &self,
        request: AbandonTargetClientValueRequest,
    ) -> ServiceResult<AbandonTargetClientValueResponse> {
        // No target client is reached: this records a decision about an item,
        // so it stays available when the gateway config is not.
        target_recovery::abandon_target_client_value(&self.database, request).await
    }

    async fn bind_target_deposit(
        &self,
        request: BindTargetDepositRequest,
    ) -> ServiceResult<BindTargetDepositResponse> {
        let backend = stability_pool_backend(self).await?;
        target_recovery::bind_target_deposit(&self.database, &backend, request).await
    }

    async fn resolve_manual_review(
        &self,
        request: ResolveManualReviewRequest,
    ) -> ServiceResult<ResolveManualReviewResponse> {
        manual_ops::resolve_manual_review(self, request).await
    }

    async fn complete_review_without_evidence(
        &self,
        request: CompleteReviewWithoutEvidenceRequest,
    ) -> ServiceResult<CompleteReviewWithoutEvidenceResponse> {
        // No chain observer is reached: the whole point of this verb is that it
        // records a completion FLIP could not verify, so it must stay available
        // when the observer is not.
        manual_ops::complete_review_without_evidence(&self.database, request).await
    }

    async fn cancel_allocation(
        &self,
        request: CancelAllocationRequest,
    ) -> ServiceResult<CancelAllocationResponse> {
        manual_ops::cancel_allocation(self, request).await
    }
}

/// Builds the stability-pool backend for an operator request.
///
/// The reconciliation surfaces reach a target client, so they need the same
/// chain backend the worker gives it — read per request, like every other
/// dependency lookup, so a config change lands without a restart.
async fn stability_pool_backend(
    context: &DaemonContext,
) -> ServiceResult<crate::stability_pool::FedimintStabilityPoolBackend> {
    let (setup, _wallet) = funds_admin::configured_wallet(context).await?;
    Ok(crate::stability_pool::FedimintStabilityPoolBackend::new(
        context.paths.federations_dir.clone(),
        context.target_fedimint_clients.clone(),
        &setup.chain_observer,
    ))
}

/// Resolves the serving runtime generation for one request.
///
/// A live restore replaces the generation while the Admin API stays bound, so
/// requests that arrive during the swap have no runtime to run against. They
/// get `Unavailable` rather than a connection error: the operator who triggered
/// the restore is the one polling, and "not right now" is the honest answer.
pub(crate) struct Live(pub(crate) DaemonContext);

impl axum::extract::FromRequestParts<DaemonShell> for Live {
    type Rejection = Response;

    async fn from_request_parts(
        _parts: &mut axum::http::request::Parts,
        shell: &DaemonShell,
    ) -> Result<Self, Self::Rejection> {
        shell.current().map(Live).ok_or_else(|| {
            let error = ServiceError::with_code(
                ServiceErrorCode::Unavailable,
                "daemon runtime is reloading after a restore; retry shortly",
            );
            (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response()
        })
    }
}

/// Builds the private Operator Admin API router.
pub(crate) fn app(context: DaemonShell) -> Router {
    let protected = Router::new()
        .route("/admin/health", get(admin_health))
        .route("/admin/v1/get_health", post(get_health))
        .route("/admin/v1/get_setup_state", post(get_setup_state))
        .route("/admin/v1/apply_setup_config", post(apply_setup_config))
        .route("/admin/v1/set_config_secret", post(set_config_secret))
        .route("/admin/v1/probe_gateway", post(probe_gateway))
        .route("/admin/v1/validate_setup", post(validate_setup))
        .route("/admin/v1/get_provider_config", post(get_provider_config))
        .route(
            "/admin/v1/update_provider_config",
            post(update_provider_config),
        )
        .route(
            "/admin/v1/install_provider_identity",
            post(install_provider_identity),
        )
        .route("/admin/v1/rotate_admin_token", post(rotate_admin_token))
        .route(
            "/admin/v1/reopen_federation_client",
            post(reopen_federation_client),
        )
        .route("/admin/v1/attestation_install", post(attestation_install))
        .route("/admin/v1/attestation_list", post(attestation_list))
        .route("/admin/v1/attestation_remove", post(attestation_remove))
        .route(
            "/admin/v1/get_holder_authorization_state",
            post(get_holder_authorization_state),
        )
        .route(
            "/admin/v1/refresh_holder_authorizations",
            post(refresh_holder_authorizations),
        )
        .route("/admin/v1/create_backup", post(create_backup))
        .route("/admin/v1/inspect_backup", post(inspect_backup))
        .route("/admin/v1/restore_backup", post(restore_backup))
        .route("/admin/v1/get_funds", post(get_funds))
        .route(
            "/admin/v1/create_deposit_address",
            post(create_deposit_address),
        )
        .route("/admin/v1/request_withdrawal", post(request_withdrawal))
        .route(
            "/admin/v1/list_wallet_operations",
            post(list_wallet_operations),
        )
        .route("/admin/v1/get_wallet_operation", post(get_wallet_operation))
        .route("/admin/v1/list_allocations", post(list_allocations))
        .route("/admin/v1/get_allocation", post(get_allocation))
        .route(
            "/admin/v1/get_verification_summary",
            post(get_verification_summary),
        )
        .route("/admin/v1/retry_funding_step", post(retry_funding_step))
        .route(
            "/admin/v1/resolve_manual_review",
            post(resolve_manual_review),
        )
        .route(
            "/admin/v1/complete_review_without_evidence",
            post(complete_review_without_evidence),
        )
        .route(
            "/admin/v1/inspect_target_client",
            post(inspect_target_client),
        )
        .route("/admin/v1/bind_target_deposit", post(bind_target_deposit))
        .route(
            "/admin/v1/abandon_target_client_value",
            post(abandon_target_client_value),
        )
        .route(
            "/admin/v1/release_federation_allocation",
            post(release_federation_allocation),
        )
        .route("/admin/v1/cancel_allocation", post(cancel_allocation))
        .route(
            "/admin/v1/get_advertisement_state",
            post(get_advertisement_state),
        )
        .route(
            "/admin/v1/republish_advertisement",
            post(republish_advertisement),
        )
        .route(
            "/admin/v1/withdraw_advertisement",
            post(withdraw_advertisement),
        )
        .route("/admin/v1/refresh_relays", post(refresh_relays))
        .route_layer(middleware::from_fn_with_state(
            context.clone(),
            require_auth,
        ));

    with_operator_ui(
        Router::new()
            .route("/health", get(health))
            .merge(protected)
            .with_state(context),
    )
}

/// Runtime context for restore-only Admin API mode.
#[derive(Clone)]
pub(crate) struct RestoreAdminContext {
    /// Boot-only daemon arguments.
    pub args: DaemonArgs,

    /// Derived daemon data-dir layout.
    pub paths: DaemonPaths,

    /// Cooperative shutdown signal.
    pub shutdown: tokio_util::sync::CancellationToken,
}

/// Builds the restore-only private Operator Admin API router.
pub(crate) fn restore_app(context: RestoreAdminContext) -> Router {
    let protected = Router::new()
        .route("/admin/health", get(restore_admin_health))
        .route("/admin/v1/get_health", post(restore_get_health))
        .route("/admin/v1/inspect_backup", post(restore_inspect_backup))
        .route("/admin/v1/restore_backup", post(restore_restore_backup))
        .route_layer(middleware::from_fn_with_state(
            context.clone(),
            require_restore_auth,
        ));

    with_operator_ui(
        Router::new()
            .route("/health", get(restore_health))
            // Everything else under the Admin API prefix. Static routes win
            // over the wildcard, so this catches only what restore mode does
            // not serve. It sits outside the auth layer deliberately: it
            // discloses nothing an unauthenticated caller cannot already infer
            // from the mode reported by `GET /health`.
            .route("/admin/v1/{*verb}", any(restore_route_unavailable))
            .merge(protected)
            .with_state(context),
    )
}

/// Answers an Admin API route that restore-only mode does not serve.
///
/// Restore mode routes four verbs. Without this catch-all every other
/// `/admin/v1/*` request falls through to Axum's default 404 — a status with no
/// body. The dashboard's request layer reads a body that is not a
/// `ServiceError` as a transport failure and reports the daemon as
/// unreachable, which sends an operator hunting for a network fault in the
/// middle of a recovery, while the daemon is up, reachable, and deliberately
/// serving a smaller API.
///
/// `Unavailable` rather than `NotFound`: the verb exists and returns when the
/// restore finishes. It is the mode that does not offer it, not the daemon that
/// does not have it.
async fn restore_route_unavailable(request: Request) -> Response {
    let error = ServiceError::with_code(
        ServiceErrorCode::Unavailable,
        format!(
            "the daemon is in restore-only mode and does not serve {}; \
             finish the restore, then retry",
            request.uri().path()
        ),
    );
    (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response()
}

// The API route layers above own authentication. Static dashboard routes are
// deliberately merged outside them so the shell and assets can load before an
// operator enters the bearer token.
#[cfg(feature = "embedded-operator-ui")]
fn with_operator_ui(router: Router) -> Router {
    router.merge(crate::operator_ui::router())
}

#[cfg(not(feature = "embedded-operator-ui"))]
fn with_operator_ui(router: Router) -> Router {
    router
}

/// Serves the private Operator Admin API until daemon shutdown.
///
/// This outlives any single runtime generation: it is bound once for the
/// process so that a live restore does not drop the operator's connection
/// mid-request, and so the response that triggered the restore can be delivered
/// after the generation serving it has stood down.
pub(crate) async fn serve(context: DaemonShell) -> anyhow::Result<()> {
    let listener = TcpListener::bind(context.args.admin_bind_address)
        .await
        .with_context(|| {
            format!(
                "failed to bind Admin API to {}",
                context.args.admin_bind_address
            )
        })?;
    let local_addr = listener
        .local_addr()
        .context("failed to read Admin API local address")?;

    tracing::info!(%local_addr, "private Admin API listening");

    axum::serve(listener, app(context.clone()))
        .with_graceful_shutdown(context.shutdown.clone().cancelled_owned())
        .await
        .context("private Admin API server failed")?;

    Ok(())
}

/// Serves the restore-only private Operator Admin API until daemon shutdown.
pub(crate) async fn serve_restore(context: RestoreAdminContext) -> anyhow::Result<()> {
    let listener = TcpListener::bind(context.args.admin_bind_address)
        .await
        .with_context(|| {
            format!(
                "failed to bind restore Admin API to {}",
                context.args.admin_bind_address
            )
        })?;
    let local_addr = listener
        .local_addr()
        .context("failed to read restore Admin API local address")?;

    tracing::info!(%local_addr, "restore Admin API listening");

    axum::serve(listener, restore_app(context.clone()))
        .with_graceful_shutdown(context.shutdown.clone().cancelled_owned())
        .await
        .context("restore Admin API server failed")?;

    Ok(())
}

async fn health(State(shell): State<DaemonShell>) -> Json<GetHealthResponse> {
    Json(shell_health_response(&shell).await.redacted_for_public())
}

/// Health for the process, not just the current generation.
///
/// Unauthenticated `/health` is the one endpoint that must answer while a
/// restore is between generations — it is how an operator watches a live
/// restore land — so it reports the shell's own view when there is no runtime
/// to ask.
async fn shell_health_response(shell: &DaemonShell) -> GetHealthResponse {
    let observed_at = crate::now_timestamp();
    if let Some(context) = shell.current() {
        let mut response = context.health_response().await;
        // A restore that failed to start is invisible in the generation's own
        // health: the generation reporting is the healthy rolled-back one.
        if let Some(error) = shell.last_restore_error() {
            response.overall_status = HealthStatus::Unhealthy;
            response.components.push(ComponentHealth {
                component: HealthComponent::Daemon,
                status: HealthStatus::Unhealthy,
                detail: Some(format!("last restore failed and was rolled back: {error}")),
                observed_at,
            });
        }
        return response;
    }

    let reloading = shell.is_reloading();
    GetHealthResponse {
        overall_status: HealthStatus::Warning,
        mode: if reloading {
            HealthMode::Reloading
        } else {
            HealthMode::NoRuntime
        },
        components: vec![ComponentHealth {
            component: HealthComponent::Daemon,
            status: HealthStatus::Warning,
            detail: Some(
                if reloading {
                    "restoring: the runtime is being rebuilt against restored state"
                } else {
                    "no runtime generation is installed"
                }
                .to_owned(),
            ),
            observed_at,
        }],
        observed_at,
    }
}

async fn restore_health(State(context): State<RestoreAdminContext>) -> Json<GetHealthResponse> {
    Json(restore_health_response(&context).redacted_for_public())
}

async fn admin_health(Live(context): Live) -> Response {
    service_response(context.get_health(GetHealthRequest).await)
}

async fn restore_admin_health(State(context): State<RestoreAdminContext>) -> Response {
    service_response(Ok(restore_health_response(&context)))
}

async fn get_health(Live(context): Live) -> Response {
    service_response(context.get_health(GetHealthRequest).await)
}

async fn restore_get_health(State(context): State<RestoreAdminContext>) -> Response {
    service_response(Ok(restore_health_response(&context)))
}

async fn get_setup_state(Live(context): Live) -> Response {
    service_response(context.get_setup_state(GetSetupStateRequest).await)
}

async fn apply_setup_config(
    Live(context): Live,
    Json(request): Json<ApplySetupConfigRequest>,
) -> Response {
    service_response(context.apply_setup_config(request).await)
}

async fn validate_setup(
    Live(context): Live,
    Json(request): Json<ValidateSetupRequest>,
) -> Response {
    service_response(context.validate_setup(request).await)
}

async fn get_provider_config(Live(context): Live) -> Response {
    service_response(context.get_provider_config(GetProviderConfigRequest).await)
}

async fn update_provider_config(
    Live(context): Live,
    Json(request): Json<UpdateProviderConfigRequest>,
) -> Response {
    service_response(context.update_provider_config(request).await)
}

async fn install_provider_identity(
    Live(context): Live,
    Json(request): Json<InstallProviderIdentityRequest>,
) -> Response {
    service_response(context.install_provider_identity(request).await)
}

async fn rotate_admin_token(
    Live(context): Live,
    Json(request): Json<RotateAdminTokenRequest>,
) -> Response {
    service_response(context.rotate_admin_token(request).await)
}

async fn reopen_federation_client(
    Live(context): Live,
    Json(request): Json<ReopenFederationClientRequest>,
) -> Response {
    service_response(context.reopen_federation_client(request).await)
}

async fn attestation_install(
    Live(context): Live,
    Json(request): Json<AttestationInstallRequest>,
) -> Response {
    service_response(context.attestation_install(request).await)
}

async fn attestation_list(Live(context): Live) -> Response {
    service_response(context.attestation_list(AttestationListRequest).await)
}

async fn attestation_remove(
    Live(context): Live,
    Json(request): Json<AttestationRemoveRequest>,
) -> Response {
    service_response(context.attestation_remove(request).await)
}

async fn get_holder_authorization_state(Live(context): Live) -> Response {
    service_response(
        context
            .get_holder_authorization_state(GetHolderAuthorizationStateRequest)
            .await,
    )
}

async fn refresh_holder_authorizations(Live(context): Live) -> Response {
    service_response(
        context
            .refresh_holder_authorizations(RefreshHolderAuthorizationsRequest)
            .await,
    )
}

async fn create_backup(Live(context): Live) -> Response {
    service_response(context.create_backup(CreateBackupRequest).await)
}

async fn inspect_backup(
    Live(context): Live,
    Json(request): Json<InspectBackupRequest>,
) -> Response {
    service_response(context.inspect_backup(request).await)
}

/// Restores a backup onto the running daemon.
///
/// The archive is extracted and validated while the current generation keeps
/// serving, so every rejection reaches the operator with the daemon untouched.
/// Only once it has passed is the generation stood down and the data dir
/// replaced; the response below is delivered from the shell's listener after
/// that generation is already gone.
async fn restore_backup(
    State(shell): State<DaemonShell>,
    Live(context): Live,
    Json(request): Json<RestoreBackupRequest>,
) -> Response {
    let running_provider = crate::identity::load_provider_identity(&context.database)
        .await
        .ok();
    let staged = match backup::stage_restore(
        &shell.args,
        &shell.paths,
        request,
        running_provider.as_ref(),
    )
    .await
    {
        Ok(staged) => staged,
        Err(error) => return service_response::<()>(Err(error)),
    };

    // Close the creation side only after archive extraction and validation, so
    // ordinary service continues during the expensive part. Taking the write
    // guard waits for any allocation already committing; requests still doing
    // external verification will observe the closed gate before they can write.
    let mut allocation_admission = context.allocation_admission.write().await;
    if let Err(error) = backup::ensure_preserves_live_allocations(
        &staged,
        &context.database,
        &mut allocation_admission,
    )
    .await
    {
        return service_response::<()>(Err(error));
    }
    let response = staged.response();
    if let Err(error) = shell.request_restore(staged, &context, &mut allocation_admission) {
        return service_response::<()>(Err(error));
    }
    drop(allocation_admission);
    service_response(Ok(response))
}

async fn restore_inspect_backup(Json(request): Json<InspectBackupRequest>) -> Response {
    service_response(backup::inspect_backup(request))
}

async fn restore_restore_backup(
    State(context): State<RestoreAdminContext>,
    Json(request): Json<RestoreBackupRequest>,
) -> Response {
    service_response(backup::restore_backup(&context.args, &context.paths, request).await)
}

async fn get_funds(Live(context): Live) -> Response {
    service_response(context.get_funds(GetFundsRequest).await)
}

async fn create_deposit_address(
    Live(context): Live,
    Json(request): Json<CreateDepositAddressRequest>,
) -> Response {
    service_response(context.create_deposit_address(request).await)
}

async fn request_withdrawal(
    Live(context): Live,
    Json(request): Json<RequestWithdrawalRequest>,
) -> Response {
    service_response(context.request_withdrawal(request).await)
}

async fn list_wallet_operations(
    Live(context): Live,
    Json(request): Json<ListWalletOperationsRequest>,
) -> Response {
    service_response(context.list_wallet_operations(request).await)
}

async fn probe_gateway(Live(context): Live, Json(request): Json<ProbeGatewayRequest>) -> Response {
    service_response(context.probe_gateway(request).await)
}

async fn set_config_secret(
    Live(context): Live,
    Json(request): Json<SetConfigSecretRequest>,
) -> Response {
    service_response(context.set_config_secret(request).await)
}

async fn get_wallet_operation(
    Live(context): Live,
    Json(request): Json<GetWalletOperationRequest>,
) -> Response {
    service_response(context.get_wallet_operation(request).await)
}

async fn list_allocations(
    Live(context): Live,
    Json(request): Json<ListAllocationsRequest>,
) -> Response {
    service_response(context.list_allocations(request).await)
}

async fn get_allocation(
    Live(context): Live,
    Json(request): Json<GetAdminAllocationRequest>,
) -> Response {
    service_response(context.get_allocation(request).await)
}

async fn get_verification_summary(
    Live(context): Live,
    Json(request): Json<GetVerificationSummaryRequest>,
) -> Response {
    service_response(context.get_verification_summary(request).await)
}

async fn retry_funding_step(
    Live(context): Live,
    Json(request): Json<RetryFundingStepRequest>,
) -> Response {
    service_response(context.retry_funding_step(request).await)
}

async fn inspect_target_client(
    Live(context): Live,
    Json(request): Json<InspectTargetClientRequest>,
) -> Response {
    service_response(context.inspect_target_client(request).await)
}

async fn abandon_target_client_value(
    Live(context): Live,
    Json(request): Json<AbandonTargetClientValueRequest>,
) -> Response {
    service_response(context.abandon_target_client_value(request).await)
}

async fn release_federation_allocation(
    Live(context): Live,
    Json(request): Json<ReleaseFederationAllocationRequest>,
) -> Response {
    service_response(context.release_federation_allocation(request).await)
}

async fn bind_target_deposit(
    Live(context): Live,
    Json(request): Json<BindTargetDepositRequest>,
) -> Response {
    service_response(context.bind_target_deposit(request).await)
}

async fn resolve_manual_review(
    Live(context): Live,
    Json(request): Json<ResolveManualReviewRequest>,
) -> Response {
    service_response(context.resolve_manual_review(request).await)
}

async fn complete_review_without_evidence(
    Live(context): Live,
    Json(request): Json<CompleteReviewWithoutEvidenceRequest>,
) -> Response {
    service_response(context.complete_review_without_evidence(request).await)
}

async fn cancel_allocation(
    Live(context): Live,
    Json(request): Json<CancelAllocationRequest>,
) -> Response {
    service_response(context.cancel_allocation(request).await)
}

async fn get_advertisement_state(Live(context): Live) -> Response {
    service_response(
        context
            .get_advertisement_state(GetAdvertisementStateRequest)
            .await,
    )
}

async fn republish_advertisement(
    Live(context): Live,
    Json(request): Json<RepublishAdvertisementRequest>,
) -> Response {
    service_response(context.republish_advertisement(request).await)
}

async fn withdraw_advertisement(
    Live(context): Live,
    Json(request): Json<WithdrawAdvertisementRequest>,
) -> Response {
    service_response(context.withdraw_advertisement(request).await)
}

async fn refresh_relays(Live(context): Live) -> Response {
    service_response(context.refresh_relays(RefreshRelaysRequest).await)
}

async fn require_auth(Live(context): Live, req: Request, next: Next) -> Response {
    // A rotated token wins outright: falling back to the boot bootstrap token
    // after a rotation would silently keep a credential the operator believes
    // they replaced.
    let rotated = match crate::admin_token::load(&context.database, &context.secret_store).await {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(?error, "failed to load rotated admin token");
            // A closed or exhausted pool is transient — it is what a restore's
            // teardown looks like to a request that cleared the reloading gate
            // a moment before it closed — so it must not be reported as an
            // internal fault the operator should investigate. Anything else
            // (unreadable storage, a record this daemon cannot decrypt) will
            // not fix itself, and 503 would wrongly invite a retry.
            let transient = error.downcast_ref::<sqlx::Error>().is_some_and(|error| {
                matches!(error, sqlx::Error::PoolClosed | sqlx::Error::PoolTimedOut)
            });
            // Break-glass. A persistent failure here means the secret store is
            // unreadable, which locks the operator out of the API they would
            // use to diagnose it. The fallback stays shut by default so an
            // induced storage failure cannot resurrect a retired credential;
            // opening it takes a restart, which is a claim on the deployment
            // rather than on the port.
            if !transient && context.args.allow_bootstrap_token_fallback {
                tracing::warn!(
                    "admin credentials are unreadable; accepting the bootstrap token because \
                     the break-glass fallback is enabled. Any rotated token is bypassed while \
                     this is set — restore the secret store and restart without it."
                );
                return require_auth_token(
                    context.args.bootstrap_admin_token.as_deref(),
                    req,
                    next,
                )
                .await;
            }

            let (status, code) = if transient {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    ServiceErrorCode::Unavailable,
                )
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ServiceErrorCode::Internal,
                )
            };
            let error = ServiceError::with_code(code, "failed to load admin credentials");
            return (status, Json(error)).into_response();
        }
    };
    let expected = rotated
        .as_deref()
        .or(context.args.bootstrap_admin_token.as_deref());
    require_auth_token(expected, req, next).await
}

async fn require_restore_auth(
    State(context): State<RestoreAdminContext>,
    req: Request,
    next: Next,
) -> Response {
    require_auth_token(context.args.bootstrap_admin_token.as_deref(), req, next).await
}

async fn require_auth_token(expected: Option<&str>, req: Request, next: Next) -> Response {
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    let valid = match (expected, provided) {
        (Some(expected), Some(provided)) => {
            expected.len() == provided.len()
                && constant_time_eq(expected.as_bytes(), provided.as_bytes())
        }
        _ => false,
    };

    if valid {
        next.run(req).await
    } else {
        let error = ServiceError::with_code(
            ServiceErrorCode::PermissionDenied,
            "invalid or missing bearer token",
        );
        (StatusCode::UNAUTHORIZED, Json(error)).into_response()
    }
}

fn restore_health_response(context: &RestoreAdminContext) -> GetHealthResponse {
    let observed_at = Timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    );
    GetHealthResponse {
        overall_status: HealthStatus::Healthy,
        mode: HealthMode::Restore,
        components: vec![
            fedi_decentralized_service_liquidity_manager::ComponentHealth {
                component: HealthComponent::Daemon,
                status: HealthStatus::Warning,
                detail: Some("normal daemon services are stopped".to_owned()),
                observed_at,
            },
            fedi_decentralized_service_liquidity_manager::ComponentHealth {
                component: HealthComponent::AdminApi,
                status: HealthStatus::Healthy,
                detail: Some(format!("bind={}", context.args.admin_bind_address)),
                observed_at,
            },
            fedi_decentralized_service_liquidity_manager::ComponentHealth {
                component: HealthComponent::Database,
                status: HealthStatus::Unknown,
                detail: Some("database is not opened until restore validation".to_owned()),
                observed_at,
            },
        ],
        observed_at,
    }
}

fn service_response<T>(result: ServiceResult<T>) -> Response
where
    T: Serialize,
{
    match result {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => (status_for_error(error.code()), Json(error)).into_response(),
    }
}

fn status_for_error(code: ServiceErrorCode) -> StatusCode {
    match code {
        ServiceErrorCode::InvalidArgument => StatusCode::BAD_REQUEST,
        ServiceErrorCode::PermissionDenied => StatusCode::FORBIDDEN,
        ServiceErrorCode::NotFound => StatusCode::NOT_FOUND,
        ServiceErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ServiceErrorCode::FailedPrecondition => StatusCode::PRECONDITION_FAILED,
        ServiceErrorCode::Internal | ServiceErrorCode::Unknown => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
#[path = "../tests/admin.rs"]
mod tests;
