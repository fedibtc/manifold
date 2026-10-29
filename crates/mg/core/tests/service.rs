use fedi_decentralized_service_manifold_guardian::{
    CreateSeatOutcome, DkgCompletionCallback, FaId, FederationName, FederationSize, GatewayApiUrl,
    GetDkgCodeRequest, GetFedimintStatsRequest, GetInviteCodeRequest, GetPeerAttestationRequest,
    GetQuoteRequest, GetQuoteResponse, GetStatusRequest, GuardianCode, GuardianFeeAccount,
    MetaConsensusBase, MetaFieldKey, MetaFieldValue, MgSignedRequest, OfferEpoch, Plan,
    ProposeFormationMetaRequest, RefusalReason, RegisterGatewayRequest, RestartDkgRequest, SeatId,
    SeatTransferGrant, SetMetaFieldRequest, StartDkgRequest, TransferSeatRequest,
};
use tempfile::TempDir;

use super::*;
use crate::facts::PortBase;
use crate::fleet::FleetConfig;
use crate::push_callback::{PushGatewayOrigin, PushGatewayOriginPolicy};
use crate::seat_process::SeatProcessSpawner;
use crate::seat_process::fake::{FakeApiState, block_forever, write_fake_fedimintd};
use crate::seat_process::{BitcoindConfig, RespawnPolicy, SeatProcessConfig};
use crate::seat_readiness::{ReadinessOutcome, ReadinessReport};
use crate::wallet::NoWallet;
use fedi_decentralized_service_manifold_guardian::DkgCompletionCallbackInput;

// Every fleet here gets its own `first_port_base`: the lib tests run
// concurrently, and a seat's fake fedimintd binds its API port, so a shared
// base makes one test's bind fail while another holds the port. Bases sit in
// 29_000..30_000, disjoint from the other mg-core test fleets and below the
// ephemeral port range.

async fn rpc(temp: &TempDir, first_port_base: u16) -> ManifoldGuardianRpc {
    rpc_with_config(temp, None, None, first_port_base).await
}

async fn rpc_with_config(
    temp: &TempDir,
    guardian_verification_fee_account: Option<Account>,
    push_gateway_origin: Option<PushGatewayOrigin>,
    first_port_base: u16,
) -> ManifoldGuardianRpc {
    rpc_with_network(
        temp,
        guardian_verification_fee_account,
        push_gateway_origin,
        bitcoin::Network::Regtest,
        first_port_base,
    )
    .await
}

async fn rpc_with_network(
    temp: &TempDir,
    guardian_verification_fee_account: Option<Account>,
    push_gateway_origin: Option<PushGatewayOrigin>,
    bitcoin_network: bitcoin::Network,
    first_port_base: u16,
) -> ManifoldGuardianRpc {
    // A fleet opens against an identity onboarding already chose; nothing
    // mints one on open.
    let db = crate::db::Db::open(temp.path()).await.unwrap();
    crate::onboarding::onboard_as_new(&db).await.unwrap();
    db.complete_onboarding_for_test(Some(1)).await.unwrap();

    let fleet = Fleet::open(
        db,
        FleetConfig {
            transfer_source: Arc::new(crate::transfer_source::UnavailableTransferSource),
            replacement_guardian_verifier: Arc::new(
                crate::replacement_guardian::UnverifiedReplacementGuardian,
            ),
            guardian_config_validator: Arc::new(crate::transfer_native::RejectGuardianConfig),
            process_spawner: SeatProcessSpawner::Fake(Arc::new(
                crate::seat_process::fake::FakeSeatProcessSpawner::default(),
            )),
            manifold_environment:
                fedi_decentralized_manifold_environment::ManifoldEnvironment::Development,
            first_port_base: PortBase::new(first_port_base).unwrap(),
            setup_payments_configured: true,
            guardian_verification_fee_account,
            respawn: RespawnPolicy::default(),
            // Tests hold the relay down and watch the retry land; a
            // production cadence would only make them slow.
            backup_scan_interval: std::time::Duration::from_millis(10),
            push_gateway_origin,
            push_callback_retry_interval: std::time::Duration::from_millis(10),
            completion_callback_invoker: Arc::new(crate::push_callback::TestCallbackInvoker),
            process: SeatProcessConfig {
                data_root: temp.path().to_owned(),
                fedimintd: write_fake_fedimintd(temp.path(), &block_forever()).await,
                bitcoin_network,
                iroh_dns: "https://dns.iroh.link/pkarr".parse().unwrap(),
                bitcoin_backend: crate::seat_process::BitcoinBackend::Bitcoind {
                    primary: BitcoindConfig {
                        url: "http://127.0.0.1:18443".to_owned(),
                        username: "user".to_owned(),
                        password: "pass".to_owned(),
                    },
                    esplora_fallback: None,
                },
            },
        },
        Arc::new(NoWallet),
    )
    .await
    .unwrap();
    fleet
        .set_seat_readiness(ReadinessReport {
            checked_at_ms: 1,
            relay: ReadinessOutcome::Pass,
            discovery: ReadinessOutcome::Pass,
            bitcoin: ReadinessOutcome::Pass,
            version: ReadinessOutcome::NotApplicable,
            uptime: ReadinessOutcome::Pass,
            uptime_comparison: None,
        })
        .await
        .unwrap();
    fleet.set_offered_price(Some(TEST_PRICE)).await.unwrap();
    ManifoldGuardianRpc::new(Arc::new(fleet), tokio::sync::watch::channel(None).1)
}

