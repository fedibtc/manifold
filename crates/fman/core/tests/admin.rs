use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use fedi_decentralized_service_fleet_manager::{FiId, Plan, QuoteId};
use tempfile::TempDir;
use tower::ServiceExt as _;

use super::*;
use crate::directory::{DirectoryPresence, OnboardingStatus};
use crate::facts::{CompletionCallbackReason, PortBase};
use crate::fleet::FleetConfig;
use crate::guardian_fee::{Collected, CollectionFailure, CollectionFailurePhase};
use crate::seat_process::SeatProcessSpawner;
use crate::seat_process::fake::{block_forever, write_fake_fedimintd};
use crate::seat_process::{BitcoindConfig, RespawnPolicy, SeatProcessConfig};

#[test]
fn malformed_seat_id_is_an_unparsable_request() {
    let error = serde_json::from_value::<AdminRequest>(serde_json::json!({
        "SeatStatus": { "seat_id": "not-a-seat-id" }
    }))
    .err()
    .expect("malformed seat id must not deserialize");

    assert_eq!(
        AdminError::unparsable(&error).kind,
        AdminErrorKind::UnparsableRequest
    );
}

#[test]
fn guardian_fee_collection_json_preserves_complete_shape_and_structures_incomplete_progress() {
    assert_eq!(
        collect_guardian_fees_json(Collected::Complete {
            claimed: fedimint_core::Amount::from_msats(100),
            recorded_claimed: fedimint_core::Amount::from_msats(100),
            awaiting_cycle: fedimint_core::Amount::from_msats(50),
        }),
        serde_json::json!({
            "claimed_msat": "100",
            "recorded_claimed_msat": "100",
            "awaiting_cycle_msat": "50",
        })
    );

    assert_eq!(
        collect_guardian_fees_json(Collected::Incomplete {
            confirmed_claimed: fedimint_core::Amount::from_msats(100),
            recorded_claimed: fedimint_core::Amount::from_msats(100),
            observed_awaiting_cycle: None,
            failure: CollectionFailure {
                phase: CollectionFailurePhase::Unlock,
                operation_submitted: true,
            },
        }),
        serde_json::json!({
            "claimed_msat": "100",
            "recorded_claimed_msat": "100",
            "awaiting_cycle_msat": null,
            "incomplete": {
                "phase": "unlock",
                "operation_submitted": true,
                "error": "guardian-fee unlock was submitted but did not complete; refresh status before retrying",
            },
        })
    );

    assert_eq!(
        collect_guardian_fees_json(Collected::Complete {
            claimed: fedimint_core::Amount::from_msats(u64::MAX),
            recorded_claimed: fedimint_core::Amount::from_msats(u64::MAX),
            awaiting_cycle: fedimint_core::Amount::ZERO,
        })["claimed_msat"],
        "18446744073709551615"
    );
}

#[test]
fn callback_admin_projection_is_closed_and_sanitized() {
    let cases = [
        (
            CompletionCallbackStatus::NotConfigured,
            serde_json::json!({"state": "not_configured"}),
        ),
        (
            CompletionCallbackStatus::Pending {
                attempts: 2,
                next_attempt_at_ms: 123,
                last_reason: Some(CompletionCallbackReason::Network),
            },
            serde_json::json!({
                "state": "pending", "attempts": 2,
                "next_attempt_at_ms": 123, "last_reason": "network",
            }),
        ),
        (
            CompletionCallbackStatus::OperatorBlocked {
                attempts: 2,
                reason: CompletionCallbackReason::GatewayOriginMissing,
            },
            serde_json::json!({
                "state": "operator_blocked", "attempts": 2,
                "reason": "gateway_origin_missing",
            }),
        ),
        (
            CompletionCallbackStatus::Delivered {
                attempts: 3,
                at_ms: 456,
            },
            serde_json::json!({"state": "delivered", "attempts": 3, "at_ms": 456}),
        ),
        (
            CompletionCallbackStatus::Terminal {
                attempts: 4,
                at_ms: 789,
                reason: CompletionCallbackReason::Decommissioned,
            },
            serde_json::json!({
                "state": "terminal", "attempts": 4, "at_ms": 789,
                "reason": "decommissioned",
            }),
        ),
    ];
    for (status, expected) in cases {
        let value = summary_json(crate::seat::SeatSummary {
            seat_id: SeatId::from(QuoteId([7; 32])),
            fi_id: FiId(
                "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
                    .parse()
                    .unwrap(),
            ),
            plan: Plan::InfiniteBestEffort { price_msats: 1 },
            created_at_ms: 1,
            payment_claim: PaymentClaimStatus::NotPaid,
            decommissioned: false,
            completion_callback: status,
            backup: None,
        });
        assert_eq!(value["completion_callback"], expected);
        let encoded = value.to_string();
        for forbidden in [
            "callback_url",
            "hook_secret",
            "idempotency_key",
            "commitment",
        ] {
            assert!(!encoded.contains(forbidden));
        }
    }
}

