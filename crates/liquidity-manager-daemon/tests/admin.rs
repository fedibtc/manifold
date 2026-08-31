use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use super::*;

use crate::config::DaemonArgs;
use crate::{DaemonPaths, SecretStore};

#[tokio::test]
async fn admin_routes_require_auth_but_health_does_not() -> anyhow::Result<()> {
    let context = test_context().await?;
    let app = app(DaemonShell::with_generation(context));

    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(health.status(), StatusCode::OK);

    #[cfg(feature = "embedded-operator-ui")]
    {
        let shell = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await?;
        assert_eq!(shell.status(), StatusCode::OK);
        let shell = String::from_utf8(to_bytes(shell.into_body(), usize::MAX).await?.to_vec())?;
        let asset_start = shell.find("/assets/").expect("Vite shell names an asset");
        let asset_end = shell[asset_start..]
            .find(&['\"', '\''][..])
            .map(|end| asset_start + end)
            .expect("Vite asset URL is quoted");
        let asset_path = &shell[asset_start..asset_end];
        let asset = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(asset_path)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await?;
        assert_eq!(
            asset.status(),
            StatusCode::OK,
            "embedded assets stay outside bearer authentication"
        );
    }

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_setup_state")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let wrong_token = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_setup_state")
                .header(header::AUTHORIZATION, "Bearer wrong-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(wrong_token.status(), StatusCode::UNAUTHORIZED);

    let authorized = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_setup_state")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(authorized.status(), StatusCode::OK);

    Ok(())
}

#[tokio::test]
async fn restore_routes_require_auth_and_limit_surface() -> anyhow::Result<()> {
    let context = test_restore_context()?;
    let app = restore_app(context);

    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(health.status(), StatusCode::OK);

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/inspect_backup")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    // A verb restore mode does not serve answers as a typed service
    // error, not as Axum's bodiless 404. The dashboard reads a body that is
    // not a `ServiceError` as a transport failure, so the empty 404 reached
    // the operator as "daemon unreachable" during the one operation this
    // mode exists for.
    let absent_normal_route = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_setup_state")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(
        absent_normal_route.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let body = axum::body::to_bytes(absent_normal_route.into_body(), usize::MAX).await?;
    let error: ServiceError = serde_json::from_slice(&body)?;
    assert_eq!(error.code(), ServiceErrorCode::Unavailable);
    assert!(error.to_string().contains("restore-only mode"));

    let health = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_health")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(health.status(), StatusCode::OK);

    Ok(())
}

#[tokio::test]
async fn attestation_routes_are_wired_to_real_service_behavior() -> anyhow::Result<()> {
    let context = test_context().await?;
    let app = app(DaemonShell::with_generation(context));

    let invalid_install = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/attestation_install")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"payload":[1,2,3]}"#))
                .expect("request builds"),
        )
        .await?;
    assert_eq!(invalid_install.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(invalid_install.into_body(), usize::MAX).await?;
    let error: ServiceError = serde_json::from_slice(&body)?;
    assert_eq!(error.code(), ServiceErrorCode::InvalidArgument);

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/attestation_list")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(list.status(), StatusCode::OK);
    let body = to_bytes(list.into_body(), usize::MAX).await?;
    let value: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(value["payloads"].as_array().map(Vec::len), Some(0));

    let remove_missing = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/attestation_remove")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":{"id":"attestation-1"}}"#))
                .expect("request builds"),
        )
        .await?;
    assert_eq!(remove_missing.status(), StatusCode::NOT_FOUND);

    Ok(())
}

