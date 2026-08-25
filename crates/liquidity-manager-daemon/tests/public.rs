use std::sync::Arc;

use super::*;
use crate::daemon::DaemonPhase;
use crate::test_support::{fixed_test_keys, sign_public_rpc_with_keys};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_service_liquidity_manager::ServiceErrorCode;
use fedi_decentralized_service_liquidity_manager::{
    AcceptedAttesterPolicy, AdvertisementConfig, AttestationSummary, BitcoinNetwork,
    CapacityConfig, ChainObserverBackendView, ChainObserverConfigView, CompletionEvidence,
    DurationSecs, FederationId, FederationLiquidityDetails, FederationName, FundingPolicyConfig,
    GatewayApiUrl, GatewayCompletionEvidence, GatewayConfigView, GatewayId, GatewayName, HashBytes,
    InviteCode, LiquidityAmountBounds, LiquidityFailureCode, PeerBadgeTrustPolicy, ProtocolVersion,
    ProviderPolicy, Pubkey, PublicRpcPayloadDomain, ReplenishmentConfig, RpcEndpointAddress,
    RpcEndpointConfig, RpcEndpointId, RpcProtocolName, RpcTransport, SetupValidationSummary,
    Sha256Digest, SourceType, Url, ValidationStatus, VerificationRequirement,
};
use nostr_sdk::Keys;

fn test_peer_badge_trust_policy() -> PeerBadgeTrustPolicy {
    let profile = ManifoldEnvironment::Development
        .profile()
        .expect("Development profile is valid");
    PeerBadgeTrustPolicy::try_new(profile.minimum_peer_badge_trust_level())
        .expect("Development PeerBadge trust policy is valid")
}

#[tokio::test]
async fn request_liquidity_accepts_gateway_request_and_status_lookup() -> anyhow::Result<()> {
    let context = test_context("phase3-accept-gateway").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway, SourceType::StabilityPool],
    )
    .await?;
    let request = signed_request(test_request(
        &provider_pubkey,
        "request-gateway",
        gateway_amounts(5_000),
    )?)?;

    let response = context.request_liquidity(request.clone()).await?;
    let allocation_status = match &response.payload.outcome {
        RequestLiquidityOutcome::Accepted(status) => status,
        outcome => panic!("expected accepted response, got {outcome:?}"),
    };
    assert_eq!(allocation_status.item_statuses.len(), 1);
    match &allocation_status.item_statuses[0].target {
        AllocationItemTarget::Gateway { amount, .. } => assert_eq!(*amount, Sats(5_000)),
        target => panic!("expected gateway target, got {target:?}"),
    }

    let status_request = signed_status_request(GetAllocationStatusRequest {
        version: PROTOCOL_VERSION,
        requester_pubkey: request.payload.requester_pubkey.clone(),
        details_payload_hash: request.payload.details_payload_hash,
        provider_pubkey,
        issued_at: now_timestamp(),
    })?;
    let status_response = context.get_allocation_status(status_request).await?;
    assert_eq!(
        status_response.payload.status.item_statuses[0].target,
        allocation_status.item_statuses[0].target
    );
    assert_eq!(
        status_response.payload.status.item_statuses[0].status,
        allocation_status.item_statuses[0].status
    );
    assert_eq!(
        status_response.payload.status.item_statuses[0].fulfilled_amount,
        None
    );

    assert_eq!(allocation_count(&context.database).await?, 1);
    assert_eq!(row_count(&context.database, "allocation_items").await?, 1);
    Ok(())
}

#[tokio::test]
async fn pipeline_rejections_persist_nothing_and_repeat() -> anyhow::Result<()> {
    // No trust-inputs mode is unavailable by construction: every input is
    // either the real network path or the preview fixture. What this guards
    // is the stateless property — a
    // rejected request persists nothing, and a repeat re-evaluates from
    // scratch instead of being answered from stored state. The
    // rejection-code mapping itself is covered by the verification unit
    // tests; this request is rejected at the admission gate because it
    // carries no endorsement.
    let mut context = test_context("phase11b-stateless-rejection").await?;
    context.verification_provider = Arc::new(crate::verification::VerificationPipeline::new(
        crate::verification::VerificationDeps {
            database: context.database.clone(),
            revocation_fetcher: Arc::new(crate::revocation::NostrRevocationFetcher),
            preview_provider: Arc::new(
                crate::federation_preview::test_fakes::FakeFederationPreviewProvider::default(),
            ),
            verification_budget: context.verification_budget.clone(),
        },
        crate::verification::TrustInputs::Fixtures,
        test_peer_badge_trust_policy(),
    ));
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway, SourceType::StabilityPool],
    )
    .await?;
    let request = signed_request(test_request(
        &provider_pubkey,
        "request-production-fail-closed",
        gateway_amounts(5_000),
    )?)?;

    let response = context.request_liquidity(request.clone()).await?;
    match &response.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => {
            assert_eq!(rejection.code, PublicRejectionCode::InvalidCredentials);
        }
        outcome => panic!("expected fail-closed rejection, got {outcome:?}"),
    }

    assert_eq!(allocation_count(&context.database).await?, 0);
    let repeated = context.request_liquidity(request).await?;
    assert_rejection(repeated, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

#[tokio::test]
async fn request_liquidity_accepts_stability_and_combined_requests() -> anyhow::Result<()> {
    let context = test_context("phase3-accept-mixed").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(50_000)),
        vec![SourceType::Gateway, SourceType::StabilityPool],
    )
    .await?;

    let stability_response = context
        .request_liquidity(signed_request(test_request(
            &provider_pubkey,
            "request-stability",
            stability_amounts(7_000),
        )?)?)
        .await?;
    assert_accepted_item_count(&stability_response, 1);

    let combined_response = context
        .request_liquidity(signed_request(test_request(
            &provider_pubkey,
            "request-combined",
            combined_amounts(5_000, 6_000),
        )?)?)
        .await?;
    assert_accepted_item_count(&combined_response, 2);
    assert_eq!(row_count(&context.database, "allocation_items").await?, 3);
    Ok(())
}

