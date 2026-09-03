use fedi_decentralized_service_fleet_manager::{
    DkgCompletionCallback, FEDERATION_METADATA_OBJECT_MAX_BYTES, FEDERATION_NAME_META_FIELD_KEY,
    FederationName, FederationSize, FormationSeatBinding, GetQuoteRequest, GuardianCode,
    GuardianFeeAccount, GuardianFeeRecipient, InviteCode, ManagerSignature, MetaConsensusBase,
    MetaFieldKey, MetaFieldValue, OfferEpoch, SeatHealth, ServiceStatus,
};
use stability_pool_client::common::Account;
use tempfile::TempDir;
use tokio::sync::Notify;

use super::*;
use crate::facts::{DkgCodeSet, SeatNo};
use crate::fedimint_api::FedimintApi;
use crate::seat::SeatPhase;
use crate::seat_process::fake::{
    FAKE_LNV2_INSTANCE_ID, FAKE_META_INSTANCE_ID, FakeApiState, FakeDkgStep, FakeSeatChildHandle,
    FakeSeatProcessSpawner, block_forever, write_fake_fedimintd,
};
use crate::seat_process::{BitcoindConfig, seat_data_dir};
use crate::wallet::NoWallet;
use crate::wallet::testutil::GatedRefundWallet;
use fedimint_server::config::driven::{ChildMessage, ChildState, PROTOCOL_VERSION};

fn owned_seat(fleet: &Fleet, fi_id: &FiId, seat_id: &SeatId) -> Result<Arc<Seat>, SeatVerbError> {
    let seat = fleet
        .seat_by_id(seat_id)
        .ok_or(SeatVerbError::UnknownSeat)?;
    if seat.facts().fi_id != *fi_id {
        return Err(SeatVerbError::UnknownSeat);
    }
    Ok(seat)
}

impl Fleet {
    async fn configure_fake_child(
        &self,
        seat_id: &SeatId,
        state: FakeApiState,
    ) -> FakeSeatChildHandle {
        self.config
            .process_spawner
            .fake()
            .configure(seat_id, state)
            .await
    }

    async fn form_fake_child(
        &self,
        seat_id: &SeatId,
        mut state: FakeApiState,
    ) -> FakeSeatChildHandle {
        let already_formed = matches!(
            self.seat_by_id(seat_id).unwrap().report().await.unwrap(),
            SeatReport::Active {
                phase: SeatPhase::Running { .. },
                ..
            }
        );
        state.complete_dkg = true;
        state.consensus_running = already_formed;
        let child = self.configure_fake_child(seat_id, state).await;
        let seat = self.seat_by_id(seat_id).unwrap();
        if !already_formed {
            let codes = scripted_dkg_codes(self, &seat.facts().fi_id, seat_id).await;
            start_dkg(self, &seat.facts().fi_id, seat_id, &codes)
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    if matches!(
                        seat.report().await.unwrap(),
                        SeatReport::Active {
                            phase: SeatPhase::Running { .. },
                            ..
                        }
                    ) {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("fake driven child persists its configuration");
        }
        seat.watchdog_tick_for_test().await;
        child
    }
}

async fn dkg_code(
    fleet: &Fleet,
    fi_id: &FiId,
    seat_id: &SeatId,
    leader: Option<&FederationName>,
) -> Result<GuardianCode, SeatVerbError> {
    let seat = owned_seat(fleet, fi_id, seat_id)?;
    seat.reject_decommissioned()?;
    seat.dkg_code(leader).await
}

async fn start_dkg(
    fleet: &Fleet,
    fi_id: &FiId,
    seat_id: &SeatId,
    codes: &[GuardianCode],
) -> Result<(), SeatVerbError> {
    let seat = owned_seat(fleet, fi_id, seat_id)?;
    seat.reject_decommissioned()?;
    seat.start_dkg(codes, None).await
}

async fn start_dkg_with_callback(
    fleet: &Fleet,
    fi_id: &FiId,
    seat_id: &SeatId,
    codes: &[GuardianCode],
    callback: &DkgCompletionCallback,
) -> Result<(), SeatVerbError> {
    let seat = owned_seat(fleet, fi_id, seat_id)?;
    seat.reject_decommissioned()?;
    let origin = fleet
        .config()
        .push_gateway_origin
        .as_ref()
        .expect("test callback origin");
    let callback = origin.validate(callback).expect("valid test callback");
    seat.start_dkg(codes, Some(callback)).await
}

async fn restart_dkg(
    fleet: &Fleet,
    fi_id: &FiId,
    seat_id: &SeatId,
    codes: &[GuardianCode],
) -> Result<ServiceStatus, SeatVerbError> {
    let seat = owned_seat(fleet, fi_id, seat_id)?;
    seat.reject_decommissioned()?;
    seat.restart_dkg(codes).await
}

fn test_key(name: &str) -> secp256k1::XOnlyPublicKey {
    let mut seed = [0_u8; 32];
    seed[..name.len()].copy_from_slice(name.as_bytes());
    secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &seed)
        .unwrap()
        .x_only_public_key()
        .0
}

fn signature(byte: u8) -> ManagerSignature {
    ManagerSignature(secp256k1::schnorr::Signature::from_byte_array([byte; 64]))
}

fn input(
    offer_epoch: OfferEpoch,
    quote: u8,
    _expires_at: u64,
    price_msats: u64,
    refund: u8,
) -> VerifiedCreateSeat {
    let fi_id = FiId(test_key("fi"));
    VerifiedCreateSeat {
        fi_id,
        quote_id: QuoteId([quote; 32]),
        quote_terms: QuoteTerms {
            quote_nonce: [quote; 32],
            offer_epoch,
            request: GetQuoteRequest {
                fi_id,
                fedimintd_version: "0.0.0-test".parse().expect("valid test version"),
                federation_size: FederationSize(7),
                plan: Plan::InfiniteBestEffort { price_msats: 0 },
                payment_federation_id: None,
                refund_issuance: None,
            },
            price_msats,
            payment: None,
        },
        payment: if price_msats != 0 {
            VerifiedPayment::TestRefund {
                federation_id: FederationId("test-payment-federation".to_owned()),
                transaction: RefundTransaction(vec![refund; 32]),
            }
        } else {
            VerifiedPayment::Free
        },
    }
}

async fn current_input(
    fleet: &Fleet,
    quote: u8,
    expires_at: u64,
    price_msats: u64,
    refund: u8,
) -> VerifiedCreateSeat {
    input(
        fleet.db.offer_epoch().await.unwrap(),
        quote,
        expires_at,
        price_msats,
        refund,
    )
}

/// Tests that probe or fake a child's API must use disjoint
/// `first_port_base` values: the lib tests run concurrently in one
/// process, so a shared base lets one test's fake answer another
/// test's probe.
/// A data root a fleet can be opened against: onboarded, as a real one is
/// before its daemon reaches [`Fleet::open`].
async fn config(temp: &TempDir, max_seats: u32, first_port_base: u16) -> FleetConfig {
    let db = Db::open(temp.path()).await.unwrap();
    if db.load_identity().await.unwrap().is_none() {
        crate::onboarding::onboard_as_new(&db).await.unwrap();
    }
    db.complete_onboarding_for_test(max_seats).await.unwrap();
    drop(db);
    FleetConfig {
        process_spawner: SeatProcessSpawner::Fake(Arc::new(
            crate::seat_process::fake::FakeSeatProcessSpawner::default(),
        )),
        manifold_environment:
            fedi_decentralized_manifold_environment::ManifoldEnvironment::Development,
        first_port_base: PortBase::new(first_port_base).unwrap(),
        respawn: RespawnPolicy::default(),
        setup_payments_configured: true,
        guardian_verification_fee_account: Some(guardian_verification_fee_account()),
        // Tests hold the relay down and watch the retry land; a
        // production cadence would only make them slow.
        backup_scan_interval: Duration::from_millis(10),
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
    }
}

async fn open_fleet(
    config: FleetConfig,
    wallet: Arc<dyn crate::wallet::EcashWallet>,
) -> anyhow::Result<Fleet> {
    let db = Db::open(&config.process.data_root).await?;
    Fleet::open(db, config, wallet).await
}

fn raw_commitment(response: Vec<u8>, byte: u8) -> SignedResponse<CreateSeatResponse> {
    SignedResponse::from_parts(response, signature(byte))
}

fn commitment(
    byte: u8,
) -> impl FnOnce(&SeatId) -> anyhow::Result<SignedResponse<CreateSeatResponse>> {
    move |seat_id| {
        Ok(raw_commitment(
            format!("created:{seat_id}").into_bytes(),
            byte,
        ))
    }
}

fn refusal_commitment(
    byte: u8,
) -> impl FnOnce(
    RefusalReason,
    Option<&RefundTransaction>,
) -> anyhow::Result<SignedResponse<CreateSeatResponse>> {
    move |_, _| Ok(raw_commitment(vec![byte], byte))
}

fn committed_seat_id(commitment: &SignedResponse<CreateSeatResponse>) -> SeatId {
    let id = std::str::from_utf8(commitment.as_parts().0)
        .unwrap()
        .strip_prefix("created:")
        .unwrap();
    SeatId::new(id).unwrap()
}

fn far_future() -> u64 {
    u64::try_from(crate::db::now_ms() / 1000).unwrap() + 60
}

/// Deterministic guardian consensus keys, matching the domain crate's own
/// fixture: the first four multiples of the secp256k1 generator.
const FIXTURE_GUARDIAN_KEYS: [&str; 4] = [
    "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
    "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
    "02f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
    "02e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd13",
];

