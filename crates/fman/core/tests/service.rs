use fedi_decentralized_service_fleet_manager::{
    CreateSeatOutcome, DkgCompletionCallback, FederationName, FederationSize, FiId,
    GetDkgCodeRequest, GetFedimintStatsRequest, GetPeerAttestationRequest, GetQuoteRequest,
    GetQuoteResponse, GuardianFeeAccount, MetaConsensusBase, MetaFieldKey, MetaFieldValue,
    OfferEpoch, Plan, ProposeFormationMetaRequest, RefusalReason, RestartDkgRequest,
    SetMetaFieldRequest, StartDkgRequest,
};
use tempfile::TempDir;

use super::*;
use crate::facts::PortBase;
use crate::fleet::FleetConfig;
use crate::seat_process::SeatProcessSpawner;
use crate::seat_process::fake::{block_forever, write_fake_fedimintd};
use crate::seat_process::{BitcoindConfig, RespawnPolicy, SeatProcessConfig};
use crate::wallet::NoWallet;
use fedi_decentralized_service_fleet_manager::DkgCompletionCallbackInput;

async fn rpc(temp: &TempDir) -> FleetManagerRpc {
    // A fleet opens against an identity onboarding already chose; nothing
    // mints one on open.
    let db = crate::db::Db::open(temp.path()).await.unwrap();
    crate::onboarding::onboard_as_new(&db).await.unwrap();
    db.complete_onboarding_for_test(1).await.unwrap();

    let fleet = Fleet::open(
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
                fedimintd: write_fake_fedimintd(temp.path(), &block_forever()).await,
                bitcoin_network: bitcoin::Network::Regtest,
                iroh_dns: "https://dns.iroh.link/pkarr".parse().unwrap(),
                bitcoin_backend: crate::seat_process::BitcoinBackend::Bitcoind(BitcoindConfig {
                    url: "http://127.0.0.1:18443".to_owned(),
                    username: "user".to_owned(),
                    password: "pass".to_owned(),
                }),
            },
        },
        Arc::new(NoWallet),
    )
    .await
    .unwrap();
    fleet.set_offered_price(Some(TEST_PRICE)).await.unwrap();
    FleetManagerRpc::new(Arc::new(fleet), None, tokio::sync::watch::channel(None).1)
}

/// The offer these tests quote against: priced at zero, so a quote needs no
/// payment federation and no wallet.
const TEST_PRICE: Msats = Msats(0);

fn test_plan() -> Plan {
    Plan::InfiniteBestEffort {
        price_msats: TEST_PRICE.0,
    }
}

#[tokio::test]
async fn release_size_capabilities_are_advertised_and_enforced() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp).await;
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

    let fi_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    for size in FEDERATION_SIZES_0_1 {
        rpc.get_quote(GetQuoteRequest {
            fi_id: FiId(fi_key.x_only_public_key().0),
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
                fi_id: FiId(fi_key.x_only_public_key().0),
                fedimintd_version: supported_fedimintd_version(),
                federation_size: FederationSize(size),
                plan: test_plan(),
                payment_federation_id: None,
                refund_issuance: None,
            })
            .await
            .unwrap_err(),
            FleetManagerError::UnsupportedFederationSize
        );
    }
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn free_quote_creates_seat_and_replays_the_same_acceptance() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp).await;
    let fi_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let fi_id = FiId(fi_key.x_only_public_key().0);
    let quote = rpc
        .get_quote(GetQuoteRequest {
            fi_id,
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
    let request = fedi_decentralized_service_fleet_manager::CreateSeatRequest {
        ts: now(),
        fi_id,
        quote,
        payment_signatures: Vec::new(),
    };
    let first = rpc
        .create_seat(SignedRequest::create(&request, &fi_key).unwrap())
        .await
        .unwrap();
    let replay = rpc
        .create_seat(SignedRequest::create(&request, &fi_key).unwrap())
        .await
        .unwrap();
    // A commitment is a function of the acceptance, not a stored blob: the
    // replay re-signs, so the payload is identical and the signature need not
    // be (ARCH-fleet-manager-storage).
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
            fi_id,
            fedimintd_version: supported_fedimintd_version(),
            federation_size: FederationSize(7),
            plan: test_plan(),
            payment_federation_id: None,
            refund_issuance: None,
        })
        .await
        .unwrap_err(),
        FleetManagerError::CapacityExhausted
    );
    rpc.fleet.shutdown().await;
}

