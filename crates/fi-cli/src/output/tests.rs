//! Exact-schema fixtures for the CLI's stable `--json` output contract.
//!
//! The renderer converts sealed fi-client trust types into explicit private
//! DTOs. Fixtures pin those DTOs directly, so candidate and selected-seat row
//! schemas remain exact without exposing test-only constructors across the
//! trust boundary.

use nostr_sdk::PublicKey as NostrPublicKey;

use super::*;

const ACTIVE_STATUS: &str = concat!(
    r#"{"formation":{"formation_id":"formation-1","intent":{"federation_name":"contract","#,
    r#""federation_size":7,"plan":"infinite_best_effort","#,
    r#""fedimintd_versions":{"minimum":{"major":0,"minor":6,"patch":0},"maximum_exclusive":{"major":0,"minor":6,"patch":1}},"fedimintd_dkg_version":{"major":0,"minor":6,"vendor":"fedi"}},"phase":"awaiting_payment_readiness","seats":[],"#,
    r#""freshness":"unsynced","action_required":null,"invite_code":null,"last_error":null}}"#
);
const PAYMENT_REQUIREMENTS: &str = concat!(
    r#"{"authorization_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","total_msats":21000,"seats":[{"index":2,"#,
    r#""quote_id":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"#,
    r#""payment_federation_id":"1111111111111111111111111111111111111111111111111111111111111111","#,
    r#""amount_msats":21000}]}"#
);

fn capture(operation: impl FnOnce(&mut CliOutput<'_>) -> anyhow::Result<()>) -> (String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    {
        let mut output = CliOutput::new(&mut stdout, &mut stderr);
        operation(&mut output).unwrap();
    }
    (
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

#[test]
fn active_status_json_has_exact_schema_on_stdout() {
    let endpoint_key = iroh::SecretKey::from_bytes(&[1; 32]);
    let service_key = secp256k1::SecretKey::from_byte_array(&[2; 32]).unwrap();
    let locator = fedi_decentralized_service_fleet_manager::Locator::new(
        iroh::EndpointAddr::new(endpoint_key.public()),
        service_key.x_only_public_key(secp256k1::SECP256K1).0,
    );
    let requirements: serde_json::Value = serde_json::from_str(PAYMENT_REQUIREMENTS).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(ACTIVE_STATUS).unwrap();
    value["formation"]["seats"] = serde_json::json!([{
        "index": 2,
        "locator": locator,
        "seat_id": null,
        "guardian_code": null,
        "phase": "quote_ready",
        "freshness": "unsynced"
    }]);
    value["formation"]["action_required"] =
        serde_json::json!({ "authorize_payments": requirements });
    let status: FiStatus = serde_json::from_value(value).unwrap();
    let (stdout, stderr) = capture(|output| output.snapshot(&status, OutputFormat::Json));

    assert_eq!(
        stdout,
        concat!(
            r#"{"formation":{"formation_id":"formation-1","intent":{"federation_name":"contract","federation_size":7,"plan":"infinite_best_effort","fedimintd_versions":{"minimum":{"major":0,"minor":6,"patch":0},"maximum_exclusive":{"major":0,"minor":6,"patch":1}},"fedimintd_dkg_version":{"major":0,"minor":6,"vendor":"fedi"}},"phase":"awaiting_payment_readiness","seats":[{"index":2,"locator":{"version":1,"endpoint_addr":{"id":"8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c","addrs":[]},"service_pubkey":"4d4b6cd1361032ca9bd2aeb9d900aa4d45d9ead80ac9423374c451a7254d0766"},"seat_id":null,"guardian_code":null,"phase":"quote_ready","freshness":"unsynced"}],"freshness":"unsynced","action_required":{"authorize_payments":{"authorization_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","total_msats":21000,"seats":[{"index":2,"quote_id":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"payment_federation_id":"1111111111111111111111111111111111111111111111111111111111111111","amount_msats":21000}]}},"payment_outputs_started":false,"invite_code":null,"last_error":null}}"#,
            "\n"
        )
    );
    assert_eq!(stderr, "");
}

#[test]
fn payment_readiness_json_has_exact_schema_on_stderr() {
    let requirements: PaymentRequirements = serde_json::from_str(PAYMENT_REQUIREMENTS).unwrap();
    let (stdout, stderr) =
        capture(|output| output.payment_requirements(&requirements, OutputFormat::Json));

    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        concat!(
            r#"{"authorizingPayments":{"authorization_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","seats":[{"amount_msats":21000,"index":2,"payment_federation_id":"1111111111111111111111111111111111111111111111111111111111111111","quote_id":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}],"total_msats":21000}}"#,
            "\n"
        )
    );
}

#[test]
fn over_cap_payment_authorization_required_has_exact_schema_on_stderr() {
    let mut requirements: PaymentRequirements = serde_json::from_str(PAYMENT_REQUIREMENTS).unwrap();
    requirements.max_total_msats = Some(20_000);
    let (stdout, stderr) =
        capture(|output| output.payment_authorization_required(&requirements, OutputFormat::Json));

    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        concat!(
            r#"{"paymentAuthorizationRequired":{"authorization_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","max_total_msats":20000,"seats":[{"amount_msats":21000,"index":2,"payment_federation_id":"1111111111111111111111111111111111111111111111111111111111111111","quote_id":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}],"total_msats":21000}}"#,
            "\n"
        )
    );
}