/// A four-guardian final config whose meta module sits at
/// [`FAKE_META_INSTANCE_ID`] rather than at instance 0, so a reader that
/// assumed a fixed instance id would find nothing.
fn fixture_client_config() -> fedimint_core::config::ClientConfig {
    use fedimint_core::config::{ClientModuleConfig, GlobalClientConfig, PeerUrl};
    use fedimint_core::core::ModuleKind;
    use fedimint_core::encoding::DynRawFallback;
    use fedimint_core::module::{CoreConsensusVersion, ModuleConsensusVersion};
    use fedimint_core::secp256k1::PublicKey;
    use fedimint_core::util::SafeUrl;

    let api_endpoints = (0..4)
        .map(|index| {
            (
                fedimint_core::PeerId::from(index as u16),
                PeerUrl {
                    url: SafeUrl::parse(&format!(
                        "iroh://{}",
                        iroh_base_035::SecretKey::from_bytes(&[0x60 + index as u8; 32]).public()
                    ))
                    .unwrap(),
                    name: format!("guardian-{index}"),
                },
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let broadcast_public_keys = (0..4)
        .map(|index| {
            (
                fedimint_core::PeerId::from(index as u16),
                FIXTURE_GUARDIAN_KEYS[index].parse::<PublicKey>().unwrap(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    fedimint_core::config::ClientConfig {
        global: GlobalClientConfig {
            api_endpoints,
            broadcast_public_keys: Some(broadcast_public_keys),
            consensus_version: CoreConsensusVersion::new(2, 1),
            meta: std::collections::BTreeMap::new(),
        },
        modules: std::collections::BTreeMap::from([
            (
                0,
                ClientModuleConfig {
                    kind: ModuleKind::from_static_str("mint"),
                    version: ModuleConsensusVersion::new(2, 0),
                    config: DynRawFallback::Raw {
                        module_instance_id: 0,
                        raw: vec![0xde, 0xad, 0xbe, 0xef],
                    },
                },
            ),
            (
                FAKE_META_INSTANCE_ID,
                ClientModuleConfig {
                    kind: ModuleKind::from_static_str("meta"),
                    version: ModuleConsensusVersion::new(0, 0),
                    config: DynRawFallback::Raw {
                        module_instance_id: FAKE_META_INSTANCE_ID,
                        raw: vec![0x01],
                    },
                },
            ),
            (
                FAKE_LNV2_INSTANCE_ID,
                ClientModuleConfig {
                    kind: ModuleKind::from_static_str("lnv2"),
                    version: ModuleConsensusVersion::new(0, 0),
                    config: DynRawFallback::Raw {
                        module_instance_id: FAKE_LNV2_INSTANCE_ID,
                        raw: vec![0x02],
                    },
                },
            ),
        ]),
    }
}

/// The invite code this seat's own fedimintd would hand out, naming `peer`.
fn fixture_invite_code(config: &fedimint_core::config::ClientConfig, peer: u16) -> String {
    fedimint_core::invite_code::InviteCode::new(
        config.global.api_endpoints[&fedimint_core::PeerId::from(peer)]
            .url
            .clone(),
        fedimint_core::PeerId::from(peer),
        config.calculate_federation_id(),
        None,
    )
    .to_string()
}

/// A running seat whose fedimintd serves the fixture federation, with this
/// guardian being `peer`.
fn running_federation(peer: u16, meta_consensus: Option<Vec<u8>>) -> FakeApiState {
    let config = fixture_client_config();
    FakeApiState {
        consensus_running: true,
        invite_code: Some(fixture_invite_code(&config, peer)),
        client_config: Some(serde_json::to_value(&config).unwrap()),
        meta_consensus,
        ..Default::default()
    }
}

fn fee_account(byte: u8) -> Account {
    crate::guardian_fee::GuardianFeeAccountKey::from_secret_bytes(&[byte; 32]).account()
}

fn fee_recipient(account: Account, weight: u64) -> GuardianFeeRecipient {
    GuardianFeeRecipient::new(GuardianFeeAccount::try_from(account).unwrap(), weight)
}

/// A published minimum of zero: these cases are about everything except the
/// floor, so they pass the one value that admits any rate.
const NO_FEE_FLOOR: u64 = 0;

fn guardian_verification_fee_account() -> Account {
    fee_account(0x31)
}

fn guardian_fee_recipients(ours: &Account) -> Vec<GuardianFeeRecipient> {
    let mut recipients = [
        fee_recipient(ours.clone(), crate::guardian_fee::GUARDIAN_RECIPIENT_WEIGHT),
        fee_recipient(
            fee_account(0x21),
            crate::guardian_fee::GUARDIAN_RECIPIENT_WEIGHT,
        ),
        fee_recipient(
            fee_account(0x22),
            crate::guardian_fee::GUARDIAN_RECIPIENT_WEIGHT,
        ),
        fee_recipient(
            fee_account(0x23),
            crate::guardian_fee::GUARDIAN_RECIPIENT_WEIGHT,
        ),
        fee_recipient(fee_account(0x30), crate::guardian_fee::FI_RECIPIENT_WEIGHT),
        fee_recipient(
            guardian_verification_fee_account(),
            crate::guardian_fee::GUARDIAN_VERIFICATION_FEE_WEIGHT,
        ),
    ];
    recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    recipients.into_iter().collect()
}

async fn pin_guardian_fee_policy(
    fleet: &Fleet,
    seat_id: &SeatId,
    directory: &str,
    recipients: &str,
) {
    assert!(
        fleet
            .db
            .pin_formation_fee_policy(seat_id, directory, recipients)
            .await
            .unwrap()
    );
}

fn fixture_guardians(ours: &Account) -> Vec<Account> {
    [
        ours.clone(),
        fee_account(0x21),
        fee_account(0x22),
        fee_account(0x23),
    ]
    .into()
}

/// Sign a seat binding for `peer` under `keys`, the way that seat's FMan
/// would.
fn peer_binding_signed_by(
    config: &fedimint_core::config::ClientConfig,
    peer: usize,
    keys: &nostr_sdk::Keys,
    guardian_fee_account: Account,
) -> fedi_decentralized_domain::FmanPeerAttestation {
    use fedi_decentralized_domain::{
        FmanPeerAttestation, FmanPeerAttestationStatement, ProtocolV1, Pubkey,
        SchnorrSignatureProof, Timestamp, federation_seats,
    };

    let federation = federation_seats(config).unwrap();
    let seat = &federation.seats()[peer];
    let attestation = FmanPeerAttestationStatement {
        fman_pubkey: Pubkey(keys.public_key().to_string()),
        federation_id: federation.federation_id().clone(),
        federation_config_hash: federation.federation_config_hash().clone(),
        peer_id: seat.peer_id.clone(),
        guardian_identity: seat.guardian_identity.clone(),
        guardian_fee_account,
        issued_at: Timestamp(1_700_000_000),
    };
    let signature = keys.sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(
        attestation.digest().unwrap(),
    ));
    FmanPeerAttestation {
        version: ProtocolV1,
        attestation,
        proof: SchnorrSignatureProof { signature },
    }
}

/// The deterministic FMan identity fixtures use for `peer`, shared between
/// directory bindings and the endpoint-signed DKG transcript so the two
/// halves of the seat-binding validation agree.
fn peer_fman_keys(peer: usize) -> nostr_sdk::Keys {
    nostr_sdk::Keys::new(
        nostr_sdk::SecretKey::from_slice(&[0x40 + peer as u8; 32])
            .expect("fixed test scalar is valid"),
    )
}

/// Sign a seat binding for `peer` under its deterministic fixture FMan
/// identity, the way a sibling guardian's FMan would.
fn peer_binding(
    config: &fedimint_core::config::ClientConfig,
    peer: usize,
    guardian_fee_account: Account,
) -> fedi_decentralized_domain::FmanPeerAttestation {
    peer_binding_signed_by(config, peer, &peer_fman_keys(peer), guardian_fee_account)
}

/// A canonical directory covering every seat of the fixture config, with
/// every seat bound to a throwaway identity. Structurally impeccable — every
/// signature verifies — which is exactly what a directory assembled by an
/// attacker holding only its own generated keys looks like.
fn fixture_directory(config: &fedimint_core::config::ClientConfig) -> String {
    fedi_decentralized_domain::FmanSeatBindings::new(
        (0..4).map(|peer| peer_binding(config, peer, fee_account(0x20 + peer as u8))),
    )
    .unwrap()
    .canonical_string()
    .unwrap()
}

fn guardian_fee_directory(config: &fedimint_core::config::ClientConfig, ours: &Account) -> String {
    let accounts = [
        ours.clone(),
        fee_account(0x21),
        fee_account(0x22),
        fee_account(0x23),
    ];
    fedi_decentralized_domain::FmanSeatBindings::new(
        accounts
            .into_iter()
            .enumerate()
            .map(|(peer, account)| peer_binding(config, peer, account)),
    )
    .unwrap()
    .canonical_string()
    .unwrap()
}

fn formation_seat_bindings(directory: &str) -> Vec<FormationSeatBinding> {
    fedi_decentralized_domain::FmanSeatBindings::parse_canonical(directory)
        .unwrap()
        .seat_bindings()
        .iter()
        .map(|binding| {
            let peer = binding.attestation.peer_id.0.parse::<u8>().unwrap();
            let signature = iroh_base_035::SecretKey::from_bytes(&[0x60 + peer; 32])
                .sign(&binding.attestation.seat_endpoint_proof_message().unwrap());
            FormationSeatBinding {
                attestation: binding.clone(),
                endpoint_proof: fedi_decentralized_domain::SeatEndpointProof {
                    signature: signature.to_bytes().to_vec(),
                },
            }
        })
        .collect()
}

fn fixture_dkg_codes(count: usize) -> DkgCodeSet {
    let codes = (0..count)
        .map(|index| bare_dkg_code(endpoint_setup(index)))
        .collect::<Vec<_>>();
    DkgCodeSet::validate(&codes, FederationSize(count as u16), &codes[0]).unwrap()
}

fn endpoint_setup(index: usize) -> fedimint_core::setup_code::PeerSetupCode {
    use fedimint_core::setup_code::{PeerEndpoints, PeerSetupCode};

    let index = u8::try_from(index).expect("test endpoint index fits u8");
    let api_secret_bytes = [0x60 + index; 32];
    let p2p_secret_bytes = [0x70 + index; 32];
    PeerSetupCode {
        name: format!("guardian-{index:02}"),
        endpoints: PeerEndpoints::Iroh {
            api_pk: iroh_base_035::SecretKey::from_bytes(&api_secret_bytes).public(),
            p2p_pk: iroh_base_035::SecretKey::from_bytes(&p2p_secret_bytes).public(),
        },
        federation_name: None,
        disable_base_fees: None,
        enabled_modules: None,
        federation_size: None,
    }
}

fn bare_dkg_code(setup: fedimint_core::setup_code::PeerSetupCode) -> GuardianCode {
    use fedimint_core::base32::{self, FEDIMINT_PREFIX};

    GuardianCode(base32::encode_prefixed(FEDIMINT_PREFIX, &setup))
}

async fn scripted_dkg_codes(fleet: &Fleet, fi_id: &FiId, seat_id: &SeatId) -> Vec<GuardianCode> {
    let own_code = dkg_code(fleet, fi_id, seat_id, None).await.unwrap();
    let mut codes = vec![own_code];
    codes.extend(fixture_dkg_codes(6).iter().cloned());
    codes
}

async fn create_free_seat(fleet: &Fleet, quote: u8) -> (FiId, SeatId) {
    let fi_id = current_input(fleet, quote, far_future(), 0, 0).await.fi_id;
    let commitment = fleet
        .create_seat(
            current_input(fleet, quote, far_future(), 0, 0).await,
            commitment(quote),
            refusal_commitment(quote),
        )
        .await
        .unwrap();
    (fi_id, committed_seat_id(&commitment))
}

#[tokio::test]
async fn create_is_atomic_idempotent_and_rebuilds_as_live() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_000).await;
    let fleet = open_fleet(config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let first_epoch = fleet.db.offer_epoch().await.unwrap();
    let first = fleet
        .create_seat(
            input(first_epoch, 1, far_future(), 0, 0),
            commitment(1),
            refusal_commitment(1),
        )
        .await
        .unwrap();
    let replay = fleet
        .create_seat(
            input(first_epoch, 1, far_future(), 0, 0),
            commitment(1),
            |_, _| panic!("a replay must never refuse an accepted quote"),
        )
        .await
        .unwrap();
    // The commitment is re-signed rather than re-served, so the replay is the
    // same acceptance, not the same bytes.
    assert_eq!(first.as_parts().0, replay.as_parts().0);
    assert_eq!(fleet.db.list_seats().await.unwrap().len(), 1);
    assert_eq!(fleet.available_slots().await, 0);
    assert_ne!(fleet.db.offer_epoch().await.unwrap(), first_epoch);
    let first_id = committed_seat_id(&first);
    let first_no = fleet.db.list_seats().await.unwrap()[0].facts.seat_no;
    assert!(fleet.decommission_seat(&first_id).await.unwrap());
    assert!(!fleet.decommission_seat(&first_id).await.unwrap());
    let stale = fleet
        .create_seat(
            input(first_epoch, 7, far_future(), 0, 0),
            commitment(7),
            refusal_commitment(7),
        )
        .await
        .unwrap();
    assert_eq!(stale, raw_commitment(vec![7], 7));
    let second = fleet
        .create_seat(
            current_input(&fleet, 4, far_future(), 0, 0).await,
            commitment(4),
            refusal_commitment(4),
        )
        .await
        .unwrap();
    let _second_id = committed_seat_id(&second);
    let seats = fleet.db.list_seats().await.unwrap();
    assert_eq!(seats.len(), 2);
    assert_ne!(seats[1].facts.seat_no, first_no);
    fleet.shutdown().await;
    drop(fleet);

    let reopened = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    assert_eq!(reopened.seat_summaries().await.unwrap().len(), 2);
    assert_eq!(reopened.available_slots().await, 0);
    reopened.shutdown().await;
}

#[tokio::test]
async fn accepted_replay_repairs_a_missing_runtime_registry_entry() {
    let temp = TempDir::new().unwrap();
    let fleet = open_fleet(config(&temp, 1, 30_070).await, Arc::new(NoWallet))
        .await
        .unwrap();
    let epoch = fleet.db.offer_epoch().await.unwrap();
    let accepted = fleet
        .create_seat(
            input(epoch, 24, far_future(), 0, 0),
            commitment(24),
            refusal_commitment(24),
        )
        .await
        .unwrap();
    let seat_id = committed_seat_id(&accepted);
    let removed = fleet
        .seats
        .write()
        .expect("seat registry lock is never poisoned")
        .remove(&seat_id)
        .unwrap();
    removed.stop().await;

    fleet
        .create_seat(
            input(epoch, 24, far_future(), 0, 0),
            commitment(24),
            |_, _| panic!("a durable acceptance must dominate the changed epoch"),
        )
        .await
        .unwrap();
    assert!(fleet.seat_by_id(&seat_id).is_some());
    assert_eq!(fleet.db.list_seats().await.unwrap().len(), 1);
    fleet.shutdown().await;
}

#[tokio::test]
async fn availability_is_bounded_by_the_remaining_lifetime_port_grid() {
    let temp = TempDir::new().unwrap();
    // Exactly two complete four-port blocks remain: 65528 and 65532.
    let fleet = open_fleet(config(&temp, 3, 65_528).await, Arc::new(NoWallet))
        .await
        .unwrap();
    assert_eq!(fleet.available_slots().await, 2);

    let (_, first_id) = create_free_seat(&fleet, 31).await;
    assert_eq!(fleet.available_slots().await, 1);
    assert!(fleet.decommission_seat(&first_id).await.unwrap());
    assert_eq!(
        fleet.available_slots().await,
        1,
        "decommission frees a live slot but never its historical port block"
    );

    let (_, second_id) = create_free_seat(&fleet, 32).await;
    assert_eq!(fleet.available_slots().await, 0);
    assert!(fleet.decommission_seat(&second_id).await.unwrap());
    assert_eq!(
        fleet.available_slots().await,
        0,
        "port exhaustion keeps availability at zero after every live slot is freed"
    );

    let mut stale = current_input(&fleet, 33, far_future(), 0, 0).await;
    let mut stale_epoch = *stale.quote_terms.offer_epoch.as_bytes();
    stale_epoch[0] ^= 1;
    stale.quote_terms.offer_epoch = OfferEpoch::from_bytes(stale_epoch);
    let refusal = fleet
        .create_seat(stale, commitment(33), |reason, refund| {
            assert_eq!(reason, RefusalReason::OfferChanged);
            assert!(refund.is_none());
            Ok(raw_commitment(vec![33], 33))
        })
        .await
        .unwrap();
    assert_eq!(refusal, raw_commitment(vec![33], 33));
    fleet.shutdown().await;
}

#[tokio::test]
async fn accepted_paid_quote_cannot_become_a_refund_after_restart() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_000).await;
    let fleet = open_fleet(config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let accepted = fleet
        .create_seat(
            current_input(&fleet, 5, far_future(), 1_000, 5).await,
            commitment(5),
            refusal_commitment(5),
        )
        .await
        .unwrap();
    let _seat_id = committed_seat_id(&accepted);
    fleet.shutdown().await;
    drop(fleet);

    let reopened = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let replay = reopened
        .create_seat(
            // A fresh allocation decision would now refuse for capacity;
            // the durable by-quote acceptance must win instead.
            current_input(&reopened, 5, far_future(), 1_000, 9).await,
            commitment(5),
            |_, _| panic!("accepted quote must not enter refund ledger"),
        )
        .await
        .unwrap();
    assert_eq!(replay.as_parts().0, accepted.as_parts().0);
    assert_eq!(reopened.db.list_seats().await.unwrap().len(), 1);
    reopened.shutdown().await;
}

/// Advertisement discovery follows the operator's offer, not payment-policy or
/// process-local wallet readiness.
#[tokio::test]
async fn advertisement_eligibility_changes_wake_publication_without_waiting_for_the_wallet() {
    let temp = TempDir::new().unwrap();
    let fleet = Arc::new(
        open_fleet(config(&temp, 1, 30_620).await, Arc::new(NoWallet))
            .await
            .unwrap(),
    );
    let host = FleetNostrHost::new(
        fleet.clone(),
        "endpoint".to_owned(),
        "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9"
            .parse()
            .unwrap(),
    );

    assert!(
        !fleet.availability_snapshot().await.accepting_seats,
        "an FMan that is not selling advertises nothing"
    );
    assert!(host.advertisement().await.is_none());

    fleet.set_offered_price(Some(Msats(0))).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), host.advertisement_changed())
        .await
        .expect("an offer change wakes advertisement publication");
    assert!(
        fleet.availability_snapshot().await.accepting_seats,
        "a give-away needs no wallet"
    );
    assert!(host.advertisement().await.is_some());

    fleet.set_offered_price(Some(Msats(1))).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), host.advertisement_changed())
        .await
        .expect("a price change wakes advertisement publication");
    assert!(
        fleet.availability_snapshot().await.accepting_seats,
        "a priced offer is discoverable before payment runtime preparation"
    );
    assert!(host.advertisement().await.is_some());
    fleet.shutdown().await;
}

#[tokio::test]
async fn a_fleet_refuses_a_different_environment_after_restart() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_640).await;
    let fleet = open_fleet(config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    fleet.shutdown().await;
    drop(fleet);

    let mut production = config;
    production.manifold_environment =
        fedi_decentralized_manifold_environment::ManifoldEnvironment::Production;
    let error = match open_fleet(production, Arc::new(NoWallet)).await {
        Ok(_) => panic!("a data root must not change Manifold environments"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("bound to the development"),
        "{error:#}"
    );
}

#[tokio::test]
async fn a_nonzero_price_is_refused_without_a_setup_payment_publisher() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 30_640).await;
    config.setup_payments_configured = false;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();

    fleet
        .set_offered_price(Some(Msats(1)))
        .await
        .expect_err("a priced seat could never be paid for in this environment");
    // Not selling, and giving seats away, remain expressible.
    fleet.set_offered_price(Some(Msats(0))).await.unwrap();
    fleet.set_offered_price(None).await.unwrap();
    fleet.shutdown().await;
}

