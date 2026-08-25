use std::sync::Arc;

use fedi_credential_sdk_protocol::HolderContext;
use fedi_decentralized_service_liquidity_manager::{
    AcceptedAttesterPolicy, AdvertisementConfig, AttestationSummary, BitcoinNetwork,
    CapacityConfig, CapacityMode, ChainObserverBackendView, ChainObserverConfigView, DurationSecs,
    FundingPolicyConfig, GatewayConfigView, GatewayId, GatewayName, GetAdvertisementStateResponse,
    ProviderPolicy, Pubkey, ReplenishmentConfig, RepublishAdvertisementResponse,
    RpcEndpointAddress, RpcEndpointConfig, RpcEndpointId, RpcProtocolName, RpcTransport, Sats,
    SetupConfigView, SetupValidationSummary, SourceType, ValidationStatus, VerificationRequirement,
    WithdrawAdvertisementResponse,
};
use nostr_sdk::Keys;

use super::*;
use crate::daemon::DaemonPhase;
use crate::nostr::{RelayPublishRequest, RelayPublishResult, RelayPublisher, RelayWithdrawRequest};
use crate::test_support::credentials::{
    UNIT_TEST_ISSUER_RELAY, holder_authorization_for_provider, issue_credential_for_holder,
    test_issuer_authority, test_issuer_context,
};

#[tokio::test]
async fn get_state_withholds_advertisement_when_not_ready() -> anyhow::Result<()> {
    let context = test_context("phase7-not-ready", crate::nostr::fake_relay_publisher()).await?;

    let state = get_state(&context).await?;

    assert!(!state.ready);
    assert_eq!(
        state.publication_status,
        AdvertisementPublicationStatus::NotReady
    );
    assert!(state.advertisement.is_none());
    assert!(state.relay_states.is_empty());
    Ok(())
}

#[tokio::test]
async fn ready_republish_refresh_and_withdraw_use_fake_relay() -> anyhow::Result<()> {
    let context =
        test_context("phase7-ready-publish", crate::nostr::fake_relay_publisher()).await?;
    let provider_pubkey =
        setup_ready_provider(&context, true, vec![Url("ws://127.0.0.1:8080".to_owned())]).await?;

    let record = republish(&context, true).await?;

    assert_eq!(record.status, AdvertisementPublicationStatus::Published);
    assert_eq!(record.relay_states.len(), 1);
    assert_eq!(record.relay_states[0].status, RelayStatus::Published);
    assert!(record.relay_states[0].last_seen_at.is_some());
    let advertisement = record
        .advertisement
        .as_ref()
        .expect("ready publish stores a signed advertisement");
    assert_eq!(advertisement.payload.provider_pubkey, provider_pubkey);
    assert_eq!(
        advertisement.payload.api_endpoints,
        vec![Url(
            "iroh://iroh-node-id?alpn=fedi%2Fflip%2Fpublic-liquidity%2F1".to_owned()
        )]
    );

    let state = get_state(&context).await?;
    assert!(state.ready);
    assert!(state.advertisement.is_some());
    assert_eq!(
        state.publication_status,
        AdvertisementPublicationStatus::Published
    );

    let refreshed = refresh_relays(&context).await?;
    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0].status, RelayStatus::Published);

    let withdrawn = withdraw(&context, Some("maintenance".to_owned())).await?;
    assert_eq!(withdrawn.status, AdvertisementPublicationStatus::Withdrawn);
    assert_eq!(withdrawn.relay_states.len(), 1);
    assert_eq!(withdrawn.relay_states[0].status, RelayStatus::Disconnected);
    assert_eq!(
        withdrawn.relay_states[0].last_error.as_deref(),
        Some("maintenance")
    );
    Ok(())
}