#[test]
fn discovery_json_has_exact_schema_on_stdout() {
    let author =
        NostrPublicKey::parse("aa4fc8665f5696e33db7e1a572e3b0f5b3d615837b0f362dcb1c8068b098c7b4")
            .unwrap();
    let discovery = fi_client::FmanDiscovery {
        candidates: Vec::new(),
        rejected: vec![fi_client::RejectedAdvertisement {
            author,
            reason: fi_client::AdvertisementRejection::Expired,
        }],
    };
    let (stdout, stderr) = capture(|output| output.discovery(&discovery, OutputFormat::Json));

    assert_eq!(
        stdout,
        concat!(
            r#"{"seen":1,"eligible":0,"candidates":[],"rejected":[{"author":"aa4fc8665f5696e33db7e1a572e3b0f5b3d615837b0f362dcb1c8068b098c7b4","reason":"expired"}]}"#,
            "\n"
        )
    );
    assert_eq!(stderr, "");
}

#[test]
fn populated_registry_row_dtos_have_exact_schemas() {
    let endpoint_key = iroh::SecretKey::from_bytes(&[1; 32]);
    let service_key = secp256k1::SecretKey::from_byte_array(&[2; 32]).unwrap();
    let locator = fi_client::Locator::new(
        iroh::EndpointAddr::new(endpoint_key.public()),
        service_key.x_only_public_key(secp256k1::SECP256K1).0,
    );
    let federation_sizes = [7, 10];
    let fedimintd_version = "0.11.1+fedi".parse().unwrap();
    let candidate = DiscoveryCandidateJson {
        fman_pubkey: "11".repeat(32),
        advertised_price_msats: 21_000,
        federation_sizes: &federation_sizes,
        fedimintd_version: &fedimintd_version,
        claimed_issuer: "22".repeat(32),
        api_endpoints: vec![ApiEndpointJson {
            transport: "iroh",
            url: "iroh://endpoint",
        }],
        locator: &locator,
        issued_at: 100,
        expires_at: 200,
    };
    assert_eq!(
        serde_json::to_string(&DiscoveryJson {
            seen: 1,
            eligible: 1,
            candidates: vec![candidate],
            rejected: vec![],
        })
        .unwrap(),
        r#"{"seen":1,"eligible":1,"candidates":[{"fmanPubkey":"1111111111111111111111111111111111111111111111111111111111111111","advertisedPriceMsats":21000,"federationSizes":[7,10],"fedimintdVersion":"0.11.1+fedi","claimedIssuer":"2222222222222222222222222222222222222222222222222222222222222222","apiEndpoints":[{"transport":"iroh","url":"iroh://endpoint"}],"locator":{"version":1,"endpoint_addr":{"id":"8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c","addrs":[]},"service_pubkey":"4d4b6cd1361032ca9bd2aeb9d900aa4d45d9ead80ac9423374c451a7254d0766"},"issuedAt":100,"expiresAt":200}],"rejected":[]}"#
    );

    let seat = SelectionSeatJson {
        fman_pubkey: "11".repeat(32),
        advertised_price_msats: 21_000,
        locator: &locator,
        issuer: "22".repeat(32),
        holder: "33".repeat(32),
        trust_level: 6,
        provenance: "fedi_attested",
    };
    assert_eq!(
        serde_json::to_string(&SelectionPreviewJson {
            seen: 1,
            eligible: 1,
            selected: 1,
            fedimintd_version_core: "0.11.1".to_owned(),
            total_advertised_msats: 21_000,
            seats: vec![seat],
            rejected: vec![RejectionJson {
                author: "44".repeat(32),
                reason: "deadline_expired",
            }],
        })
        .unwrap(),
        r#"{"seen":1,"eligible":1,"selected":1,"fedimintdVersionCore":"0.11.1","totalAdvertisedMsats":21000,"seats":[{"fmanPubkey":"1111111111111111111111111111111111111111111111111111111111111111","advertisedPriceMsats":21000,"locator":{"version":1,"endpoint_addr":{"id":"8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c","addrs":[]},"service_pubkey":"4d4b6cd1361032ca9bd2aeb9d900aa4d45d9ead80ac9423374c451a7254d0766"},"issuer":"2222222222222222222222222222222222222222222222222222222222222222","holder":"3333333333333333333333333333333333333333333333333333333333333333","trustLevel":6,"provenance":"fedi_attested"}],"rejected":[{"author":"4444444444444444444444444444444444444444444444444444444444444444","reason":"deadline_expired"}]}"#
    );
}