#[tokio::test]
async fn operator_settings_are_database_owned_and_persisted() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_600).await;
    let fleet = open_fleet(config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let price = Msats(10_000_000);
    let advertised_plan = Plan::InfiniteBestEffort {
        price_msats: price.0,
    };
    let federation_id = FederationId("test-payment-federation".to_owned());
    fleet.set_offered_price(Some(price)).await.unwrap();
    fleet
        .db
        .replace_setup_payment_policy(r#"{"event":1}"#, std::slice::from_ref(&federation_id))
        .await
        .unwrap();

    let availability = fleet.availability_snapshot().await;
    assert!(
        availability.accepting_seats,
        "membership gates a priced offer"
    );
    // The stored offer is a price, so the advertised plan is rebuilt from it:
    // advertisement and quote cannot disagree about what is being sold.
    assert_eq!(availability.plans, vec![advertised_plan.clone()]);
    fleet.shutdown().await;
    drop(fleet);

    let reopened = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let settings = reopened.operator_settings().await;
    assert_eq!(settings.plans(), vec![advertised_plan.clone()]);
    assert_eq!(settings.payment_federations, vec![federation_id.clone()]);

    // The database is the source of truth for every reader.
    reopened.db.set_offered_price(None).await.unwrap();
    reopened
        .db
        .replace_setup_payment_policy(r#"{"event":2}"#, &[])
        .await
        .unwrap();
    let changed = reopened.operator_settings().await;
    assert_eq!(changed.plans(), vec![]);
    assert!(changed.payment_federations.is_empty());
    reopened.shutdown().await;
}

#[tokio::test]
async fn unformed_idle_seat_keeps_a_parked_child() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 31_205).await;
    config.respawn.initial_backoff = Duration::from_millis(10);
    config.respawn.max_backoff = Duration::from_millis(20);
    let spawner = Arc::new(FakeSeatProcessSpawner::default());
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 130).await;
    dkg_code(&fleet, &fi_id, &seat_id, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(spawner.spawn_count(), 1);
    assert!(
        matches!(
            fleet.seat_by_id(&seat_id).unwrap().report().await.unwrap(),
            SeatReport::Active {
                phase: SeatPhase::Created,
                health: SeatHealth::Unavailable
            }
        ),
        "minting a stateless guardian code does not create a phase"
    );

    fleet.shutdown().await;
}

#[tokio::test]
async fn guardian_code_name_is_stable_and_seat_distinct() {
    use fedimint_core::base32::{self, FEDIMINT_PREFIX};
    use fedimint_core::setup_code::PeerSetupCode;

    let temp = TempDir::new().unwrap();
    let fleet = open_fleet(config(&temp, 2, 31_270).await, Arc::new(NoWallet))
        .await
        .unwrap();
    let (first_fi, first_seat) = create_free_seat(&fleet, 138).await;
    let (second_fi, second_seat) = create_free_seat(&fleet, 139).await;

    let first = dkg_code(&fleet, &first_fi, &first_seat, None)
        .await
        .unwrap();
    assert_eq!(
        dkg_code(&fleet, &first_fi, &first_seat, None)
            .await
            .unwrap(),
        first,
    );
    let second = dkg_code(&fleet, &second_fi, &second_seat, None)
        .await
        .unwrap();
    let setup = |code: GuardianCode| {
        base32::decode_prefixed::<PeerSetupCode>(FEDIMINT_PREFIX, &code.0).unwrap()
    };
    let first_name = setup(first).name;
    let second_name = setup(second).name;

    assert_eq!(first_name, format!("fm-{}", &first_seat.to_string()[..8]));
    assert_eq!(second_name, format!("fm-{}", &second_seat.to_string()[..8]));
    assert_ne!(first_name, second_name);

    fleet.shutdown().await;
}

#[tokio::test]
async fn ceremony_child_failure_respawns_a_parked_child() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 31_206).await;
    config.respawn.initial_backoff = Duration::from_millis(10);
    config.respawn.max_backoff = Duration::from_millis(20);
    let spawner = Arc::new(FakeSeatProcessSpawner::scripted([vec![vec![
        FakeDkgStep::Crash,
    ]]]));
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 131).await;
    let codes = scripted_dkg_codes(&fleet, &fi_id, &seat_id).await;

    assert!(matches!(
        start_dkg(&fleet, &fi_id, &seat_id, &codes).await,
        Err(SeatVerbError::Internal(_))
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(spawner.spawn_count(), 2, "a failed ceremony is replaced");
    assert_eq!(spawner.request_count(), 1);

    fleet.shutdown().await;
}

async fn assert_terminal_start_cleans_session(step: FakeDkgStep, quote: u8, first_port: u16) {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, first_port).await;
    config.respawn.initial_backoff = Duration::from_millis(10);
    let spawner = Arc::new(FakeSeatProcessSpawner::scripted([vec![vec![step], vec![]]]));
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, quote).await;
    let codes = scripted_dkg_codes(&fleet, &fi_id, &seat_id).await;

    assert!(start_dkg(&fleet, &fi_id, &seat_id, &codes).await.is_err());
    tokio::time::timeout(Duration::from_secs(1), async {
        while spawner.spawn_count() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("terminal start outcome replaces the child");
    assert!(matches!(
        fleet.seat_by_id(&seat_id).unwrap().cached_report_for_test(),
        SeatReport::Active {
            phase: SeatPhase::Created,
            health: SeatHealth::Unavailable,
        }
    ));

    start_dkg(&fleet, &fi_id, &seat_id, &codes)
        .await
        .expect("replacement child accepts a fresh session");
    fleet.shutdown().await;
}

#[tokio::test]
async fn every_terminal_start_outcome_leaves_a_clean_retry_slot() {
    assert_terminal_start_cleans_session(
        FakeDkgStep::Message(ChildMessage::ParamsRejected {
            reason: "invalid params".to_owned(),
        }),
        133,
        31_209,
    )
    .await;
    assert_terminal_start_cleans_session(
        FakeDkgStep::Message(ChildMessage::DkgFailed {
            reason: "local DKG failure".to_owned(),
        }),
        134,
        31_210,
    )
    .await;
    assert_terminal_start_cleans_session(FakeDkgStep::Crash, 135, 31_211).await;
}

#[tokio::test]
async fn acknowledged_dkg_failure_clears_the_session_before_replacement() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 31_212).await;
    config.respawn.initial_backoff = Duration::from_millis(10);
    let spawner = Arc::new(FakeSeatProcessSpawner::scripted([vec![
        vec![
            FakeDkgStep::Message(ChildMessage::DkgStarted {}),
            FakeDkgStep::Message(ChildMessage::DkgFailed {
                reason: "failure after acknowledgement".to_owned(),
            }),
        ],
        vec![],
    ]]));
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 136).await;
    let codes = scripted_dkg_codes(&fleet, &fi_id, &seat_id).await;

    start_dkg(&fleet, &fi_id, &seat_id, &codes)
        .await
        .expect("the child acknowledges before its later DKG failure");
    tokio::time::timeout(Duration::from_secs(1), async {
        while spawner.spawn_count() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("failed acknowledged ceremony is replaced");
    assert!(matches!(
        fleet.seat_by_id(&seat_id).unwrap().cached_report_for_test(),
        SeatReport::Active {
            phase: SeatPhase::Created,
            health: SeatHealth::Unavailable,
        }
    ));
    start_dkg(&fleet, &fi_id, &seat_id, &codes)
        .await
        .expect("replacement accepts a fresh ceremony");
    fleet.shutdown().await;
}

#[tokio::test]
async fn start_ack_timeout_reaps_and_replaces_the_child() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 31_208).await;
    config.respawn.initial_backoff = Duration::from_millis(10);
    let spawner = Arc::new(FakeSeatProcessSpawner::scripted(std::iter::once(vec![
        vec![FakeDkgStep::Hang],
    ])));
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 131).await;
    let codes = scripted_dkg_codes(&fleet, &fi_id, &seat_id).await;

    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(12),
            start_dkg(&fleet, &fi_id, &seat_id, &codes)
        )
        .await
        .expect("start acknowledgement bound elapsed"),
        Err(SeatVerbError::Internal(_))
    ));
    tokio::time::timeout(Duration::from_secs(1), async {
        while spawner.spawn_count() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("timed out child is replaced");
    fleet.shutdown().await;
}

#[tokio::test]
async fn failed_child_stop_permanently_refuses_replacement_but_records_decommission() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 31_207).await;
    let spawner = Arc::new(FakeSeatProcessSpawner::scripted([vec![vec![
        FakeDkgStep::FailStop,
        FakeDkgStep::Message(ChildMessage::DkgStarted {}),
    ]]]));
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 132).await;
    let codes = scripted_dkg_codes(&fleet, &fi_id, &seat_id).await;
    start_dkg(&fleet, &fi_id, &seat_id, &codes).await.unwrap();
    assert!(matches!(
        fleet.seat_by_id(&seat_id).unwrap().cached_report_for_test(),
        SeatReport::Active {
            phase: SeatPhase::DkgInProgress,
            health: SeatHealth::Healthy,
        }
    ));

    assert!(matches!(
        restart_dkg(&fleet, &fi_id, &seat_id, &codes).await,
        Err(SeatVerbError::Internal(_))
    ));
    assert!(matches!(
        fleet.seat_by_id(&seat_id).unwrap().cached_report_for_test(),
        SeatReport::Active {
            phase: SeatPhase::Created,
            health: SeatHealth::Unavailable,
        }
    ));
    assert!(matches!(
        restart_dkg(&fleet, &fi_id, &seat_id, &codes).await,
        Err(SeatVerbError::Internal(_))
    ));
    // The durable mark lands before the stop is attempted, so an unprovable
    // stop is reported to the caller while the seat stays terminal. The
    // operator's decision is recorded once; it is not lost to a child that
    // cannot be reaped.
    assert!(fleet.decommission_seat(&seat_id).await.is_err());
    assert_eq!(spawner.spawn_count(), 1);
    assert!(fleet.seat_by_id(&seat_id).unwrap().is_decommissioned());
    assert!(
        fleet
            .safe_event_journals()
            .contains(&SafeEventJournal::Seat {
                seat_id: seat_id.clone()
            }),
        "terminal seats retain their safe-event journal selector"
    );
    assert!(
        fleet
            .safe_event_journal_dir(&SafeEventJournal::Seat { seat_id })
            .is_some()
    );

    // Shutdown observes the same unproved slot but still closes the loop.
    fleet.shutdown().await;
}

#[tokio::test]
async fn driven_dkg_persists_config_before_a_later_child_death() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 31_230).await;
    let invite = InviteCode(fixture_invite_code(&fixture_client_config(), 0));
    config.respawn.initial_backoff = Duration::from_millis(10);
    let spawner = Arc::new(FakeSeatProcessSpawner::scripted([vec![
        vec![
            FakeDkgStep::Message(ChildMessage::DkgStarted {}),
            FakeDkgStep::Message(ChildMessage::ConfigPersisted {
                invite_code: invite.0.clone(),
                api_url: "ws://127.0.0.1:1".to_owned(),
            }),
            FakeDkgStep::Crash,
        ],
        vec![FakeDkgStep::Message(ChildMessage::Hello {
            proto: PROTOCOL_VERSION,
            code_version: "fake-fedimintd".to_owned(),
            state: ChildState::AlreadyConfigured {
                invite_code: invite.0.clone(),
            },
        })],
    ]]));
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 122).await;
    let codes = scripted_dkg_codes(&fleet, &fi_id, &seat_id).await;

    start_dkg(&fleet, &fi_id, &seat_id, &codes).await.unwrap();
    for _ in 0..500 {
        if fleet
            .db
            .formed_federation_invite(&seat_id)
            .await
            .unwrap()
            .as_ref()
            == Some(&invite)
        {
            tokio::time::timeout(Duration::from_secs(1), async {
                while spawner.spawn_count() < 2 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("formed child respawns after capped backoff");
            assert!(matches!(
                fleet.seat_by_id(&seat_id).unwrap().cached_report_for_test(),
                SeatReport::Active {
                    phase: SeatPhase::Running { .. },
                    ..
                }
            ));
            fleet.shutdown().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("ConfigPersisted was not durably mirrored before the scripted crash");
}

#[tokio::test]
async fn configured_child_hello_repairs_missing_invite_without_status_request() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 31_240).await;
    let invite = InviteCode(fixture_invite_code(&fixture_client_config(), 0));
    config.process_spawner =
        SeatProcessSpawner::Fake(Arc::new(FakeSeatProcessSpawner::scripted([vec![vec![
            FakeDkgStep::Message(ChildMessage::Hello {
                proto: PROTOCOL_VERSION,
                code_version: "fake-fedimintd".to_owned(),
                state: ChildState::AlreadyConfigured {
                    invite_code: invite.0.clone(),
                },
            }),
        ]]])));
    let fleet = open_fleet(config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (_fi_id, seat_id) = create_free_seat(&fleet, 124).await;
    fleet.shutdown().await;
    drop(fleet);
    tokio::fs::create_dir_all(seat_data_dir(&config.process, SeatNo(0)))
        .await
        .unwrap();
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    for _ in 0..500 {
        if fleet
            .db
            .formed_federation_invite(&seat_id)
            .await
            .unwrap()
            .as_ref()
            == Some(&invite)
        {
            assert!(matches!(
                fleet.seat_by_id(&seat_id).unwrap().cached_report_for_test(),
                SeatReport::Active {
                    phase: SeatPhase::Running { .. },
                    ..
                }
            ));
            fleet.shutdown().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("AlreadyConfigured was not durably mirrored");
}

#[tokio::test]
async fn formed_seat_without_its_final_directory_reports_data_loss() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 31_245).await;
    let spawner = Arc::new(FakeSeatProcessSpawner::default());
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let fleet = open_fleet(config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 125).await;
    let codes = scripted_dkg_codes(&fleet, &fi_id, &seat_id).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let invite = InviteCode(fixture_invite_code(&fixture_client_config(), 0));
    fleet
        .form_fake_child(&seat_id, running_federation(0, None))
        .await;
    tokio::fs::remove_dir_all(seat_data_dir(&config.process, seat.facts().seat_no))
        .await
        .unwrap();
    fleet.shutdown().await;
    drop(fleet);

    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    assert_eq!(
        spawner.spawn_count(),
        1,
        "DataLoss retains its parked child"
    );

    assert!(matches!(
        seat.report().await.unwrap(),
        SeatReport::Active {
            phase: SeatPhase::DataLoss { invite_code },
            health: SeatHealth::Unavailable,
        } if invite_code == invite
    ));
    assert!(
        matches!(
            seat.cached_report_for_test(),
            SeatReport::Active {
                phase: SeatPhase::DataLoss { .. },
                ..
            }
        ),
        "the status timeout fallback preserves DataLoss"
    );
    assert!(matches!(
        start_dkg(&fleet, &fi_id, &seat_id, &codes).await,
        Err(SeatVerbError::WrongState {
            status: ServiceStatus::DataLoss
        })
    ));
    assert!(matches!(
        seat.restart_dkg(&codes).await,
        Err(SeatVerbError::WrongState {
            status: ServiceStatus::DataLoss
        })
    ));
    assert_eq!(
        spawner.spawn_count(),
        2,
        "a known-formed DataLoss seat keeps its child"
    );
    fleet.shutdown().await;
}

/// Decommission is a capacity decision, not a destruction of guardian
/// material. A federation may still depend on this seat's key shares, so the
/// final data directory outlives the terminal record.
#[tokio::test]
async fn decommission_retains_the_final_guardian_data_directory() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 31_249).await;
    let fleet = open_fleet(config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 133).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let data_dir = seat_data_dir(&config.process, seat.facts().seat_no);
    fleet
        .form_fake_child(&seat_id, running_federation(0, None))
        .await;
    assert!(tokio::fs::metadata(&data_dir).await.is_ok());

    fleet.decommission_seat(&seat_id).await.unwrap();

    assert!(fleet.seat_by_id(&seat_id).unwrap().is_decommissioned());
    assert!(
        tokio::fs::metadata(&data_dir).await.is_ok(),
        "decommission must retain the final guardian data directory"
    );

    fleet.shutdown().await;
}