#[tokio::test]
async fn holder_authorization_state_is_readable_before_anything_is_enrolled() -> anyhow::Result<()>
{
    let context = test_context().await?;
    let provider_pubkey = crate::identity::load_provider_identity(&context.database)
        .await?
        .0;
    let app = app(DaemonShell::with_generation(context));

    // The console needs the provider pubkey to draw the QR, and it needs it
    // before any authorization exists — that is the whole point of the
    // route. Nothing has been read yet, which is `checking` rather than a
    // claim that no Holder has authorized.
    let state = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_holder_authorization_state")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(state.status(), StatusCode::OK);
    let body = to_bytes(state.into_body(), usize::MAX).await?;
    let value: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(
        value["provider_pubkey"].as_str(),
        Some(provider_pubkey.as_str())
    );
    assert_eq!(value["status"]["state"].as_str(), Some("checking"));

    // Reconciling works with no operator relay config at all, because it
    // reads the environment-pinned relays rather than the ones this
    // provider advertises on. The test context's fetcher answers for no
    // relay, so the environment relay is reported as failed — which is what
    // proves the route targeted it.
    let refresh = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/refresh_holder_authorizations")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(refresh.status(), StatusCode::OK);
    let body = to_bytes(refresh.into_body(), usize::MAX).await?;
    let value: serde_json::Value = serde_json::from_slice(&body)?;
    // Every relay failing is `relay_error`, not a claim that no Holder has
    // authorized this provider. An operator can act on the first and
    // cannot act on the second, so they must not read the same.
    assert_eq!(value["status"]["state"].as_str(), Some("relay_error"));
    assert_eq!(value["relays_answered"], 0);
    assert!(
        !value["relays_failed"]
            .as_array()
            .expect("relays_failed is an array")
            .is_empty(),
        "the environment relay should have been attempted: {value}"
    );

    Ok(())
}

#[tokio::test]
async fn phase10_manual_operation_routes_return_not_found_dtos() -> anyhow::Result<()> {
    let context = test_context().await?;
    let app = app(DaemonShell::with_generation(context));

    let routes = [
        (
            "/admin/v1/retry_funding_step",
            r#"{"federation_id":"federation-1","item_id":null,"operation_id":null}"#,
        ),
        (
            "/admin/v1/cancel_allocation",
            r#"{"federation_id":"federation-1","reason":"operator requested"}"#,
        ),
        (
            "/admin/v1/resolve_manual_review",
            r#"{"operation_id":"operation-1","resolution":"safe_to_retry","txid":null,"reason":null}"#,
        ),
        // Reaches no chain observer, deliberately: it exists for the case
        // where the observer is the thing that is unavailable.
        (
            "/admin/v1/complete_review_without_evidence",
            r#"{"operation_id":"operation-1","txid":"abc","reason":"confirmed out of band"}"#,
        ),
        // Reaches no target client, so an unconfigured daemon can answer
        // for the allocation rather than for the gateway.
        (
            "/admin/v1/abandon_target_client_value",
            r#"{"federation_id":"federation-1","reason":"pool rejects provision"}"#,
        ),
        // Reaches nothing outside the database: it decides who may request
        // a federation, not how that federation is funded.
        (
            "/admin/v1/release_federation_allocation",
            r#"{"federation_id":"federation-1","reason":"binding wedged"}"#,
        ),
    ];

    for (uri, body) in routes {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::AUTHORIZATION, "Bearer test-admin-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("request builds"),
            )
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        let value: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(value["status"], "not_found");
    }

    // `get_wallet_operation` is not in that list: it is a read, so an
    // absent operation is a `not_found` service error rather than a
    // `not_found` DTO. It is routed here because without it the dashboard
    // cannot show what a frozen send was, and an unrouted verb reaches the
    // operator as "daemon unreachable".
    let absent_operation = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_wallet_operation")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"operation_id":"operation-1"}"#))
                .expect("request builds"),
        )
        .await?;
    assert_eq!(absent_operation.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(absent_operation.into_body(), usize::MAX).await?;
    let error: ServiceError = serde_json::from_slice(&body)?;
    assert_eq!(error.code(), ServiceErrorCode::NotFound);

    // The target-client routes are not in that list because they are not
    // database-only: reaching a target client needs a configured gateway,
    // so an unconfigured daemon owes the operator that answer rather than
    // reporting the allocation missing.
    for uri in [
        "/admin/v1/inspect_target_client",
        "/admin/v1/bind_target_deposit",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::AUTHORIZATION, "Bearer test-admin-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"federation_id":"federation-1","operation_id":"target-op-1","reason":null}"#,
                    ))
                    .expect("request builds"),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED, "{uri}");
    }

    Ok(())
}