/// A seat given away settles against nothing, so terms naming a payment
/// federation or refund outputs are not expressible for it: refuse rather than
/// quote something the requester did not ask for.
#[tokio::test]
async fn a_give_away_refuses_payment_material() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp).await;
    let fi_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let request = GetQuoteRequest {
        fi_id: FiId(fi_key.x_only_public_key().0),
        fedimintd_version: supported_fedimintd_version(),
        federation_size: FederationSize(7),
        plan: test_plan(),
        payment_federation_id: Some(fedi_decentralized_service_fleet_manager::FederationId(
            "some-federation".to_owned(),
        )),
        refund_issuance: None,
    };
    assert!(matches!(
        rpc.get_quote(request).await,
        Err(FleetManagerError::PlanNotOffered)
    ));
}

#[tokio::test]
async fn a_priced_quote_refuses_a_federation_outside_the_accepted_set() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp).await;
    let price = Msats(1_000);
    rpc.fleet.set_offered_price(Some(price)).await.unwrap();
    let fi_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let request = GetQuoteRequest {
        fi_id: FiId(fi_key.x_only_public_key().0),
        fedimintd_version: supported_fedimintd_version(),
        federation_size: FederationSize(7),
        plan: Plan::InfiniteBestEffort {
            price_msats: price.0,
        },
        payment_federation_id: Some(fedi_decentralized_service_fleet_manager::FederationId(
            "not-in-the-common-set".to_owned(),
        )),
        refund_issuance: None,
    };
    // The accepted set is empty here, so any named federation is outside it —
    // as is naming none at all for a priced seat.
    assert!(matches!(
        rpc.get_quote(request.clone()).await,
        Err(FleetManagerError::PaymentFederationNotAccepted)
    ));
    assert!(matches!(
        rpc.get_quote(GetQuoteRequest {
            payment_federation_id: None,
            ..request
        })
        .await,
        Err(FleetManagerError::PaymentFederationNotAccepted)
    ));
}