#[tokio::test]
async fn request_liquidity_rejects_invalid_bounds_and_sources() -> anyhow::Result<()> {
    let context = test_context("phase3-invalid-bounds").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;

    assert_rejection(
        context
            .request_liquidity(signed_request(test_request(
                &provider_pubkey,
                "request-zero-min",
                LiquidityAmountBounds {
                    gateway_min_amount: Sats(0),
                    gateway_max_amount: None,
                    stability_min_amount: Sats(0),
                    stability_max_amount: None,
                },
            )?)?)
            .await?,
        PublicRejectionCode::InvalidAmountBounds,
    );
    assert_rejection(
        context
            .request_liquidity(signed_request(test_request(
                &provider_pubkey,
                "request-bad-max",
                LiquidityAmountBounds {
                    gateway_min_amount: Sats(10_000),
                    gateway_max_amount: Some(Sats(9_999)),
                    stability_min_amount: Sats(0),
                    stability_max_amount: None,
                },
            )?)?)
            .await?,
        PublicRejectionCode::InvalidAmountBounds,
    );
    assert_rejection(
        context
            .request_liquidity(signed_request(test_request(
                &provider_pubkey,
                "request-unrequested-max",
                LiquidityAmountBounds {
                    gateway_min_amount: Sats(0),
                    gateway_max_amount: Some(Sats(1)),
                    stability_min_amount: Sats(5_000),
                    stability_max_amount: None,
                },
            )?)?)
            .await?,
        PublicRejectionCode::InvalidAmountBounds,
    );
    assert_rejection(
        context
            .request_liquidity(signed_request(test_request(
                &provider_pubkey,
                "request-unsupported-source",
                stability_amounts(5_000),
            )?)?)
            .await?,
        PublicRejectionCode::UnsupportedSourceType,
    );
    Ok(())
}

#[tokio::test]
async fn request_liquidity_rejects_network_expiry_and_details_hash() -> anyhow::Result<()> {
    let context = test_context("phase3-network-expiry-details").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;

    let mut wrong_network = test_request(
        &provider_pubkey,
        "request-wrong-network",
        gateway_amounts(5_000),
    )?;
    wrong_network.network = BitcoinNetwork::Bitcoin;
    let expected_hash = request_liquidity_details_hash_for_request(&wrong_network)?;
    wrong_network.details_payload_hash = expected_hash;
    assert_rejection(
        context
            .request_liquidity(signed_request(wrong_network)?)
            .await?,
        PublicRejectionCode::UnsupportedNetwork,
    );

    let mut expired = test_request(&provider_pubkey, "request-expired", gateway_amounts(5_000))?;
    expired.expires_at = Timestamp(now_timestamp().0.saturating_sub(1));
    expired.details_payload_hash = request_liquidity_details_hash_for_request(&expired)?;
    assert_rejection(
        context.request_liquidity(signed_request(expired)?).await?,
        PublicRejectionCode::RequestExpired,
    );

    let mut wrong_details = test_request(
        &provider_pubkey,
        "request-wrong-details",
        gateway_amounts(5_000),
    )?;
    wrong_details.details_payload_hash = Sha256Digest([0xff; 32]);
    assert_rejection(
        context
            .request_liquidity(signed_request(wrong_details)?)
            .await?,
        PublicRejectionCode::InvalidDetailsPayload,
    );

    let mut wrong_version = test_request(
        &provider_pubkey,
        "request-wrong-version",
        gateway_amounts(5_000),
    )?;
    wrong_version.version = ProtocolVersion(999);
    wrong_version.details_payload_hash =
        request_liquidity_details_hash_for_request(&wrong_version)?;
    assert_rejection(
        context
            .request_liquidity(signed_request(wrong_version)?)
            .await?,
        PublicRejectionCode::VersionUnsupported,
    );
    Ok(())
}

#[tokio::test]
async fn public_rejection_conformance_matrix_is_stable() -> anyhow::Result<()> {
    let cases = [
        (
            "zero-minimums",
            PublicRejectionCode::InvalidAmountBounds,
            CapacityMode::ExplicitCap,
            Some(Sats(20_000)),
        ),
        (
            "max-below-minimum",
            PublicRejectionCode::InvalidAmountBounds,
            CapacityMode::ExplicitCap,
            Some(Sats(20_000)),
        ),
        (
            "unsupported-source",
            PublicRejectionCode::UnsupportedSourceType,
            CapacityMode::ExplicitCap,
            Some(Sats(20_000)),
        ),
        (
            "wrong-network",
            PublicRejectionCode::UnsupportedNetwork,
            CapacityMode::ExplicitCap,
            Some(Sats(20_000)),
        ),
        (
            "expired",
            PublicRejectionCode::RequestExpired,
            CapacityMode::ExplicitCap,
            Some(Sats(20_000)),
        ),
        (
            "wrong-details-hash",
            PublicRejectionCode::InvalidDetailsPayload,
            CapacityMode::ExplicitCap,
            Some(Sats(20_000)),
        ),
        (
            "wrong-version",
            PublicRejectionCode::VersionUnsupported,
            CapacityMode::ExplicitCap,
            Some(Sats(20_000)),
        ),
        (
            "insufficient-capacity",
            PublicRejectionCode::InsufficientCapacity,
            CapacityMode::ExplicitCap,
            Some(Sats(4_999)),
        ),
    ];

    for (name, expected_code, capacity_mode, explicit_cap) in cases {
        let context = test_context(&format!("phase10-public-rejection-{name}")).await?;
        let provider_pubkey = setup_ready_provider(
            &context,
            capacity_mode,
            explicit_cap,
            vec![SourceType::Gateway],
        )
        .await?;
        let mut request = test_request(
            &provider_pubkey,
            &format!("request-{name}"),
            gateway_amounts(5_000),
        )?;
        match name {
            "zero-minimums" => {
                request.amounts = LiquidityAmountBounds {
                    gateway_min_amount: Sats(0),
                    gateway_max_amount: None,
                    stability_min_amount: Sats(0),
                    stability_max_amount: None,
                };
                request.details_payload_hash =
                    request_liquidity_details_hash_for_request(&request)?;
            }
            "max-below-minimum" => {
                request.amounts.gateway_max_amount = Some(Sats(4_999));
                request.details_payload_hash =
                    request_liquidity_details_hash_for_request(&request)?;
            }
            "unsupported-source" => {
                request.amounts = stability_amounts(5_000);
                request.details_payload_hash =
                    request_liquidity_details_hash_for_request(&request)?;
            }
            "wrong-network" => {
                request.network = BitcoinNetwork::Bitcoin;
                request.details_payload_hash =
                    request_liquidity_details_hash_for_request(&request)?;
            }
            "expired" => {
                request.expires_at = Timestamp(now_timestamp().0.saturating_sub(1));
                request.details_payload_hash =
                    request_liquidity_details_hash_for_request(&request)?;
            }
            "wrong-details-hash" => {
                request.details_payload_hash = Sha256Digest([0xee; 32]);
            }
            "wrong-version" => {
                request.version = ProtocolVersion(999);
                request.details_payload_hash =
                    request_liquidity_details_hash_for_request(&request)?;
            }
            "insufficient-capacity" => {}
            _ => unreachable!("unhandled conformance case {name}"),
        }

        let response = context.request_liquidity(signed_request(request)?).await?;
        assert_rejection(response, expected_code);
    }
    Ok(())
}