/// The offer these tests quote against: priced at zero, so a quote needs no
/// payment federation and no wallet.
const TEST_PRICE: Msats = Msats(0);

fn test_plan() -> Plan {
    Plan::InfiniteBestEffort {
        price_msats: TEST_PRICE.0,
    }
}

async fn rpc_with_owned_seat(
    temp: &TempDir,
    guardian_verification_fee_account: Option<Account>,
    first_port_base: u16,
) -> (ManifoldGuardianRpc, Keypair, FaId, SeatId) {
    rpc_with_owned_seat_and_origin(
        temp,
        guardian_verification_fee_account,
        None,
        first_port_base,
    )
    .await
}

async fn rpc_with_owned_seat_and_origin(
    temp: &TempDir,
    guardian_verification_fee_account: Option<Account>,
    push_gateway_origin: Option<PushGatewayOrigin>,
    first_port_base: u16,
) -> (ManifoldGuardianRpc, Keypair, FaId, SeatId) {
    let rpc = rpc_with_config(
        temp,
        guardian_verification_fee_account,
        push_gateway_origin,
        first_port_base,
    )
    .await;
    let owner_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let owner_id = FaId(owner_key.x_only_public_key().0);
    let quote = rpc
        .get_quote(GetQuoteRequest {
            fa_id: owner_id,
            fedimintd_version: supported_fedimintd_version(),
            federation_size: FederationSize(7),
            plan: test_plan(),
            payment_federation_id: None,
            refund_issuance: None,
        })
        .await
        .unwrap();
    let created = rpc
        .create_seat(
            SignedRequest::create(
                &fedi_decentralized_service_manifold_guardian::CreateSeatRequest {
                    ts: now(),
                    fa_id: owner_id,
                    quote,
                    payment_signatures: Vec::new(),
                },
                &owner_key,
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .verify(&rpc.signing_key.x_only_public_key().0)
        .unwrap()
        .into_inner();
    let CreateSeatOutcome::Accepted { seat_id, .. } = created.outcome else {
        panic!("free seat was refused")
    };
    (rpc, owner_key, owner_id, seat_id)
}

fn endpoint_setup(index: usize) -> fedimint_core::setup_code::PeerSetupCode {
    use fedimint_core::setup_code::{PeerEndpoints, PeerSetupCode};

    let index = u8::try_from(index).expect("test endpoint index fits u8");
    PeerSetupCode {
        name: format!("guardian-{index:02}"),
        endpoints: PeerEndpoints::Iroh {
            api_pk: iroh_base_035::SecretKey::from_bytes(&[0x60 + index; 32]).public(),
            p2p_pk: iroh_base_035::SecretKey::from_bytes(&[0x70 + index; 32]).public(),
        },
        federation_name: None,
        disable_base_fees: None,
        enabled_modules: None,
        federation_size: None,
        fedimint_version: "0.12.0".to_owned(),
        network: bitcoin::Network::Regtest,
    }
}

fn bare_dkg_code(setup: fedimint_core::setup_code::PeerSetupCode) -> GuardianCode {
    use fedimint_core::base32::{self, FEDIMINT_PREFIX};

    GuardianCode(base32::encode_prefixed(FEDIMINT_PREFIX, &setup))
}

async fn valid_dkg_codes(
    rpc: &ManifoldGuardianRpc,
    owner_key: &Keypair,
    owner_id: FaId,
    seat_id: &SeatId,
) -> Vec<GuardianCode> {
    rpc.fleet
        .config()
        .process_spawner
        .fake()
        .configure(seat_id, FakeApiState::default())
        .await;
    let own_code = rpc
        .get_dkg_code(
            SignedRequest::create(
                &GetDkgCodeRequest {
                    ts: now(),
                    fa_id: owner_id,
                    seat_id: seat_id.clone(),
                    federation_name: None,
                },
                owner_key,
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .guardian_code;
    let mut codes = vec![own_code];
    codes.extend((1..7).map(|index| bare_dkg_code(endpoint_setup(index))));
    codes
}

#[tokio::test]
async fn start_dkg_ignores_callback_when_no_push_gateway_is_configured() {
    let temp = TempDir::new().unwrap();
    let (rpc, owner_key, owner_id, seat_id) = rpc_with_owned_seat(&temp, None, 29_000).await;
    let guardian_codes = valid_dkg_codes(&rpc, &owner_key, owner_id, &seat_id).await;
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: "https://push.example/hooks/id/secret".to_owned(),
        idempotency_key: "formation-dkg-complete".to_owned(),
    })
    .unwrap();

    rpc.start_dkg(
        SignedRequest::create(
            &StartDkgRequest {
                ts: now(),
                fa_id: owner_id,
                seat_id: seat_id.clone(),
                guardian_codes,
                completion_callback: Some(callback),
            },
            &owner_key,
        )
        .unwrap(),
    )
    .await
    .unwrap();

    let row = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT completion_callback, completion_callback_status \
         FROM completion_callbacks WHERE quote_id = ?",
    )
    .bind(seat_id.as_bytes().as_slice())
    .fetch_one(&rpc.fleet.database_pool())
    .await
    .unwrap();
    assert_eq!(row, (None, "not_configured".to_owned()));
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn start_dkg_rejects_off_origin_callback_when_gateway_is_configured() {
    let temp = TempDir::new().unwrap();
    let origin =
        PushGatewayOrigin::parse("https://push.example/", PushGatewayOriginPolicy::HttpsOnly)
            .unwrap();
    let (rpc, owner_key, owner_id, seat_id) =
        rpc_with_owned_seat_and_origin(&temp, None, Some(origin), 29_010).await;
    let guardian_codes = valid_dkg_codes(&rpc, &owner_key, owner_id, &seat_id).await;
    let callback = DkgCompletionCallback::new(DkgCompletionCallbackInput {
        callback_url: "https://attacker.example/hooks/id/secret".to_owned(),
        idempotency_key: "formation-dkg-complete".to_owned(),
    })
    .unwrap();

    let error = rpc
        .start_dkg(
            SignedRequest::create(
                &StartDkgRequest {
                    ts: now(),
                    fa_id: owner_id,
                    seat_id,
                    guardian_codes,
                    completion_callback: Some(callback),
                },
                &owner_key,
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, ManifoldGuardianError::InvalidDkgInput(message)
            if message.starts_with("invalid DKG completion callback:"))
    );
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn release_size_capabilities_are_advertised_and_enforced() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp, 29_020).await;
    let availability = rpc
        .get_availability(GetAvailabilityRequest {})
        .await
        .unwrap();
    assert_eq!(
        availability.federation_sizes,
        FEDERATION_SIZES_0_1.map(FederationSize)
    );
    assert!(availability.federation_sizes.contains(&FederationSize(8)));
    assert!(availability.federation_sizes.contains(&FederationSize(20)));

    let fa_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    for size in FEDERATION_SIZES_0_1 {
        rpc.get_quote(GetQuoteRequest {
            fa_id: FaId(fa_key.x_only_public_key().0),
            fedimintd_version: supported_fedimintd_version(),
            federation_size: FederationSize(size),
            plan: test_plan(),
            payment_federation_id: None,
            refund_issuance: None,
        })
        .await
        .unwrap_or_else(|error| panic!("size {size} must be accepted: {error}"));
    }
    for size in [6, 21] {
        assert_eq!(
            rpc.get_quote(GetQuoteRequest {
                fa_id: FaId(fa_key.x_only_public_key().0),
                fedimintd_version: supported_fedimintd_version(),
                federation_size: FederationSize(size),
                plan: test_plan(),
                payment_federation_id: None,
                refund_issuance: None,
            })
            .await
            .unwrap_err(),
            ManifoldGuardianError::UnsupportedFederationSize
        );
    }
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn free_quote_creates_seat_and_replays_the_same_acceptance() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp, 29_030).await;
    let fa_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let fa_id = FaId(fa_key.x_only_public_key().0);
    let quote = rpc
        .get_quote(GetQuoteRequest {
            fa_id,
            fedimintd_version: supported_fedimintd_version(),
            federation_size: FederationSize(7),
            plan: test_plan(),
            payment_federation_id: None,
            refund_issuance: None,
        })
        .await
        .unwrap();
    quote
        .clone()
        .verify(&rpc.signing_key.x_only_public_key().0)
        .unwrap();
    let request = fedi_decentralized_service_manifold_guardian::CreateSeatRequest {
        ts: now(),
        fa_id,
        quote,
        payment_signatures: Vec::new(),
    };
    let first = rpc
        .create_seat(SignedRequest::create(&request, &fa_key).unwrap())
        .await
        .unwrap();
    let replay = rpc
        .create_seat(SignedRequest::create(&request, &fa_key).unwrap())
        .await
        .unwrap();
    // A commitment is a function of the acceptance, not a stored blob: the
    // replay re-signs, so the payload is identical and the signature need not
    // be (ARCH-manifold-guardian-storage).
    assert_eq!(first.as_parts().0, replay.as_parts().0);
    replay
        .verify(&rpc.signing_key.x_only_public_key().0)
        .unwrap();
    let accepted = first
        .verify(&rpc.signing_key.x_only_public_key().0)
        .unwrap()
        .into_inner();
    let CreateSeatOutcome::Accepted {
        seat_id,
        guardian_fee_account,
    } = accepted.outcome
    else {
        panic!("free seat was refused")
    };
    assert_eq!(
        guardian_fee_account.as_account(),
        &rpc.fleet.guardian_fee_account_descriptor(&seat_id),
        "the signed acceptance commits the mnemonic-derived account for this exact seat"
    );
    assert_eq!(
        rpc.get_quote(GetQuoteRequest {
            fa_id,
            fedimintd_version: supported_fedimintd_version(),
            federation_size: FederationSize(7),
            plan: test_plan(),
            payment_federation_id: None,
            refund_issuance: None,
        })
        .await
        .unwrap_err(),
        ManifoldGuardianError::CapacityExhausted
    );
    rpc.fleet.shutdown().await;
}