#[tokio::test]
async fn quote_epoch_changes_only_when_quote_settings_change() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp).await;
    let fi_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let fi_id = FiId(fi_key.x_only_public_key().0);
    let request = GetQuoteRequest {
        fi_id,
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
                    fi_id,
                    quote,
                    payment_signatures: Vec::new(),
                },
                &fi_key,
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
async fn create_seat_rejects_quote_bound_to_another_fi() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp).await;
    let quoted_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let caller_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let quote = rpc
        .get_quote(GetQuoteRequest {
            fi_id: FiId(quoted_key.x_only_public_key().0),
            fedimintd_version: supported_fedimintd_version(),
            federation_size: FederationSize(7),
            plan: test_plan(),
            payment_federation_id: None,
            refund_issuance: None,
        })
        .await
        .unwrap();
    let request = fedi_decentralized_service_fleet_manager::CreateSeatRequest {
        ts: now(),
        fi_id: FiId(caller_key.x_only_public_key().0),
        quote,
        payment_signatures: Vec::new(),
    };
    assert_eq!(
        rpc.create_seat(SignedRequest::create(&request, &caller_key).unwrap())
            .await
            .unwrap_err(),
        FleetManagerError::InvalidPayment
    );
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn create_seat_rejects_a_signed_incoherent_quote() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp).await;
    let fi_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let fi_id = FiId(fi_key.x_only_public_key().0);
    let quote = SignedResponse::create(
        &GetQuoteResponse {
            terms: QuoteTerms {
                quote_nonce: [1; 32],
                offer_epoch: OfferEpoch::from_bytes([0; 32]),
                request: GetQuoteRequest {
                    fi_id,
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
        fi_id,
        quote,
        payment_signatures: Vec::new(),
    };

    assert_eq!(
        rpc.create_seat(SignedRequest::create(&request, &fi_key).unwrap())
            .await
            .unwrap_err(),
        FleetManagerError::InvalidPayment
    );
    assert!(rpc.fleet.seat_summaries().await.unwrap().is_empty());
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn wrong_owner_precedes_policy_and_unsupported_results() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp).await;
    let victim_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let victim_id = FiId(victim_key.x_only_public_key().0);
    let quote = rpc
        .get_quote(GetQuoteRequest {
            fi_id: victim_id,
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
                &fedi_decentralized_service_fleet_manager::CreateSeatRequest {
                    ts: now(),
                    fi_id: victim_id,
                    quote,
                    payment_signatures: Vec::new(),
                },
                &victim_key,
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

    let attacker_key = Keypair::new(secp256k1::SECP256K1, &mut rand::thread_rng());
    let attacker_id = FiId(attacker_key.x_only_public_key().0);

    let get_code = GetDkgCodeRequest {
        ts: now(),
        fi_id: attacker_id,
        seat_id: seat_id.clone(),
        federation_name: Some(FederationName("victim federation".to_owned())),
    };
    assert_eq!(
        rpc.get_dkg_code(SignedRequest::create(&get_code, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        FleetManagerError::UnknownSeat
    );

    let restart = RestartDkgRequest {
        ts: now(),
        fi_id: attacker_id,
        seat_id: seat_id.clone(),
        guardian_codes: Vec::new(),
    };
    assert_eq!(
        rpc.restart_dkg(SignedRequest::create(&restart, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        FleetManagerError::UnknownSeat
    );

    // Once ownership succeeds, malformed human-facing names remain typed
    // policy errors and are rejected before the unavailable child is touched.
    let invalid_name = GetDkgCodeRequest {
        ts: now(),
        fi_id: victim_id,
        seat_id: seat_id.clone(),
        federation_name: Some(FederationName("federation\nname".to_owned())),
    };
    assert!(matches!(
        rpc.get_dkg_code(SignedRequest::create(&invalid_name, &victim_key).unwrap())
            .await,
        Err(FleetManagerError::InvalidDkgInput(_))
    ));

    let callback_start = StartDkgRequest {
        ts: now(),
        fi_id: victim_id,
        seat_id: seat_id.clone(),
        guardian_codes: Vec::new(),
        completion_callback: Some(
            DkgCompletionCallback::new(DkgCompletionCallbackInput {
                callback_url: "https://attacker.example/hooks/id/secret".to_owned(),
                idempotency_key: "formation-dkg-complete".to_owned(),
            })
            .unwrap(),
        ),
    };
    assert!(matches!(
        rpc.start_dkg(SignedRequest::create(&callback_start, &victim_key).unwrap())
            .await,
        Err(FleetManagerError::InvalidDkgInput(_))
    ));

    let peer_attestation = GetPeerAttestationRequest {
        ts: now(),
        fi_id: attacker_id,
        seat_id: seat_id.clone(),
    };
    assert_eq!(
        rpc.get_peer_attestation(SignedRequest::create(&peer_attestation, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        FleetManagerError::UnknownSeat
    );

    let set_meta = SetMetaFieldRequest {
        ts: now(),
        fi_id: attacker_id,
        seat_id: seat_id.clone(),
        expected_base: MetaConsensusBase::Absent,
        key: MetaFieldKey("fedi:test".to_owned()),
        value: MetaFieldValue("value".to_owned()),
    };
    assert_eq!(
        rpc.set_meta_field(SignedRequest::create(&set_meta, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        FleetManagerError::UnknownSeat
    );

    let formation_meta = ProposeFormationMetaRequest {
        ts: now(),
        fi_id: attacker_id,
        seat_id: seat_id.clone(),
        expected_base: MetaConsensusBase::Absent,
        seat_bindings: vec![],
        fi_fee_account: GuardianFeeAccount::try_from(
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
        FleetManagerError::UnknownSeat
    );

    let owner_account = rpc.fleet.guardian_fee_account_descriptor(&seat_id);
    let production_formation_request = ProposeFormationMetaRequest {
        ts: now(),
        fi_id: victim_id,
        seat_id: seat_id.clone(),
        expected_base: MetaConsensusBase::Absent,
        seat_bindings: vec![],
        fi_fee_account: GuardianFeeAccount::try_from(owner_account.clone()).unwrap(),
        guardian_verification_fee_account: GuardianFeeAccount::try_from(owner_account).unwrap(),
        send_ppm: 5_000,
    };
    assert_eq!(
        rpc.propose_formation_meta(
            SignedRequest::create(&production_formation_request, &victim_key).unwrap(),
        )
        .await
        .unwrap_err(),
        FleetManagerError::GuardianVerificationFeeAccountUnavailable,
        "an unconfigured Guardian Verification Fee account fails closed before child access",
    );

    let configured_account = Account::single(
        bitcoin::secp256k1::PublicKey::from_secret_key(
            bitcoin::secp256k1::SECP256K1,
            &bitcoin::secp256k1::SecretKey::from_slice(&[29; 32]).unwrap(),
        ),
        stability_pool_client::common::AccountType::BtcDepositor,
    );
    let mut configured_rpc = rpc.clone();
    configured_rpc.guardian_verification_fee_account = Some(configured_account);
    assert_eq!(
        configured_rpc
            .propose_formation_meta(
                SignedRequest::create(&production_formation_request, &victim_key).unwrap(),
            )
            .await
            .unwrap_err(),
        FleetManagerError::GuardianVerificationFeeAccountMismatch,
        "a mismatched stated account fails before the seat child or vote path",
    );

    let stats = GetFedimintStatsRequest {
        ts: now(),
        fi_id: attacker_id,
        seat_id,
    };
    assert_eq!(
        rpc.get_fedimint_stats(SignedRequest::create(&stats, &attacker_key).unwrap())
            .await
            .unwrap_err(),
        FleetManagerError::UnknownSeat
    );
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

fn trust_material_request(
    federation_id: &str,
    config_hash: Vec<u8>,
) -> fedi_decentralized_service_fleet_manager::GetFederationTrustMaterialRequest {
    fedi_decentralized_service_fleet_manager::GetFederationTrustMaterialRequest {
        version: fedi_decentralized_domain::ProtocolV1,
        federation_id: fedi_decentralized_service_fleet_manager::FederationId(
            federation_id.to_owned(),
        ),
        federation_config_hash: fedi_decentralized_domain::HashBytes(config_hash),
        peer_ids: vec![],
    }
}

#[tokio::test]
async fn trust_material_is_unsupported_until_a_source_is_bound() {
    // Before the runtime source is bound, FMan has no complete holder
    // authorization input. Answering with an empty document would let a verifier
    // read "not participating" as "participating but untrusted".
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp).await;

    let error = rpc
        .get_federation_trust_material(trust_material_request("fed", vec![1, 2, 3]))
        .await
        .unwrap_err();

    assert!(
        matches!(error, FleetManagerError::UnsupportedVerb { .. }),
        "expected UnsupportedVerb, got {error:?}"
    );
    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn trust_material_is_signed_and_bound_to_the_requested_federation() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp).await;
    rpc.bind_trust_material_source(Arc::new(FakeTrustMaterialSource {
        authorizations: vec![],
    }));

    let request = trust_material_request("fed-a", vec![1, 2, 3]);
    let response = rpc
        .get_federation_trust_material(request.clone())
        .await
        .unwrap();

    // This FMan runs no seat in that federation, which is a fact about the
    // federation rather than a failure: an empty attestation list, still signed.
    assert!(response.material.peer_attestations.is_empty());

    let now = fedi_decentralized_domain::Timestamp(response.material.issued_at.0);
    response
        .verify_for_request(&request, now, 3600)
        .expect("the FMan's own response verifies for the request it answered");

    // The signature covers the federation binding, so the same response cannot
    // be replayed as an answer about a different federation.
    let other_federation = trust_material_request("fed-b", vec![1, 2, 3]);
    assert!(
        response
            .verify_for_request(&other_federation, now, 3600)
            .is_err(),
        "material must not verify for a federation it does not name"
    );
    let other_config = trust_material_request("fed-a", vec![9, 9, 9]);
    assert!(
        response
            .verify_for_request(&other_config, now, 3600)
            .is_err(),
        "material must not verify for a different config revision"
    );

    rpc.fleet.shutdown().await;
}

#[tokio::test]
async fn trust_material_rejects_a_malformed_request() {
    let temp = TempDir::new().unwrap();
    let rpc = rpc(&temp).await;
    rpc.bind_trust_material_source(Arc::new(FakeTrustMaterialSource {
        authorizations: vec![],
    }));

    // A duplicated peer filter entry is refused by the shared request
    // validator, before any seat state is consulted.
    let mut request = trust_material_request("fed-a", vec![1, 2, 3]);
    request.peer_ids = vec![
        fedi_decentralized_domain::PeerId("0".to_owned()),
        fedi_decentralized_domain::PeerId("0".to_owned()),
    ];

    assert!(
        rpc.get_federation_trust_material(request).await.is_err(),
        "a duplicated peer filter must be refused"
    );
    rpc.fleet.shutdown().await;
}
