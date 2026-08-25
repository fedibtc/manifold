//! Operator-protected adapters for one registered FMan telemetry endpoint.

use std::str::FromStr as _;

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, Response, StatusCode, header},
};
use fedi_decentralized_service_fleet_manager::{
    GUARDIAN_TELEMETRY_ALPN, GuardianMetricsResponse, GuardianTelemetryApi as _,
    GuardianTelemetryApiClient, ListGuardianTelemetrySeatsRequest,
    ListGuardianTelemetrySeatsResponse, MAX_GUARDIAN_METRICS_BODY_BYTES,
    ScrapeGuardianMetricsRequest, SeatId, TelemetryCapability,
};
use fedi_iroh_rpc::{
    RpcClient,
    iroh::{EndpointAddr, EndpointId},
};

use crate::{ApiError, AppState};

pub(crate) async fn list_guardian_telemetry_seats(
    State(state): State<AppState>,
    Path(fman_pubkey): Path<String>,
) -> Result<Json<ListGuardianTelemetrySeatsResponse>, ApiError> {
    let (client, capability) = telemetry_client(&state, &fman_pubkey).await?;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.list_guardian_telemetry_seats(ListGuardianTelemetrySeatsRequest { capability }),
    )
    .await
    .map_err(|_| telemetry_unavailable())?
    .map_err(|error| {
        tracing::warn!(code = ?error.code(), "guardian telemetry seat listing failed");
        telemetry_unavailable()
    })?;
    Ok(Json(response))
}

/// Pull one seat on demand. This route is mounted only on the operator router.
pub(crate) async fn scrape_guardian_telemetry(
    State(state): State<AppState>,
    Path((fman_pubkey, seat_id)): Path<(String, String)>,
) -> Result<Response<Body>, ApiError> {
    let seat_id = SeatId::new(seat_id).map_err(|_| not_found())?;
    let (client, capability) = telemetry_client(&state, &fman_pubkey).await?;
    let upstream = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.scrape_guardian_metrics(ScrapeGuardianMetricsRequest {
            seat_id,
            capability,
        }),
    )
    .await
    .map_err(|_| telemetry_unavailable())?
    .map_err(|error| {
        tracing::warn!(code = ?error.code(), "guardian telemetry scrape failed");
        telemetry_unavailable()
    })?;

    raw_http_response(upstream)
}

async fn telemetry_client(
    state: &AppState,
    fman_pubkey: &str,
) -> Result<(GuardianTelemetryApiClient, TelemetryCapability), ApiError> {
    if !valid_fman_pubkey(fman_pubkey) {
        return Err(not_found());
    }
    let runtime = state
        .telemetry_runtime()
        .ok_or_else(telemetry_unavailable)?;
    let target = runtime
        .repository()
        .target(fman_pubkey)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "telemetry target lookup failed");
            internal_error()
        })?
        .ok_or_else(not_found)?;
    let aad = format!("guardian-telemetry:{fman_pubkey}");
    let plaintext = runtime
        .cipher()
        .decrypt(
            &target.secret_nonce,
            &target.secret_ciphertext,
            aad.as_bytes(),
        )
        .map_err(|_| {
            tracing::error!("telemetry target decryption failed");
            internal_error()
        })?;
    let (iroh_endpoint_id, capability): (String, [u8; 32]) = serde_json::from_slice(&plaintext)
        .map_err(|_| {
            tracing::error!("telemetry target plaintext had an invalid shape");
            internal_error()
        })?;
    let endpoint_id = EndpointId::from_str(&iroh_endpoint_id).map_err(|_| {
        tracing::error!("stored telemetry endpoint id was invalid");
        internal_error()
    })?;
    let endpoint = runtime.collector_endpoint().await.map_err(|error| {
        tracing::warn!(error = %error, "telemetry collector Iroh endpoint unavailable");
        telemetry_unavailable()
    })?;
    let connection = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        endpoint.connect(EndpointAddr::new(endpoint_id), GUARDIAN_TELEMETRY_ALPN),
    )
    .await
    .map_err(|_| telemetry_unavailable())?
    .map_err(|error| {
        tracing::warn!(error = %error, "guardian telemetry Iroh connection failed");
        telemetry_unavailable()
    })?;
    let client = GuardianTelemetryApiClient::from_rpc_client(RpcClient::with_limits(
        connection,
        4 * 1024,
        MAX_GUARDIAN_METRICS_BODY_BYTES + 64 * 1024,
    ));
    Ok((client, TelemetryCapability::from_bytes(capability)))
}

fn valid_fman_pubkey(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn raw_http_response(upstream: GuardianMetricsResponse) -> Result<Response<Body>, ApiError> {
    let status = StatusCode::from_u16(upstream.status_code).map_err(|_| internal_error())?;
    let mut response = Response::builder().status(status);
    if let Some(content_type) = upstream.content_type {
        response = response.header(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&content_type).map_err(|_| internal_error())?,
        );
    }
    if let Some(content_encoding) = upstream.content_encoding {
        response = response.header(
            header::CONTENT_ENCODING,
            HeaderValue::from_str(&content_encoding).map_err(|_| internal_error())?,
        );
    }
    response
        .body(Body::from(upstream.body))
        .map_err(|_| internal_error())
}

fn not_found() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "telemetry_target_not_found",
        "target not found",
    )
}

fn telemetry_unavailable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "telemetry_unavailable",
        "telemetry is temporarily unavailable",
    )
}

fn internal_error() -> ApiError {
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        "internal server error",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn raw_adapter_preserves_status_content_metadata_and_body() {
        let body = vec![0, b'a', 0xff, b'\n'];
        let response = raw_http_response(GuardianMetricsResponse {
            status_code: 429,
            content_type: Some("application/openmetrics-text; version=1.0.0".to_owned()),
            content_encoding: Some("identity".to_owned()),
            body: body.clone(),
        })
        .unwrap();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/openmetrics-text; version=1.0.0"
        );
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "identity");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), body.len())
                .await
                .unwrap(),
            body
        );
    }

    #[test]
    fn fman_identity_is_canonical_and_bounded() {
        assert!(valid_fman_pubkey(&"ab".repeat(32)));
        assert!(!valid_fman_pubkey(&"AB".repeat(32)));
        assert!(!valid_fman_pubkey(&"ab".repeat(31)));
    }
}