#[tokio::test]
async fn formed_watchdog_tracks_health_without_refetching_the_durable_invite() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 31_247).await;
    let fleet = open_fleet(config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 127).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let invite = InviteCode(fixture_invite_code(&fixture_client_config(), 0));
    let data_dir = seat_data_dir(&config.process, seat.facts().seat_no);
    let invite_gate = Arc::new(Notify::new());
    let fake = fleet
        .form_fake_child(
            &seat_id,
            FakeApiState {
                invite_code: Some(invite.0.clone()),
                invite_gate: Some(invite_gate),
                ..Default::default()
            },
        )
        .await;

    seat.watchdog_tick_for_test().await;

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), seat.report())
            .await
            .expect("formed status does not call the blocked invite endpoint")
            .unwrap(),
        SeatReport::Active {
            phase: SeatPhase::Running {
                invite_code: invite
            },
            health: SeatHealth::Healthy,
        }
    );

    tokio::fs::remove_dir_all(&data_dir).await.unwrap();
    seat.watchdog_tick_for_test().await;
    tokio::fs::create_dir_all(&data_dir).await.unwrap();
    assert!(matches!(
        seat.report().await.unwrap(),
        SeatReport::Active {
            phase: SeatPhase::Running { .. },
            health: SeatHealth::Unavailable,
        }
    ));
    seat.watchdog_tick_for_test().await;

    fake.set_consensus_running(false);
    seat.watchdog_tick_for_test().await;
    assert!(matches!(
        seat.report().await.unwrap(),
        SeatReport::Active {
            phase: SeatPhase::Running { .. },
            health: SeatHealth::Unavailable,
        }
    ));

    fake.set_consensus_running(true);
    seat.watchdog_tick_for_test().await;
    assert!(matches!(
        seat.report().await.unwrap(),
        SeatReport::Active {
            phase: SeatPhase::Running { .. },
            health: SeatHealth::Healthy,
        }
    ));
    fleet.shutdown().await;
}

#[tokio::test]
async fn status_is_prompt_while_the_seat_loop_is_waiting_on_health() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 31_248).await;
    let fleet = open_fleet(config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 128).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let invite = InviteCode(fixture_invite_code(&fixture_client_config(), 0));

    let probe_entered = Arc::new(Notify::new());
    let probe_gate = Arc::new(Notify::new());
    let fake = fleet
        .form_fake_child(
            &seat_id,
            FakeApiState {
                invite_code: Some(invite.0.clone()),
                ..Default::default()
            },
        )
        .await;
    fake.set_consensus_running(false);
    seat.watchdog_tick_for_test().await;
    fake.modify_state(|state| {
        state.consensus_running = true;
        state.probe_entered = Some(probe_entered.clone());
        state.probe_gate = Some(probe_gate.clone());
    });

    let watchdog = {
        let seat = seat.clone();
        tokio::spawn(async move { seat.watchdog_tick_for_test().await })
    };
    probe_entered.notified().await;
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(100), seat.report())
            .await
            .expect("status does not wait for the occupied seat loop")
            .unwrap(),
        SeatReport::Active {
            phase: SeatPhase::Running {
                invite_code: invite,
            },
            health: SeatHealth::Unavailable,
        }
    );
    probe_gate.notify_one();
    watchdog.await.unwrap();
    fleet.shutdown().await;
}

#[tokio::test]
async fn watchdog_never_probes_unformed_data_loss_or_decommissioned_seats() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 3, 31_249).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();

    let (_, unformed_id) = create_free_seat(&fleet, 129).await;
    let unformed = fleet.seat_by_id(&unformed_id).unwrap();
    let unformed_fake = fleet
        .configure_fake_child(
            &unformed_id,
            FakeApiState {
                consensus_running: true,
                ..Default::default()
            },
        )
        .await;
    unformed.watchdog_tick_for_test().await;
    assert_eq!(unformed_fake.state().probe_calls, 0);

    let (_, data_loss_id) = create_free_seat(&fleet, 130).await;
    let data_loss = fleet.seat_by_id(&data_loss_id).unwrap();
    let data_loss_fake = fleet
        .form_fake_child(&data_loss_id, running_federation(0, None))
        .await;
    let data_loss_probe_calls = data_loss_fake.state().probe_calls;
    tokio::fs::remove_dir_all(seat_data_dir(
        &fleet.config.process,
        data_loss.facts().seat_no,
    ))
    .await
    .unwrap();
    data_loss.watchdog_tick_for_test().await;
    assert_eq!(data_loss_fake.state().probe_calls, data_loss_probe_calls);

    let (decommissioned_fi, decommissioned_id) = create_free_seat(&fleet, 131).await;
    let decommissioned = fleet.seat_by_id(&decommissioned_id).unwrap();
    let decommissioned_codes =
        scripted_dkg_codes(&fleet, &decommissioned_fi, &decommissioned_id).await;
    let decommissioned_fake = fleet
        .configure_fake_child(
            &decommissioned_id,
            FakeApiState {
                consensus_running: true,
                ..Default::default()
            },
        )
        .await;
    assert!(decommissioned.decommission().await.unwrap());
    assert!(matches!(
        decommissioned.restart_dkg(&decommissioned_codes).await,
        Err(SeatVerbError::WrongState {
            status: ServiceStatus::Decommissioned
        })
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(decommissioned_fake.state().probe_calls, 0);
    fleet.shutdown().await;
}

#[tokio::test]
async fn restart_losing_the_completion_race_reports_formed_without_starting_again() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 31_246).await;
    let invite = InviteCode(fixture_invite_code(&fixture_client_config(), 0));
    let spawner = Arc::new(FakeSeatProcessSpawner::scripted([vec![
        vec![
            FakeDkgStep::Message(ChildMessage::DkgStarted {}),
            FakeDkgStep::InstallFinalOnStop,
        ],
        vec![FakeDkgStep::Message(ChildMessage::Hello {
            proto: PROTOCOL_VERSION,
            code_version: "fake-fedimintd".to_owned(),
            state: ChildState::AlreadyConfigured {
                invite_code: invite.0.clone(),
            },
        })],
    ]]));
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 126).await;
    let codes = scripted_dkg_codes(&fleet, &fi_id, &seat_id).await;
    start_dkg(&fleet, &fi_id, &seat_id, &codes).await.unwrap();
    assert_eq!(
        restart_dkg(&fleet, &fi_id, &seat_id, &codes).await.unwrap(),
        ServiceStatus::Running
    );
    assert_eq!(
        fleet.db.formed_federation_invite(&seat_id).await.unwrap(),
        Some(invite)
    );
    assert_eq!(
        spawner.spawn_count(),
        2,
        "the unknown-formed race replaces the child"
    );
    assert!(matches!(
        fleet
            .seat_by_id(&seat_id)
            .unwrap()
            .restart_dkg(&codes)
            .await,
        Err(SeatVerbError::WrongState {
            status: ServiceStatus::Running
        })
    ));
    assert_eq!(
        spawner.spawn_count(),
        2,
        "a known-formed restart is refused without touching the guardian"
    );
    fleet.shutdown().await;
}

#[tokio::test]
async fn restart_from_uninitialized_replaces_the_child_and_starts_dkg() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 31_250).await;
    let spawner = Arc::new(FakeSeatProcessSpawner::default());
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 136).await;
    let old_child = fleet
        .configure_fake_child(&seat_id, FakeApiState::default())
        .await;
    let codes = scripted_dkg_codes(&fleet, &fi_id, &seat_id).await;

    assert_eq!(
        restart_dkg(&fleet, &fi_id, &seat_id, &codes).await.unwrap(),
        ServiceStatus::DkgInProcess
    );
    assert_eq!(
        fleet.seat_by_id(&seat_id).unwrap().cached_report_for_test(),
        SeatReport::Active {
            phase: SeatPhase::DkgInProgress,
            health: SeatHealth::Healthy,
        }
    );
    let replacement = fleet
        .configure_fake_child(
            &seat_id,
            FakeApiState {
                consensus_running: true,
                ..Default::default()
            },
        )
        .await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let api = FedimintApi::new(
        fleet.fedimint_connectors.clone(),
        seat.ports().api(),
        &fleet.identity().derive_seat_keys(&seat_id).api_auth,
    );
    api.probe().await.unwrap();
    old_child.set_consensus_running(false);
    assert!(replacement.state().consensus_running);
    assert_eq!(spawner.spawn_count(), 2);
    assert_eq!(spawner.request_count(), 1);
    fleet.shutdown().await;
}

#[tokio::test]
async fn restart_replaces_an_acknowledged_ceremony_while_start_refuses_it() {
    let temp = TempDir::new().unwrap();
    let mut config = config(&temp, 1, 31_251).await;
    let spawner = Arc::new(FakeSeatProcessSpawner::default());
    config.process_spawner = SeatProcessSpawner::Fake(spawner.clone());
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 137).await;
    let codes = scripted_dkg_codes(&fleet, &fi_id, &seat_id).await;
    start_dkg(&fleet, &fi_id, &seat_id, &codes).await.unwrap();

    assert!(matches!(
        start_dkg(&fleet, &fi_id, &seat_id, &codes).await,
        Err(SeatVerbError::WrongState {
            status: ServiceStatus::DkgInProcess
        })
    ));
    assert_eq!(
        restart_dkg(&fleet, &fi_id, &seat_id, &codes).await.unwrap(),
        ServiceStatus::DkgInProcess
    );
    assert_eq!(spawner.spawn_count(), 2);
    assert_eq!(spawner.request_count(), 2);
    fleet.shutdown().await;
}

#[tokio::test]
async fn start_dkg_rejects_invalid_bare_codes_before_any_side_effect() {
    use fedimint_core::base32::{self, FEDIMINT_PREFIX};
    use fedimint_core::setup_code::PeerSetupCode;

    let temp = TempDir::new().unwrap();
    let fleet = open_fleet(config(&temp, 1, 30_220).await, Arc::new(NoWallet))
        .await
        .unwrap();
    let (fi_id, seat_id) = create_free_seat(&fleet, 53).await;
    let _seat = fleet.seat_by_id(&seat_id).unwrap();
    let fake = fleet
        .configure_fake_child(&seat_id, FakeApiState::default())
        .await;

    let own_code = dkg_code(&fleet, &fi_id, &seat_id, None).await.unwrap();
    let peers = fixture_dkg_codes(6);
    let mut valid = vec![own_code.clone()];
    valid.extend(peers.iter().cloned());

    let replace_peer = |replacement: GuardianCode| {
        let mut codes = valid.clone();
        codes[1] = replacement;
        codes
    };
    let replace_own = |replacement: GuardianCode| {
        let mut codes = valid.clone();
        codes[0] = replacement;
        codes
    };
    let mut invalid = Vec::<(&str, Vec<GuardianCode>, &str)>::new();
    invalid.push((
        "invalid upstream setup",
        replace_peer(GuardianCode("not-a-fedimint-setup-code".to_owned())),
        "Invalid Prefix",
    ));
    let mut duplicate_setup = valid.clone();
    duplicate_setup[2] = duplicate_setup[1].clone();
    invalid.push((
        "duplicate setup code",
        duplicate_setup,
        "duplicate guardian code",
    ));

    let mut altered_own: PeerSetupCode =
        base32::decode_prefixed(FEDIMINT_PREFIX, &own_code.0).unwrap();
    altered_own.name.push_str("-substituted");
    invalid.push((
        "own setup changed",
        replace_own(bare_dkg_code(altered_own)),
        "own guardian code failed deterministic recomputation",
    ));

    let without_own = fixture_dkg_codes(7);
    invalid.push((
        "own setup missing",
        without_own.iter().cloned().collect(),
        "own guardian code missing",
    ));

    let probe_calls_before = fake.state().probe_calls;
    for (case, codes, expected_error) in invalid {
        let error = start_dkg(&fleet, &fi_id, &seat_id, &codes)
            .await
            .expect_err(case);
        assert!(
            matches!(&error, SeatVerbError::InvalidDkgInput(message) if message.contains(expected_error)),
            "{case}: {error:?}"
        );
        let child = fake.state();
        assert_eq!(
            child.probe_calls, probe_calls_before,
            "{case}: child probed"
        );
        assert!(!child.consensus_running, "{case}: DKG started");
        assert!(
            fleet
                .db
                .formed_federation_invite(&seat_id)
                .await
                .unwrap()
                .is_none(),
            "{case}: formed record written"
        );
    }
    fleet.shutdown().await;
}

/// A canonical directory whose entry for `own_peer` is signed by `own_keys`
/// — the shape an honest FI assembles for the guardian holding those keys.
fn fixture_directory_bound_to(
    config: &fedimint_core::config::ClientConfig,
    own_peer: usize,
    own_keys: &nostr_sdk::Keys,
) -> String {
    fedi_decentralized_domain::FmanSeatBindings::new((0..4).map(|peer| {
        if peer == own_peer {
            peer_binding_signed_by(config, peer, own_keys, fee_account(0x20 + peer as u8))
        } else {
            peer_binding(config, peer, fee_account(0x20 + peer as u8))
        }
    }))
    .unwrap()
    .canonical_string()
    .unwrap()
}

#[tokio::test]
async fn peer_attestation_binds_this_seats_own_config_hash_and_peer() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_930).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 61).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    // This guardian is peer 2, so an implementation that assumed the first
    // peer, or read the peer from anywhere but the invite code, gets it wrong.
    let _fake = fleet
        .form_fake_child(&seat_id, running_federation(2, None))
        .await;

    let binding = seat.federation_binding().await.unwrap();
    let expected = fedi_decentralized_domain::federation_seats(&fixture_client_config()).unwrap();

    // The config hash is the value a verifier independently re-derives from
    // the invite-code download; a one-byte disagreement silently fails every
    // liquidity request, so it is the assertion that matters most here.
    assert_eq!(
        binding.federation.federation_config_hash(),
        expected.federation_config_hash()
    );
    assert_eq!(binding.federation.federation_id(), expected.federation_id());
    assert_eq!(binding.seat.peer_id.0, "2");
    assert_eq!(binding.seat.guardian_identity.0, FIXTURE_GUARDIAN_KEYS[2]);
    fleet.shutdown().await;
}