#[test]
fn discovery_human_output_lists_typed_rejections() {
    let author =
        NostrPublicKey::parse("aa4fc8665f5696e33db7e1a572e3b0f5b3d615837b0f362dcb1c8068b098c7b4")
            .unwrap();
    let discovery = fi_client::FmanDiscovery {
        candidates: Vec::new(),
        rejected: vec![fi_client::RejectedAdvertisement {
            author,
            reason: fi_client::AdvertisementRejection::Expired,
        }],
    };
    let (stdout, stderr) = capture(|output| output.discovery(&discovery, OutputFormat::Human));

    assert_eq!(
        stdout,
        concat!(
            "discovered 1 advertisement(s): 0 eligible, 1 rejected\n",
            "rejections:\n",
            "  aa4fc8665f5696e33db7e1a572e3b0f5b3d615837b0f362dcb1c8068b098c7b4: Expired\n",
        )
    );
    assert_eq!(stderr, "");
}

#[test]
fn funded_wallet_notice_is_suppressed_in_json_mode() {
    let (stdout, stderr) = capture(|output| {
        output.wallet_funded(
            fedimint_core::Amount::from_msats(21_000),
            OutputFormat::Json,
        )
    });

    assert_eq!(stdout, "");
    assert_eq!(stderr, "");
}