#[tokio::test]
async fn public_allocation_status_reports_item_states_without_an_aggregate() -> anyhow::Result<()> {
    let context = test_context("public-independent-statuses").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway, SourceType::StabilityPool],
    )
    .await?;
    let request = signed_request(test_request(
        &provider_pubkey,
        "mixed-status",
        combined_amounts(5_000, 5_000),
    )?)?;
    context.request_liquidity(request.clone()).await?;
    sqlx::query("UPDATE allocation_items SET status = CASE source_type WHEN 'gateway' THEN 'failed' ELSE 'running' END WHERE federation_id = ?")
        .bind(&request.payload.federation_details.federation_id.0).execute(context.database.pool()).await?;
    let response = context
        .get_allocation_status(signed_status_request(GetAllocationStatusRequest {
            version: PROTOCOL_VERSION,
            requester_pubkey: request.payload.requester_pubkey.clone(),
            details_payload_hash: request.payload.details_payload_hash,
            provider_pubkey,
            issued_at: now_timestamp(),
        })?)
        .await?;
    assert!(
        response
            .payload
            .status
            .item_statuses
            .iter()
            .any(|item| item.status == ItemAllocationStatus::Failed)
    );
    assert!(
        response
            .payload
            .status
            .item_statuses
            .iter()
            .any(|item| item.status == ItemAllocationStatus::Running)
    );
    Ok(())
}

#[tokio::test]
async fn request_liquidity_rejects_when_capacity_is_unavailable() -> anyhow::Result<()> {
    let context = test_context("phase3-capacity").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(4_999)),
        vec![SourceType::Gateway],
    )
    .await?;
    assert_rejection(
        context
            .request_liquidity(signed_request(test_request(
                &provider_pubkey,
                "request-insufficient-cap",
                gateway_amounts(5_000),
            )?)?)
            .await?,
        PublicRejectionCode::InsufficientCapacity,
    );

    let no_observation_context = test_context("phase4-no-wallet-observation").await?;
    let no_observation_provider = setup_ready_provider(
        &no_observation_context,
        CapacityMode::AvailableFunds,
        None,
        vec![SourceType::Gateway],
    )
    .await?;
    sqlx::query("DELETE FROM wallet_balance_observations")
        .execute(no_observation_context.database.pool())
        .await?;
    assert_rejection(
        no_observation_context
            .request_liquidity(signed_request(test_request(
                &no_observation_provider,
                "request-no-wallet-observation",
                gateway_amounts(5_000),
            )?)?)
            .await?,
        PublicRejectionCode::InsufficientCapacity,
    );
    Ok(())
}

/// An operator can lower the allocation cap below what is already reserved,
/// or raise the fee reserve above what the wallet holds. Both are ordinary
/// wind-down actions and neither is refused.
///
/// What must hold is that admission stays *closed* under them rather than
/// merely inconsistent: the cap is reduced by active reservations with a
/// saturating subtraction and then taken as a minimum against the
/// wallet-backed figure, so a configuration its own deployment already
/// exceeds admits nothing. Without that, a lowered cap would be a state in
/// which the recorded budget is violated *and* new work is still accepted
/// against it.
#[tokio::test]
async fn a_configuration_already_exceeded_admits_nothing() -> anyhow::Result<()> {
    let context = test_context("phase3-config-lowering").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;

    // One accepted allocation reserving real capacity.
    context
        .request_liquidity(signed_request(test_request(
            &provider_pubkey,
            "lowering-first",
            gateway_amounts(9_000),
        )?)?)
        .await?;

    // The operator lowers the cap beneath what that allocation reserves.
    setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(1_000)),
        vec![SourceType::Gateway],
    )
    .await?;

    assert_rejection(
        context
            .request_liquidity(signed_request(test_request(
                &provider_pubkey,
                "lowering-second",
                gateway_amounts(500),
            )?)?)
            .await?,
        PublicRejectionCode::InsufficientCapacity,
    );
    // Even an amount inside the new cap on its own is refused, because the
    // outstanding reservation consumes all of it.
    assert_eq!(allocation_count(&context.database).await?, 1);
    Ok(())
}

#[tokio::test]
async fn request_liquidity_uses_available_wallet_funds() -> anyhow::Result<()> {
    let context = test_context("phase4-available-funds").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::AvailableFunds,
        None,
        vec![SourceType::Gateway],
    )
    .await?;
    let response = context
        .request_liquidity(signed_request(test_request(
            &provider_pubkey,
            "request-available-funds",
            gateway_amounts(5_000),
        )?)?)
        .await?;
    assert_accepted_item_count(&response, 1);
    Ok(())
}

#[tokio::test]
async fn request_liquidity_is_idempotent_and_detects_conflict() -> anyhow::Result<()> {
    let context = test_context("phase3-idempotency").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;
    let request = signed_request(test_request(
        &provider_pubkey,
        "request-duplicate",
        gateway_amounts(5_000),
    )?)?;

    let first = context.request_liquidity(request.clone()).await?;
    let second = context.request_liquidity(request).await?;
    // Repeats are answered semantically from the allocation's current
    // state with a fresh signature, not by replaying stored bytes.
    let (first_status, second_status) = match (&first.payload.outcome, &second.payload.outcome) {
        (
            RequestLiquidityOutcome::Accepted(first_status),
            RequestLiquidityOutcome::Accepted(second_status),
        ) => (first_status, second_status),
        outcomes => panic!("expected accepted responses, got {outcomes:?}"),
    };
    assert_eq!(
        first_status.details_payload_hash,
        second_status.details_payload_hash
    );
    assert_eq!(first_status.item_statuses, second_status.item_statuses);
    assert_eq!(allocation_count(&context.database).await?, 1);
    assert_eq!(row_count(&context.database, "allocation_items").await?, 1);

    let conflict = context
        .request_liquidity(signed_request(test_request(
            &provider_pubkey,
            "request-duplicate",
            gateway_amounts(6_000),
        )?)?)
        .await?;
    assert_rejection(conflict, PublicRejectionCode::RequestConflict);
    assert_eq!(allocation_count(&context.database).await?, 1);
    Ok(())
}