/// The operator's route out of a published advertisement, over HTTP.
///
/// Every other test in this file calls `republish`, `get_state` and `withdraw`
/// as module functions, which proves the logic and nothing about the surface
/// the operator actually reaches. Three things live only on that surface: the
/// route points at the intended handler, `reason` survives JSON into the module
/// that writes it to the relay row, and the response carries the two fields the
/// dashboard renders. A withdrawal is the one advertisement verb an operator
/// invokes under pressure, so a silent break in that wiring is worth a test.
#[tokio::test]
async fn admin_advertisement_routes_publish_report_and_withdraw() -> anyhow::Result<()> {
    let context = test_context(
        "advertisement-admin-routes",
        crate::nostr::fake_relay_publisher(),
    )
    .await?;
    setup_ready_provider(&context, true, vec![Url("ws://127.0.0.1:8080".to_owned())]).await?;
    let app = crate::admin::app(crate::daemon::DaemonShell::with_generation(context));

    let published: RepublishAdvertisementResponse =
        admin_call(&app, "republish_advertisement", r#"{"force":true}"#).await?;
    assert_eq!(
        published.publication_status,
        AdvertisementPublicationStatus::Published
    );

    let withdrawn: WithdrawAdvertisementResponse = admin_call(
        &app,
        "withdraw_advertisement",
        r#"{"reason":"scheduled maintenance"}"#,
    )
    .await?;
    assert_eq!(
        withdrawn.publication_status,
        AdvertisementPublicationStatus::Withdrawn
    );
    // The reason is the operator's own words and the only explanation the relay
    // row carries. Asserting it here is what proves the body reached the module
    // rather than being dropped by a handler that ignores its request type.
    assert_eq!(withdrawn.relay_states.len(), 1);
    assert_eq!(
        withdrawn.relay_states[0].last_error.as_deref(),
        Some("scheduled maintenance")
    );

    let state: GetAdvertisementStateResponse =
        admin_call(&app, "get_advertisement_state", "{}").await?;
    assert_eq!(
        state.publication_status,
        AdvertisementPublicationStatus::Withdrawn
    );

    // The withdrawal verb is a privileged effect and must sit behind the same
    // bearer check as the rest of the admin surface.
    use tower::ServiceExt;
    let unauthenticated = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/admin/v1/withdraw_advertisement")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(r#"{"reason":null}"#))
                .expect("request builds"),
        )
        .await?;
    assert_eq!(
        unauthenticated.status(),
        axum::http::StatusCode::UNAUTHORIZED
    );

    Ok(())
}

/// Posts an authenticated admin verb and decodes its success body.
async fn admin_call<T: serde::de::DeserializeOwned>(
    app: &axum::Router,
    verb: &str,
    body: &str,
) -> anyhow::Result<T> {
    use tower::ServiceExt;

    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/admin/v1/{verb}"))
                .header(axum::http::header::AUTHORIZATION, "Bearer test-admin-token")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(body.to_owned()))
                .expect("request builds"),
        )
        .await?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    anyhow::ensure!(
        status == axum::http::StatusCode::OK,
        "{verb} answered {status}: {}",
        String::from_utf8_lossy(&bytes)
    );
    Ok(serde_json::from_slice(&bytes)?)
}

#[tokio::test]
async fn a_failed_withdrawal_is_recorded_rather_than_only_logged() -> anyhow::Result<()> {
    let context = test_context(
        "relay-withdraw-failure",
        crate::nostr::failing_withdraw_relay_publisher(),
    )
    .await?;
    setup_ready_provider(&context, true, vec![Url("ws://127.0.0.1:8080".to_owned())]).await?;
    assert_eq!(
        republish(&context, true).await?.status,
        AdvertisementPublicationStatus::Published
    );

    let withdrawn = withdraw(&context, Some("maintenance".to_owned())).await?;

    // The advertisement may still be on the relay, so the relay row must not
    // claim it was taken down. Recording `Disconnected` with the withdrawal
    // reason would leave the failure visible only in the log.
    assert_eq!(withdrawn.relay_states.len(), 1);
    assert_eq!(withdrawn.relay_states[0].status, RelayStatus::Failed);
    assert!(
        withdrawn.relay_states[0]
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("withdraw failed")),
        "the row must retain why the withdrawal failed, got {:?}",
        withdrawn.relay_states[0].last_error
    );
    Ok(())
}

#[tokio::test]
async fn relay_health_separates_a_storage_failure_from_no_configured_relays() -> anyhow::Result<()>
{
    let context = test_context(
        "relay-health-storage-failure",
        crate::nostr::fake_relay_publisher(),
    )
    .await?;

    // No rows at all is the genuine "nothing configured yet" case.
    assert_eq!(
        relay_health_component(&context, now_timestamp())
            .await
            .status,
        HealthStatus::Unknown
    );

    // A stored status that does not parse must make the read fail. Turning it
    // into an empty list would report the same Unknown as above, hiding the
    // fault behind a normal-looking startup state.
    sqlx::query("INSERT INTO relay_publications (relay_url, status) VALUES (?, ?)")
        .bind("wss://relay.example")
        .bind("not-a-relay-status")
        .execute(context.database.pool())
        .await?;

    let health = relay_health_component(&context, now_timestamp()).await;
    assert_eq!(health.status, HealthStatus::Unhealthy);
    assert!(
        health
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("could not be read")),
        "the detail must name the storage failure, got {:?}",
        health.detail
    );
    Ok(())
}

#[tokio::test]
async fn startup_config_reconcile_waits_for_public_endpoint_identity() -> anyhow::Result<()> {
    let context = test_context(
        "startup-config-before-public-endpoint",
        crate::nostr::fake_relay_publisher(),
    )
    .await?;
    setup_ready_provider(&context, true, vec![Url("ws://127.0.0.1:8080".to_owned())]).await?;
    assert_eq!(
        republish(&context, true).await?.status,
        AdvertisementPublicationStatus::Published
    );

    context.daemon_state.write().await.public_iroh_node_id = None;
    reconcile_after_config_change(&context).await?;

    assert_eq!(
        load_advertisement_record(&context).await?.status,
        AdvertisementPublicationStatus::Published,
        "an Admin call during startup must not race the endpoint bind and withdraw the ad"
    );
    Ok(())
}

