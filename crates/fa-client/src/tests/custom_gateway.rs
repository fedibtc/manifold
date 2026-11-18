use super::*;

use crate::{CustomGatewayAttachment, CustomGatewayPhase};

const GATEWAY_A: &str = "iroh://8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c";
const GATEWAY_B: &str = "iroh://d5611c6297fe6fdb1a6875d17f81ea5968436fdf899e70a6a30d8f9b6f37c181";

fn gateway(url: &str) -> GatewayApiUrl {
    GatewayApiUrl::try_from(url).unwrap()
}

fn attach_options() -> MaintenanceRunOptions {
    MaintenanceRunOptions::new(MaintenanceRunOptionsConfig {
        poll_interval: Duration::from_millis(1),
        run_timeout: Duration::from_millis(500),
        request_timeout: Duration::from_millis(100),
    })
    .unwrap()
}

fn provider() -> liquidity_api::Pubkey {
    liquidity_api::Pubkey(liquidity_provider_keys().public_key().to_string())
}

fn registrations(mg_state: &MgState) -> Vec<(usize, String)> {
    mg_state
        .gateway_registrations
        .lock()
        .expect("test lock")
        .clone()
}

async fn assert_liquidity_refused(
    client: &TestClient,
    formation_id: &FormationId,
    connector: &TestLiquidityConnector,
    expected: &CustomGatewayAttachment,
) {
    let error = client
        .start_liquidity_for_test(
            formation_id,
            &provider(),
            LiquidityRequestIntent::gateway(90_000, None),
            connector,
            &TestLiquidityVerifier,
        )
        .await
        .expect_err("a custom attachment refuses FLIP liquidity");
    assert!(
        matches!(&error, FaError::CustomGatewayExists { attachment } if attachment == expected),
        "unexpected error: {error}"
    );
    assert!(connector.0.requests.lock().expect("test lock").is_empty());
    assert!(
        client
            .list_liquidity_operations(None, 10)
            .await
            .unwrap()
            .operations
            .is_empty(),
        "the refusal persists no liquidity operation",
    );
}

#[tokio::test]
async fn custom_gateway_attaches_through_a_threshold_and_is_final() {
    let (client, formation_id, mg_state, connector) = formed_client_for_liquidity().await;
    assert_eq!(client.custom_gateway_attachment().await.unwrap(), None);
    *mg_state.offline_indices.lock().expect("test lock") = [0, 1].into_iter().collect();

    let attached = client
        .attach_custom_gateway(gateway(GATEWAY_A), attach_options())
        .await
        .expect("a threshold of live guardians attaches the gateway");
    let expected = CustomGatewayAttachment {
        gateway_api: gateway(GATEWAY_A),
        phase: CustomGatewayPhase::Attached,
    };
    assert_eq!(attached, expected);
    assert_eq!(
        client.custom_gateway_attachment().await.unwrap(),
        Some(expected.clone())
    );
    let after_attach = registrations(&mg_state);
    assert_eq!(
        after_attach.len(),
        usize::from(MIN_FEDERATION_SIZE) - 2,
        "every live guardian stores the gateway",
    );
    assert!(after_attach.iter().all(|(_, url)| url == GATEWAY_A));

    // Final: the same URL is a no-op and another URL is refused.
    mg_state.connect_calls.store(0, Ordering::SeqCst);
    assert_eq!(
        client
            .attach_custom_gateway(gateway(GATEWAY_A), attach_options())
            .await
            .unwrap(),
        expected
    );
    let error = client
        .attach_custom_gateway(gateway(GATEWAY_B), attach_options())
        .await
        .expect_err("a completed attachment is final");
    assert!(
        matches!(&error, FaError::CustomGatewayExists { attachment } if attachment == &expected),
        "unexpected error: {error}"
    );
    assert_eq!(mg_state.connect_calls.load(Ordering::SeqCst), 0);
    assert_eq!(registrations(&mg_state), after_attach);

    assert_liquidity_refused(&client, &formation_id, &connector, &expected).await;
}