#[tokio::test]
async fn repeat_fast_path_is_bound_to_the_original_requester() -> anyhow::Result<()> {
    let context = test_context("phase3-repeat-requester-binding").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;
    let request = signed_request(test_request(
        &provider_pubkey,
        "request-requester-binding",
        gateway_amounts(5_000),
    )?)?;
    context.request_liquidity(request.clone()).await?;

    // Complete the item with funding evidence so the test observes
    // exactly what a leaking fast path would hand out.
    let federation_id = &request.payload.federation_details.federation_id;
    let evidence = CompletionEvidence::Gateway(GatewayCompletionEvidence {
        gateway_id: GatewayId("gateway-1".to_owned()),
        gateway_api: GatewayApiUrl::try_from("https://gateway.example").unwrap(),
        fulfilled_amount: Sats(5_000),
        observed_gateway_balance: Sats(5_000),
        observed_at: now_timestamp(),
        withdrawal_txid: Some("victim-withdrawal-txid".to_owned()),
        wallet_operation_id: None,
    });
    allocation_store::complete_item(
        &context.database,
        federation_id,
        &allocation_store::item_id(federation_id, SourceType::Gateway),
        Sats(5_000),
        evidence.clone(),
    )
    .await?;

    // The original requester's repeat still gets the current status,
    // evidence included.
    let retry = context.request_liquidity(request.clone()).await?;
    let retry_status = match &retry.payload.outcome {
        RequestLiquidityOutcome::Accepted(status) => status,
        outcome => panic!("expected accepted retry, got {outcome:?}"),
    };
    assert_eq!(
        retry_status.item_statuses[0].completion_evidence,
        Some(evidence)
    );

    // A different requester replaying the victim's details commitment
    // signs validly as itself, but the copied hash is not canonical for
    // its own details, so it is rejected without seeing any status.
    let mut attacker_request = test_request(
        &provider_pubkey,
        "request-requester-binding",
        gateway_amounts(5_000),
    )?;
    attacker_request.requester_pubkey = Pubkey(attacker_keys().public_key().to_hex());
    attacker_request.details_payload_hash = request.payload.details_payload_hash;
    let attacker_response = context
        .request_liquidity(sign_public_rpc_with_keys(
            PublicRpcPayloadDomain::RequestLiquidityRequest,
            attacker_request,
            &attacker_keys(),
        )?)
        .await?;
    assert_rejection(
        attacker_response,
        PublicRejectionCode::InvalidDetailsPayload,
    );

    // The stored allocation is untouched: still the victim's, still one
    // allocation with one item.
    let stored_requester: String =
        sqlx::query_scalar("SELECT requester_pubkey FROM allocations WHERE federation_id = ?")
            .bind(&federation_id.0)
            .fetch_one(context.database.pool())
            .await?;
    assert_eq!(stored_requester, request.payload.requester_pubkey.0);
    assert_eq!(allocation_count(&context.database).await?, 1);
    assert_eq!(row_count(&context.database, "allocation_items").await?, 1);
    Ok(())
}

/// A verified requester takes over an allocation that holds nothing.
///
/// `SPEC-flip-rpc`: one allocation per federation
/// is what stops a published endorsement being drawn down repeatedly, but it
/// also decided *who* held that allocation. With `insert_allocation`'s
/// `INSERT OR IGNORE` as the table's only production writer and no
/// production `UPDATE` or `DELETE`, that decision was permanent — an idle
/// allocation excluded an equally-credentialed requester from a federation
/// forever while holding nothing of value.
#[tokio::test]
async fn a_verified_requester_takes_over_an_allocation_that_holds_nothing() -> anyhow::Result<()> {
    let context = test_context("phase3-takeover-idle").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;

    let first = signed_request(test_request(
        &provider_pubkey,
        "takeover-idle",
        gateway_amounts(5_000),
    )?)?;
    context.request_liquidity(first.clone()).await?;
    let federation_id = &first.payload.federation_details.federation_id;

    // Drive the allocation idle through a production path: the item fails
    // with nothing fulfilled. `fail_item` is what the gateway worker calls.
    allocation_store::fail_item(
        &context.database,
        federation_id,
        &allocation_store::item_id(federation_id, SourceType::Gateway),
        LiquidityFailureCode::GatewayAttachFailed,
        "gateway never attached",
    )
    .await?;

    let mut second = test_request(&provider_pubkey, "takeover-idle", gateway_amounts(5_000))?;
    second.requester_pubkey = Pubkey(attacker_keys().public_key().to_hex());
    second.details_payload_hash = request_liquidity_details_hash_for_request(&second)?;
    let response = context
        .request_liquidity(sign_public_rpc_with_keys(
            PublicRpcPayloadDomain::RequestLiquidityRequest,
            second.clone(),
            &attacker_keys(),
        )?)
        .await?;
    assert!(
        matches!(
            response.payload.outcome,
            RequestLiquidityOutcome::Accepted(_)
        ),
        "an idle allocation must not exclude a verified requester: {:?}",
        response.payload.outcome
    );

    // The federation still has exactly one allocation, and it is the new
    // requester's. The old item is gone with the allocation it belonged to.
    let stored_requester: String =
        sqlx::query_scalar("SELECT requester_pubkey FROM allocations WHERE federation_id = ?")
            .bind(&federation_id.0)
            .fetch_one(context.database.pool())
            .await?;
    assert_eq!(stored_requester, second.requester_pubkey.0);
    assert_eq!(allocation_count(&context.database).await?, 1);
    assert_eq!(row_count(&context.database, "allocation_items").await?, 1);

    // The release is auditable rather than silent.
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'take_over_idle_allocation'",
    )
    .fetch_one(context.database.pool())
    .await?;
    assert_eq!(audited, 1);
    Ok(())
}

/// An allocation that still holds work is not taken over.
///
/// This is the half the ruling keeps: once real work starts the binding
/// locks, which is the state that actually matters. A fresh allocation's
/// item is `pending`, which reserves, so no state has to be constructed to
/// reach it.
#[tokio::test]
async fn an_allocation_that_still_holds_work_is_not_taken_over() -> anyhow::Result<()> {
    let context = test_context("phase3-takeover-busy").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;

    let first = signed_request(test_request(
        &provider_pubkey,
        "takeover-busy",
        gateway_amounts(5_000),
    )?)?;
    context.request_liquidity(first.clone()).await?;
    let federation_id = &first.payload.federation_details.federation_id;

    let mut second = test_request(&provider_pubkey, "takeover-busy", gateway_amounts(5_000))?;
    second.requester_pubkey = Pubkey(attacker_keys().public_key().to_hex());
    second.details_payload_hash = request_liquidity_details_hash_for_request(&second)?;
    let response = context
        .request_liquidity(sign_public_rpc_with_keys(
            PublicRpcPayloadDomain::RequestLiquidityRequest,
            second,
            &attacker_keys(),
        )?)
        .await?;
    assert_rejection(response, PublicRejectionCode::RequestConflict);

    let stored_requester: String =
        sqlx::query_scalar("SELECT requester_pubkey FROM allocations WHERE federation_id = ?")
            .bind(&federation_id.0)
            .fetch_one(context.database.pool())
            .await?;
    assert_eq!(
        stored_requester, first.payload.requester_pubkey.0,
        "the incumbent must keep an allocation that still reserves"
    );
    assert_eq!(allocation_count(&context.database).await?, 1);
    Ok(())
}