/// The full first-run sequence over HTTP, in one process: a daemon that
/// booted with no provider key accepts an identity install, and the
/// bootstrap token it was reachable with can then be rotated away.
#[tokio::test]
async fn identity_install_and_token_rotation_land_over_http() -> anyhow::Result<()> {
    let (context, provider_secret_hex) = crate::test_support::unconfigured_identity_test_context(
        "admin-http-live-reconfiguration",
        crate::nostr::fake_relay_publisher(),
        crate::test_support::static_verification_provider(),
    )
    .await?;
    let app = app(DaemonShell::with_generation(context));

    // An unconfigured daemon has no setup config yet, so the install must
    // succeed and simply report why it is not ready — not fail.
    let installed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/install_provider_identity")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"nostr_secret_key":"{provider_secret_hex}"}}"#
                )))
                .expect("request builds"),
        )
        .await?;
    assert_eq!(installed.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(installed.into_body(), usize::MAX).await?)?;
    assert_eq!(body["installed"], serde_json::Value::Bool(true));
    assert_eq!(body["public_ready"], serde_json::Value::Bool(false));
    assert!(body["not_ready_reason"].is_string());

    // Rotation takes effect immediately: the bootstrap token stops working
    // in the same process that accepted it a moment ago.
    let rotated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/rotate_admin_token")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"new_token":"rotated-admin-token-value"}"#))
                .expect("request builds"),
        )
        .await?;
    assert_eq!(rotated.status(), StatusCode::OK);

    let with_old_token = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_setup_state")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(with_old_token.status(), StatusCode::UNAUTHORIZED);

    let with_new_token = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_setup_state")
                .header(header::AUTHORIZATION, "Bearer rotated-admin-token-value")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(with_new_token.status(), StatusCode::OK);

    Ok(())
}

/// Unreadable secret storage locks the Admin API rather than falling back
/// to the bootstrap token, so an induced storage failure cannot resurrect a
/// retired credential. That leaves the operator without the API they would
/// use to fix it, which the break-glass flag reopens — deliberately at boot,
/// so using it is a claim on the deployment.
#[tokio::test]
async fn unreadable_credentials_lock_out_unless_break_glass_is_set() -> anyhow::Result<()> {
    // A rotated token this daemon's secret store cannot decrypt: a
    // persistent failure, not the transient one a reload produces.
    let foreign_store = crate::secret_store::SecretStore::from_hex_key(
        &crate::secret_store::SecretStore::generate_hex_key(),
    )?;

    let locked = test_context().await?;
    crate::admin_token::rotate(
        &locked.database,
        &foreign_store,
        "rotated-admin-token-value",
    )
    .await?;
    let response = app(DaemonShell::with_generation(locked))
        .oneshot(setup_state_request())
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "the bootstrap token must not be accepted by default"
    );

    let mut break_glass = test_context().await?;
    break_glass.args.allow_bootstrap_token_fallback = true;
    crate::admin_token::rotate(
        &break_glass.database,
        &foreign_store,
        "rotated-admin-token-value",
    )
    .await?;
    let response = app(DaemonShell::with_generation(break_glass))
        .oneshot(setup_state_request())
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    Ok(())
}

fn setup_state_request() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/admin/v1/get_setup_state")
        .header(header::AUTHORIZATION, "Bearer test-admin-token")
        .body(Body::empty())
        .expect("request builds")
}