#[tokio::test]
async fn a_stored_ready_setup_without_an_attester_policy_is_not_advertisable() -> anyhow::Result<()>
{
    let context = test_context(
        "readiness-empty-attester-policy",
        crate::nostr::fake_relay_publisher(),
    )
    .await?;
    let relays = vec![Url("ws://127.0.0.1:8080".to_owned())];
    setup_ready_provider(&context, true, relays.clone()).await?;
    assert!(public_readiness(&context).await?.ready);
    assert_eq!(
        republish(&context, true).await?.status,
        AdvertisementPublicationStatus::Published
    );

    // The upgrade and restore case: a config accepted before
    // `provider_policy_check` existed keeps its stored `ready` status, and
    // no validation pass re-examines it. Readiness must derive the
    // invariant rather than trust that status.
    let mut config = test_setup_config(true, relays);
    config.policy.accepted_attester_policies.clear();
    sqlx::query("UPDATE setup_state SET config_view_json = ? WHERE id = 1")
        .bind(serde_json::to_string(&config)?)
        .execute(context.database.pool())
        .await?;

    let readiness = public_readiness(&context).await?;
    assert!(!readiness.ready);
    assert_eq!(
        readiness.reason.as_deref(),
        Some("no accepted attester policy is configured")
    );

    // And the publication path acts on it, not just the readiness report:
    // an already published advertisement is taken down.
    reconcile_after_config_change(&context).await?;
    assert_eq!(
        load_advertisement_record(&context).await?.status,
        AdvertisementPublicationStatus::NotReady
    );
    Ok(())
}

#[tokio::test]
async fn republish_embeds_only_authorizations_naming_the_active_provider() -> anyhow::Result<()> {
    let context = test_context(
        "phase11b-trust-envelopes",
        crate::nostr::fake_relay_publisher(),
    )
    .await?;
    // The ready provider carries one enrolled envelope, which also
    // satisfies the trust-envelope readiness gate for republish.
    let provider_pubkey = setup_ready_provider_without_envelope(
        &context,
        true,
        vec![Url("ws://127.0.0.1:8080".to_owned())],
    )
    .await?;
    let enrolled =
        crate::test_support::enroll_provider_trust_envelope(&context.database, &provider_pubkey)
            .await?;

    // A relay also serves an authorization naming a different provider.
    // It reaches ingest and must not reach the advertisement.
    let issuer = test_issuer_context();
    let authority = test_issuer_authority(&issuer, UNIT_TEST_ISSUER_RELAY)?;
    let other_holder = HolderContext::generate();
    let other_credential = issue_credential_for_holder(&issuer, &authority, &other_holder)?;
    let wrong_provider = Pubkey(Keys::generate().public_key().to_hex());
    let wrong_authorization =
        holder_authorization_for_provider(&other_holder, &other_credential, &wrong_provider)?;
    let wrong_event = crate::test_support::credentials::flip_authorization_event(
        &other_holder,
        &wrong_authorization,
        &other_credential,
        &wrong_provider,
    )?;
    let relay = Url(crate::test_support::UNIT_TEST_AUTHORIZATION_RELAY.to_owned());
    let outcome = crate::holder_authorization::refresh(
        &context.database,
        &crate::test_support::StaticHolderAuthorizationFetcher::serving(vec![wrong_event]),
        &provider_pubkey,
        &[relay],
    )
    .await?;
    assert_eq!(
        outcome.candidates_verified, 0,
        "an authorization naming another provider must not enrol here"
    );

    let record = republish(&context, true).await?;
    let advertisement = record
        .advertisement
        .expect("ready publish stores a signed advertisement");
    assert_eq!(advertisement.payload.holder_authorizations.len(), 1);
    let envelope = &advertisement.payload.holder_authorizations[0];
    assert_eq!(envelope.holder_authorization, enrolled.authorization);
    assert_eq!(envelope.signed_credential, enrolled.credential);

    // A second Holder enrolling for this provider is carried alongside the
    // first: the advertisement accumulates authorizations, one per badge.
    let second =
        crate::test_support::enroll_provider_trust_envelope(&context.database, &provider_pubkey)
            .await?;
    reconcile_after_config_change(&context).await?;
    let record = load_advertisement_record(&context).await?;
    let advertisement = record
        .advertisement
        .expect("reconcile republishes a signed advertisement");
    assert_eq!(advertisement.payload.holder_authorizations.len(), 2);
    assert!(
        advertisement
            .payload
            .holder_authorizations
            .iter()
            .any(|envelope| envelope.holder_authorization == second.authorization)
    );
    assert!(
        advertisement
            .payload
            .holder_authorizations
            .iter()
            .all(|envelope| envelope.holder_authorization != wrong_authorization)
    );
    Ok(())
}