/// A completed allocation is not taken over either, even with no item
/// reserving and no operation pending.
///
/// This pins the third term of the idle predicate on its own.
/// `allocations.committed_amount_sats` cannot carry it: that column is
/// written once by `insert_allocation` and no production statement updates
/// it, so reading "no committed value" off it would make every allocation
/// permanently non-idle and the takeover dead code.
/// `allocation_items.fulfilled_amount_sats` is what records delivered value.
#[tokio::test]
async fn an_allocation_that_delivered_value_is_not_taken_over() -> anyhow::Result<()> {
    let context = test_context("phase3-takeover-fulfilled").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;

    let first = signed_request(test_request(
        &provider_pubkey,
        "takeover-fulfilled",
        gateway_amounts(5_000),
    )?)?;
    context.request_liquidity(first.clone()).await?;
    let federation_id = &first.payload.federation_details.federation_id;

    allocation_store::complete_item(
        &context.database,
        federation_id,
        &allocation_store::item_id(federation_id, SourceType::Gateway),
        Sats(5_000),
        CompletionEvidence::Gateway(GatewayCompletionEvidence {
            gateway_id: GatewayId("gateway-1".to_owned()),
            gateway_api: GatewayApiUrl::try_from("https://gateway.example").unwrap(),
            fulfilled_amount: Sats(5_000),
            observed_gateway_balance: Sats(5_000),
            observed_at: now_timestamp(),
            withdrawal_txid: Some("takeover-fulfilled-txid".to_owned()),
            wallet_operation_id: None,
        }),
    )
    .await?;

    let mut second = test_request(
        &provider_pubkey,
        "takeover-fulfilled",
        gateway_amounts(5_000),
    )?;
    second.requester_pubkey = Pubkey(attacker_keys().public_key().to_hex());
    second.details_payload_hash = request_liquidity_details_hash_for_request(&second)?;
    let response = context
        .request_liquidity(sign_public_rpc_with_keys(
            PublicRpcPayloadDomain::RequestLiquidityRequest,
            second,
            &attacker_keys(),
        )?)
        .await?;
    assert_rejection(response, PublicRejectionCode::RequestConflict);

    let stored_requester: String =
        sqlx::query_scalar("SELECT requester_pubkey FROM allocations WHERE federation_id = ?")
            .bind(&federation_id.0)
            .fetch_one(context.database.pool())
            .await?;
    assert_eq!(stored_requester, first.payload.requester_pubkey.0);
    Ok(())
}

/// `RequestLiquidity` must not answer "does this federation have an
/// allocation?" for a caller that has established nothing.
///
/// Public authentication verifies the signature against the key the request
/// declares, so anyone can produce a validly signed request under a fresh
/// key of their own. If the repeat fast path answered such a caller — with
/// `request_conflict` for a hash that differs from the stored one — then
/// one signature and a guessed federation id would tell it whether an
/// allocation exists, since a federation with none produces
/// `invalid_details_payload` instead.
///
/// The test is a comparison, not an assertion about one code: what matters
/// is that the two situations are indistinguishable, whatever the response
/// happens to be.
#[tokio::test]
async fn an_unrelated_caller_cannot_tell_whether_an_allocation_exists() -> anyhow::Result<()> {
    let context = test_context("phase3-existence-probe").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;

    // One federation is allocated; the other has never been seen.
    let victim = signed_request(test_request(
        &provider_pubkey,
        "probe-allocated",
        gateway_amounts(5_000),
    )?)?;
    context.request_liquidity(victim.clone()).await?;

    // The prober holds no endorsement, is not the requester, and does not
    // know the stored commitment, so it supplies an arbitrary hash.
    let probe = |federation_id: &str| -> anyhow::Result<Signed<RequestLiquidityRequest>> {
        let mut request = test_request(&provider_pubkey, federation_id, gateway_amounts(5_000))?;
        request.requester_pubkey = Pubkey(attacker_keys().public_key().to_hex());
        request.details_payload_hash = Sha256Digest([9; 32]);
        sign_public_rpc_with_keys(
            PublicRpcPayloadDomain::RequestLiquidityRequest,
            request,
            &attacker_keys(),
        )
    };

    let allocated = context.request_liquidity(probe("probe-allocated")?).await?;
    let unallocated = context
        .request_liquidity(probe("probe-never-seen")?)
        .await?;

    let code = |response: &Signed<RequestLiquidityResponse>| match &response.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => Some(rejection.code),
        RequestLiquidityOutcome::Accepted(_) => None,
    };
    assert_eq!(
        code(&allocated),
        code(&unallocated),
        "an allocated federation must answer a stranger exactly as an \
         unallocated one does"
    );
    assert_ne!(
        code(&allocated),
        Some(PublicRejectionCode::RequestConflict),
        "request_conflict is the code that names the row"
    );

    // Nothing was disclosed and nothing was written.
    let stored_requester: String =
        sqlx::query_scalar("SELECT requester_pubkey FROM allocations WHERE federation_id = ?")
            .bind(&victim.payload.federation_details.federation_id.0)
            .fetch_one(context.database.pool())
            .await?;
    assert_eq!(stored_requester, victim.payload.requester_pubkey.0);
    assert_eq!(allocation_count(&context.database).await?, 1);
    Ok(())
}