#[tokio::test]
async fn register_gateway_uses_the_discovered_lnv2_module_and_admin_auth() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_940).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 66).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, None))
        .await;
    let _baseline = fake.state();
    let gateway = fedimint_core::util::SafeUrl::parse("https://gateway.example/api").unwrap();

    assert!(seat.register_gateway(gateway.clone()).await.unwrap());
    assert!(!seat.register_gateway(gateway).await.unwrap());
    let state = fake.state();
    assert_eq!(
        state.lnv2_gateways.into_iter().collect::<Vec<_>>(),
        vec!["https://gateway.example/api"]
    );
    assert_eq!(
        state.lnv2_gateway_auth,
        vec![
            Some(
                fleet
                    .identity()
                    .derive_seat_keys(&seat_id)
                    .api_auth
                    .as_str()
                    .to_owned()
            );
            2
        ]
    );
    fleet.shutdown().await;
}

#[tokio::test]
async fn federation_binding_is_refused_before_consensus() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_950).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 62).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let _fake = fleet
        .configure_fake_child(&seat_id, FakeApiState::default())
        .await;

    // Nothing to attest to before DKG: there is no final config yet.
    assert!(matches!(
        seat.federation_binding().await,
        Err(SeatVerbError::WrongState {
            status: ServiceStatus::New
        })
    ));
    fleet.shutdown().await;
}

#[tokio::test]
async fn one_fman_capability_authorizes_nonblocking_discovery_metrics_and_journals() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 2, 30_960).await;
    let fleet = open_fleet(config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (_, first) = create_free_seat(&fleet, 69).await;
    let (_, second) = create_free_seat(&fleet, 70).await;

    let (generation, capability) = fleet.telemetry_registration_capability();
    assert_eq!(generation, 0);
    assert_eq!(
        fleet.telemetry_registration_capability(),
        (generation, capability.clone()),
        "an idempotent resend must reuse one generation/capability pair"
    );
    fleet.authorize_telemetry(&capability).unwrap();
    assert!(matches!(
        fleet.authorize_telemetry(
            &fedi_decentralized_service_fleet_manager::TelemetryCapability::from_bytes([0xff; 32])
        ),
        Err(TelemetryAccessError::Unauthorized)
    ));

    fleet.reenroll_telemetry().await.unwrap();
    tokio::time::timeout(
        Duration::from_millis(50),
        fleet.telemetry_registration_changed(),
    )
    .await
    .expect("rotation leaves a registration wake permit");
    let (rotated_generation, rotated) = fleet.telemetry_registration_capability();
    assert_eq!(rotated_generation, generation + 1);
    assert_ne!(rotated, capability);
    assert!(matches!(
        fleet.authorize_telemetry(&capability),
        Err(TelemetryAccessError::Unauthorized)
    ));
    fleet.authorize_telemetry(&rotated).unwrap();

    // Created seats have unavailable children. Discovery reads only the
    // shared cache, so it still lists both immediately with no invite.
    let seats = fleet.telemetry_seats();
    assert_eq!(seats.len(), 2);
    assert!(seats.iter().all(|seat| seat.invite_code.is_none()));
    assert_eq!(
        seats
            .iter()
            .map(|seat| seat.seat_id.clone())
            .collect::<Vec<_>>(),
        {
            let mut ids = vec![first.clone(), second.clone()];
            ids.sort();
            ids
        }
    );

    let mut expected = vec![
        SafeEventJournal::Fman,
        SafeEventJournal::Seat { seat_id: first },
        SafeEventJournal::Seat { seat_id: second },
    ];
    expected[1..].sort();
    assert_eq!(fleet.safe_event_journals(), expected);
    assert_eq!(
        fleet.safe_event_journal_dir(&SafeEventJournal::Fman),
        Some(config.process.data_root.join("safe-events/fman"))
    );

    fleet.shutdown().await;
}

#[tokio::test]
async fn global_telemetry_rotation_survives_restart() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_970).await;
    let fleet = open_fleet(config.clone(), Arc::new(NoWallet))
        .await
        .unwrap();
    let (original_generation, original) = fleet.telemetry_registration_capability();
    fleet.reenroll_telemetry().await.unwrap();
    let (rotated_generation, rotated) = fleet.telemetry_registration_capability();
    assert_eq!(rotated_generation, original_generation + 1);
    assert_ne!(rotated, original);
    fleet.shutdown().await;
    drop(fleet);

    let reopened = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    assert_eq!(
        reopened.telemetry_registration_capability(),
        (rotated_generation, rotated.clone())
    );
    assert!(matches!(
        reopened.authorize_telemetry(&original),
        Err(TelemetryAccessError::Unauthorized)
    ));
    reopened.authorize_telemetry(&rotated).unwrap();
    reopened.shutdown().await;
}

#[tokio::test]
async fn meta_write_refuses_unknown_keys_and_bad_values_without_submitting() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_960).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 63).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, None))
        .await;
    let baseline = fake.state();

    // A key with no compiled validator is refused rather than relayed.
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::Absent,
            MetaFieldKey("fedi:something_else".to_owned()),
            MetaFieldValue("anything".to_owned()),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaKeyRefused)
    ));
    let child_calls = fake.state();
    assert_eq!(child_calls.probe_calls, baseline.probe_calls);
    assert_eq!(
        child_calls.client_config_calls,
        baseline.client_config_calls
    );
    assert_eq!(
        child_calls.meta_consensus_calls,
        baseline.meta_consensus_calls
    );

    // An absolute resource cap still runs before any child call.
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::Absent,
            MetaFieldKey(
                fedi_decentralized_service_fleet_manager::FEDERATION_NAME_META_FIELD_KEY.to_owned(),
            ),
            MetaFieldValue(" ".repeat(65_537)),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));
    let child_calls = fake.state();
    assert_eq!(child_calls.probe_calls, baseline.probe_calls);
    assert_eq!(
        child_calls.client_config_calls,
        baseline.client_config_calls
    );
    assert_eq!(
        child_calls.meta_consensus_calls,
        baseline.meta_consensus_calls
    );
    assert!(child_calls.meta_submissions.is_empty());

    // Even an unknown key at the transport scale is classified without
    // touching the child or logging the attacker-controlled bytes.
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::Absent,
            MetaFieldKey("x".repeat(65_537)),
            MetaFieldValue("anything".to_owned()),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaKeyRefused)
    ));
    let child_calls = fake.state();
    assert_eq!(child_calls.probe_calls, baseline.probe_calls);
    assert_eq!(
        child_calls.client_config_calls,
        baseline.client_config_calls
    );
    assert_eq!(
        child_calls.meta_consensus_calls,
        baseline.meta_consensus_calls
    );

    // A directory that is not canonical JSON.
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::Absent,
            MetaFieldKey(fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY.to_owned()),
            MetaFieldValue("{\"version\":1}".to_owned()),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaKeyRefused)
    ));

    // A well-formed directory for a *different* federation: the FMan checks
    // the value against its own live config, not merely its shape.
    let mut other = fixture_client_config();
    other.global.consensus_version = fedimint_core::module::CoreConsensusVersion::new(2, 0);
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::Absent,
            MetaFieldKey(fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY.to_owned()),
            MetaFieldValue(fixture_directory(&other)),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaKeyRefused)
    ));

    assert!(
        fake.state().meta_submissions.is_empty(),
        "a refused proposal must never reach fedimintd"
    );
    fleet.shutdown().await;
}

#[tokio::test]
async fn set_meta_refuses_formation_owned_directory_and_recipient_keys() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_965).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 68).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, None))
        .await;
    let baseline = fake.state();

    for key in [
        fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY,
        crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY,
    ] {
        assert!(matches!(
            seat.submit_meta_field(
                MetaConsensusBase::Absent,
                MetaFieldKey(key.to_owned()),
                MetaFieldValue("not inspected".to_owned()),
                NO_FEE_FLOOR,
                None,
            )
            .await,
            Err(SeatVerbError::MetaKeyRefused)
        ));
    }
    let state = fake.state();
    assert_eq!(state.client_config_calls, baseline.client_config_calls);
    assert_eq!(state.meta_consensus_calls, baseline.meta_consensus_calls);
    assert!(state.meta_submissions.is_empty());
    fleet.shutdown().await;
}

#[tokio::test]
async fn meta_write_submits_the_directory_and_keeps_the_other_fields() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_970).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 64).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    // The federation already publishes an unrelated meta field; the write is a
    // read-modify-write over the whole object, so losing it would be silent.
    let existing = serde_json::to_vec(&serde_json::json!({ "fedi:welcome": "hello" })).unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(existing.clone())))
        .await;

    // This guardian is peer 0, so the honest directory binds that seat to
    // this install's own attestation key.
    let directory = fixture_directory_bound_to(
        &fixture_client_config(),
        0,
        &fleet.identity().derive_service_nostr_keys(),
    );
    seat.propose_formation_meta(
        MetaConsensusBase::from_consensus(Some((0, &existing))),
        formation_seat_bindings(&directory),
        fee_account(0x30),
        5_000,
        NO_FEE_FLOOR,
        guardian_verification_fee_account(),
    )
    .await
    .unwrap();

    // A different field is a distinct whole-object target and is valid only
    // after the first target has become the fresh consensus base.
    let adopted_directory = fake.state().meta_submissions[0].clone();
    let adopted_revision = fake.set_meta_consensus(Some(adopted_directory.clone()));

    // Guardianito validates the trimmed semantic value but submits the exact
    // original string. Preserve that behavior at the semantic maximum.
    let padded_name = format!(" {} ", "n".repeat(30));
    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((adopted_revision, &adopted_directory))),
        MetaFieldKey(
            fedi_decentralized_service_fleet_manager::FEDERATION_NAME_META_FIELD_KEY.to_owned(),
        ),
        MetaFieldValue(padded_name.clone()),
        NO_FEE_FLOOR,
        Some(guardian_verification_fee_account()),
    )
    .await
    .unwrap();

    let submissions = fake.state().meta_submissions;
    assert_eq!(submissions.len(), 2);
    let submitted: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&submissions[0]).unwrap();
    assert_eq!(
        submitted["fedi:welcome"],
        serde_json::Value::String("hello".to_owned()),
    );
    // A JSON *string* field, which is how FLIP's preview reads it back.
    assert_eq!(
        submitted[fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY],
        serde_json::Value::String(directory.clone()),
    );
    let padded_submission: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&submissions[1]).unwrap();
    assert_eq!(
        padded_submission[fedi_decentralized_service_fleet_manager::FEDERATION_NAME_META_FIELD_KEY],
        serde_json::Value::String(padded_name),
    );
    assert_eq!(
        padded_submission[fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY],
        serde_json::Value::String(directory),
    );
    // The vote is admin-authed; an unauthenticated submit is not a vote.
    assert_eq!(
        fake.state().meta_submission_auth[0].as_deref(),
        Some(
            fleet
                .identity()
                .derive_seat_keys(&seat_id)
                .api_auth
                .as_str()
        ),
    );
    fleet.shutdown().await;
}

#[tokio::test]
async fn generic_meta_write_revalidates_every_carried_guardian_fee_entitlement() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_972).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 70).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let ours = fleet.guardian_fee_account_descriptor(&seat_id);
    let guardian_verification_fee_account = guardian_verification_fee_account();
    let directory = guardian_fee_directory(&fixture_client_config(), &ours);
    let guardians = fixture_guardians(&ours);

    // This value is payer-parseable but substitutes the formation-selected FI
    // account while preserving its fixed weight.
    // An unrelated name vote must not copy it forward as this guardian's vote.
    let mut hostile_recipients = guardian_fee_recipients(&ours);
    let fi = hostile_recipients
        .iter_mut()
        .find(|recipient| recipient.weight == crate::guardian_fee::FI_RECIPIENT_WEIGHT)
        .unwrap();
    *fi = fee_recipient(fee_account(0x32), crate::guardian_fee::FI_RECIPIENT_WEIGHT);
    hostile_recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    let hostile_value =
        fedi_decentralized_service_fleet_manager::canonical_guardian_fee_recipient_list(
            &hostile_recipients,
        )
        .unwrap();
    let admitted_value = crate::guardian_fee::canonical_proposal(
        5_000,
        &guardian_fee_recipients(&ours),
        &guardians,
        &guardian_verification_fee_account,
    )
    .unwrap();
    pin_guardian_fee_policy(&fleet, &seat_id, &directory, &admitted_value).await;
    let hostile = serde_json_canonicalizer::to_vec(&std::collections::BTreeMap::from([
        (
            fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY,
            serde_json::Value::String(directory.clone()),
        ),
        (
            crate::guardian_fee::SEND_PPM_META_KEY,
            serde_json::Value::String("5000".to_owned()),
        ),
        (
            crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY,
            serde_json::Value::String(hostile_value),
        ),
    ]))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(hostile.clone())))
        .await;
    let reported = fleet.guardian_fee_policy(&seat_id).await.unwrap();
    assert_eq!(
        reported.our_share.map(|(weight, _)| weight),
        Some(crate::guardian_fee::GUARDIAN_RECIPIENT_WEIGHT),
        "the hostile policy deliberately keeps this seat's entry plausible"
    );
    assert!(
        !reported.share_matches_policy(),
        "a substituted FI account must not be reported as matching merely because this seat still has weight one"
    );
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::from_consensus(Some((0, &hostile))),
            MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
            MetaFieldValue("Never copied".to_owned()),
            NO_FEE_FLOOR,
            Some(guardian_verification_fee_account.clone())
        )
        .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));
    assert!(fake.state().meta_submissions.is_empty());

    let valid_value = crate::guardian_fee::canonical_proposal(
        5_000,
        &guardian_fee_recipients(&ours),
        &guardians,
        &guardian_verification_fee_account,
    )
    .unwrap();
    let valid = serde_json_canonicalizer::to_vec(&std::collections::BTreeMap::from([
        (
            fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY,
            serde_json::Value::String(directory),
        ),
        (
            crate::guardian_fee::SEND_PPM_META_KEY,
            serde_json::Value::String("5000".to_owned()),
        ),
        (
            crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY,
            serde_json::Value::String(valid_value),
        ),
    ]))
    .unwrap();
    let valid_revision = fake.set_meta_consensus(Some(valid.clone()));
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::from_consensus(Some((valid_revision, &valid))),
            MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
            MetaFieldValue("Production still fails closed".to_owned()),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));
    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((valid_revision, &valid))),
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Authenticated carry-forward".to_owned()),
        NO_FEE_FLOOR,
        Some(guardian_verification_fee_account),
    )
    .await
    .expect("a canonical authenticated policy can accompany maintenance");
    assert_eq!(fake.state().meta_submissions.len(), 1);
    fleet.shutdown().await;
}