#[test]
fn post_formation_consensus_json_has_exact_schemas() {
    let (stdout, stderr) = capture(|output| {
        output.metadata_consensus(
            "fedi:welcome_message",
            "Welcome to staging",
            OutputFormat::Json,
        )
    });
    assert_eq!(
        stdout,
        concat!(
            r#"{"field":"fedi:welcome_message","value":"Welcome to staging","consensusReached":true}"#,
            "\n"
        )
    );
    assert_eq!(stderr, "");

    let (stdout, stderr) =
        capture(|output| output.guardian_fee_consensus(5_000, OutputFormat::Json));
    assert_eq!(
        stdout,
        concat!(r#"{"sendPpm":5000,"consensusReached":true}"#, "\n")
    );
    assert_eq!(stderr, "");
}

#[test]
fn payment_wallet_funding_json_has_exact_schemas() {
    let federation_id: fedimint_core::config::FederationId = "11".repeat(32).parse().unwrap();
    let operation_id = fedimint_core::core::OperationId([0x22; 32]);

    let (stdout, stderr) = capture(|output| {
        output.payment_wallet_joined(
            federation_id,
            fedimint_core::Amount::from_msats(21_000),
            OutputFormat::Json,
        )
    });
    assert_eq!(
        stdout,
        concat!(
            r#"{"balanceMsats":21000,"federationId":"1111111111111111111111111111111111111111111111111111111111111111"}"#,
            "\n"
        )
    );
    assert_eq!(stderr, "");

    let (stdout, stderr) = capture(|output| {
        output.payment_wallet_balance(
            federation_id,
            fedimint_core::Amount::from_msats(22_000),
            OutputFormat::Json,
        )
    });
    assert_eq!(
        stdout,
        concat!(
            r#"{"balanceMsats":22000,"federationId":"1111111111111111111111111111111111111111111111111111111111111111"}"#,
            "\n"
        )
    );
    assert_eq!(stderr, "");

    let (stdout, stderr) = capture(|output| {
        output.payment_wallet_accounting(
            federation_id,
            fedimint_core::Amount::from_msats(22_000),
            crate::payer::PaymentWalletAccounting {
                received_input_msats: 100_000,
                receive_fee_msats: 250,
                setup_output_msats: 70_000,
                setup_fee_msats: 750,
                setup_transaction_count: 7,
            },
            OutputFormat::Json,
        )
    });
    assert_eq!(
        stdout,
        concat!(
            r#"{"balanceMsats":22000,"federationId":"1111111111111111111111111111111111111111111111111111111111111111","receiveFeeMsats":250,"receivedInputMsats":100000,"setupFeeMsats":750,"setupOutputMsats":70000,"setupTransactionCount":7}"#,
            "\n"
        )
    );
    assert_eq!(stderr, "");

    let (stdout, stderr) = capture(|output| {
        output.payment_wallet_deposit_address(federation_id, "tb1q-demo", OutputFormat::Json)
    });
    assert_eq!(
        stdout,
        concat!(
            r#"{"address":"tb1q-demo","federationId":"1111111111111111111111111111111111111111111111111111111111111111"}"#,
            "\n"
        )
    );
    assert_eq!(stderr, "");

    let (stdout, stderr) = capture(|output| {
        output.payment_wallet_balance_reached(
            federation_id,
            fedimint_core::Amount::from_msats(25_000),
            fedimint_core::Amount::from_msats(24_000),
            OutputFormat::Json,
        )
    });
    assert_eq!(
        stdout,
        concat!(
            r#"{"balanceMsats":25000,"federationId":"1111111111111111111111111111111111111111111111111111111111111111","minimumMsats":24000}"#,
            "\n"
        )
    );
    assert_eq!(stderr, "");

    let (stdout, stderr) = capture(|output| {
        output.payment_wallet_invoice(
            federation_id,
            "lntbs-demo",
            operation_id,
            fedimint_core::Amount::from_msats(30_000),
            OutputFormat::Json,
        )
    });
    assert_eq!(
        stdout,
        concat!(
            r#"{"amountMsats":30000,"federationId":"1111111111111111111111111111111111111111111111111111111111111111","invoice":"lntbs-demo","operationId":"2222222222222222222222222222222222222222222222222222222222222222"}"#,
            "\n"
        )
    );
    assert_eq!(stderr, "");

    let (stdout, stderr) = capture(|output| {
        output.payment_wallet_invoice_settled(
            federation_id,
            operation_id,
            crate::wallet::PaymentWalletInvoiceState::Claimed,
            fedimint_core::Amount::from_msats(30_000),
            OutputFormat::Json,
        )
    });
    assert_eq!(
        stdout,
        concat!(
            r#"{"balanceMsats":30000,"federationId":"1111111111111111111111111111111111111111111111111111111111111111","operationId":"2222222222222222222222222222222222222222222222222222222222222222","state":"claimed"}"#,
            "\n"
        )
    );
    assert_eq!(stderr, "");

    let (stdout, stderr) = capture(|output| {
        output.payment_wallet_guardian_fee_remitted(
            federation_id,
            operation_id,
            fedimint_core::Amount::from_msats(40_000),
            OutputFormat::Json,
        )
    });
    assert_eq!(
        stdout,
        concat!(
            r#"{"amountMsats":40000,"federationId":"1111111111111111111111111111111111111111111111111111111111111111","operationId":"2222222222222222222222222222222222222222222222222222222222222222"}"#,
            "\n"
        )
    );
    assert_eq!(stderr, "");

    for (state, state_name) in [
        (crate::wallet::PaymentWalletInvoiceState::Expired, "expired"),
        (crate::wallet::PaymentWalletInvoiceState::Failure, "failure"),
    ] {
        let (stdout, stderr) = capture(|output| {
            output.payment_wallet_invoice_settled(
                federation_id,
                operation_id,
                state,
                fedimint_core::Amount::from_msats(30_000),
                OutputFormat::Json,
            )
        });
        assert_eq!(
            stdout,
            format!(
                "{{\"balanceMsats\":30000,\"federationId\":\"{}\",\"operationId\":\"{}\",\"state\":\"{state_name}\"}}\n",
                "11".repeat(32),
                "22".repeat(32),
            )
        );
        assert_eq!(stderr, "");
    }
}

#[test]
fn liquidity_operation_json_is_one_stable_stdout_value() {
    let snapshot = fi_client::LiquidityOperationSnapshot {
        operation_id: fi_client::LiquidityOperationId("11".repeat(32)),
        formation_id: fi_client::FormationId("formation-1".to_owned()),
        provider_pubkey: fi_client::Pubkey("provider-1".to_owned()),
        endpoint_hint: fedi_decentralized_service_liquidity_manager::Url(
            "iroh://provider".to_owned(),
        ),
        details_payload_hash: fi_client::Sha256Digest([2; 32]),
        amounts: fi_client::LiquidityAmountBounds {
            gateway_min_amount: fi_client::Sats(100_000),
            gateway_max_amount: Some(fi_client::Sats(200_000)),
            stability_min_amount: fi_client::Sats(0),
            stability_max_amount: None,
        },
        phase: fi_client::LiquidityOperationPhase::Prepared,
        item_statuses: Vec::new(),
        rejection_code: None,
        gateway_view_verified: false,
    };
    let (stdout, stderr) =
        capture(|output| output.liquidity_snapshot(&snapshot, OutputFormat::Json));

    assert_eq!(stdout.lines().count(), 1);
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "operation_id": "11".repeat(32),
            "formation_id": "formation-1",
            "provider_pubkey": "provider-1",
            "endpoint_hint": "iroh://provider",
            "details_payload_hash": vec![2; 32],
            "amounts": {
                "gateway_min_amount": 100_000,
                "gateway_max_amount": 200_000,
                "stability_min_amount": 0,
                "stability_max_amount": null
            },
            "phase": "prepared",
            "item_statuses": [],
            "rejection_code": null,
            "gateway_view_verified": false
        })
    );
    assert_eq!(stderr, "");
}