/// The same property for a prober that reaches verification.
///
/// The test above supplies a non-canonical commitment, so both of its
/// probes stop at `invalid_details_payload` inside pre-validation. That
/// exercises the fast path's non-requester arm, but it never shows that the
/// two federations stay indistinguishable once a probe passes
/// pre-validation — and a real adversary supplies a canonical commitment,
/// because the commitment covers `requester_pubkey` and it has its own.
///
/// So this probe computes the canonical hash under the attacker's own key
/// and runs against a real verification pipeline, which rejects it for
/// holding no endorsement. Both federations must still answer identically,
/// now through pre-validation, the verification budget, and verification.
///
/// A caller that *does* hold an endorsement is answered `request_conflict` for
/// an allocated federation. That disclosure is deliberate and accepted by
/// `SPEC-flip-rpc`, and is not a case this test
/// covers.
#[tokio::test]
async fn a_caller_without_an_endorsement_cannot_tell_whether_an_allocation_exists()
-> anyhow::Result<()> {
    let mut context = test_context("phase3-existence-probe-verified").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;

    // Allocate the victim federation while the pass-all double is still in
    // place: the allocation is the state under test, not the path to it.
    let victim = signed_request(test_request(
        &provider_pubkey,
        "probe-allocated",
        gateway_amounts(5_000),
    )?)?;
    context.request_liquidity(victim.clone()).await?;
    assert_eq!(allocation_count(&context.database).await?, 1);

    // A real pipeline from here on, so a probe carrying no endorsement is
    // rejected by verification rather than short-circuiting before it.
    context.verification_provider = Arc::new(crate::verification::VerificationPipeline::new(
        crate::verification::VerificationDeps {
            database: context.database.clone(),
            revocation_fetcher: Arc::new(crate::revocation::NostrRevocationFetcher),
            preview_provider: Arc::new(
                crate::federation_preview::test_fakes::FakeFederationPreviewProvider::default(),
            ),
            verification_budget: context.verification_budget.clone(),
        },
        crate::verification::TrustInputs::Fixtures,
        test_peer_badge_trust_policy(),
    ));

    // Canonical commitment under the attacker's own key, so pre-validation
    // passes and the request reaches the budget and verification.
    let probe = |federation_id: &str| -> anyhow::Result<Signed<RequestLiquidityRequest>> {
        let mut request = test_request(&provider_pubkey, federation_id, gateway_amounts(5_000))?;
        request.requester_pubkey = Pubkey(attacker_keys().public_key().to_hex());
        request.details_payload_hash = request_liquidity_details_hash_for_request(&request)?;
        sign_public_rpc_with_keys(
            PublicRpcPayloadDomain::RequestLiquidityRequest,
            request,
            &attacker_keys(),
        )
    };

    let allocated = context.request_liquidity(probe("probe-allocated")?).await?;
    let unallocated = context
        .request_liquidity(probe("probe-never-seen")?)
        .await?;

    let code = |response: &Signed<RequestLiquidityResponse>| match &response.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => Some(rejection.code),
        RequestLiquidityOutcome::Accepted(_) => None,
    };
    assert_eq!(
        code(&allocated),
        code(&unallocated),
        "an allocated federation must answer an unendorsed caller exactly as \
         an unallocated one does, all the way through verification"
    );
    // Pin the shared answer, so the assertion above cannot be satisfied by
    // both probes regressing to some other common code.
    assert_eq!(
        code(&allocated),
        Some(PublicRejectionCode::InvalidCredentials),
        "the probe must be refused by verification, not earlier"
    );

    // The victim's allocation is untouched and no second one was created.
    let stored_requester: String =
        sqlx::query_scalar("SELECT requester_pubkey FROM allocations WHERE federation_id = ?")
            .bind(&victim.payload.federation_details.federation_id.0)
            .fetch_one(context.database.pool())
            .await?;
    assert_eq!(stored_requester, victim.payload.requester_pubkey.0);
    assert_eq!(allocation_count(&context.database).await?, 1);
    Ok(())
}

/// A signature is only as reusable as the window it commits to.
///
/// The details hash covers `issued_at` and `expires_at`, so without a
/// ceiling on the distance between them a requester chose its own — years
/// out if it liked — and one signature stayed deliverable for that whole
/// time. Every cost of evaluating it could then be incurred again from that
/// one signature.
#[tokio::test]
async fn a_request_window_longer_than_the_ceiling_is_refused() -> anyhow::Result<()> {
    let context = test_context("phase3-request-lifetime").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;

    let mut request = test_request(&provider_pubkey, "long-window", gateway_amounts(5_000))?;
    request.expires_at = Timestamp(request.issued_at.0 + MAX_REQUEST_LIFETIME_SECS + 1);
    request.details_payload_hash = request_liquidity_details_hash_for_request(&request)?;
    assert_rejection(
        context.request_liquidity(signed_request(request)?).await?,
        PublicRejectionCode::RequestExpired,
    );

    // The same trick through a future `issued_at`: a window inside the
    // ceiling, placed far enough ahead to stay live indefinitely.
    let mut future = test_request(&provider_pubkey, "future-window", gateway_amounts(5_000))?;
    future.issued_at = Timestamp(now_timestamp().0 + 86_400);
    future.expires_at = Timestamp(future.issued_at.0 + 600);
    future.details_payload_hash = request_liquidity_details_hash_for_request(&future)?;
    assert_rejection(
        context.request_liquidity(signed_request(future)?).await?,
        PublicRejectionCode::RequestExpired,
    );

    // An ordinary window is unaffected.
    let ordinary = test_request(&provider_pubkey, "ordinary-window", gateway_amounts(5_000))?;
    let accepted = context.request_liquidity(signed_request(ordinary)?).await?;
    assert!(
        matches!(
            accepted.payload.outcome,
            RequestLiquidityOutcome::Accepted(_)
        ),
        "{:?}",
        accepted.payload.outcome
    );
    Ok(())
}

#[tokio::test]
async fn rejected_request_repeats_are_re_evaluated() -> anyhow::Result<()> {
    let context = test_context("phase3-rejected-duplicate").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;
    let request = signed_request(test_request(
        &provider_pubkey,
        "request-rejected-duplicate",
        LiquidityAmountBounds {
            gateway_min_amount: Sats(0),
            gateway_max_amount: None,
            stability_min_amount: Sats(0),
            stability_max_amount: None,
        },
    )?)?;

    let first = context.request_liquidity(request.clone()).await?;
    let second = context.request_liquidity(request).await?;
    assert_rejection(first, PublicRejectionCode::InvalidAmountBounds);
    assert_rejection(second, PublicRejectionCode::InvalidAmountBounds);
    assert_eq!(allocation_count(&context.database).await?, 0);
    Ok(())
}

#[tokio::test]
async fn concurrent_duplicate_request_creates_one_allocation() -> anyhow::Result<()> {
    let context = test_context("phase3-concurrent-duplicate").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;
    let request = signed_request(test_request(
        &provider_pubkey,
        "request-concurrent",
        gateway_amounts(5_000),
    )?)?;

    let (first, second) = tokio::join!(
        context.request_liquidity(request.clone()),
        context.request_liquidity(request)
    );
    assert_accepted_item_count(&first?, 1);
    assert_accepted_item_count(&second?, 1);
    assert_eq!(allocation_count(&context.database).await?, 1);
    assert_eq!(row_count(&context.database, "allocation_items").await?, 1);
    Ok(())
}

#[tokio::test]
async fn provider_info_reports_ready_config() -> anyhow::Result<()> {
    let context = test_context("phase3-provider-info").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;
    let request = sign_public_rpc_with_keys(
        PublicRpcPayloadDomain::GetProviderInfoRequest,
        GetProviderInfoRequest {
            version: PROTOCOL_VERSION,
            requester_pubkey: Pubkey(requester_keys().public_key().to_hex()),
            provider_pubkey,
            issued_at: now_timestamp(),
            advertisement_hash: Sha256Digest([1; 32]),
            client_supported_versions: vec![PROTOCOL_VERSION],
        },
        &requester_keys(),
    )?;

    let response = context.get_provider_info(request).await?;
    assert_eq!(
        response.payload.api_endpoint_id,
        RpcEndpointId("endpoint-1".to_owned())
    );
    assert_eq!(response.payload.outcome, ProviderInfoOutcome::Available);
    assert_eq!(
        response.payload.supported_sources,
        vec![SourceType::Gateway]
    );
    Ok(())
}