#[tokio::test]
async fn carried_policy_rejects_a_threshold_replaced_self_signed_directory() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_973).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 70).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let ours = fleet.guardian_fee_account_descriptor(&seat_id);
    let verification = guardian_verification_fee_account();
    let admitted_directory = guardian_fee_directory(&fixture_client_config(), &ours);
    let admitted_recipients = crate::guardian_fee::canonical_proposal(
        5_000,
        &guardian_fee_recipients(&ours),
        &fixture_guardians(&ours),
        &verification,
    )
    .unwrap();
    pin_guardian_fee_policy(&fleet, &seat_id, &admitted_directory, &admitted_recipients).await;

    let hostile_directory = fixture_directory(&fixture_client_config());
    let hostile_guardians = (0..4)
        .map(|peer| fee_account(0x20 + peer))
        .collect::<Vec<_>>();
    let mut hostile_recipients = hostile_guardians
        .iter()
        .cloned()
        .map(|account| fee_recipient(account, crate::guardian_fee::GUARDIAN_RECIPIENT_WEIGHT))
        .collect::<Vec<_>>();
    hostile_recipients.push(fee_recipient(
        fee_account(0x30),
        crate::guardian_fee::FI_RECIPIENT_WEIGHT,
    ));
    hostile_recipients.push(fee_recipient(
        verification.clone(),
        crate::guardian_fee::GUARDIAN_VERIFICATION_FEE_WEIGHT,
    ));
    hostile_recipients.sort_by_key(|recipient| recipient.account.as_account().id());
    let hostile_recipients = crate::guardian_fee::canonical_proposal(
        5_000,
        &hostile_recipients,
        &hostile_guardians,
        &verification,
    )
    .unwrap();
    let hostile = serde_json_canonicalizer::to_vec(&std::collections::BTreeMap::from([
        (
            fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY,
            serde_json::Value::String(hostile_directory),
        ),
        (
            crate::guardian_fee::SEND_PPM_META_KEY,
            serde_json::Value::String("5000".to_owned()),
        ),
        (
            crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY,
            serde_json::Value::String(hostile_recipients),
        ),
    ]))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(hostile.clone())))
        .await;

    let reported = fleet.guardian_fee_policy(&seat_id).await.unwrap();
    assert!(!reported.share_matches_policy());
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::from_consensus(Some((0, &hostile))),
            MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
            MetaFieldValue("Never copied".to_owned()),
            NO_FEE_FLOOR,
            Some(verification),
        )
        .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));
    assert!(fake.state().meta_submissions.is_empty());
    fleet.shutdown().await;
}

#[tokio::test]
async fn carried_policy_rejects_a_non_string_directory_without_a_formation_pin() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_974).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 70).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let hostile = serde_json_canonicalizer::to_vec(&std::collections::BTreeMap::from([(
        fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY,
        serde_json::Value::Null,
    )]))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(hostile.clone())))
        .await;

    assert!(
        !fleet
            .guardian_fee_policy(&seat_id)
            .await
            .unwrap()
            .share_matches_policy()
    );
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::from_consensus(Some((0, &hostile))),
            MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
            MetaFieldValue("Never copied".to_owned()),
            NO_FEE_FLOOR,
            Some(guardian_verification_fee_account()),
        )
        .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));
    assert!(fake.state().meta_submissions.is_empty());
    fleet.shutdown().await;
}

#[tokio::test]
async fn meta_write_refuses_a_stale_base_without_submitting() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_975).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 65).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let existing = serde_json::to_vec(&serde_json::json!({ "existing": "preserve me" })).unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(existing.clone())))
        .await;

    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::Absent,
            MetaFieldKey(
                fedi_decentralized_service_fleet_manager::FEDERATION_NAME_META_FIELD_KEY.to_owned(),
            ),
            MetaFieldValue("New Federation".to_owned()),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaConsensusChanged)
    ));
    assert!(
        fake.state().meta_submissions.is_empty(),
        "a stale mutation must never cast a guardian vote"
    );
    fleet.shutdown().await;
}

#[tokio::test]
async fn meta_write_enforces_the_complete_object_cap_before_parse_and_submit() {
    fn padding_object(total_bytes: usize) -> Vec<u8> {
        let empty = serde_json_canonicalizer::to_vec(&serde_json::json!({ "padding": "" }))
            .expect("fixture canonicalizes");
        assert!(total_bytes >= empty.len());
        serde_json_canonicalizer::to_vec(&serde_json::json!({
            "padding": "a".repeat(total_bytes - empty.len())
        }))
        .expect("fixture canonicalizes")
    }

    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_976).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 66).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();

    let target_without_padding =
        serde_json_canonicalizer::to_vec(&std::collections::BTreeMap::from([
            (
                FEDERATION_NAME_META_FIELD_KEY,
                serde_json::Value::String("Bounded Federation".to_owned()),
            ),
            ("padding", serde_json::Value::String(String::new())),
        ]))
        .unwrap();
    let padding = "a".repeat(FEDERATION_METADATA_OBJECT_MAX_BYTES - target_without_padding.len());
    let existing = serde_json_canonicalizer::to_vec(&serde_json::json!({
        "padding": padding,
    }))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(existing.clone())))
        .await;
    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((0, &existing))),
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Bounded Federation".to_owned()),
        NO_FEE_FLOOR,
        None,
    )
    .await
    .expect("the inclusive complete-object cap is accepted");
    assert_eq!(
        fake.state().meta_submissions[0].len(),
        FEDERATION_METADATA_OBJECT_MAX_BYTES
    );

    let target_oversized_padding =
        "a".repeat(FEDERATION_METADATA_OBJECT_MAX_BYTES - target_without_padding.len() + 1);
    let under_cap_source = serde_json_canonicalizer::to_vec(&serde_json::json!({
        "padding": target_oversized_padding,
    }))
    .unwrap();
    assert!(under_cap_source.len() < FEDERATION_METADATA_OBJECT_MAX_BYTES);
    let fake = fleet
        .form_fake_child(
            &seat_id,
            running_federation(0, Some(under_cap_source.clone())),
        )
        .await;
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::from_consensus(Some((0, &under_cap_source))),
            MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
            MetaFieldValue("Bounded Federation".to_owned()),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));
    assert!(
        fake.state().meta_submissions.is_empty(),
        "an under-cap source whose merged target exceeds the cap is never submitted"
    );

    let oversized = padding_object(FEDERATION_METADATA_OBJECT_MAX_BYTES + 1);
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(oversized.clone())))
        .await;
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::from_consensus(Some((0, &oversized))),
            MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
            MetaFieldValue("Never Parsed".to_owned()),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));
    let calls = fake.state();
    assert_eq!(calls.meta_consensus_calls, 1);
    assert!(calls.meta_submissions.is_empty());
    fleet.shutdown().await;
}

#[tokio::test]
async fn same_base_admission_fences_a_handler_delayed_before_the_seat_queue() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_977).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 67).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let original = serde_json_canonicalizer::to_vec(&serde_json::json!({
        "unrelated": "preserved",
    }))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(original.clone())))
        .await;
    let base = MetaConsensusBase::from_consensus(Some((0, &original)));

    // Logical request A exists first but is held before `commands.send`, as an
    // independently scheduled Iroh handler can be. Request B reaches the
    // single-owner seat loop first and submits a different target for O.
    let release_a = Arc::new(tokio::sync::Notify::new());
    let mut delayed_a = tokio::spawn({
        let seat = seat.clone();
        let release_a = release_a.clone();
        async move {
            release_a.notified().await;
            seat.submit_meta_field(
                base,
                MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
                MetaFieldValue("Older A".to_owned()),
                NO_FEE_FLOOR,
                None,
            )
            .await
        }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut delayed_a)
            .await
            .is_err(),
        "logical request A is held before entering the seat queue"
    );

    seat.submit_meta_field(
        base,
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Newer B".to_owned()),
        NO_FEE_FLOOR,
        None,
    )
    .await
    .expect("request B submits first");
    assert_eq!(fake.state().meta_submissions.len(), 1);

    // Consensus is deliberately still O. Without the per-base admission
    // fence, delayed A would pass the ordinary base check here and cast a
    // conflicting late whole-object vote. The refusal is the distinct
    // pinned-target error, not the retryable stale-base answer: rereading
    // cannot clear it while consensus stays at O.
    release_a.notify_one();
    assert!(matches!(
        delayed_a.await.unwrap(),
        Err(SeatVerbError::MetaTargetConflict)
    ));
    let state = fake.state();
    assert_eq!(state.meta_consensus.as_deref(), Some(original.as_slice()));
    assert_eq!(state.meta_submissions.len(), 1);
    let accepted: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&state.meta_submissions[0]).unwrap();
    assert_eq!(
        accepted.get(FEDERATION_NAME_META_FIELD_KEY),
        Some(&serde_json::Value::String("Newer B".to_owned()))
    );
    assert_eq!(
        accepted.get("unrelated"),
        Some(&serde_json::Value::String("preserved".to_owned()))
    );

    // Once peers adopt B, the old request is a plain stale base — the
    // ordinary retryable `MetaConsensusChanged`, byte-identical to before the
    // pinned-target distinction — and no A proposal can restore the original
    // whole object.
    fake.set_meta_consensus(Some(state.meta_submissions[0].clone()));
    assert!(matches!(
        seat.submit_meta_field(
            base,
            MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
            MetaFieldValue("Older A".to_owned()),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaConsensusChanged)
    ));
    assert_eq!(fake.state().meta_submissions.len(), 1);
    fleet.shutdown().await;
}

#[tokio::test]
async fn same_base_admission_survives_an_ambiguous_submit_response() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_978).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 68).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let original = serde_json_canonicalizer::to_vec(&serde_json::json!({
        "unrelated": "preserved",
    }))
    .unwrap();
    let fake = fleet
        .form_fake_child(
            &seat_id,
            FakeApiState {
                fail_meta_submit_after_record_once: true,
                ..running_federation(0, Some(original.clone()))
            },
        )
        .await;
    let base = MetaConsensusBase::from_consensus(Some((0, &original)));

    let first = seat
        .submit_meta_field(
            base,
            MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
            MetaFieldValue("Admitted B".to_owned()),
            NO_FEE_FLOOR,
            None,
        )
        .await;
    assert!(first.is_err(), "the fake withholds the success response");
    assert_eq!(fake.state().meta_submissions.len(), 1);

    assert!(matches!(
        seat.submit_meta_field(
            base,
            MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
            MetaFieldValue("Conflicting A".to_owned()),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaTargetConflict)
    ));
    assert_eq!(fake.state().meta_submissions.len(), 1);

    seat.submit_meta_field(
        base,
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Admitted B".to_owned()),
        NO_FEE_FLOOR,
        None,
    )
    .await
    .expect("exact replay remains permitted after an ambiguous response");
    let state = fake.state();
    assert_eq!(state.meta_submissions.len(), 2);
    assert_eq!(state.meta_submissions[0], state.meta_submissions[1]);
    fleet.shutdown().await;
}

#[tokio::test]
async fn recurring_content_under_a_fresh_revision_stales_a_delayed_handler() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_979).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 69).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let original = serde_json_canonicalizer::to_vec(&std::collections::BTreeMap::from([
        (
            FEDERATION_NAME_META_FIELD_KEY,
            serde_json::Value::String("Original O".to_owned()),
        ),
        (
            "unrelated",
            serde_json::Value::String("preserved".to_owned()),
        ),
    ]))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(original.clone())))
        .await;
    let original_base = MetaConsensusBase::from_consensus(Some((0, &original)));

    let release_a = Arc::new(tokio::sync::Notify::new());
    let delayed_a = tokio::spawn({
        let seat = seat.clone();
        let release_a = release_a.clone();
        async move {
            release_a.notified().await;
            seat.submit_meta_field(
                original_base,
                MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
                MetaFieldValue("Delayed A".to_owned()),
                NO_FEE_FLOOR,
                None,
            )
            .await
        }
    });

    seat.submit_meta_field(
        original_base,
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Newer B".to_owned()),
        NO_FEE_FLOOR,
        None,
    )
    .await
    .expect("O to B submits");
    let target_b = fake.state().meta_submissions[0].clone();
    let revision_b = fake.set_meta_consensus(Some(target_b.clone()));

    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((revision_b, &target_b))),
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Original O".to_owned()),
        NO_FEE_FLOOR,
        None,
    )
    .await
    .expect("B to exact O submits");
    assert_eq!(
        fake.state().meta_submissions[1],
        original,
        "the second adoption restores the exact original bytes"
    );
    fake.set_meta_consensus(Some(original));

    // The board returned to byte-exact O, but under a fresh consensus
    // revision: the delayed handler's old-occurrence base no longer names
    // live consensus, so it is refused as an ordinary stale base — before,
    // and instead of, any admission-map consultation.
    release_a.notify_one();
    assert!(matches!(
        delayed_a.await.unwrap(),
        Err(SeatVerbError::MetaConsensusChanged)
    ));
    assert_eq!(
        fake.state().meta_submissions.len(),
        2,
        "a delayed handler from an earlier occurrence casts no late vote"
    );
    fleet.shutdown().await;
}

#[tokio::test]
async fn superseded_pin_is_discarded_when_the_base_moves() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_983).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 72).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let original = serde_json_canonicalizer::to_vec(&serde_json::json!({
        "unrelated": "preserved",
    }))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(original.clone())))
        .await;

    // Pin the first occurrence to target one.
    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((0, &original))),
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("First Target".to_owned()),
        NO_FEE_FLOOR,
        None,
    )
    .await
    .expect("the first occurrence admits its target");
    let adopted = fake.state().meta_submissions[0].clone();
    let revision = fake.set_meta_consensus(Some(adopted.clone()));

    // The base moved: the old pin is for an occurrence that can never recur,
    // so a different target on the live occurrence is admitted, not
    // conflicted against the dead pin.
    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((revision, &adopted))),
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Second Target".to_owned()),
        NO_FEE_FLOOR,
        None,
    )
    .await
    .expect("a superseded pin does not fence the live occurrence");
    assert_eq!(fake.state().meta_submissions.len(), 2);

    // The replacement pin is live: equivocation on the same occurrence is
    // still conflicted, and exact replay still passes.
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::from_consensus(Some((revision, &adopted))),
            MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
            MetaFieldValue("Third Target".to_owned()),
            NO_FEE_FLOOR,
            None
        )
        .await,
        Err(SeatVerbError::MetaTargetConflict)
    ));
    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((revision, &adopted))),
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Second Target".to_owned()),
        NO_FEE_FLOOR,
        None,
    )
    .await
    .expect("exact replay of the live pin remains permitted");
    let submissions = fake.state().meta_submissions;
    assert_eq!(submissions.len(), 3);
    assert_eq!(submissions[1], submissions[2]);
    fleet.shutdown().await;
}

#[tokio::test]
async fn revert_then_depart_in_a_new_direction_does_not_wedge() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_982).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 71).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let state_a = serde_json_canonicalizer::to_vec(&std::collections::BTreeMap::from([
        (
            FEDERATION_NAME_META_FIELD_KEY,
            serde_json::Value::String("Name A".to_owned()),
        ),
        (
            "unrelated",
            serde_json::Value::String("preserved".to_owned()),
        ),
    ]))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(state_a.clone())))
        .await;

    // Rename A -> B: admits target B for A's first occurrence.
    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((0, &state_a))),
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Name B".to_owned()),
        NO_FEE_FLOOR,
        None,
    )
    .await
    .expect("A to B submits");
    let state_b = fake.state().meta_submissions[0].clone();
    let revision_b = fake.set_meta_consensus(Some(state_b.clone()));

    // Revert B -> A: consensus returns to byte-exact A under a fresh revision.
    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((revision_b, &state_b))),
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Name A".to_owned()),
        NO_FEE_FLOOR,
        None,
    )
    .await
    .expect("B back to exact A submits");
    assert_eq!(fake.state().meta_submissions[1], state_a);
    let revision_a_again = fake.set_meta_consensus(Some(state_a.clone()));

    // Depart in a new direction, A -> C. Content A recurred, but this is a
    // fresh occurrence with a fresh base, so the old admission (A's first
    // occurrence -> B) cannot pin it: ordinary do/undo/redo-differently must
    // never wedge until processes restart.
    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((revision_a_again, &state_a))),
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Name C".to_owned()),
        NO_FEE_FLOOR,
        None,
    )
    .await
    .expect("recurred A departs toward C without a pinned-target refusal");
    let submissions = fake.state().meta_submissions;
    assert_eq!(submissions.len(), 3);
    let adopted: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&submissions[2]).unwrap();
    assert_eq!(
        adopted.get(FEDERATION_NAME_META_FIELD_KEY),
        Some(&serde_json::Value::String("Name C".to_owned()))
    );
    assert_eq!(
        adopted.get("unrelated"),
        Some(&serde_json::Value::String("preserved".to_owned()))
    );
    fleet.shutdown().await;
}