/// The Admin API must not present a published envelope as though the
/// verification path still passes it.
///
/// `republish` builds envelopes from the re-verifying store, so the relay path
/// is safe. `get_state` is the other way an envelope reaches a reader, and it
/// reads the `provider_advertisements` row straight back out with no
/// re-verification at all.
///
/// The published payload is signed over the envelopes it carries, so the
/// repair reports the discrepancy rather than editing the payload; a
/// filtered advertisement would carry a proof that no longer checks.
#[tokio::test]
async fn admin_state_reports_published_envelopes_that_no_longer_verify() -> anyhow::Result<()> {
    let context = test_context(
        "admin-state-reverifies-envelopes",
        crate::nostr::fake_relay_publisher(),
    )
    .await?;
    let provider_pubkey =
        setup_ready_provider(&context, true, vec![Url("ws://127.0.0.1:8080".to_owned())]).await?;
    crate::test_support::enroll_provider_trust_envelope(&context.database, &provider_pubkey)
        .await?;

    let record = republish(&context, true).await?;
    let published = record
        .advertisement
        .expect("ready publish stores a signed advertisement");
    let published_count = published.payload.holder_authorizations.len();
    assert!(
        published_count > 1,
        "this test needs more than one published envelope, so that dropping one \
         leaves the provider ready"
    );

    // Control: while the enrolment stands, the published envelope verifies
    // and the count is zero. Without this the assertion below would pass on
    // a deployment that never published an envelope at all.
    let state = get_state(&context).await?;
    assert_eq!(state.unverified_holder_authorization_count, 0);
    assert_eq!(
        state
            .advertisement
            .as_ref()
            .expect("a ready provider reports its advertisement")
            .payload
            .holder_authorizations
            .len(),
        published_count,
        "the control must actually have envelopes to verify"
    );

    // One enrolment stops verifying. A revocation, an expiry, and a hostile
    // write to the row all reach this same state; the read cannot tell them
    // apart and does not need to.
    //
    // Only one, deliberately. Dropping every enrolment makes the provider
    // not ready, and a not-ready provider withholds its advertisement
    // entirely — correct, but it would test the readiness gate instead of
    // this sink.
    sqlx::query(
        "DELETE FROM holder_authorization_events \
         WHERE rowid = (SELECT MAX(rowid) FROM holder_authorization_events)",
    )
    .execute(context.database.pool())
    .await?;

    let state = get_state(&context).await?;
    assert_eq!(
        state.unverified_holder_authorization_count, 1,
        "the published envelope that no longer verifies must be reported"
    );
    let still_published = state
        .advertisement
        .expect("the advertisement is still what was published");
    assert_eq!(
        still_published.payload.holder_authorizations.len(),
        published_count,
        "the signed payload must be returned intact, not filtered"
    );
    assert_eq!(
        still_published.proof, published.proof,
        "editing the payload would invalidate the proof returned with it"
    );

    Ok(())
}

#[tokio::test]
async fn production_readiness_requires_an_enrolled_authorization() -> anyhow::Result<()> {
    let context = test_context(
        "phase11b-production-envelope-readiness",
        crate::nostr::fake_relay_publisher(),
    )
    .await?;
    assert!(context.auth_provider().await.mode().signing_ready);
    let provider_pubkey = setup_ready_provider_without_envelope(
        &context,
        true,
        vec![Url("ws://127.0.0.1:8080".to_owned())],
    )
    .await?;

    let readiness = public_readiness(&context).await?;
    assert!(!readiness.ready);
    assert_eq!(
        readiness.reason.as_deref(),
        Some("no Holder authorization is enrolled for this provider")
    );

    crate::test_support::enroll_provider_trust_envelope(&context.database, &provider_pubkey)
        .await?;

    let readiness = public_readiness(&context).await?;
    assert!(readiness.ready, "reason: {:?}", readiness.reason);
    Ok(())
}

/// Installing the key must clear the signing gate in place. A daemon that boots
/// with no provider key holds a fail-closed auth provider; if installing a key
/// did not replace it, `signing_ready` could become true only in a *later*
/// process, which is the onboarding restart this design exists to remove.
#[tokio::test]
async fn installing_the_provider_identity_clears_the_signing_gate_in_place() -> anyhow::Result<()> {
    let (context, provider_secret_hex) = crate::test_support::unconfigured_identity_test_context(
        "readiness-live-identity-install",
        crate::nostr::fake_relay_publisher(),
        crate::test_support::static_verification_provider(),
    )
    .await?;

    // Get past the phase/recovery gates so the signing gate is the one
    // under test; the config itself needs no identity to persist.
    {
        let mut state = context.daemon_state.write().await;
        state.phase = DaemonPhase::Ready;
        state.recovery_complete = true;
        state.public_iroh_node_id = Some(crate::test_support::TEST_IROH_NODE_ID.to_owned());
    }
    persist_ready_setup_state(&context, true, vec![Url("ws://127.0.0.1:8080".to_owned())]).await?;

    assert!(
        !context.auth_provider().await.mode().signing_ready,
        "a daemon booted without a key must start fail-closed"
    );
    let readiness = public_readiness(&context).await?;
    assert!(!readiness.ready);
    assert_eq!(
        readiness.reason.as_deref(),
        Some("provider signing key is not installed")
    );

    let (provider_pubkey, installed) = context
        .install_provider_signing_identity(&provider_secret_hex)
        .await?;
    assert!(installed);

    // Same process, same context: the gate is gone.
    assert!(context.auth_provider().await.mode().signing_ready);
    let readiness = public_readiness(&context).await?;
    assert_ne!(
        readiness.reason.as_deref(),
        Some("provider signing key is not installed"),
        "the signing gate must clear without a restart"
    );

    // And the deployment can go all the way to ready in this same process.
    setup_ready_provider_without_envelope(
        &context,
        true,
        vec![Url("ws://127.0.0.1:8080".to_owned())],
    )
    .await?;
    crate::test_support::enroll_provider_trust_envelope(&context.database, &provider_pubkey)
        .await?;
    let readiness = public_readiness(&context).await?;
    assert!(readiness.ready, "reason: {:?}", readiness.reason);
    Ok(())
}