#[tokio::test]
async fn allocation_status_hides_wrong_lookup_keys_and_rejects_wrong_provider() -> anyhow::Result<()>
{
    let context = test_context("phase3-status-auth").await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;
    let request = signed_request(test_request(
        &provider_pubkey,
        "request-status-auth",
        gateway_amounts(5_000),
    )?)?;
    context.request_liquidity(request.clone()).await?;

    let wrong_details = signed_status_request(GetAllocationStatusRequest {
        version: PROTOCOL_VERSION,
        requester_pubkey: request.payload.requester_pubkey.clone(),
        details_payload_hash: Sha256Digest([9; 32]),
        provider_pubkey: provider_pubkey.clone(),
        issued_at: now_timestamp(),
    })?;
    let wrong_details_error = context
        .get_allocation_status(wrong_details)
        .await
        .expect_err("wrong details hash should not find allocation");
    assert_eq!(wrong_details_error.code(), ServiceErrorCode::NotFound);

    let wrong_provider = signed_status_request(GetAllocationStatusRequest {
        version: PROTOCOL_VERSION,
        requester_pubkey: request.payload.requester_pubkey,
        details_payload_hash: request.payload.details_payload_hash,
        provider_pubkey: Pubkey("wrong-provider".to_owned()),
        issued_at: now_timestamp(),
    })?;
    let wrong_provider_error = context
        .get_allocation_status(wrong_provider)
        .await
        .expect_err("wrong provider should be rejected");
    assert_eq!(
        wrong_provider_error.code(),
        ServiceErrorCode::PermissionDenied
    );
    Ok(())
}

/// A request that expires while its trust material is being verified must
/// not be newly accepted.
///
/// `pre_validation_failure` checks expiry before verification, and verification
/// may outlast the request that triggered it. Without a second check between the
/// two, anyone choosing a near-expiry request and delaying a verification
/// dependency has FLIP commit an allocation and sign `accepted` for a request
/// that has already expired.
#[tokio::test]
async fn a_request_that_expires_during_verification_is_not_accepted() -> anyhow::Result<()> {
    let context = crate::test_support::production_test_context(
        "expiry-during-verification",
        crate::nostr::fake_relay_publisher(),
        std::sync::Arc::new(ExpireDuringVerification),
    )
    .await?;
    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;

    let mut request = test_request(&provider_pubkey, "request-expiring", gateway_amounts(5_000))?;
    // Valid when it arrives, expired by the time verification returns. One
    // second is the smallest window the protocol's second-granularity clock
    // can express.
    request.issued_at = now_timestamp();
    request.expires_at = Timestamp(request.issued_at.0 + 1);
    request.details_payload_hash = request_liquidity_details_hash_for_request(&request)?;

    let response = context.request_liquidity(signed_request(request)?).await?;
    match &response.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => assert_eq!(
            rejection.code,
            PublicRejectionCode::RequestExpired,
            "expected an expiry rejection, got {rejection:?}"
        ),
        outcome => panic!("an expired request must not be accepted, got {outcome:?}"),
    }

    // A rejection that still committed the allocation would be the same
    // defect wearing a different response.
    let allocations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM allocations")
        .fetch_one(context.database.pool())
        .await?;
    assert_eq!(
        allocations, 0,
        "no allocation may be committed for an expired request"
    );

    Ok(())
}

/// A request verified under a superseded setup snapshot must not commit.
///
/// Begin a request under revision R, pause its verifier, commit an Admin
/// revision R+1 that removes what admitted it, then release verification.
/// Unfenced, the allocation commits afterwards, authorized by a snapshot that no
/// longer exists.
#[tokio::test]
async fn a_request_verified_under_a_superseded_setup_does_not_commit() -> anyhow::Result<()> {
    let database_for_verifier = std::sync::Arc::new(std::sync::Mutex::new(None));
    let context = crate::test_support::production_test_context(
        "superseded-setup-during-verification",
        crate::nostr::fake_relay_publisher(),
        std::sync::Arc::new(SupersedeSetupDuringVerification {
            database: database_for_verifier.clone(),
        }),
    )
    .await?;
    *database_for_verifier.lock().expect("verifier database") = Some(context.database.clone());

    let provider_pubkey = setup_ready_provider(
        &context,
        CapacityMode::ExplicitCap,
        Some(Sats(20_000)),
        vec![SourceType::Gateway],
    )
    .await?;
    let request = test_request(
        &provider_pubkey,
        "request-superseded",
        gateway_amounts(5_000),
    )?;

    let response = context.request_liquidity(signed_request(request)?).await?;
    match &response.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => assert_eq!(
            rejection.code,
            PublicRejectionCode::ProviderUnavailable,
            "expected a superseded-setup rejection, got {rejection:?}"
        ),
        outcome => {
            panic!("a superseded snapshot must not authorize an allocation, got {outcome:?}")
        }
    }

    let allocations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM allocations")
        .fetch_one(context.database.pool())
        .await?;
    assert_eq!(
        allocations, 0,
        "no allocation may commit under a snapshot an Admin update superseded"
    );

    Ok(())
}

/// Commits a setup revision while it verifies.
///
/// It bumps `setup_state.revision` directly, which is what
/// `apply_setup_config` and `update_provider_config` do atomically as part of
/// their own writes. The fence under test compares revisions, so the
/// increment is the whole of what those verbs contribute to it.
#[derive(Debug)]
struct SupersedeSetupDuringVerification {
    database: std::sync::Arc<std::sync::Mutex<Option<Database>>>,
}

#[async_trait::async_trait]
impl crate::verification::VerificationProvider for SupersedeSetupDuringVerification {
    fn mode(&self) -> crate::verification::VerificationModeInfo {
        crate::test_support::StaticVerificationProvider.mode()
    }

    async fn verify(
        &self,
        request: &RequestLiquidityRequest,
        config: &SetupConfigView,
    ) -> crate::verification::VerificationOutcome {
        let database = self
            .database
            .lock()
            .expect("verifier database")
            .clone()
            .expect("verifier database is installed before the request runs");
        sqlx::query("UPDATE setup_state SET revision = revision + 1 WHERE id = 1")
            .execute(database.pool())
            .await
            .expect("bump the setup revision");
        crate::test_support::StaticVerificationProvider
            .verify(request, config)
            .await
    }
}

/// Verifies successfully, but not before the request it is verifying has
/// expired.
///
/// It waits out that request's own window rather than a fixed duration, so
/// the test does not depend on how long the rest of the pass takes.
#[derive(Debug)]
struct ExpireDuringVerification;

#[async_trait::async_trait]
impl crate::verification::VerificationProvider for ExpireDuringVerification {
    fn mode(&self) -> crate::verification::VerificationModeInfo {
        crate::test_support::StaticVerificationProvider.mode()
    }

    async fn verify(
        &self,
        request: &RequestLiquidityRequest,
        config: &SetupConfigView,
    ) -> crate::verification::VerificationOutcome {
        while now_timestamp().0 <= request.expires_at.0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        crate::test_support::StaticVerificationProvider
            .verify(request, config)
            .await
    }
}

