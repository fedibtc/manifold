use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
};

use crate::{
    ApiError, AppState, AuthenticatedJson, AuthenticatedRecipient, CreateHookRequest,
    CreateHookResponse, DEFAULT_RATE_LIMIT_MAX_REQUESTS, DEFAULT_RATE_LIMIT_WINDOW_SECONDS,
    HookAdmissionLimits, HookAdmissionOutcome, HookId, HookOpenBehavior, HookPrivacy,
    HookRepository, ListHooksResponse, MAX_RATE_LIMIT_REQUESTS_PER_WINDOW,
    MAX_RATE_LIMIT_WINDOW_SECONDS, RecipientId, RevokeHookResponse,
    validation::{
        MAX_BODY_LEN, MAX_ID_LEN, MAX_KIND_LEN, MAX_LABEL_LEN, MAX_TITLE_LEN, validate_data,
        validate_no_reserved_data_keys, validate_optional_string, validate_required_string,
    },
};

/// Handles creation of a new notification hook.
pub(crate) async fn create_hook(
    State(state): State<AppState>,
    AuthenticatedJson {
        recipient_id,
        payload: request,
    }: AuthenticatedJson<CreateHookRequest>,
) -> Result<impl IntoResponse, ApiError> {
    validate_create_hook_request(&request, state.config().production_mode())?;
    enforce_hook_creation_rate_limit(&state, &recipient_id).await?;
    let guard = state
        .acquire_database_write_lock()
        .await
        .map_err(crate::app_state::database_write_admission_error)?;
    let limits = state.config().rate_limits();
    let outcome = HookRepository::new(state.database().pool().clone())
        .admit_hook(
            &recipient_id,
            &request,
            state.registration_eligibility(),
            HookAdmissionLimits {
                max_active_per_recipient: limits.max_active_hooks_per_recipient,
                max_active_global: limits.max_active_hooks_global,
                max_total_rows: limits.max_hook_rows_global,
                reclamation_batch_size: limits.admission_gc_batch_size,
            },
        )
        .await
        .map_err(internal_error)?;
    drop(guard);
    let (hook, hook_token) = match outcome {
        HookAdmissionOutcome::Created(created) => (created.record, created.token),
        HookAdmissionOutcome::TargetUnavailable => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "registration_not_found",
                "target installation registration not found",
            ));
        }
        HookAdmissionOutcome::RecipientCapacityExceeded => {
            return Err(rate_limited(
                "max_active_hooks_exceeded",
                "max active hooks exceeded",
            ));
        }
        HookAdmissionOutcome::GlobalCapacityExceeded => {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "global_hook_capacity_exceeded",
                "hook capacity is unavailable",
            ));
        }
        HookAdmissionOutcome::GlobalRowCapacityExceeded => {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "hook_row_capacity_exceeded",
                "hook storage capacity is unavailable",
            ));
        }
    };

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));

    Ok((
        headers,
        Json(CreateHookResponse::new(
            hook,
            &hook_token,
            state.config().public_base_url(),
        )),
    ))
}

/// Handles listing hooks for a recipient.
pub(crate) async fn list_hooks(
    State(state): State<AppState>,
    AuthenticatedRecipient(recipient_id): AuthenticatedRecipient,
) -> Result<Json<ListHooksResponse>, ApiError> {
    let hooks = HookRepository::new(state.database().pool().clone())
        .list_for_recipient(&recipient_id)
        .await
        .map_err(internal_error)?;

    Ok(Json(ListHooksResponse { hooks }))
}

/// Handles revocation of one hook.
pub(crate) async fn revoke_hook(
    State(state): State<AppState>,
    AuthenticatedRecipient(recipient_id): AuthenticatedRecipient,
    Path(hook_id): Path<String>,
) -> Result<Json<RevokeHookResponse>, ApiError> {
    validate_required_string("hook_id", &hook_id, MAX_ID_LEN)?;
    let guard = state
        .acquire_database_write_lock()
        .await
        .map_err(crate::app_state::database_write_admission_error)?;
    let revoked = HookRepository::new(state.database().pool().clone())
        .revoke(&HookId(hook_id), &recipient_id)
        .await
        .map_err(internal_error)?;
    drop(guard);
    if !revoked {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "hook_not_found",
            "hook not found",
        ));
    }

    Ok(Json(RevokeHookResponse::revoked()))
}

pub(crate) fn internal_error(_err: sqlx::Error) -> ApiError {
    ApiError::internal_server_error()
}

async fn enforce_hook_creation_rate_limit(
    state: &AppState,
    recipient_id: &RecipientId,
) -> Result<(), ApiError> {
    let limits = state.config().rate_limits();
    if 0 < limits.hook_creations_per_recipient
        && !state
            .rate_limiters()
            .check(
                crate::rate_limits::RateLimitFamily::HookCreationRecipient,
                format!("hook-create:{}", recipient_id.0),
                limits.hook_creations_per_recipient,
                limits.hook_creation_window_seconds,
            )
            .await
    {
        return Err(rate_limited(
            "hook_creation_rate_limited",
            "hook creation rate limited",
        ));
    }
    Ok(())
}