#[tokio::test]
async fn readiness_loss_withdraws_and_hides_advertisement() -> anyhow::Result<()> {
    let context = test_context(
        "phase7-readiness-loss",
        crate::nostr::fake_relay_publisher(),
    )
    .await?;
    setup_ready_provider(&context, true, vec![Url("ws://127.0.0.1:8080".to_owned())]).await?;
    republish(&context, true).await?;

    {
        let mut state = context.daemon_state.write().await;
        state.phase = DaemonPhase::Recovering;
    }
    reconcile_after_config_change(&context).await?;

    let state = get_state(&context).await?;
    assert!(!state.ready);
    assert_eq!(
        state.publication_status,
        AdvertisementPublicationStatus::NotReady
    );
    assert!(state.advertisement.is_none());
    assert_eq!(state.relay_states.len(), 1);
    assert_eq!(state.relay_states[0].status, RelayStatus::Disconnected);
    Ok(())
}

#[tokio::test]
async fn relay_publish_failure_is_visible_in_admin_state() -> anyhow::Result<()> {
    let context = test_context(
        "phase7-relay-failure",
        Arc::new(FailingRelayPublisher) as Arc<dyn RelayPublisher>,
    )
    .await?;
    setup_ready_provider(&context, true, vec![Url("ws://127.0.0.1:8080".to_owned())]).await?;

    let record = republish(&context, true).await?;

    assert_eq!(record.status, AdvertisementPublicationStatus::Failed);
    assert_eq!(record.relay_states.len(), 1);
    assert_eq!(record.relay_states[0].status, RelayStatus::Failed);
    assert_eq!(
        record.relay_states[0].last_error.as_deref(),
        Some("relay down")
    );
    let state = get_state(&context).await?;
    assert!(state.ready);
    assert_eq!(
        state.publication_status,
        AdvertisementPublicationStatus::Failed
    );
    Ok(())
}

/// A withdrawal survives every automatic republication trigger.
///
/// This is the whole point of the durable `withdrawn_at`. Withdrawal moves
/// local state and expires the relay events; it changes no configuration, and
/// readiness is derived from configuration. Without this gate every automatic
/// path — the reconcile tick, the config verbs, and "refresh relays" alike —
/// finds the deployment ready and puts it straight back on the market under a
/// fresh signature, so an operator reading `Withdrawn` is advertising.
#[tokio::test]
async fn a_withdrawal_survives_every_automatic_republication() -> anyhow::Result<()> {
    let recorder = Arc::new(RecordingRelayPublisher::default());
    let context = test_context(
        "withdraw-durable",
        recorder.clone() as Arc<dyn RelayPublisher>,
    )
    .await?;
    setup_ready_provider(&context, true, vec![Url("ws://127.0.0.1:8080".to_owned())]).await?;
    republish(&context, true).await?;

    withdraw(&context, Some("maintenance".to_owned())).await?;
    let withdrawn = load_advertisement_record(&context).await?;
    assert_eq!(withdrawn.status, AdvertisementPublicationStatus::Withdrawn);
    let withdrawn_at = withdrawn.withdrawn_at.expect("withdrawal is recorded");

    // Readiness is unchanged by a withdrawal: nothing about the deployment
    // became unfit, which is exactly why the publisher would otherwise undo it.
    assert!(public_readiness(&context).await?.ready);

    // The publisher tick, and the path every config verb and the
    // holder-authorization refresh go through.
    reconcile_after_config_change(&context).await?;
    // The dashboard's relay control, which republishes.
    refresh_relays(&context).await?;
    // And a non-forced republish.
    republish(&context, false).await?;

    let after = load_advertisement_record(&context).await?;
    assert_eq!(
        after.status,
        AdvertisementPublicationStatus::Withdrawn,
        "an automatic pass must not put a withdrawn provider back on the market"
    );
    assert_eq!(after.withdrawn_at, Some(withdrawn_at));
    assert_eq!(
        get_state(&context).await?.withdrawn_at,
        Some(withdrawn_at),
        "the operator's decision is readable, so the screen can name it"
    );

    // The operator's own republish is the way back, and clears the mark.
    let republished = republish(&context, true).await?;
    assert_eq!(
        republished.status,
        AdvertisementPublicationStatus::Published
    );
    assert_eq!(republished.withdrawn_at, None);
    Ok(())
}