/// A seat given away settles against nothing, so terms naming a payment
/// federation or refund outputs are not expressible for it: refuse rather than
/// quote something the requester did not ask for.
#[tokio::test]
async fn a_give_away_refuses_payment_material() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp, 29_040).await;
    let fa_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let request = GetQuoteRequest {
        fa_id: FaId(fa_key.x_only_public_key().0),
        fedimintd_version: supported_fedimintd_version(),
        federation_size: FederationSize(7),
        plan: test_plan(),
        payment_federation_id: Some(fedi_decentralized_service_manifold_guardian::FederationId(
            "some-federation".to_owned(),
        )),
        refund_issuance: None,
    };
    assert!(matches!(
        rpc.get_quote(request).await,
        Err(ManifoldGuardianError::PlanNotOffered)
    ));
}

#[tokio::test]
async fn a_priced_quote_refuses_a_federation_outside_the_accepted_set() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp, 29_050).await;
    let price = Msats(1_000);
    rpc.fleet.set_offered_price(Some(price)).await.unwrap();
    let fa_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let request = GetQuoteRequest {
        fa_id: FaId(fa_key.x_only_public_key().0),
        fedimintd_version: supported_fedimintd_version(),
        federation_size: FederationSize(7),
        plan: Plan::InfiniteBestEffort {
            price_msats: price.0,
        },
        payment_federation_id: Some(fedi_decentralized_service_manifold_guardian::FederationId(
            "not-in-the-common-set".to_owned(),
        )),
        refund_issuance: None,
    };
    // The accepted set is empty here, so any named federation is outside it —
    // as is naming none at all for a priced seat.
    assert!(matches!(
        rpc.get_quote(request.clone()).await,
        Err(ManifoldGuardianError::PaymentFederationNotAccepted)
    ));
    assert!(matches!(
        rpc.get_quote(GetQuoteRequest {
            payment_federation_id: None,
            ..request
        })
        .await,
        Err(ManifoldGuardianError::PaymentFederationNotAccepted)
    ));
}