#[tokio::test]
async fn admin_socket_round_trips_operator_verbs() {
    let temp = TempDir::new().unwrap();
    let fedimintd_path = write_fake_fedimintd(temp.path(), &block_forever()).await;
    // A fleet opens against an identity onboarding already chose; nothing
    // mints one on open.
    let db = crate::db::Db::open(temp.path()).await.unwrap();
    crate::onboarding::onboard_as_new(&db).await.unwrap();
    db.complete_onboarding_for_test(1).await.unwrap();

    let fleet = Arc::new(
        Fleet::open(
            db,
            FleetConfig {
                process_spawner: SeatProcessSpawner::Fake(Arc::new(
                    crate::seat_process::fake::FakeSeatProcessSpawner::default(),
                )),
                manifold_environment:
                    fedi_decentralized_manifold_environment::ManifoldEnvironment::Development,
                first_port_base: PortBase::new(30_000).unwrap(),
                setup_payments_configured: true,
                respawn: RespawnPolicy::default(),
                // Tests hold the relay down and watch the retry land; a
                // production cadence would only make them slow.
                backup_scan_interval: std::time::Duration::from_millis(10),
                push_gateway_origin: None,
                push_callback_retry_interval: std::time::Duration::from_millis(10),
                completion_callback_invoker: Arc::new(crate::push_callback::TestCallbackInvoker),
                process: SeatProcessConfig {
                    data_root: temp.path().to_owned(),
                    fedimintd: fedimintd_path,
                    bitcoin_network: bitcoin::Network::Regtest,
                    iroh_dns: "https://dns.iroh.link/pkarr".parse().unwrap(),
                    bitcoin_backend: crate::seat_process::BitcoinBackend::Bitcoind(
                        BitcoindConfig {
                            url: "http://127.0.0.1:18443".to_owned(),
                            username: "user".to_owned(),
                            password: "pass".to_owned(),
                        },
                    ),
                },
            },
            Arc::new(crate::wallet::NoWallet),
        )
        .await
        .unwrap(),
    );
    let path = socket_path(temp.path());
    // No directory runtime behind the socket: the socket reads whatever was
    // last published, so a channel that will never be written again is the
    // whole of what it depends on.
    let (presence_tx, presence) = tokio::sync::watch::channel(DirectoryPresence {
        service_nostr_pubkey: fleet.identity().derive_service_nostr_keys().public_key(),
        onboarding: OnboardingStatus::Checking,
        latest_fman_version: None,
    });
    let phase = OperatorPhase::fleet(fleet.clone(), presence);
    let server = serve(&phase, &path).unwrap();

    let ask = |request: AdminRequest| {
        let path = path.clone();
        async move { super::request(&path, &request).await.unwrap() }
    };

    // Replace and read back the offer.
    let plans = ask(AdminRequest::SetPrice {
        price_msats: Some(10_000),
    })
    .await
    .unwrap();
    assert_eq!(
        plans["plans"],
        serde_json::json!([{"InfiniteBestEffort": {"price_msats": 10_000}}]),
    );
    assert_eq!(
        ask(AdminRequest::ShowPlans).await.unwrap(),
        plans,
        "SetPrice answers the same view ShowPlans serves"
    );

    // Selling nothing is stated by setting no price, and is what a fresh
    // FMan starts at.
    assert_eq!(
        ask(AdminRequest::SetPrice { price_msats: None })
            .await
            .unwrap()["plans"],
        serde_json::json!([]),
    );

    // Payment federations and seats start empty; unknowns answer typed
    // failures.
    let federations = ask(AdminRequest::ListPaymentFederations).await.unwrap();
    assert_eq!(federations["federations"].as_array().unwrap().len(), 0);
    assert_eq!(
        ask(AdminRequest::ListSeats).await.unwrap()["seats"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let unknown = ask(AdminRequest::SeatStatus {
        seat_id: SeatId::new("0000000000000000000000000000000000000000000000000000000000000000")
            .unwrap(),
    })
    .await
    .unwrap_err();
    assert!(unknown.message.contains("unknown seat"), "{unknown:?}");

    assert_eq!(
        ask(AdminRequest::PayoutDestination).await.unwrap()["destination"],
        serde_json::Value::Null
    );
    let payout = ask(AdminRequest::SetPayoutDestination {
        destination: Some("operator@example.com".to_owned()),
    })
    .await
    .unwrap();
    assert_eq!(payout["destination"], "operator@example.com");
    let sweep = ask(AdminRequest::SweepPaymentFees {
        federation_id: FederationId("nothere".to_owned()),
        request_id: "admin-test-payment".parse().unwrap(),
    })
    .await
    .unwrap_err();
    assert!(sweep.message.contains("no wallet"), "{sweep:?}");

    // The mnemonic is served to the connected operator (its only
    // retrieval path — generation does not log it).
    let mnemonic = ask(AdminRequest::ShowMnemonic).await.unwrap();
    assert_eq!(
        mnemonic["mnemonic"].as_str().unwrap(),
        fleet.identity().phrase()
    );

    // No directory runtime is behind the socket, so onboarding still waits
    // for an authorization while exposing both identities — the service key
    // this fleet signs with, and the Nostr key a Holder authorizes.
    let onboarding = ask(AdminRequest::Onboarding).await.unwrap();
    assert_eq!(
        onboarding["fman_name"].as_str().unwrap(),
        fedi_decentralized_service_fleet_manager::FmanName::from_fman_id(
            fleet.identity().derive_service_nostr_keys().public_key(),
        )
        .as_str(),
    );
    assert_eq!(
        onboarding["service_pubkey"].as_str().unwrap(),
        fleet.identity().derive_service_pubkey().to_string()
    );
    assert_eq!(onboarding["nostr"]["state"], "checking");
    assert_eq!(
        onboarding["service_nostr_pubkey"].as_str().unwrap(),
        fleet
            .identity()
            .derive_service_nostr_keys()
            .public_key()
            .to_string()
    );
    assert_eq!(onboarding["fman_version"]["current"], "0.1.0");
    assert_eq!(
        onboarding["fman_version"]["latest"],
        serde_json::Value::Null
    );
    assert_eq!(onboarding["fman_version"]["update_required"], false);

    presence_tx.send_modify(|presence| {
        presence.latest_fman_version = Some("0.2.0".parse().unwrap());
    });
    let onboarding = ask(AdminRequest::Onboarding).await.unwrap();
    assert_eq!(onboarding["fman_version"]["latest"], "0.2.0");
    assert_eq!(onboarding["fman_version"]["update_required"], true);

    assert!(
        ask(AdminRequest::RefreshHolderAuthorizations)
            .await
            .is_err()
    );

    assert_eq!(
        ask(AdminRequest::ReenrollTelemetry).await.unwrap(),
        serde_json::json!({ "telemetry_reenrollment": "scheduled" })
    );

    // Onboarding is behind this fleet, and happens once: the verbs that
    // choose an identity are refused by a daemon that already has one. The
    // refusal is asserted by its discriminant, not by its prose — that is the
    // whole point of carrying one.
    let already = ask(AdminRequest::OnboardAsNew { if_needed: false })
        .await
        .unwrap_err();
    assert_eq!(already.kind, super::AdminErrorKind::AlreadyOnboarded);
    // ...unless the caller asked for an onboarded host rather than for
    // onboarding, which is what an orchestrator restarting a daemon wants and
    // what keeps the refusal message out of another program's control flow.
    assert_eq!(
        ask(AdminRequest::OnboardAsNew { if_needed: true })
            .await
            .unwrap()["onboarded"],
        "already"
    );
    let already = ask(AdminRequest::OnboardFromBackup {
        mnemonic: fleet.identity().phrase().to_owned(),
        acknowledge_original_host_is_gone: true,
    })
    .await
    .unwrap_err();
    assert_eq!(already.kind, super::AdminErrorKind::AlreadyOnboarded);

    // The HTTP adapter invokes the same dispatcher and marks even ordinary
    // admin responses as non-cacheable because other verbs return secrets.
    let over_socket = ask(AdminRequest::ShowPlans).await.unwrap();
    let response = crate::admin_http::router(
        &crate::admin::OperatorPhase::fleet(fleet, presence_tx.subscribe()),
        crate::admin_http::AdminHttpAuth::TrustedProxy,
    )
    .oneshot(
        Request::post("/api/admin")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&AdminRequest::ShowPlans).unwrap(),
            ))
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let over_http: Result<Value, super::AdminError> = serde_json::from_slice(&body).unwrap();
    assert_eq!(over_http.unwrap(), over_socket);

    server.abort();
}