/// A relay withdrawal is only honoured when the deletion request is signed
/// by the author of the event it names, so the key this path hands the
/// publisher has to be the one the publication used. The real-relay
/// fixture drives the publisher directly and cannot see this seam.
#[tokio::test]
async fn withdraw_asks_the_relay_under_the_publishing_identity() -> anyhow::Result<()> {
    let recorder = Arc::new(RecordingRelayPublisher::default());
    let context = test_context(
        "withdraw-identity",
        recorder.clone() as Arc<dyn RelayPublisher>,
    )
    .await?;
    let provider_pubkey =
        setup_ready_provider(&context, true, vec![Url("ws://127.0.0.1:8080".to_owned())]).await?;
    republish(&context, true).await?;

    withdraw(&context, Some("maintenance".to_owned())).await?;

    let withdrawals = recorder
        .withdrawals
        .lock()
        .expect("recorder is not poisoned");
    assert_eq!(withdrawals.len(), 1, "one relay, one withdrawal");
    assert_eq!(withdrawals[0].relay_url.0, "ws://127.0.0.1:8080");
    assert_eq!(withdrawals[0].reason.as_deref(), Some("maintenance"));
    let withdrawing_pubkey = nostr_sdk::Keys::parse(&withdrawals[0].nostr_secret_key_hex)?
        .public_key()
        .to_string();
    assert_eq!(
        withdrawing_pubkey, provider_pubkey.0,
        "withdrawal must be signed by the advertised provider identity"
    );
    Ok(())
}

/// An expiry carries no trust standing, whatever the stored row holds.
///
/// `republish` builds `holder_authorizations` from the verified store, but
/// the row it writes is reloaded by `load_advertisement_record` with no
/// verification and `sign_advertisement` checks only `provider_pubkey`. So
/// while the expiry copied `..published.payload`, anything that got bytes
/// into that column — a restore of a hostile backup, a direct write — had
/// them re-signed under the live provider key and pushed to every relay
/// with no operator action, through
/// `run_publisher_task` -> `reconcile_after_config_change` -> `withdraw`.
///
/// The envelope here is genuine, which is the point: the expiry must drop
/// it because expiries carry no standing at all, not because this one
/// failed a check that the path does not perform.
#[tokio::test]
async fn an_expiry_does_not_republish_envelopes_from_the_stored_row() -> anyhow::Result<()> {
    let recorder = Arc::new(RecordingRelayPublisher::default());
    let context = test_context(
        "withdraw-drops-envelopes",
        recorder.clone() as Arc<dyn RelayPublisher>,
    )
    .await?;
    let provider_pubkey = setup_ready_provider_without_envelope(
        &context,
        true,
        vec![Url("ws://127.0.0.1:8080".to_owned())],
    )
    .await?;
    crate::test_support::enroll_provider_trust_envelope(&context.database, &provider_pubkey)
        .await?;

    let record = republish(&context, true).await?;
    assert_eq!(
        record
            .advertisement
            .expect("ready publish stores a signed advertisement")
            .payload
            .holder_authorizations
            .len(),
        1,
        "the live advertisement carries the enrolled envelope"
    );

    withdraw(&context, Some("maintenance".to_owned())).await?;

    let withdrawals = recorder
        .withdrawals
        .lock()
        .expect("recorder is not poisoned");
    assert_eq!(withdrawals.len(), 1, "one relay, one withdrawal");
    let expired: serde_json::Value = serde_json::from_str(&withdrawals[0].expired_content)?;
    assert_eq!(
        expired["payload"]["holder_authorizations"],
        serde_json::json!([]),
        "an expiry must not carry envelopes copied out of the stored row"
    );
    Ok(())
}