#[tokio::test]
async fn quote_epoch_changes_only_when_quote_settings_change() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp, 29_060).await;
    let fa_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let fa_id = FaId(fa_key.x_only_public_key().0);
    let request = GetQuoteRequest {
        fa_id,
        fedimintd_version: supported_fedimintd_version(),
        federation_size: FederationSize(7),
        plan: test_plan(),
        payment_federation_id: None,
        refund_issuance: None,
    };
    let quote = rpc.get_quote(request.clone()).await.unwrap();
    let epoch = quote
        .clone()
        .verify(&rpc.signing_key.x_only_public_key().0)
        .unwrap()
        .terms
        .offer_epoch;
    rpc.fleet.set_offered_price(Some(TEST_PRICE)).await.unwrap();
    assert_eq!(rpc.fleet.quote_offer().await.unwrap().epoch, epoch);

    rpc.fleet
        .set_offered_price(Some(Msats(1_000)))
        .await
        .unwrap();
    assert_ne!(rpc.fleet.quote_offer().await.unwrap().epoch, epoch);
    let response = rpc
        .create_seat(
            SignedRequest::create(
                &CreateSeatRequest {
                    ts: now(),
                    fa_id,
                    quote,
                    payment_signatures: Vec::new(),
                },
                &fa_key,
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .verify(&rpc.signing_key.x_only_public_key().0)
        .unwrap();
    assert!(matches!(
        response.outcome,
        CreateSeatOutcome::Refused {
            reason: RefusalReason::OfferChanged,
            ..
        }
    ));
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn create_seat_rejects_quote_bound_to_another_fa() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp, 29_070).await;
    let quoted_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let caller_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let quote = rpc
        .get_quote(GetQuoteRequest {
            fa_id: FaId(quoted_key.x_only_public_key().0),
            fedimintd_version: supported_fedimintd_version(),
            federation_size: FederationSize(7),
            plan: test_plan(),
            payment_federation_id: None,
            refund_issuance: None,
        })
        .await
        .unwrap();
    let request = fedi_decentralized_service_manifold_guardian::CreateSeatRequest {
        ts: now(),
        fa_id: FaId(caller_key.x_only_public_key().0),
        quote,
        payment_signatures: Vec::new(),
    };
    assert_eq!(
        rpc.create_seat(SignedRequest::create(&request, &caller_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::InvalidPayment
    );
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn create_seat_rejects_a_signed_incoherent_quote() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp, 29_080).await;
    let fa_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let fa_id = FaId(fa_key.x_only_public_key().0);
    let quote = SignedResponse::create(
        &GetQuoteResponse {
            terms: QuoteTerms {
                quote_nonce: [1; 32],
                offer_epoch: OfferEpoch::from_bytes([0; 32]),
                request: GetQuoteRequest {
                    fa_id,
                    fedimintd_version: supported_fedimintd_version(),
                    federation_size: FederationSize(7),
                    plan: test_plan(),
                    payment_federation_id: None,
                    refund_issuance: None,
                },
                // A manager signature authenticates these bytes but must not
                // make an incoherent price/payment pair redeemable.
                price_msats: 1,
                payment: None,
            },
        },
        &rpc.signing_key,
    )
    .unwrap();
    let request = CreateSeatRequest {
        ts: now(),
        fa_id,
        quote,
        payment_signatures: Vec::new(),
    };

    assert_eq!(
        rpc.create_seat(SignedRequest::create(&request, &fa_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::InvalidPayment
    );
    assert!(rpc.fleet.seat_summaries().await.unwrap().is_empty());
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn wrong_owner_precedes_policy_and_unsupported_results() {
    let temp = TempDir::new().unwrap();
    let (rpc, victim_key, victim_id, seat_id) = rpc_with_owned_seat(&temp, None, 29_090).await;

    let attacker_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let attacker_id = FaId(attacker_key.x_only_public_key().0);

    let remove = RemoveSeatRequest {
        ts: now(),
        fa_id: attacker_id,
        seat_id: seat_id.clone(),
    };
    assert_eq!(
        rpc.remove_seat(SignedRequest::create(&remove, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat,
    );
    let transfer = SeatTransferGrant {
        ts: now(),
        fa_id: attacker_id,
        federation_id: fedi_decentralized_service_manifold_guardian::FederationId("test".into()),
        source: fedi_decentralized_service_manifold_guardian::Locator::new(
            fedi_iroh_rpc::iroh::EndpointAddr::new(
                fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[71; 32]).public(),
            ),
            rpc.fleet.identity().derive_service_pubkey(),
        ),
        source_seat_id: seat_id.clone(),
        destination_service_pubkey: attacker_key.x_only_public_key().0,
        destination_seat_id: seat_id.clone(),
    };
    let pull = TransferSeatRequest {
        ts: now(),
        destination_service_pubkey: attacker_key.x_only_public_key().0,
        grant: SignedRequest::create(&transfer, &attacker_key).unwrap(),
    };
    assert_eq!(
        rpc.transfer_seat(MgSignedRequest::create(&pull, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat,
    );

    let get_code = GetDkgCodeRequest {
        ts: now(),
        fa_id: attacker_id,
        seat_id: seat_id.clone(),
        federation_name: Some(FederationName("victim federation".to_owned())),
    };
    assert_eq!(
        rpc.get_dkg_code(SignedRequest::create(&get_code, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat
    );

    let restart = RestartDkgRequest {
        ts: now(),
        fa_id: attacker_id,
        seat_id: seat_id.clone(),
        guardian_codes: Vec::new(),
    };
    assert_eq!(
        rpc.restart_dkg(SignedRequest::create(&restart, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat
    );

    let start = StartDkgRequest {
        ts: now(),
        fa_id: attacker_id,
        seat_id: seat_id.clone(),
        guardian_codes: Vec::new(),
        completion_callback: None,
    };
    assert_eq!(
        rpc.start_dkg(SignedRequest::create(&start, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat
    );

    let status = GetStatusRequest {
        ts: now(),
        fa_id: attacker_id,
        seat_id: seat_id.clone(),
    };
    assert_eq!(
        rpc.get_status(SignedRequest::create(&status, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat
    );

    let invite = GetInviteCodeRequest {
        ts: now(),
        fa_id: attacker_id,
        seat_id: seat_id.clone(),
    };
    assert_eq!(
        rpc.get_invite_code(SignedRequest::create(&invite, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat
    );

    // Once ownership succeeds, malformed human-facing names remain typed
    // policy errors and are rejected before the unavailable child is touched.
    let invalid_name = GetDkgCodeRequest {
        ts: now(),
        fa_id: victim_id,
        seat_id: seat_id.clone(),
        federation_name: Some(FederationName("federation\nname".to_owned())),
    };
    assert!(matches!(
        rpc.get_dkg_code(SignedRequest::create(&invalid_name, &victim_key).unwrap())
            .await,
        Err(ManifoldGuardianError::InvalidDkgInput(_))
    ));

    let peer_attestation = GetPeerAttestationRequest {
        ts: now(),
        fa_id: attacker_id,
        seat_id: seat_id.clone(),
    };
    assert_eq!(
        rpc.get_peer_attestation(SignedRequest::create(&peer_attestation, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat
    );

    let set_meta = SetMetaFieldRequest {
        ts: now(),
        fa_id: attacker_id,
        seat_id: seat_id.clone(),
        expected_base: MetaConsensusBase::Absent,
        key: MetaFieldKey("fedi:test".to_owned()),
        value: MetaFieldValue("value".to_owned()),
    };
    assert_eq!(
        rpc.set_meta_field(SignedRequest::create(&set_meta, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat
    );

    let formation_meta = ProposeFormationMetaRequest {
        ts: now(),
        fa_id: attacker_id,
        seat_id: seat_id.clone(),
        expected_base: MetaConsensusBase::Absent,
        seat_bindings: vec![],
        fa_fee_account: GuardianFeeAccount::try_from(
            rpc.fleet.guardian_fee_account_descriptor(&seat_id),
        )
        .unwrap(),
        guardian_verification_fee_account: GuardianFeeAccount::try_from(
            rpc.fleet.guardian_fee_account_descriptor(&seat_id),
        )
        .unwrap(),
        send_ppm: 5_000,
    };
    assert_eq!(
        rpc.propose_formation_meta(SignedRequest::create(&formation_meta, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat
    );

    let register_gateway = RegisterGatewayRequest {
        ts: now(),
        fa_id: attacker_id,
        seat_id: seat_id.clone(),
        gateway_api: GatewayApiUrl::try_from("https://gateway.example/").unwrap(),
    };
    assert_eq!(
        rpc.register_gateway(SignedRequest::create(&register_gateway, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat
    );

    let owner_account = rpc.fleet.guardian_fee_account_descriptor(&seat_id);
    let production_formation_request = ProposeFormationMetaRequest {
        ts: now(),
        fa_id: victim_id,
        seat_id: seat_id.clone(),
        expected_base: MetaConsensusBase::Absent,
        seat_bindings: vec![],
        fa_fee_account: GuardianFeeAccount::try_from(owner_account.clone()).unwrap(),
        guardian_verification_fee_account: GuardianFeeAccount::try_from(owner_account).unwrap(),
        send_ppm: 5_000,
    };
    assert_eq!(
        rpc.propose_formation_meta(
            SignedRequest::create(&production_formation_request, &victim_key).unwrap(),
        )
        .await
        .unwrap_err(),
        ManifoldGuardianError::GuardianVerificationFeeAccountUnavailable,
        "an unconfigured Guardian Verification Fee account fails closed before child access",
    );

    let configured_account = Account::single(
        bitcoin::secp256k1::PublicKey::from_secret_key(
            bitcoin::secp256k1::SECP256K1,
            &bitcoin::secp256k1::SecretKey::from_slice(&[29; 32]).unwrap(),
        ),
        stability_pool_client::common::AccountType::BtcDepositor,
    );
    let configured_temp = TempDir::new().unwrap();
    let (configured_rpc, configured_owner_key, configured_owner_id, configured_seat_id) =
        rpc_with_owned_seat(&configured_temp, Some(configured_account), 29_100).await;
    let configured_owner_account = configured_rpc
        .fleet
        .guardian_fee_account_descriptor(&configured_seat_id);
    let mismatched_formation_request = ProposeFormationMetaRequest {
        ts: now(),
        fa_id: configured_owner_id,
        seat_id: configured_seat_id,
        expected_base: MetaConsensusBase::Absent,
        seat_bindings: vec![],
        fa_fee_account: GuardianFeeAccount::try_from(configured_owner_account.clone()).unwrap(),
        guardian_verification_fee_account: GuardianFeeAccount::try_from(configured_owner_account)
            .unwrap(),
        send_ppm: 5_000,
    };
    assert_eq!(
        configured_rpc
            .propose_formation_meta(
                SignedRequest::create(&mismatched_formation_request, &configured_owner_key)
                    .unwrap(),
            )
            .await
            .unwrap_err(),
        ManifoldGuardianError::GuardianVerificationFeeAccountMismatch,
        "a mismatched stated account fails before the seat child or vote path",
    );

    let stats = GetFedimintStatsRequest {
        ts: now(),
        fa_id: attacker_id,
        seat_id,
    };
    assert_eq!(
        rpc.get_fedimint_stats(SignedRequest::create(&stats, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        ManifoldGuardianError::UnknownSeat
    );
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn transfer_source_and_removal_validate_requests_on_bitcoin() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc_with_network(&temp, None, None, bitcoin::Network::Bitcoin, 29_110).await;
    let key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let fa_id = FaId(key.x_only_public_key().0);
    let seat_id = SeatId::from(fedi_decentralized_service_manifold_guardian::QuoteId(
        [72; 32],
    ));
    let remove = RemoveSeatRequest {
        ts: Timestamp(0),
        fa_id,
        seat_id: seat_id.clone(),
    };
    assert!(matches!(
        rpc.remove_seat(SignedRequest::create(&remove, &key).unwrap())
            .await,
        Err(ManifoldGuardianError::Unauthorized),
    ));
    let transfer = SeatTransferGrant {
        ts: Timestamp(0),
        fa_id,
        federation_id: fedi_decentralized_service_manifold_guardian::FederationId("test".into()),
        source: fedi_decentralized_service_manifold_guardian::Locator::new(
            fedi_iroh_rpc::iroh::EndpointAddr::new(
                fedi_iroh_rpc::iroh::SecretKey::from_bytes(&[73; 32]).public(),
            ),
            rpc.fleet.identity().derive_service_pubkey(),
        ),
        source_seat_id: seat_id.clone(),
        destination_service_pubkey: key.x_only_public_key().0,
        destination_seat_id: seat_id,
    };
    let pull = TransferSeatRequest {
        ts: Timestamp(0),
        destination_service_pubkey: key.x_only_public_key().0,
        grant: SignedRequest::create(&transfer, &key).unwrap(),
    };
    assert!(matches!(
        rpc.transfer_seat(MgSignedRequest::create(&pull, &key).unwrap())
            .await,
        Err(ManifoldGuardianError::Unauthorized),
    ));
    rpc.fleet.shutdown().await;
}

/// A trust-material source with no relay behind it.
struct FakeTrustMaterialSource {
    authorizations: Vec<fedi_decentralized_domain::HolderAuthorizationEnvelope>,
}

impl crate::service::TrustMaterialSource for FakeTrustMaterialSource {
    fn iroh_endpoint_url(&self) -> fedi_decentralized_domain::Url {
        fedi_decentralized_domain::Url("iroh://test-endpoint".to_owned())
    }

    fn holder_authorizations(&self) -> Vec<fedi_decentralized_domain::HolderAuthorizationEnvelope> {
        self.authorizations.clone()
    }
}

fn trust_material_request()
-> fedi_decentralized_service_manifold_guardian::GetMgTrustMaterialRequest {
    fedi_decentralized_service_manifold_guardian::GetMgTrustMaterialRequest {
        version: fedi_decentralized_domain::ProtocolV1,
    }
}

#[tokio::test]
async fn trust_material_is_unsupported_until_a_source_is_bound() {
    // Before the runtime source is bound, MG has no complete holder
    // authorization input. Answering with an empty document would let a verifier
    // read "not participating" as "participating but untrusted".
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp, 29_120).await;

    let error = rpc
        .get_fman_trust_material(trust_material_request())
        .await
        .unwrap_err();

    assert!(
        matches!(error, ManifoldGuardianError::UnsupportedVerb { .. }),
        "expected UnsupportedVerb, got {error:?}"
    );
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn trust_material_is_signed_for_the_mg_identity() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp, 29_130).await;
    rpc.bind_trust_material_source(Arc::new(FakeTrustMaterialSource {
        authorizations: vec![],
    }));

    let response = rpc
        .get_fman_trust_material(trust_material_request())
        .await
        .unwrap();
    let expected = fedi_decentralized_domain::Pubkey(rpc.attestation_keys.public_key().to_string());
    let now = fedi_decentralized_domain::Timestamp(response.material.issued_at.0);
    response
        .verify_for_mg(&expected, now, 3600)
        .expect("the MG's own response verifies for its consensus-listed identity");

    rpc.fleet.shutdown().await;
}