/// The reloading gate closes before the pool does, so a request can clear
/// the gate and still find the pool closed under it. That is transient and
/// must read as such — reporting an internal fault would make every restore
/// look like a daemon bug.
#[tokio::test]
async fn a_closed_pool_reports_unavailable_rather_than_an_internal_fault() -> anyhow::Result<()> {
    let context = test_context().await?;
    context.database.close().await;
    let app = app(DaemonShell::with_generation(context));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_setup_state")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}

/// While a restore swaps the data dir there is no runtime to serve from.
/// The Admin API stays bound through that window, so it has to say so
/// rather than answer from a runtime that is being torn down.
#[tokio::test]
async fn admin_routes_report_unavailable_while_the_runtime_reloads() -> anyhow::Result<()> {
    let context = test_context().await?;
    let shell = DaemonShell::with_generation(context.clone());
    // A real restore, not just a missing generation: the two are different
    // states of the process and the mode below is what separates them.
    let staged = crate::backup::StagedRestore::for_test(
        context.paths.data_dir.join("reloading-staged-restore"),
        vec![fedi_decentralized_service_liquidity_manager::BackupStateGroup::Database],
    );
    {
        let mut admission = context.allocation_admission.write().await;
        shell.request_restore(staged, &context, &mut admission)?;
    }
    shell.uninstall();
    let app = app(shell);

    let reloading = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_setup_state")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(reloading.status(), StatusCode::SERVICE_UNAVAILABLE);

    // Health is how an operator watches the restore land, so it must answer
    // from the shell even with no generation installed.
    let health = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(health.status(), StatusCode::OK);
    let body = axum::body::to_bytes(health.into_body(), usize::MAX).await?;
    let parsed: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(parsed["overall_status"], "warning");
    // The reloading state reaches an unauthenticated caller as the typed
    // mode, not as prose: `redacted_for_public` drops every detail, and
    // during the swap no authenticated route answers to carry it instead.
    assert_eq!(
        parsed["mode"], "reloading",
        "health should name the reloading state, got: {parsed}"
    );

    Ok(())
}

/// A process with no generation and no restore is starting, not restoring.
///
/// The Admin API binds concurrently with the first generation build, so
/// every daemon passes through this state on the way up — long enough to
/// serve requests, because building a generation opens the database and the
/// target federation clients. Reporting it as `reloading` told an operator
/// their daemon was recovering from a backup when it was starting normally.
#[tokio::test]
async fn health_reports_no_runtime_before_the_first_generation_installs() -> anyhow::Result<()> {
    let context = test_context().await?;
    let shell = DaemonShell::with_generation(context);
    shell.uninstall();
    let app = app(shell);

    let health = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(health.status(), StatusCode::OK);
    let body = axum::body::to_bytes(health.into_body(), usize::MAX).await?;
    let parsed: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(
        parsed["mode"], "no_runtime",
        "no generation and no restore is not a reload, got: {parsed}"
    );

    Ok(())
}

/// Nothing is open on a fresh daemon, so the remediation call reports that
/// rather than erroring; the next use opens a client either way.
#[tokio::test]
async fn reopening_an_unopened_federation_client_reports_nothing_closed() -> anyhow::Result<()> {
    let context = test_context().await?;
    let app = app(DaemonShell::with_generation(context));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/reopen_federation_client")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"federation_id":"federation-1"}"#))
                .expect("request builds"),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await?)?;
    assert_eq!(body["closed"], serde_json::Value::Bool(false));
    Ok(())
}