pub(crate) fn rate_limited(code: &'static str, message: &'static str) -> ApiError {
    ApiError::new(StatusCode::TOO_MANY_REQUESTS, code, message)
}

fn validate_create_hook_request(
    request: &CreateHookRequest,
    production_mode: bool,
) -> Result<(), ApiError> {
    validate_required_string("installation_id", &request.installation_id.0, MAX_ID_LEN)?;
    validate_optional_string("label", request.label.as_deref(), MAX_LABEL_LEN)?;
    if let Some(kind) = request.notification.kind.as_ref() {
        validate_required_string("kind", &kind.0, MAX_KIND_LEN)?;
    }
    validate_optional_string("workflow", request.open.workflow.as_deref(), 64)?;
    validate_optional_string("action", request.open.action.as_deref(), 64)?;
    validate_optional_string("deep_link", request.open.deep_link.as_deref(), 256)?;
    validate_app_open_contract(request)?;
    validate_optional_string(
        "title",
        request.notification.title.as_deref(),
        MAX_TITLE_LEN,
    )?;
    validate_optional_string("body", request.notification.body.as_deref(), MAX_BODY_LEN)?;
    validate_data(&request.data)?;
    validate_no_reserved_data_keys(&request.data)?;
    if request
        .policy
        .expires_in_seconds
        .is_some_and(|seconds| seconds <= 0 || 31_536_000 < seconds)
    {
        return Err(ApiError::bad_request(
            "expires_in_seconds_out_of_range",
            "expires_in_seconds is out of range",
        ));
    }
    if request.policy.expires_in_seconds.is_none() {
        return Err(ApiError::bad_request(
            "ttl_seconds_required",
            "hooks must expire",
        ));
    }
    if request
        .policy
        .max_uses
        .is_some_and(|max_uses| max_uses <= 0)
    {
        return Err(ApiError::bad_request(
            "max_uses_out_of_range",
            "max_uses is out of range",
        ));
    }
    let window = request
        .policy
        .rate_limit
        .as_ref()
        .and_then(|rate_limit| rate_limit.window_seconds)
        .unwrap_or(DEFAULT_RATE_LIMIT_WINDOW_SECONDS);
    let max_requests = request
        .policy
        .rate_limit
        .as_ref()
        .and_then(|rate_limit| rate_limit.max_requests)
        .unwrap_or(DEFAULT_RATE_LIMIT_MAX_REQUESTS);
    if !(1..=MAX_RATE_LIMIT_WINDOW_SECONDS).contains(&window) {
        return Err(ApiError::bad_request(
            "rate_limit_window_seconds_out_of_range",
            "rate_limit_window_seconds is out of range",
        ));
    }
    if !(1..=MAX_RATE_LIMIT_REQUESTS_PER_WINDOW).contains(&max_requests) {
        return Err(ApiError::bad_request(
            "rate_limit_max_requests_out_of_range",
            "rate_limit_max_requests is out of range",
        ));
    }
    if production_mode
        && (DEFAULT_RATE_LIMIT_MAX_REQUESTS < max_requests
            || window < DEFAULT_RATE_LIMIT_WINDOW_SECONDS)
    {
        return Err(ApiError::bad_request(
            "production_hook_rate_limit_too_permissive",
            "production mode does not permit per-hook high-rate exceptions",
        ));
    }
    Ok(())
}

fn validate_app_open_contract(request: &CreateHookRequest) -> Result<(), ApiError> {
    let open_behavior = request.open.open_behavior.unwrap_or_default();
    if open_behavior == HookOpenBehavior::OpenWorkflow
        && request.open.workflow.as_deref().is_none_or(str::is_empty)
    {
        return Err(ApiError::bad_request(
            "workflow_required",
            "workflow is required for open_workflow",
        ));
    }
    if open_behavior == HookOpenBehavior::OpenDeepLink
        && request.open.deep_link.as_deref().is_none_or(str::is_empty)
    {
        return Err(ApiError::bad_request(
            "deep_link_required",
            "deep_link is required for open_deep_link",
        ));
    }
    if let Some(deep_link) = request.open.deep_link.as_deref() {
        validate_deep_link(deep_link)?;
    }
    let _privacy: HookPrivacy = request.notification.privacy.unwrap_or_default();
    Ok(())
}

fn validate_deep_link(deep_link: &str) -> Result<(), ApiError> {
    let valid_fedi_link = deep_link
        .strip_prefix("fedi://")
        .is_some_and(|rest| !rest.trim().is_empty() && !rest.starts_with('/'));
    let valid_app_path = deep_link.starts_with('/') && !deep_link.starts_with("//");
    if !(valid_fedi_link || valid_app_path) {
        return Err(ApiError::bad_request(
            "deep_link_invalid",
            "deep_link must be an app-owned fedi:// URL or absolute app path",
        ));
    }
    Ok(())
}