/// The expiry carries nothing from the stored row, and a tampered stored
/// timestamp cannot strand the withdrawal.
///
/// The envelope test above pins one field. This pins the class: spreading
/// `..published.payload` would let every field reach every relay re-signed
/// under the live provider key while signing checks only `provider_pubkey`.
/// Two of them are live hazards — `display` would bypass the validation
/// `republish` enforces, and a far-future `issued_at` would push `created_at`
/// past what relays accept, so the withdrawal never lands.
#[tokio::test]
async fn an_expiry_carries_nothing_from_the_stored_row() -> anyhow::Result<()> {
    let recorder = Arc::new(RecordingRelayPublisher::default());
    let context = test_context(
        "withdraw-drops-stored-fields",
        recorder.clone() as Arc<dyn RelayPublisher>,
    )
    .await?;
    setup_ready_provider(&context, true, vec![Url("ws://127.0.0.1:8080".to_owned())]).await?;
    republish(&context, true).await?;

    // Stand in for any writer of that column the verification path does not
    // cover: a restored hostile backup, or a direct write. `backup.rs`
    // carries no checksum or signature, so this vector is unauthenticated.
    let stored: String = sqlx::query_scalar(
        "SELECT signed_advertisement_json FROM provider_advertisements WHERE id = 1",
    )
    .fetch_one(context.database.pool())
    .await?;
    let mut document: serde_json::Value = serde_json::from_str(&stored)?;
    document["payload"]["api_endpoints"] = serde_json::json!(["ws://attacker.invalid"]);
    document["payload"]["relay_hints"] = serde_json::json!(["ws://attacker.invalid"]);
    document["payload"]["issued_at"] = serde_json::json!(4_000_000_000u64);
    sqlx::query("UPDATE provider_advertisements SET signed_advertisement_json = ? WHERE id = 1")
        .bind(serde_json::to_string(&document)?)
        .execute(context.database.pool())
        .await?;

    withdraw(&context, Some("maintenance".to_owned())).await?;

    let withdrawals = recorder
        .withdrawals
        .lock()
        .expect("recorder is not poisoned");
    assert_eq!(withdrawals.len(), 1, "one relay, one withdrawal");
    let published = &withdrawals[0].expired_content;
    assert!(
        !published.contains("attacker.invalid"),
        "no stored field reaches the published expiry: {published}"
    );

    let expired: serde_json::Value = serde_json::from_str(published)?;
    for field in ["api_endpoints", "relay_hints", "holder_authorizations"] {
        assert_eq!(
            expired["payload"][field],
            serde_json::json!([]),
            "{field} must be empty on an expiry"
        );
    }
    assert_eq!(expired["payload"]["display"], serde_json::Value::Null);

    // The far-future stamp is clamped, so relays still accept the event and
    // the withdrawal actually withdraws.
    let issued = expired["payload"]["issued_at"]
        .as_u64()
        .expect("issued_at is a number");
    assert!(
        issued < 4_000_000_000,
        "a tampered stored timestamp must not reach created_at: {issued}"
    );
    assert_eq!(
        withdrawals[0].expired_created_at.0, issued,
        "the relay event is stamped with the clamped value"
    );
    Ok(())
}

#[derive(Debug, Default)]
struct RecordingRelayPublisher {
    withdrawals: std::sync::Mutex<Vec<RelayWithdrawRequest>>,
}

#[async_trait::async_trait]
impl RelayPublisher for RecordingRelayPublisher {
    async fn publish(&self, _request: RelayPublishRequest) -> Result<RelayPublishResult, String> {
        Ok(RelayPublishResult {
            event_id: "recorded-event".to_owned(),
        })
    }