/// `GET /health` is unauthenticated by design, so nothing it returns may be
/// operator-private.
///
/// The paired authenticated read is what keeps this honest. Asserting only
/// that the public body lacks a secret passes just as well when the health
/// document stops carrying one at all, and a redaction test that cannot
/// fail is worth nothing — so each absence below is paired with the
/// presence that makes it meaningful.
#[tokio::test]
async fn unauthenticated_health_withholds_what_the_authenticated_verb_discloses()
-> anyhow::Result<()> {
    let context = test_context().await?;
    let sqlite_path = context.database.path().display().to_string();
    let app = app(DaemonShell::with_generation(context));

    let authenticated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/v1/get_health")
                .header(header::AUTHORIZATION, "Bearer test-admin-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(authenticated.status(), StatusCode::OK);
    let authenticated = String::from_utf8(
        to_bytes(authenticated.into_body(), usize::MAX)
            .await?
            .to_vec(),
    )?;

    // Negative control: these are the disclosures the projection exists to
    // withhold. If either stops appearing here, the assertions below have
    // stopped testing anything and this test says so first.
    assert!(
        authenticated.contains(&sqlite_path),
        "authenticated health should disclose the database path, got: {authenticated}"
    );
    assert!(
        authenticated.contains("auth_mode="),
        "authenticated health should disclose the auth mode, got: {authenticated}"
    );

    let public = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await?;
    assert_eq!(public.status(), StatusCode::OK);
    let body = to_bytes(public.into_body(), usize::MAX).await?;
    let parsed: serde_json::Value = serde_json::from_slice(&body)?;
    let public = String::from_utf8(body.to_vec())?;

    assert!(
        !public.contains(&sqlite_path),
        "public health leaked the database path: {public}"
    );
    assert!(
        !public.contains("auth_mode="),
        "public health leaked the auth mode: {public}"
    );

    // The blunt rule, asserted as the rule rather than as a list of the
    // fields that happen to be sensitive today.
    let components = parsed["components"]
        .as_array()
        .expect("health reports components");
    assert!(
        !components.is_empty(),
        "public health should still name its components, got: {parsed}"
    );
    for component in components {
        assert_eq!(
            component["detail"],
            serde_json::Value::Null,
            "public health carried a detail string: {component}"
        );
        assert!(
            component["component"].is_string() && component["status"].is_string(),
            "public health should keep component and status, got: {component}"
        );
    }
    assert_eq!(parsed["mode"], "normal");

    Ok(())
}

async fn test_context() -> anyhow::Result<DaemonContext> {
    crate::test_support::production_test_context(
        "admin-http",
        crate::nostr::fake_relay_publisher(),
        crate::test_support::static_verification_provider(),
    )
    .await
}

fn test_restore_context() -> anyhow::Result<RestoreAdminContext> {
    let data_dir = test_data_dir("admin-http-restore");
    let args = DaemonArgs {
        manifold_environment:
            fedi_decentralized_manifold_environment::ManifoldEnvironment::Development,
        data_dir: data_dir.clone(),
        sqlite_path: data_dir.join("flip.sqlite"),
        admin_bind_address: "127.0.0.1:0".parse()?,
        public_bind_address: "127.0.0.1:0".parse()?,
        bootstrap_admin_token: Some("test-admin-token".to_owned()),
        secret_store_key: Some(SecretStore::generate_hex_key()),
        allow_bootstrap_token_fallback: false,
        mode: crate::config::DaemonMode::Restore,
        provider_nostr_secret_key: None,
        trust_fixtures_dir: None,
        max_open_target_clients: crate::target_fedimint::DEFAULT_MAX_OPEN_TARGET_CLIENTS,
        allow_private_federation_endpoints: false,
    };
    let paths = DaemonPaths {
        data_dir,
        sqlite_path: args.sqlite_path.clone(),
        secret_store_key: args.data_dir.join("secret-store.key"),
        federations_dir: args.data_dir.join("federations"),
        lock_file: args.data_dir.join("flip.lock"),
    };

    Ok(RestoreAdminContext {
        args,
        paths,
        shutdown: CancellationToken::new(),
        restore_target: crate::backup::RestoreTarget::default(),
    })
}

fn test_data_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_nanos();
    std::env::temp_dir()
        .join("fedi-flip-tests")
        .join(format!("{name}-{}-{nanos}", std::process::id()))
}