#[tokio::test]
async fn unfinished_custom_gateway_resumes_after_reopen_and_is_not_replaced() {
    let (client, _, mg_state, _) = formed_client_for_liquidity().await;
    *mg_state.offline_indices.lock().expect("test lock") =
        (0..usize::from(MIN_FEDERATION_SIZE)).collect();
    let error = client
        .attach_custom_gateway(gateway(GATEWAY_A), attach_options())
        .await
        .expect_err("no guardian is reachable");
    assert!(
        matches!(error, FaError::MaintenanceConvergence { .. }),
        "unexpected error: {error}"
    );
    let unfinished = CustomGatewayAttachment {
        gateway_api: gateway(GATEWAY_A),
        phase: CustomGatewayPhase::Attaching,
    };
    assert_eq!(
        client.custom_gateway_attachment().await.unwrap(),
        Some(unfinished.clone())
    );

    // A new process finds the unfinished attachment in storage.
    client.shutdown().await.unwrap();
    let store = client.inner.store.reopen_for_test();
    let registry = client.inner.ports.registry.clone();
    drop(client);
    let (payments, _) = TestPayments::new();
    let client = open_client_with_store_and_registry(
        store,
        payments,
        mg_state.clone(),
        MgConfig::given_away(),
        registry,
    )
    .await;
    client
        .resume()
        .await
        .expect("reconcile through federation consensus");
    let formation_id = formed(&client.status()).formation_id.clone();
    assert_eq!(
        client.custom_gateway_attachment().await.unwrap(),
        Some(unfinished.clone())
    );

    let error = client
        .attach_custom_gateway(gateway(GATEWAY_B), attach_options())
        .await
        .expect_err("an unfinished attachment is resumed, not replaced");
    assert!(
        matches!(&error, FaError::CustomGatewayExists { attachment } if attachment == &unfinished),
        "unexpected error: {error}"
    );
    let connector =
        TestLiquidityConnector(TestLiquidityProviderState::new(client.inner.store.clone()));
    assert_liquidity_refused(&client, &formation_id, &connector, &unfinished).await;
    assert!(registrations(&mg_state).is_empty());

    mg_state.offline_indices.lock().expect("test lock").clear();
    let attached = client
        .attach_custom_gateway(gateway(GATEWAY_A), attach_options())
        .await
        .expect("the same URL resumes the attachment");
    assert_eq!(attached.phase, CustomGatewayPhase::Attached);
    assert_eq!(
        client.custom_gateway_attachment().await.unwrap(),
        Some(attached)
    );
    let registered = registrations(&mg_state);
    assert_eq!(registered.len(), usize::from(MIN_FEDERATION_SIZE));
    assert!(registered.iter().all(|(_, url)| url == GATEWAY_A));
}

#[tokio::test]
async fn live_or_completed_liquidity_refuses_a_custom_gateway() {
    let (client, formation_id, mg_state, connector) = formed_client_for_liquidity().await;
    let operation = client
        .start_liquidity_for_test(
            &formation_id,
            &provider(),
            LiquidityRequestIntent::gateway(90_000, None),
            &connector,
            &TestLiquidityVerifier,
        )
        .await
        .unwrap();
    assert_eq!(operation.phase, LiquidityOperationPhase::Accepted);

    let assert_refused = |error: FaError| {
        assert!(
            matches!(
                &error,
                FaError::LiquidityOperationExists { operation_id }
                    if operation_id == &operation.operation_id
            ),
            "unexpected error: {error}"
        );
    };
    assert_refused(
        client
            .attach_custom_gateway(gateway(GATEWAY_B), attach_options())
            .await
            .expect_err("a live liquidity operation refuses a custom gateway"),
    );
    assert!(registrations(&mg_state).is_empty());

    {
        let mut allocations = connector.0.allocations.lock().expect("test lock");
        let allocation = allocations
            .get_mut(&operation.details_payload_hash)
            .expect("provider allocation exists");
        complete_gateway_allocation(allocation, gateway(GATEWAY_A));
    }
    let completed = client
        .resume_liquidity_for_test(&operation.operation_id, &connector, &TestLiquidityVerifier)
        .await
        .unwrap();
    assert!(completed.gateway_view_verified);
    let after_flip = registrations(&mg_state);

    // Even FLIP's own gateway URL: the FLIP attachment is the source.
    for url in [GATEWAY_A, GATEWAY_B] {
        assert_refused(
            client
                .attach_custom_gateway(gateway(url), attach_options())
                .await
                .expect_err("a completed liquidity operation refuses a custom gateway"),
        );
    }
    assert_eq!(registrations(&mg_state), after_flip);
    assert_eq!(client.custom_gateway_attachment().await.unwrap(), None);
}

#[tokio::test]
async fn rejected_liquidity_leaves_the_federation_free_for_a_custom_gateway() {
    let expired_for = crate::liquidity::FA_LIQUIDITY_EXPIRY_GRACE.as_secs() + 60;
    let (client, _, mg_state, connector, operation_id) =
        expired_prepared_liquidity(expired_for).await;

    // Prepared is live: refused before the provider ends it.
    assert!(matches!(
        client
            .attach_custom_gateway(gateway(GATEWAY_A), attach_options())
            .await,
        Err(FaError::LiquidityOperationExists { .. })
    ));
    let ended = client
        .resume_liquidity_for_test(&operation_id, &connector, &TestLiquidityVerifier)
        .await
        .unwrap();
    assert_eq!(ended.phase, LiquidityOperationPhase::Rejected);

    let attached = client
        .attach_custom_gateway(gateway(GATEWAY_A), attach_options())
        .await
        .expect("a rejected request is not a gateway source");
    assert_eq!(attached.phase, CustomGatewayPhase::Attached);
    assert_eq!(
        registrations(&mg_state).len(),
        usize::from(MIN_FEDERATION_SIZE)
    );
}