async fn test_context(name: &str) -> anyhow::Result<DaemonContext> {
    crate::test_support::production_test_context(
        name,
        crate::nostr::fake_relay_publisher(),
        crate::test_support::static_verification_provider(),
    )
    .await
}

async fn setup_ready_provider(
    context: &DaemonContext,
    capacity_mode: CapacityMode,
    explicit_cap: Option<Sats>,
    supported_sources: Vec<SourceType>,
) -> anyhow::Result<Pubkey> {
    let provider_pubkey = identity::load_provider_identity(&context.database).await?;
    crate::test_support::enroll_provider_trust_envelope(&context.database, &provider_pubkey)
        .await?;
    let config = test_setup_config(capacity_mode, explicit_cap, supported_sources);
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
    crate::wallet::observe_balance_serially(
        &context.database,
        &crate::wallet::WalletBackendBalance {
            network: BitcoinNetwork::Regtest,
            spendable: Sats(1_000_000),
            observed_at: now_timestamp(),
        },
    )
    .await?;
    Ok(provider_pubkey)
}

fn test_setup_config(
    capacity_mode: CapacityMode,
    explicit_cap: Option<Sats>,
    supported_sources: Vec<SourceType>,
) -> SetupConfigView {
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
        relays: vec![Url("ws://127.0.0.1:8080".to_owned())],
        capacity: CapacityConfig {
            mode: capacity_mode,
            explicit_cap,
            supported_sources,
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
            ready_advertisement_enabled: true,
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

fn test_request(
    provider_pubkey: &Pubkey,
    federation_id: &str,
    amounts: LiquidityAmountBounds,
) -> anyhow::Result<RequestLiquidityRequest> {
    let mut request = RequestLiquidityRequest {
        version: PROTOCOL_VERSION,
        requester_pubkey: Pubkey(requester_keys().public_key().to_hex()),
        provider_pubkey: provider_pubkey.clone(),
        issued_at: now_timestamp(),
        network: BitcoinNetwork::Regtest,
        amounts,
        details_payload_hash: Sha256Digest([0; 32]),
        fman_endorsement: None,
        fman_trust_material: None,
        federation_details: test_federation_details(federation_id),
        expires_at: Timestamp(now_timestamp().0 + 600),
    };
    request.details_payload_hash = request_liquidity_details_hash_for_request(&request)?;
    Ok(request)
}

fn test_federation_details(federation_id: &str) -> FederationLiquidityDetails {
    FederationLiquidityDetails {
        invite_code: InviteCode(format!("fedimint://invite-{federation_id}")),
        federation_id: FederationId(federation_id.to_owned()),
        federation_name: FederationName(format!("Federation {federation_id}")),
        federation_config_hash: HashBytes(vec![2; 32]),
        fleet_seat_hints: Vec::new(),
        revocation_locations: Vec::new(),
    }
}

fn gateway_amounts(amount: u64) -> LiquidityAmountBounds {
    LiquidityAmountBounds {
        gateway_min_amount: Sats(amount),
        gateway_max_amount: None,
        stability_min_amount: Sats(0),
        stability_max_amount: None,
    }
}

fn stability_amounts(amount: u64) -> LiquidityAmountBounds {
    LiquidityAmountBounds {
        gateway_min_amount: Sats(0),
        gateway_max_amount: None,
        stability_min_amount: Sats(amount),
        stability_max_amount: None,
    }
}

fn combined_amounts(gateway: u64, stability: u64) -> LiquidityAmountBounds {
    LiquidityAmountBounds {
        gateway_min_amount: Sats(gateway),
        gateway_max_amount: None,
        stability_min_amount: Sats(stability),
        stability_max_amount: None,
    }
}

fn requester_keys() -> Keys {
    fixed_test_keys(1)
}

fn attacker_keys() -> Keys {
    fixed_test_keys(2)
}

fn signed_request(
    request: RequestLiquidityRequest,
) -> anyhow::Result<Signed<RequestLiquidityRequest>> {
    sign_public_rpc_with_keys(
        PublicRpcPayloadDomain::RequestLiquidityRequest,
        request,
        &requester_keys(),
    )
}

fn signed_status_request(
    request: GetAllocationStatusRequest,
) -> anyhow::Result<Signed<GetAllocationStatusRequest>> {
    sign_public_rpc_with_keys(
        PublicRpcPayloadDomain::GetAllocationStatusRequest,
        request,
        &requester_keys(),
    )
}

fn assert_accepted_item_count(response: &Signed<RequestLiquidityResponse>, count: usize) {
    match &response.payload.outcome {
        RequestLiquidityOutcome::Accepted(status) => {
            assert_eq!(status.item_statuses.len(), count);
        }
        outcome => panic!("expected accepted response, got {outcome:?}"),
    }
}

fn assert_rejection(
    response: Signed<RequestLiquidityResponse>,
    expected_code: PublicRejectionCode,
) {
    match response.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => {
            assert_eq!(rejection.code, expected_code);
        }
        outcome => panic!("expected rejected response, got {outcome:?}"),
    }
}

async fn row_count(database: &Database, table: &str) -> anyhow::Result<i64> {
    let query = format!("SELECT COUNT(*) FROM {table}");
    Ok(sqlx::query_scalar(&query)
        .fetch_one(database.pool())
        .await?)
}

async fn allocation_count(database: &Database) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM allocations")
        .fetch_one(database.pool())
        .await?)
}

mod transport {
    use std::io::Read;
    use std::time::{SystemTime, UNIX_EPOCH};

    use anyhow::Context;

    use crate::public::{
        PUBLIC_ENDPOINT_ADDR_FILE, PUBLIC_ENDPOINT_ADDR_TEMP_FILE, write_public_endpoint_addr_file,
    };

    #[tokio::test]
    async fn endpoint_address_replacement_keeps_open_reader_on_complete_old_file()
    -> anyhow::Result<()> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before Unix epoch")?
            .as_nanos();
        let directory = std::env::temp_dir()
            .join("fedi-flip-tests")
            .join(format!("endpoint-address-{}-{unique}", std::process::id()));
        tokio::fs::create_dir_all(&directory).await?;
        let path = directory.join(PUBLIC_ENDPOINT_ADDR_FILE);
        let old_json = br#"{"id":"old","addrs":[]}"#;
        let new_json = br#"{"id":"new","addrs":[]}"#;
        tokio::fs::write(&path, old_json).await?;
        let mut reader = std::fs::File::open(&path)?;

        write_public_endpoint_addr_file(&path, new_json).await?;

        let mut reader_contents = Vec::new();
        reader.read_to_end(&mut reader_contents)?;
        assert_eq!(reader_contents, old_json);
        assert_eq!(tokio::fs::read(&path).await?, new_json);
        assert!(!directory.join(PUBLIC_ENDPOINT_ADDR_TEMP_FILE).exists());
        tokio::fs::remove_dir_all(directory).await?;
        Ok(())
    }
}