    async fn withdraw(&self, request: RelayWithdrawRequest) -> Result<(), String> {
        self.withdrawals
            .lock()
            .expect("recorder is not poisoned")
            .push(request);
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct FailingRelayPublisher;

#[async_trait::async_trait]
impl RelayPublisher for FailingRelayPublisher {
    async fn publish(&self, _request: RelayPublishRequest) -> Result<RelayPublishResult, String> {
        Err("relay down".to_owned())
    }

    async fn withdraw(&self, _request: RelayWithdrawRequest) -> Result<(), String> {
        Ok(())
    }
}

/// Readiness that fails between two relay publications stops the second.
///
/// Every readiness condition matters here, not just the database-backed ones.
/// The database inputs are fenced by taking the writer before the recheck; the
/// in-memory ones — daemon phase, recovery, endpoint identity, signing
/// readiness, verification inputs — need their own mechanism, and publication is
/// where they bite hardest: every relay is a separate assertion reached after a
/// separate network round trip, and the readiness that held for the first says
/// nothing about the second.
#[tokio::test]
async fn readiness_that_fails_between_relays_stops_the_next_publication() -> anyhow::Result<()> {
    let publisher = crate::nostr::ReadinessFlippingRelayPublisher::default();
    let published = publisher.published.clone();
    let state_slot = publisher.state.clone();
    let context = test_context(
        "readiness-fence-between-relays",
        Arc::new(publisher) as Arc<dyn RelayPublisher>,
    )
    .await?;
    state_slot
        .set(context.daemon_state.clone())
        .map_err(|_| anyhow::anyhow!("the state slot is filled once"))?;

    setup_ready_provider(
        &context,
        true,
        vec![
            Url("ws://127.0.0.1:8080".to_owned()),
            Url("ws://127.0.0.1:8081".to_owned()),
            Url("ws://127.0.0.1:8082".to_owned()),
        ],
    )
    .await?;

    let record = republish(&context, true).await?;

    assert_eq!(
        published.lock().expect("relay log is not poisoned").len(),
        1,
        "publication must stop at the relay after readiness failed, not run the loop out"
    );
    assert_eq!(
        record.status,
        AdvertisementPublicationStatus::NotReady,
        "the record must report the readiness that actually holds"
    );
    let readiness = public_readiness(&context).await?;
    assert_eq!(
        readiness.reason.as_deref(),
        Some("startup recovery is not complete")
    );
    Ok(())
}

async fn test_context(
    name: &str,
    relay_publisher: Arc<dyn RelayPublisher>,
) -> anyhow::Result<DaemonContext> {
    crate::test_support::production_test_context(
        name,
        relay_publisher,
        crate::test_support::static_verification_provider(),
    )
    .await
}

async fn setup_ready_provider(
    context: &DaemonContext,
    ready_advertisement_enabled: bool,
    relays: Vec<Url>,
) -> anyhow::Result<Pubkey> {
    let provider_pubkey =
        setup_ready_provider_without_envelope(context, ready_advertisement_enabled, relays).await?;
    crate::test_support::enroll_provider_trust_envelope(&context.database, &provider_pubkey)
        .await?;
    Ok(provider_pubkey)
}

async fn setup_ready_provider_without_envelope(
    context: &DaemonContext,
    ready_advertisement_enabled: bool,
    relays: Vec<Url>,
) -> anyhow::Result<Pubkey> {
    let provider_pubkey = identity::load_provider_identity(&context.database).await?;
    persist_ready_setup_state(context, ready_advertisement_enabled, relays).await?;
    Ok(provider_pubkey)
}

/// Persists a ready setup config and daemon state without requiring an
/// installed provider identity, so a test can exercise the pre-install
/// readiness path.
async fn persist_ready_setup_state(
    context: &DaemonContext,
    ready_advertisement_enabled: bool,
    relays: Vec<Url>,
) -> anyhow::Result<()> {
    let config = test_setup_config(ready_advertisement_enabled, relays);
    let config_json = serde_json::to_string(&config)?;
    let validation_json = serde_json::to_string(&SetupValidationSummary {
        status: ValidationStatus::Passed,
        checks: Vec::new(),
    })?;
    sqlx::query(
        "INSERT INTO setup_state \
         (id, status, config_view_json, latest_validation_json, revision, created_at, updated_at) \
         VALUES (1, 'ready', ?, ?, 1, unixepoch(), unixepoch()) \
         ON CONFLICT(id) DO UPDATE SET \
           status = excluded.status, \
           config_view_json = excluded.config_view_json, \
           latest_validation_json = excluded.latest_validation_json, \
           updated_at = unixepoch()",
    )
    .bind(config_json)
    .bind(validation_json)
    .execute(context.database.pool())
    .await?;
    {
        let mut state = context.daemon_state.write().await;
        state.phase = DaemonPhase::Ready;
        state.recovery_complete = true;
        state.public_iroh_node_id = Some(crate::test_support::TEST_IROH_NODE_ID.to_owned());
    }
    Ok(())
}

fn test_setup_config(ready_advertisement_enabled: bool, relays: Vec<Url>) -> SetupConfigView {
    SetupConfigView {
        network: BitcoinNetwork::Regtest,
        gateway: GatewayConfigView {
            gateway_id: GatewayId("gateway-1".to_owned()),
            gateway_name: GatewayName("Gateway One".to_owned()),
            admin_url: "http://127.0.0.1:8175".to_owned(),
            has_admin_credential: true,
            identity_metadata: Vec::new(),
        },
        chain_observer: ChainObserverConfigView {
            backend: ChainObserverBackendView::Esplora {
                url: Url("http://127.0.0.1:3002".to_owned()),
            },
        },
        relays,
        capacity: CapacityConfig {
            mode: CapacityMode::ExplicitCap,
            explicit_cap: Some(Sats(20_000)),
            supported_sources: vec![SourceType::Gateway],
        },
        funding_policy: FundingPolicyConfig::defaults_for_network(BitcoinNetwork::Regtest),
        replenishment: ReplenishmentConfig {
            warning_threshold: Sats(10_000),
            critical_threshold: Sats(5_000),
        },
        advertised_endpoint: RpcEndpointConfig {
            endpoint_id: Some(RpcEndpointId("endpoint-1".to_owned())),
            transport: RpcTransport::Iroh,
            address: RpcEndpointAddress("iroh-node-id".to_owned()),
            discovery_hints: Vec::new(),
            rpc_protocol_name: RpcProtocolName("fedi/flip/public-liquidity/1".to_owned()),
        },
        advertisement: AdvertisementConfig {
            republish_interval: DurationSecs(600),
            ready_advertisement_enabled,
        },
        provider_display: None,
        policy: ProviderPolicy {
            accepted_attester_policies: vec![AcceptedAttesterPolicy {
                attester_pubkey: Pubkey("attester-1".to_owned()),
                verification_requirement: VerificationRequirement::AllTrusted,
            }],
            supported_networks: vec![BitcoinNetwork::Regtest],
        },
        attestation_summary: AttestationSummary::default(),
    }
}