#[tokio::test]
async fn formation_meta_requires_paired_endpoint_proof_identity_and_key() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_982).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 71).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, None))
        .await;
    let directory = fixture_directory_bound_to(
        &fixture_client_config(),
        0,
        &fleet.identity().derive_service_nostr_keys(),
    );
    let bindings = formation_seat_bindings(&directory);

    // A proof lifted from another seat: signed by a genuine endpoint key, but
    // over that seat's attestation digest rather than this one's.
    let mut wrong_peer = bindings.clone();
    wrong_peer[1].endpoint_proof = wrong_peer[0].endpoint_proof.clone();
    assert!(matches!(
        seat.propose_formation_meta(
            MetaConsensusBase::Absent,
            wrong_peer,
            fee_account(0x30),
            1_500,
            1_500,
            guardian_verification_fee_account(),
        )
        .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));

    let mut wrong_key = bindings.clone();
    wrong_key[1].endpoint_proof.signature = iroh_base_035::SecretKey::from_bytes(&[0x7f; 32])
        .sign(
            &fedi_decentralized_domain::FmanSeatBindings::parse_canonical(&directory)
                .unwrap()
                .seat_bindings()[1]
                .attestation
                .seat_endpoint_proof_message()
                .unwrap(),
        )
        .to_bytes()
        .to_vec();
    assert!(matches!(
        seat.propose_formation_meta(
            MetaConsensusBase::Absent,
            wrong_key,
            fee_account(0x30),
            1_500,
            1_500,
            guardian_verification_fee_account(),
        )
        .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));
    assert!(fake.state().meta_submissions.is_empty());

    seat.propose_formation_meta(
        MetaConsensusBase::Absent,
        bindings,
        fee_account(0x30),
        1_500,
        1_500,
        guardian_verification_fee_account(),
    )
    .await
    .unwrap();
    let submissions = fake.state().meta_submissions;
    assert_eq!(submissions.len(), 1);
    let submitted: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&submissions[0]).unwrap();
    assert_eq!(
        submitted.get(fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY),
        Some(&serde_json::Value::String(directory))
    );
    assert_eq!(
        submitted.get(crate::guardian_fee::SEND_PPM_META_KEY),
        Some(&serde_json::Value::String("1500".to_owned()))
    );
    assert!(submitted.contains_key(crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY));
    fleet.shutdown().await;
}

#[tokio::test]
async fn formation_meta_rejects_rate_below_floor_before_child_access() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_983).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 72).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, None))
        .await;
    let baseline = fake.state();

    assert!(matches!(
        seat.propose_formation_meta(
            MetaConsensusBase::Absent,
            vec![],
            fee_account(0x30),
            1_499,
            1_500,
            guardian_verification_fee_account(),
        )
        .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));
    let state = fake.state();
    assert_eq!(state.client_config_calls, baseline.client_config_calls);
    assert_eq!(state.meta_consensus_calls, baseline.meta_consensus_calls);
    fleet.shutdown().await;
}

#[tokio::test]
async fn formation_meta_distinguishes_an_already_published_directory() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_984).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 73).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let directory = fixture_directory_bound_to(
        &fixture_client_config(),
        0,
        &fleet.identity().derive_service_nostr_keys(),
    );
    let existing = serde_json::to_vec(&serde_json::json!({
        fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY: directory,
    }))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(existing.clone())))
        .await;

    assert!(matches!(
        seat.propose_formation_meta(
            MetaConsensusBase::from_consensus(Some((0, &existing))),
            vec![],
            fee_account(0x30),
            5_000,
            NO_FEE_FLOOR,
            guardian_verification_fee_account(),
        )
        .await,
        Err(SeatVerbError::FormationMetaAlreadyPublished)
    ));
    assert!(fake.state().meta_submissions.is_empty());
    fleet.shutdown().await;
}

#[tokio::test]
async fn fee_and_metadata_proposals_cross_block_on_a_shared_admitted_base() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_981).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 70).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let ours = fleet.guardian_fee_account_descriptor(&seat_id);
    let directory = guardian_fee_directory(&fixture_client_config(), &ours);
    let recipients = crate::guardian_fee::canonical_proposal(
        5,
        &guardian_fee_recipients(&ours),
        &fixture_guardians(&ours),
        &guardian_verification_fee_account(),
    )
    .unwrap();
    pin_guardian_fee_policy(&fleet, &seat_id, &directory, &recipients).await;
    let original = serde_json_canonicalizer::to_vec(&std::collections::BTreeMap::from([
        (
            fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY,
            serde_json::Value::String(directory),
        ),
        (
            crate::guardian_fee::SEND_PPM_META_KEY,
            serde_json::Value::String("5".to_owned()),
        ),
        (
            crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY,
            serde_json::Value::String(recipients),
        ),
        (
            "unrelated",
            serde_json::Value::String("preserved".to_owned()),
        ),
    ]))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(original.clone())))
        .await;

    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((0, &original))),
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Metadata First".to_owned()),
        NO_FEE_FLOOR,
        Some(guardian_verification_fee_account()),
    )
    .await
    .expect("the metadata mutation admits base O first");
    assert_eq!(fake.state().meta_submissions.len(), 1);

    // The fee verb commits to the same base O and shares the live target pin,
    // so its policy-valid but different whole-object target is the distinct
    // pinned refusal, not a stale base: fee and metadata proposals
    // cross-block until consensus moves or the process restarts.
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::from_consensus(Some((0, &original))),
            MetaFieldKey(crate::guardian_fee::SEND_PPM_META_KEY.to_owned()),
            MetaFieldValue((42).to_string()),
            NO_FEE_FLOOR,
            Some(guardian_verification_fee_account())
        )
        .await,
        Err(SeatVerbError::MetaTargetConflict)
    ));
    assert_eq!(
        fake.state().meta_submissions.len(),
        1,
        "the cross-blocked fee proposal must never cast a conflicting vote"
    );
    fleet.shutdown().await;
}

#[tokio::test]
async fn guardian_fee_proposal_uses_the_same_stale_base_guard() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 31_010).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 65).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let existing = serde_json::to_vec(&serde_json::json!({ "existing": "preserve me" })).unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(existing)))
        .await;
    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::Absent,
            MetaFieldKey(crate::guardian_fee::SEND_PPM_META_KEY.to_owned()),
            MetaFieldValue((42).to_string()),
            NO_FEE_FLOOR,
            Some(guardian_verification_fee_account())
        )
        .await,
        Err(SeatVerbError::MetaConsensusChanged)
    ));
    assert!(
        fake.state().meta_submissions.is_empty(),
        "a stale fee mutation must never cast a guardian vote"
    );
    fleet.shutdown().await;
}

#[tokio::test]
async fn set_meta_fee_rate_refuses_invalid_values_without_child_access() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_980).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 65).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, None))
        .await;
    let baseline = fake.state();

    for value in [
        "not-a-number".to_owned(),
        (crate::guardian_fee::MAX_SEND_PPM + 1).to_string(),
        "1499".to_owned(),
    ] {
        assert!(matches!(
            seat.submit_meta_field(
                MetaConsensusBase::Absent,
                MetaFieldKey(crate::guardian_fee::SEND_PPM_META_KEY.to_owned()),
                MetaFieldValue(value),
                1_500,
                Some(guardian_verification_fee_account()),
            )
            .await,
            Err(SeatVerbError::MetaValueInvalid)
        ));
    }
    let state = fake.state();
    assert_eq!(state.client_config_calls, baseline.client_config_calls);
    assert_eq!(state.meta_consensus_calls, baseline.meta_consensus_calls);
    assert!(state.meta_submissions.is_empty());
    fleet.shutdown().await;
}

#[tokio::test]
async fn seat_status_guardian_fee_reads_the_seat_without_a_wallet_client() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_985).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 66).await;
    let _seat = fleet.seat_by_id(&seat_id).unwrap();
    let ours = fleet.guardian_fee_account_descriptor(&seat_id);
    let guardian_verification_fee_account = guardian_verification_fee_account();
    let recipients = crate::guardian_fee::canonical_proposal(
        42,
        &guardian_fee_recipients(&ours),
        &fixture_guardians(&ours),
        &guardian_verification_fee_account,
    )
    .unwrap();
    let directory = guardian_fee_directory(&fixture_client_config(), &ours);
    pin_guardian_fee_policy(&fleet, &seat_id, &directory, &recipients).await;
    let meta = serde_json::to_vec(&std::collections::BTreeMap::from([
        (
            fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY,
            serde_json::Value::String(directory),
        ),
        (
            crate::guardian_fee::SEND_PPM_META_KEY,
            serde_json::Value::String("42".to_owned()),
        ),
        (
            crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY,
            serde_json::Value::String(recipients),
        ),
    ]))
    .unwrap();
    let _fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(meta)))
        .await;

    let guardian_fee = crate::admin::read_seat_guardian_fee(&fleet, &seat_id).await;
    assert_eq!(guardian_fee["share_matches_policy"], true);
    assert_eq!(guardian_fee["send_ppm"], 42);
    assert_eq!(guardian_fee["our_weight"], 1);
    assert_eq!(guardian_fee["total_weight"], 9);
    assert!(guardian_fee.get("policy_error").is_none());
    fleet.shutdown().await;
}

#[tokio::test]
async fn guardian_fee_proposal_replaces_partial_live_metadata() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 30_990).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 66).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let ours = fleet.guardian_fee_account_descriptor(&seat_id);
    // The live source is deliberately partial. The proposal is judged on its
    // own contents and updates both values; it does not compare itself to a
    // previous policy or consult config metadata.
    let existing = serde_json::to_vec(&serde_json::json!({
        "fedi:guardian_fee_send_ppm": "5",
        "fedi:fman_seat_bindings": guardian_fee_directory(&fixture_client_config(), &ours),
        "fedi:welcome": "hello",
    }))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(existing.clone())))
        .await;
    let guardian_verification_fee_account = guardian_verification_fee_account();
    let _recipients = guardian_fee_recipients(&ours);

    assert!(matches!(
        seat.submit_meta_field(
            MetaConsensusBase::from_consensus(Some((0, &existing))),
            MetaFieldKey(crate::guardian_fee::SEND_PPM_META_KEY.to_owned()),
            MetaFieldValue("42".to_owned()),
            NO_FEE_FLOOR,
            Some(guardian_verification_fee_account),
        )
        .await,
        Err(SeatVerbError::MetaValueInvalid)
    ));

    assert!(fake.state().meta_submissions.is_empty());
    fleet.shutdown().await;
}

/// The published floor is enforced by this FMan: a CLI-driven FI proposing
/// below it gets no vote, and the refusal happens before the seat touches its
/// child.
#[tokio::test]
async fn guardian_fee_proposal_below_the_published_minimum_casts_no_vote() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 31_020).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 71).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let ours = fleet.guardian_fee_account_descriptor(&seat_id);
    let directory = guardian_fee_directory(&fixture_client_config(), &ours);
    let recipients = crate::guardian_fee::canonical_proposal(
        5,
        &guardian_fee_recipients(&ours),
        &fixture_guardians(&ours),
        &guardian_verification_fee_account(),
    )
    .unwrap();
    pin_guardian_fee_policy(&fleet, &seat_id, &directory, &recipients).await;
    let existing = serde_json_canonicalizer::to_vec(&std::collections::BTreeMap::from([
        (
            fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY,
            serde_json::Value::String(directory),
        ),
        (
            crate::guardian_fee::SEND_PPM_META_KEY,
            serde_json::Value::String("5".to_owned()),
        ),
        (
            crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY,
            serde_json::Value::String(recipients),
        ),
    ]))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(existing.clone())))
        .await;
    let floor = fedi_decentralized_domain::DEFAULT_SETUP_PAYMENT_MIN_FEE_PPM;

    for send_ppm in [0, floor - 1] {
        assert!(matches!(
            seat.submit_meta_field(
                MetaConsensusBase::from_consensus(Some((0, &existing))),
                MetaFieldKey(crate::guardian_fee::SEND_PPM_META_KEY.to_owned()),
                MetaFieldValue((send_ppm).to_string()),
                floor,
                Some(guardian_verification_fee_account())
            )
            .await,
            Err(SeatVerbError::MetaValueInvalid)
        ));
    }
    let refused = fake.state();
    assert!(
        refused.meta_submissions.is_empty(),
        "a sub-minimum proposal must never cast a guardian vote"
    );
    assert_eq!(
        refused.client_config_calls, 0,
        "the floor is checked before the seat reaches its child"
    );

    // Exactly at the floor is acceptable: the bound is inclusive.
    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((0, &existing))),
        MetaFieldKey(crate::guardian_fee::SEND_PPM_META_KEY.to_owned()),
        MetaFieldValue((floor).to_string()),
        floor,
        Some(guardian_verification_fee_account()),
    )
    .await
    .expect("a proposal at the published minimum is admitted");
    let state = fake.state();
    assert_eq!(state.meta_submissions.len(), 1);
    let submitted: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&state.meta_submissions[0]).unwrap();
    assert_eq!(submitted["fedi:guardian_fee_send_ppm"], floor.to_string());
    fleet.shutdown().await;
}

