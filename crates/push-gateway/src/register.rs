use axum::{
    Extension, Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
};
use std::net::SocketAddr;

use crate::{
    ApiError, AppState, AuthenticatedEmptyBody, AuthenticatedJson, AuthenticatedRecipient,
    DeviceInstallationId, PushProviderErrorKind, PushRegistrationRepository, QueryPayload,
    RecipientId, RegisterInstallationRequest, RegisterInstallationResponse,
    RegistrationAdmissionLimits, RegistrationAdmissionOutcome, RegistrationManagementQuery,
    hook_management::internal_error,
    validation::{
        MAX_FCM_TOKEN_LEN, MAX_ID_LEN, MAX_PLATFORM_LEN, validate_optional_string,
        validate_required_string,
    },
};

pub(crate) async fn register_installation(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    AuthenticatedJson {
        recipient_id,
        payload: request,
    }: AuthenticatedJson<RegisterInstallationRequest>,
) -> Result<Json<RegisterInstallationResponse>, ApiError> {
    validate_register_installation_request(&request)?;
    let peer = peer.map(|Extension(ConnectInfo(peer))| peer);
    enforce_registration_write_rate_limit(&state, &headers, peer, &recipient_id).await?;
    state
        .push_provider()
        .validate_registration(&request.fcm_token)
        .await
        .map_err(registration_validation_error)?;

    let guard = state
        .acquire_database_write_lock()
        .await
        .map_err(crate::app_state::database_write_admission_error)?;
    let limits = state.config().rate_limits();
    let outcome = PushRegistrationRepository::new(state.database().pool().clone())
        .admit_installation(
            &recipient_id,
            &request,
            state.registration_eligibility(),
            RegistrationAdmissionLimits {
                max_active_per_recipient: limits.max_active_installations_per_recipient,
                max_active_global: limits.max_active_installations_global,
                max_total_rows: limits.max_registration_rows_global,
                reclamation_batch_size: limits.admission_gc_batch_size,
            },
        )
        .await
        .map_err(internal_error)?;
    drop(guard);
    match outcome {
        RegistrationAdmissionOutcome::Registered => {
            Ok(Json(RegisterInstallationResponse::registered()))
        }
        RegistrationAdmissionOutcome::RecipientCapacityExceeded => {
            Err(crate::hook_management::rate_limited(
                "max_active_installations_exceeded",
                "max active installations exceeded",
            ))
        }
        RegistrationAdmissionOutcome::GlobalCapacityExceeded => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "global_installation_capacity_exceeded",
            "registration capacity is unavailable",
        )),
        RegistrationAdmissionOutcome::GlobalRowCapacityExceeded => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "registration_row_capacity_exceeded",
            "registration storage capacity is unavailable",
        )),
        RegistrationAdmissionOutcome::TokenBoundToDifferentInstallation => Err(ApiError::new(
            StatusCode::CONFLICT,
            "fcm_token_bound_to_different_installation",
            "FCM token is already registered to a different installation",
        )),
    }
}

fn registration_validation_error(error: crate::PushProviderError) -> ApiError {
    match error.kind() {
        PushProviderErrorKind::InvalidToken => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_fcm_token",
            "FCM token is not valid for this application",
        ),
        PushProviderErrorKind::InvalidPayload | PushProviderErrorKind::Unavailable => {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "registration_validation_unavailable",
                "registration validation is unavailable",
            )
        }
    }
}

async fn enforce_registration_write_rate_limit(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    recipient_id: &RecipientId,
) -> Result<(), ApiError> {
    let limits = state.config().rate_limits();
    let source = crate::rate_limits::effective_source_prefix(
        peer,
        headers,
        limits.trusted_proxy_cidrs.as_ref(),
    );
    // Consume the low-cardinality source budget before allocating a rotating
    // recipient/source key.
    if 0 < limits.registration_changes_per_source_prefix
        && !state
            .rate_limiters()
            .check(
                crate::rate_limits::RateLimitFamily::RegistrationSource,
                source.clone(),
                limits.registration_changes_per_source_prefix,
                limits.registration_change_window_seconds,
            )
            .await
    {
        return Err(crate::hook_management::rate_limited(
            "registration_source_rate_limited",
            "registration source rate limited",
        ));
    }
    if 0 < limits.registration_changes_per_recipient_source
        && !state
            .rate_limiters()
            .check(
                crate::rate_limits::RateLimitFamily::RegistrationRecipientSource,
                format!("{}:{source}", recipient_id.0),
                limits.registration_changes_per_recipient_source,
                limits.registration_change_window_seconds,
            )
            .await
    {
        return Err(crate::hook_management::rate_limited(
            "registration_rate_limited",
            "registration rate limited",
        ));
    }
    Ok(())
}

/// Handles deletion/unregistration of one app installation.
pub(crate) async fn unregister_installation(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    AuthenticatedRecipient(recipient_id): AuthenticatedRecipient,
    Path(installation_id): Path<String>,
) -> Result<Json<RegisterInstallationResponse>, ApiError> {
    let installation_id = validate_installation_id(installation_id)?;
    enforce_registration_write_rate_limit(
        &state,
        &headers,
        peer.map(|Extension(ConnectInfo(peer))| peer),
        &recipient_id,
    )
    .await?;
    let guard = state
        .acquire_database_write_lock()
        .await
        .map_err(crate::app_state::database_write_admission_error)?;
    let deleted = PushRegistrationRepository::new(state.database().pool().clone())
        .delete_installation(&recipient_id, &installation_id)
        .await
        .map_err(internal_error)?;
    drop(guard);
    if !deleted {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "registration_not_found",
            "registration not found",
        ));
    }

    Ok(Json(RegisterInstallationResponse::unregistered()))
}

/// Handles disabling of one app installation without deleting its lifecycle row.
pub(crate) async fn disable_installation(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Path(installation_id): Path<String>,
    QueryPayload(query): QueryPayload<RegistrationManagementQuery>,
    AuthenticatedEmptyBody(recipient_id): AuthenticatedEmptyBody,
) -> Result<Json<RegisterInstallationResponse>, ApiError> {
    validate_optional_string("disabled_reason", query.reason.as_deref(), 120)?;
    let installation_id = validate_installation_id(installation_id)?;
    enforce_registration_write_rate_limit(
        &state,
        &headers,
        peer.map(|Extension(ConnectInfo(peer))| peer),
        &recipient_id,
    )
    .await?;
    let guard = state
        .acquire_database_write_lock()
        .await
        .map_err(crate::app_state::database_write_admission_error)?;
    let disabled = PushRegistrationRepository::new(state.database().pool().clone())
        .disable_installation(&recipient_id, &installation_id, query.reason.as_deref())
        .await
        .map_err(internal_error)?;
    drop(guard);
    if !disabled {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "registration_not_found",
            "registration not found",
        ));
    }

    Ok(Json(RegisterInstallationResponse::disabled()))
}

fn validate_register_installation_request(
    request: &RegisterInstallationRequest,
) -> Result<(), ApiError> {
    validate_required_string("installation_id", &request.installation_id.0, MAX_ID_LEN)?;
    validate_required_string("fcm_token", &request.fcm_token.0, MAX_FCM_TOKEN_LEN)?;
    validate_optional_string(
        "platform",
        request
            .platform
            .as_ref()
            .map(|platform| platform.0.as_str()),
        MAX_PLATFORM_LEN,
    )?;
    Ok(())
}

fn validate_installation_id(installation_id: String) -> Result<DeviceInstallationId, ApiError> {
    validate_required_string("installation_id", &installation_id, MAX_ID_LEN)?;
    Ok(DeviceInstallationId(installation_id))
}