/// The regression the placement of the floor check protects: a federation that
/// agreed a rate below a later-published minimum stays maintainable. An
/// unrelated field update revalidates the fee keys it carries forward, and
/// that revalidation must not apply the floor — otherwise a rename would be
/// unvotable and the federation would freeze.
#[tokio::test]
async fn an_unrelated_meta_write_carries_a_sub_minimum_fee_rate_forward() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 31_024).await;
    let fleet = open_fleet(config, Arc::new(NoWallet)).await.unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 72).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let ours = fleet.guardian_fee_account_descriptor(&seat_id);
    // 5 ppm: a policy this federation legitimately adopted, far below the
    // 1,500-ppm published minimum. Built through the real
    // canonicalizer, which also pins that it accepts a sub-minimum rate.
    let carried_send_ppm = "5";
    let carried_recipients = crate::guardian_fee::canonical_proposal(
        5,
        &guardian_fee_recipients(&ours),
        &fixture_guardians(&ours),
        &guardian_verification_fee_account(),
    )
    .expect("a rate below the published floor is still a canonical fee policy");
    let directory = guardian_fee_directory(&fixture_client_config(), &ours);
    pin_guardian_fee_policy(&fleet, &seat_id, &directory, &carried_recipients).await;
    let existing = serde_json_canonicalizer::to_vec(&std::collections::BTreeMap::from([
        (
            fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY,
            serde_json::Value::String(directory),
        ),
        (
            crate::guardian_fee::SEND_PPM_META_KEY,
            serde_json::Value::String(carried_send_ppm.to_owned()),
        ),
        (
            crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY,
            serde_json::Value::String(carried_recipients.clone()),
        ),
    ]))
    .unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(existing.clone())))
        .await;

    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((0, &existing))),
        MetaFieldKey(FEDERATION_NAME_META_FIELD_KEY.to_owned()),
        MetaFieldValue("Renamed".to_owned()),
        NO_FEE_FLOOR,
        Some(guardian_verification_fee_account()),
    )
    .await
    .expect("renaming a federation whose agreed rate is below the floor must still work");

    let state = fake.state();
    assert_eq!(state.meta_submissions.len(), 1);
    let submitted: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&state.meta_submissions[0]).unwrap();
    assert_eq!(
        submitted[FEDERATION_NAME_META_FIELD_KEY],
        serde_json::Value::String("Renamed".to_owned())
    );
    assert_eq!(
        submitted[crate::guardian_fee::SEND_PPM_META_KEY],
        serde_json::Value::String(carried_send_ppm.to_owned()),
        "the carried sub-minimum rate is preserved, not raised or dropped"
    );
    assert_eq!(
        submitted[crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY],
        serde_json::Value::String(carried_recipients)
    );
    fleet.shutdown().await;
}

/// One proposal against one seat, over a meta map the fake serves in the
/// given byte order. The seat is fixed across calls on purpose: the account
/// id is derived per seat, so two seats could not vote for the same list and
/// comparing their submissions would prove nothing about canonicalization.
async fn canonical_guardian_fee_submission(
    fleet: &Fleet,
    seat_id: &SeatId,
    current: &[u8],
    directory: &str,
) -> Vec<u8> {
    let seat = fleet.seat_by_id(seat_id).unwrap();
    let ours = fleet.guardian_fee_account_descriptor(seat_id);
    let mut current_fields: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(current).unwrap();
    current_fields.insert(
        fedi_decentralized_domain::FMAN_SEAT_BINDINGS_META_FIELD_KEY.to_owned(),
        serde_json::Value::String(directory.to_owned()),
    );
    current_fields.insert(
        crate::guardian_fee::SEND_PPM_META_KEY.to_owned(),
        serde_json::Value::String("5".to_owned()),
    );
    let recipients = crate::guardian_fee::canonical_proposal(
        5,
        &guardian_fee_recipients(&ours),
        &fixture_guardians(&ours),
        &guardian_verification_fee_account(),
    )
    .unwrap();
    pin_guardian_fee_policy(fleet, seat_id, directory, &recipients).await;
    current_fields.insert(
        crate::guardian_fee::REMITTANCE_ACCOUNT_META_KEY.to_owned(),
        serde_json::Value::String(recipients),
    );
    let current = serde_json::to_vec(&current_fields).unwrap();
    let fake = fleet
        .form_fake_child(&seat_id, running_federation(0, Some(current.clone())))
        .await;
    seat.submit_meta_field(
        MetaConsensusBase::from_consensus(Some((0, &current))),
        MetaFieldKey(crate::guardian_fee::SEND_PPM_META_KEY.to_owned()),
        MetaFieldValue((42).to_string()),
        NO_FEE_FLOOR,
        Some(guardian_verification_fee_account()),
    )
    .await
    .unwrap();
    let submission = fake.state().meta_submissions.into_iter().next().unwrap();
    submission
}

#[tokio::test]
async fn guardian_fee_proposal_canonicalizes_meta_map_independent_of_input_order() {
    let temp = TempDir::new().unwrap();
    let fleet = open_fleet(config(&temp, 1, 31_000).await, Arc::new(NoWallet))
        .await
        .unwrap();
    let (_, seat_id) = create_free_seat(&fleet, 67).await;
    let directory = guardian_fee_directory(
        &fixture_client_config(),
        &fleet.guardian_fee_account_descriptor(&seat_id),
    );

    let first = canonical_guardian_fee_submission(
        &fleet,
        &seat_id,
        br#"{"fedi:welcome":"hello","other":"value"}"#,
        &directory,
    )
    .await;
    let second = canonical_guardian_fee_submission(
        &fleet,
        &seat_id,
        br#"{"other":"value","fedi:welcome":"hello"}"#,
        &directory,
    )
    .await;

    assert_eq!(first, second);
    fleet.shutdown().await;
}

/// Records what the durability gate published, and can be made to fail the
/// way a relay outage would.
struct FakeBackupSink {
    published: std::sync::Mutex<Vec<crate::backup::SeatBackupDocument>>,
    fails: std::sync::atomic::AtomicBool,
}

impl FakeBackupSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            published: std::sync::Mutex::new(Vec::new()),
            fails: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn published(&self) -> Vec<crate::backup::SeatBackupDocument> {
        self.published.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl crate::backup::BackupSink for FakeBackupSink {
    async fn publish(&self, publication: &crate::backup::SeatPublication) -> anyhow::Result<()> {
        if self.fails.load(std::sync::atomic::Ordering::SeqCst) {
            anyhow::bail!("relay is down");
        }
        // Only seat documents are what these tests are about; the guardian
        // archive rides along and is checked in `restore`.
        self.published
            .lock()
            .unwrap()
            .push(publication.document.clone());
        Ok(())
    }

    fn format_version(&self) -> u32 {
        1
    }
}

/// Write the config files fedimintd leaves behind once DKG completes.
async fn write_guardian_config_files(config: &FleetConfig, seat_no: crate::facts::SeatNo) {
    let dir = seat_data_dir(&config.process, seat_no);
    tokio::fs::create_dir_all(&dir).await.unwrap();
    for (name, contents) in [
        ("private.encrypt", "deadbeef"),
        ("private.salt", "c2FsdA"),
        ("local.json", r#"{"api_bind":"127.0.0.1:1"}"#),
        ("consensus.json", r#"{"api_endpoints":{}}"#),
    ] {
        tokio::fs::write(dir.join(name), contents).await.unwrap();
    }
}

/// Run the ceremony for real against a fake in setup mode (it reaches
/// consensus on `start_dkg`). The archive's publication is gated on the
/// durable formed record, so a test that wants a guardian-carrying document
/// must complete a real session, not just fake an API already serving consensus.
async fn run_test_dkg(fleet: &Fleet, fi_id: &FiId, seat_id: &SeatId) -> FakeSeatChildHandle {
    let _ = fi_id;
    fleet
        .form_fake_child(seat_id, FakeApiState::default())
        .await
}

/// Publication happens off every foreground path now, so a test that wants to
/// observe one waits for it instead of assuming the call that triggered it
/// also performed it.
async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what} never happened");
}

async fn fleet_with_backup(config: FleetConfig, sink: Arc<FakeBackupSink>) -> Fleet {
    Fleet::open_with_wallet(
        Db::open(&config.process.data_root).await.unwrap(),
        config,
        async |_| Ok(Arc::new(NoWallet) as _),
        async |_| Ok(sink as _),
    )
    .await
    .unwrap()
}

/// The relay is the FMan's second copy, not a prerequisite for its function.
/// A wedged relay must cost a formed seat nothing: the invite code is public
/// information every peer already serves, and withholding this guardian's copy
/// would break the FI without protecting anyone.
#[tokio::test]
async fn a_wedged_relay_never_withholds_an_invite_code() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 31_100).await;
    let sink = FakeBackupSink::new();
    let fleet = fleet_with_backup(config.clone(), sink.clone()).await;
    let (fi_id, seat_id) = create_free_seat(&fleet, 41).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();

    // Creating the seat queues one document, long before DKG: it carries the
    // seat's facts but no guardian config yet.
    eventually("the creation document is published", || {
        sink.published().len() == 1
    })
    .await;
    assert!(sink.published()[0].guardian.is_none());

    let _fake = run_test_dkg(&fleet, &fi_id, &seat_id).await;
    write_guardian_config_files(&config, seat.facts().seat_no).await;
    sink.fails.store(true, std::sync::atomic::Ordering::SeqCst);

    assert_eq!(
        seat.invite_code().await.unwrap(),
        InviteCode("fed11-fake-invite".to_owned())
    );
    let report = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let report = seat.report().await.unwrap();
            if matches!(
                report,
                SeatReport::Active {
                    phase: SeatPhase::Running { .. },
                    health: SeatHealth::Healthy,
                }
            ) {
                return report;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the watchdog observes the running fake");
    assert_eq!(
        report,
        SeatReport::Active {
            phase: SeatPhase::Running {
                invite_code: InviteCode("fed11-fake-invite".to_owned())
            },
            health: SeatHealth::Healthy,
        }
    );

    // The seat stayed queued the whole time, so the config-carrying document
    // lands under its own steam once the relay is back.
    sink.fails.store(false, std::sync::atomic::Ordering::SeqCst);
    eventually("the retry lands the config-carrying document", || {
        sink.published()
            .last()
            .is_some_and(|document| document.guardian.is_some())
    })
    .await;
    let published = sink.published();
    assert_eq!(published.last().unwrap().seat_id, seat_id);
    // The document names the archive published beside it. The invite is not
    // asserted here: publications converge rather than being atomic, so the
    // first document carrying an archive may predate the invite's record.
    let digest = &published
        .last()
        .unwrap()
        .guardian
        .as_ref()
        .unwrap()
        .archive_sha256;
    assert_eq!(
        digest.len(),
        64,
        "an archive digest, not a placeholder: {digest}"
    );
}

/// A seat running consensus with no config on disk: the publisher holds the
/// document back — a seat that looks backed up while its shares exist only on
/// one disk is the failure this refusal exists for — but it holds *only* the
/// publication. The seat serves its invite as usual.
#[tokio::test]
async fn a_missing_guardian_archive_holds_the_publication_not_the_seat() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 31_104).await;
    let sink = FakeBackupSink::new();
    let fleet = fleet_with_backup(config.clone(), sink.clone()).await;
    let (fi_id, seat_id) = create_free_seat(&fleet, 42).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    eventually("the creation document is published", || {
        sink.published().len() == 1
    })
    .await;
    // A real ceremony with no config files written: the observation records,
    // the archive does not exist, and the refusal must hold the publication.
    let _fake = run_test_dkg(&fleet, &fi_id, &seat_id).await;

    let invite = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(invite) = seat.invite_code().await {
                return invite;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("ConfigPersisted records the invite without a read-side fallback");
    assert_eq!(invite, InviteCode("fed11-fake-invite".to_owned()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let published = sink.published();
    assert_eq!(published.len(), 1, "no config-less document was published");
    assert!(published[0].guardian.is_none());
}

/// A formed seat's document must carry everything a restore cannot rederive:
/// the federation it guards (probe-derived and otherwise nowhere durable), the
/// guard that makes `RestartDKG` refuse, and — once retired — the tombstone
/// that stops a restore resurrecting it.
#[tokio::test]
async fn a_formed_seats_document_carries_its_federation_guard_and_tombstone() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 1, 31_112).await;
    let sink = FakeBackupSink::new();
    let fleet = fleet_with_backup(config.clone(), sink.clone()).await;
    let (fi_id, seat_id) = create_free_seat(&fleet, 44).await;
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    let _fake = run_test_dkg(&fleet, &fi_id, &seat_id).await;

    write_guardian_config_files(&config, seat.facts().seat_no).await;
    // The probe that observes consensus records the federation invite and then
    // queues the document, so drive probes until a document carrying both
    // lands. Publications converge rather than being atomic with the writes
    // they describe: a publication already in flight when the invite was
    // recorded carries the config without it, and the mark republishes.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let _ = seat.invite_code().await;
            if sink.published().last().is_some_and(|document| {
                document
                    .guardian
                    .as_ref()
                    .is_some_and(|config| config.federation_invite.is_some())
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the formed document is published");

    let formed = sink.published().pop().unwrap();
    let guardian = formed.guardian.as_ref().unwrap();
    assert_eq!(
        guardian.federation_invite,
        Some(InviteCode("fed11-fake-invite".to_owned())),
        "the digest is useless without the federation it belongs to"
    );
    // No ceremony state is carried: the presence of a guardian archive is
    // itself the record that consensus was observed, because the publisher
    // names one only once it exists. A restore reads the `RestartDKG` guard
    // off that.
    assert_eq!(formed.decommissioned_at_ms, None);

    // Retiring the seat republishes, so a restore does not bring it back.
    assert!(fleet.decommission_seat(&seat_id).await.unwrap());
    eventually("the tombstone is published", || {
        sink.published()
            .last()
            .is_some_and(|document| document.decommissioned_at_ms.is_some())
    })
    .await;
    let retired = sink.published().pop().unwrap();
    assert!(retired.decommissioned_at_ms.is_some());
    assert_eq!(
        retired.guardian.as_ref().unwrap().federation_invite,
        guardian.federation_invite,
        "the tombstone republish must not drop what earlier publications recorded"
    );
}

/// Backup events are addressable: each publication *replaces* the last at the
/// same coordinate. So the invite gate's document has to carry the payment the
/// creation-time one recorded — a partial republish here would erase the FI's
/// money from the only off-site copy of it, and erase it precisely at the
/// moment the seat became worth recovering.
#[tokio::test]
async fn a_later_publication_never_drops_a_seats_payment() {
    let temp = TempDir::new().unwrap();
    let config = config(&temp, 2, 31_116).await;
    let sink = FakeBackupSink::new();
    let wallet = Arc::new(GatedRefundWallet::settling(ClaimOutcome::Success));
    let fleet = Fleet::open_with_wallet(
        Db::open(&config.process.data_root).await.unwrap(),
        config.clone(),
        async |_| Ok(wallet.clone() as _),
        async |_| Ok(sink.clone() as _),
    )
    .await
    .unwrap();

    let input = current_input(&fleet, 45, far_future(), 1_000, 45).await;
    let fi_id = input.fi_id;
    let commitment = fleet
        .create_seat(input, commitment(45), refusal_commitment(45))
        .await
        .unwrap();
    let seat_id = committed_seat_id(&commitment);
    let seat = fleet.seat_by_id(&seat_id).unwrap();
    eventually("the creation document is published", || {
        !sink.published().is_empty()
    })
    .await;
    let paid = sink.published().pop().unwrap();
    assert!(paid.payment.is_some());

    let _fake = run_test_dkg(&fleet, &fi_id, &seat_id).await;
    write_guardian_config_files(&config, seat.facts().seat_no).await;
    let _ = seat.invite_code().await;
    eventually("the formed document is published", || {
        sink.published()
            .last()
            .is_some_and(|document| document.guardian.is_some())
    })
    .await;

    let formed = sink.published().pop().unwrap();
    assert!(formed.guardian.is_some());
    assert_eq!(
        formed.payment, paid.payment,
        "the invite gate's document must still carry the accepted payment"
    );
}
#[path = "fleet/callback.rs"]
mod callback;
