use std::path::PathBuf;

use fedi_credential_sdk_protocol::{HolderContext, SignedCredential as SdkSignedCredential};
use fedi_decentralized_service_liquidity_manager::{
    AcceptedAttesterPolicy, BitcoinNetwork, FederationId, FederationLiquidityDetails,
    FederationName, FleetSeat, FleetSeatId, FmanEndorsement, FmanPeerAttestation,
    FmanPeerAttestationStatement, GetFederationTrustMaterialResponse, GuardianIdentity, HashBytes,
    InviteCode, PeerId, ProtocolV1, ProtocolVersion, ProviderPolicy, Sats, SchnorrSignatureProof,
    Sha256Digest, Url,
};
use nostr_sdk::Keys;
use nostr_sdk::secp256k1::Message;

use super::*;
use crate::attestation_store;
use crate::database::Database;
use crate::federation_preview::test_fakes::FakeFederationPreviewProvider;
use crate::federation_preview::{FederationPreview, PreviewPeer};
use crate::revocation::test_fakes::FakeRevocationFetcher;
use crate::test_support::credentials::{
    UNIT_TEST_ISSUER_RELAY, UNIT_TEST_PEER_BADGE_TRUST_LEVEL, attestation_payload,
    holder_authorization_for_provider, issue_credential_for_holder,
    issue_credential_for_holder_with_trust_level, test_foreign_issuer_context,
    test_issuer_authority, test_issuer_context,
};
use fedi_decentralized_service_liquidity_manager::Timestamp;

const ISSUER_RELAY: &str = UNIT_TEST_ISSUER_RELAY;

/// A real Fedimint invite code: the admission gate parses one to learn the
/// federation, so a placeholder string cannot reach the pipeline.
///
/// `seed` selects the federation, so tests can build a second, unrelated
/// federation's invite code.
fn test_invite(seed: u8) -> (String, String) {
    let federation_id =
        fedimint_core::config::FederationId::from_str(&format!("{seed:02x}").repeat(32))
            .expect("fixture federation id parses");
    let invite = fedimint_core::invite_code::InviteCode::new(
        fedimint_core::util::SafeUrl::parse("wss://guardian-0.example:5000")
            .expect("fixture guardian URL parses"),
        fedimint_core::PeerId::from(0),
        federation_id,
        None,
    );
    (invite.to_string(), federation_id.to_string())
}

struct Fman {
    keys: Keys,
    pubkey_hex: String,
    peers: Vec<usize>,
}

/// Sign a seat attestation for `fman` over `preview`'s federation.
fn attestation_for(
    preview: &FederationPreview,
    fman: &Fman,
    peer_id: &str,
    guardian: &str,
) -> FmanPeerAttestation {
    let account_seed = peer_id.parse::<u8>().unwrap_or(0).saturating_add(1);
    let account_key = bitcoin::secp256k1::PublicKey::from_secret_key(
        bitcoin::secp256k1::SECP256K1,
        &bitcoin::secp256k1::SecretKey::from_slice(&[account_seed; 32])
            .expect("fixed test scalar is valid"),
    );
    let statement = FmanPeerAttestationStatement {
        fman_pubkey: Pubkey(fman.pubkey_hex.clone()),
        federation_id: preview.federation_id.clone(),
        federation_config_hash: preview.federation_config_hash.clone(),
        peer_id: PeerId(peer_id.to_owned()),
        guardian_identity: GuardianIdentity(guardian.to_owned()),
        guardian_fee_account: serde_json::from_value(serde_json::json!({
            "acc_type": "BtcDepositor",
            "pub_keys": [account_key.to_string()],
            "threshold": 1,
        }))
        .expect("test guardian-fee account decodes"),
        issued_at: now_timestamp(),
    };
    let message = Message::from_digest(statement.digest().expect("statement digests"));

    FmanPeerAttestation {
        version: ProtocolV1,
        attestation: statement,
        proof: SchnorrSignatureProof {
            signature: fman.keys.sign_schnorr(&message),
        },
    }
}

/// Issue a badge to `fman` and bind it with a holder authorization.
fn envelope_for(
    issuer: &fedi_credential_sdk_protocol::IssuerContext,
    authority: &fedi_credential_sdk_protocol::IssuerAuthority,
    fman: &Fman,
) -> anyhow::Result<(
    fedi_credential_sdk_protocol::HolderAuthorization,
    SdkSignedCredential,
)> {
    let holder = HolderContext::generate();
    let credential = issue_credential_for_holder(issuer, authority, &holder)?;
    let authorization =
        holder_authorization_for_provider(&holder, &credential, &Pubkey(fman.pubkey_hex.clone()))?;

    Ok((authorization, credential))
}

/// Signed trust material for `fman`, as that FMan's own daemon would serve
/// it: attestations for every seat the fixture assigns it, plus whatever
/// holder authorizations the test wants it to carry.
fn material_for(
    preview: &FederationPreview,
    fman: &Fman,
    holder_authorizations: Vec<HolderAuthorizationEnvelope>,
) -> GetFederationTrustMaterialResponse {
    let now = now_timestamp();
    let material = FmanFederationTrustMaterial {
        fman_pubkey: Pubkey(fman.pubkey_hex.clone()),
        federation_id: preview.federation_id.clone(),
        federation_config_hash: preview.federation_config_hash.clone(),
        issued_at: now,
        expires_at: Timestamp(now.0 + 600),
        public_api_urls: vec![Url(format!("iroh://{}", fman.pubkey_hex))],
        peer_attestations: fman
            .peers
            .iter()
            .map(|peer| {
                attestation_for(
                    preview,
                    fman,
                    &peer.to_string(),
                    &format!("guardian-{peer}"),
                )
            })
            .collect(),
        holder_authorizations,
    };
    let message = nostr_sdk::secp256k1::Message::from_digest(
        material.digest().expect("fixture material digests"),
    );
    GetFederationTrustMaterialResponse {
        version: ProtocolV1,
        material,
        proof: SchnorrSignatureProof {
            signature: fman.keys.sign_schnorr(&message),
        },
    }
}

/// A complete, valid endorsement: seat attestation plus trust envelope.
fn endorsement_for(
    issuer: &fedi_credential_sdk_protocol::IssuerContext,
    authority: &fedi_credential_sdk_protocol::IssuerAuthority,
    preview: &FederationPreview,
    fman: &Fman,
    peer_id: &str,
    guardian: &str,
) -> anyhow::Result<FmanEndorsement> {
    let (holder_authorization, signed_credential) = envelope_for(issuer, authority, fman)?;

    Ok(FmanEndorsement {
        attestation: attestation_for(preview, fman, peer_id, guardian),
        trust: HolderAuthorizationEnvelope {
            holder_authorization,
            signed_credential,
        },
    })
}

struct Harness {
    database: Database,
    issuer: fedi_credential_sdk_protocol::IssuerContext,
    authority: fedi_credential_sdk_protocol::IssuerAuthority,
    attester_hex: String,
    preview: FederationPreview,
    fmans: Vec<Fman>,
    preview_provider: std::sync::Arc<FakeFederationPreviewProvider>,
    /// Trust material the requester will carry. Tests build it up per FMan; an
    /// identity nothing programs is simply absent from the request, which is
    /// what "unresolvable" means.
    trust_material: std::sync::Mutex<Vec<GetFederationTrustMaterialResponse>>,
    revocation: std::sync::Arc<FakeRevocationFetcher>,
    invite: String,
    endorsement: FmanEndorsement,
}

impl Harness {
    async fn new(
        name: &str,
        peer_count: usize,
        consensus_threshold: u32,
        assignments: &[&[usize]],
    ) -> anyhow::Result<Self> {
        let database = Database::connect(test_data_dir(name).join("flip.sqlite")).await?;
        let issuer = test_issuer_context();
        let authority = test_issuer_authority(&issuer, ISSUER_RELAY)?;
        let attester_hex = authority.issuer.issuer_id_pubkey.0.to_string();
        attestation_store::install(
            &database,
            fedi_decentralized_service_liquidity_manager::AttestationInstallRequest {
                payload: attestation_payload(&authority)?,
            },
        )
        .await?;

        let fmans: Vec<Fman> = assignments
            .iter()
            .enumerate()
            .map(|(index, peers)| {
                let keys = Keys::generate();
                let _ = index;
                Fman {
                    pubkey_hex: keys.public_key().to_string(),
                    keys,
                    peers: peers.to_vec(),
                }
            })
            .collect();
        let (invite, invite_federation_id) = test_invite(0xab);
        let mut preview = FederationPreview {
            federation_id: FederationId(invite_federation_id),
            federation_config_hash: HashBytes(vec![1, 2, 3, 4]),
            network: BitcoinNetwork::Regtest,
            peers: (0..peer_count)
                .map(|index| PreviewPeer {
                    peer_id: PeerId(index.to_string()),
                    guardian_identity: GuardianIdentity(format!("guardian-{index}")),
                })
                .collect(),
            consensus_threshold,
            fman_seat_bindings_metadata: None,
            module_kinds: ["wallet", "mint", STABILITY_POOL_MODULE_KIND]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        };
        // The directory the FI would have written: every previewed seat
        // bound to the FMan the assignment gives it.
        preview.fman_seat_bindings_metadata = Some(
            FmanSeatBindings::new(fmans.iter().flat_map(|fman| {
                fman.peers.iter().map(|peer| {
                    attestation_for(
                        &preview,
                        fman,
                        &peer.to_string(),
                        &format!("guardian-{peer}"),
                    )
                })
            }))
            .map_err(|error| anyhow::anyhow!("fixture seat bindings: {error}"))?
            .canonical_string()
            .map_err(|error| anyhow::anyhow!("fixture seat bindings: {error}"))?,
        );
        let preview_provider = std::sync::Arc::new(FakeFederationPreviewProvider::default());
        preview_provider.respond_ok(&invite, preview.clone());
        let revocation = std::sync::Arc::new(FakeRevocationFetcher::default());
        revocation.respond_ok(ISSUER_RELAY, vec![]);

        // Every request the harness builds carries a valid endorsement, so
        // the stages past the gate stay reachable. Gate rejections are
        // covered by replacing it deliberately.
        let endorsement =
            endorsement_for(&issuer, &authority, &preview, &fmans[0], "0", "guardian-0")?;

        Ok(Self {
            database,
            issuer,
            authority,
            attester_hex,
            preview,
            fmans,
            preview_provider,
            trust_material: std::sync::Mutex::new(Vec::new()),
            revocation,
            invite,
            endorsement,
        })
    }

    /// A valid endorsement naming `peer_id`/`guardian`, signed by
    /// `fmans[fman_index]` and badged by the installed issuer.
    fn endorsement_from(
        &self,
        fman_index: usize,
        peer_id: &str,
        guardian: &str,
    ) -> anyhow::Result<FmanEndorsement> {
        endorsement_for(
            &self.issuer,
            &self.authority,
            &self.preview,
            &self.fmans[fman_index],
            peer_id,
            guardian,
        )
    }

    fn provider(&self) -> VerificationPipeline {
        self.provider_with_budget(Arc::new(VerificationBudget::default()))
    }

    fn provider_with_budget(
        &self,
        verification_budget: Arc<VerificationBudget>,
    ) -> VerificationPipeline {
        VerificationPipeline::new(
            VerificationDeps {
                database: self.database.clone(),
                revocation_fetcher: self.revocation.clone(),
                preview_provider: self.preview_provider.clone(),
                verification_budget,
            },
            TrustInputs::Fixtures,
            PeerBadgeTrustPolicy::try_new(UNIT_TEST_PEER_BADGE_TRUST_LEVEL)
                .expect("test trust policy is valid"),
        )
    }

    fn config(&self, requirement: VerificationRequirement) -> SetupConfigView {
        let mut config = base_setup_config();
        config.policy = ProviderPolicy {
            accepted_attester_policies: vec![AcceptedAttesterPolicy {
                attester_pubkey: Pubkey(self.attester_hex.clone()),
                verification_requirement: requirement,
            }],
            supported_networks: vec![BitcoinNetwork::Regtest],
        };
        config
    }

    fn request(&self) -> RequestLiquidityRequest {
        let now = now_timestamp();
        RequestLiquidityRequest {
            version: ProtocolVersion(1),
            requester_pubkey: Pubkey("requester-pubkey".to_owned()),
            provider_pubkey: Pubkey("provider-pubkey".to_owned()),
            issued_at: now,
            network: BitcoinNetwork::Regtest,
            amounts: fedi_decentralized_service_liquidity_manager::LiquidityAmountBounds {
                gateway_min_amount: Sats(5_000),
                gateway_max_amount: None,
                stability_min_amount: Sats(0),
                stability_max_amount: None,
            },
            details_payload_hash: Sha256Digest([0; 32]),
            fman_endorsement: Some(self.endorsement.clone()),
            fman_trust_material: Some(
                self.trust_material
                    .lock()
                    .expect("fixture material lock")
                    .clone(),
            ),
            federation_details: FederationLiquidityDetails {
                invite_code: InviteCode(self.invite.clone()),
                federation_id: self.preview.federation_id.clone(),
                federation_name: FederationName("Target Federation".to_owned()),
                federation_config_hash: self.preview.federation_config_hash.clone(),
                fleet_seat_hints: vec![],
                revocation_locations: vec![],
            },
            expires_at: Timestamp(now.0 + 600),
        }
    }

    fn trust_envelope_for(
        &self,
        fman: &Fman,
    ) -> anyhow::Result<(
        fedi_credential_sdk_protocol::HolderAuthorization,
        SdkSignedCredential,
    )> {
        envelope_for(&self.issuer, &self.authority, fman)
    }

    fn trust_envelope_for_at_level(
        &self,
        fman: &Fman,
        trust_level: u64,
    ) -> anyhow::Result<HolderAuthorizationEnvelope> {
        let holder = HolderContext::generate();
        let signed_credential = issue_credential_for_holder_with_trust_level(
            &self.issuer,
            &self.authority,
            &holder,
            trust_level,
        )?;
        let holder_authorization = holder_authorization_for_provider(
            &holder,
            &signed_credential,
            &Pubkey(fman.pubkey_hex.clone()),
        )?;
        Ok(HolderAuthorizationEnvelope {
            holder_authorization,
            signed_credential,
        })
    }

    /// Carry trust material for this FMan holding one badge from the
    /// installed issuer, making the identity trustable.
    fn program_trusted(&self, fman_index: usize) -> anyhow::Result<SdkSignedCredential> {
        let fman = &self.fmans[fman_index];
        let (holder_authorization, signed_credential) = self.trust_envelope_for(fman)?;
        self.push_material(material_for(
            &self.preview,
            fman,
            vec![HolderAuthorizationEnvelope {
                holder_authorization,
                signed_credential: signed_credential.clone(),
            }],
        ));
        Ok(signed_credential)
    }

    /// Carry trust material holding no badges: the identity is answered for
    /// but stays untrusted.
    fn program_untrusted(&self, fman_index: usize) {
        let fman = &self.fmans[fman_index];
        self.push_material(material_for(&self.preview, fman, vec![]));
    }

    /// Replaces any existing entry for the same FMan: material is that
    /// FMan's one current answer, so a test re-programming an identity is
    /// changing what it serves, not adding a second opinion.
    fn push_material(&self, response: GetFederationTrustMaterialResponse) {
        let mut material = self.trust_material.lock().expect("fixture material lock");
        material.retain(|existing| existing.material.fman_pubkey != response.material.fman_pubkey);
        material.push(response);
    }

    async fn verify(&self, requirement: VerificationRequirement) -> VerificationOutcome {
        self.provider()
            .verify(&self.request(), &self.config(requirement))
            .await
    }

    /// Verify a request asking for a nonzero stability-pool minimum.
    ///
    /// The default request asks for gateway liquidity only, so nothing else
    /// in this module reaches the source-capability gate.
    async fn verify_stability(&self, requirement: VerificationRequirement) -> VerificationOutcome {
        let mut request = self.request();
        request.amounts.stability_min_amount = Sats(5_000);
        self.provider()
            .verify(&request, &self.config(requirement))
            .await
    }

    /// Re-publish the previewed config without the stability-pool module,
    /// as an ordinary wallet/mint federation would have it.
    fn drop_stability_module(&mut self) {
        self.preview.module_kinds.remove(STABILITY_POOL_MODULE_KIND);
        self.preview_provider
            .respond_ok(&self.invite, self.preview.clone());
    }
}

#[tokio::test]
async fn budget_uses_the_endorsed_invite_id_not_the_declared_id() -> anyhow::Result<()> {
    let harness = Harness::new("canonical-budget-key", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;
    let provider = harness.provider_with_budget(Arc::new(VerificationBudget::new(
        std::time::Duration::from_secs(60),
        1,
        16,
    )));
    let config = harness.config(VerificationRequirement::AllTrusted);

    assert!(
        provider
            .verify(&harness.request(), &config)
            .await
            .rejection
            .is_none()
    );

    let mut evasion_attempt = harness.request();
    evasion_attempt.federation_details.federation_id =
        FederationId("requester-chosen-fresh-key".to_owned());
    let outcome = provider.verify(&evasion_attempt, &config).await;
    assert_rejects(&outcome, PublicRejectionCode::ProviderUnavailable);
    assert!(
        outcome
            .rejection
            .as_ref()
            .and_then(|rejection| rejection.reason.as_deref())
            .is_some_and(|reason| reason.contains("allowance"))
    );
    Ok(())
}

fn assert_rejects(outcome: &VerificationOutcome, code: PublicRejectionCode) {
    let rejection = outcome
        .rejection
        .as_ref()
        .unwrap_or_else(|| panic!("expected {code:?} rejection, got acceptance"));
    assert_eq!(rejection.code, code, "reason: {:?}", rejection.reason);
    assert_eq!(
        outcome.summary.policy_result,
        VerificationCheckStatus::Failed
    );
    assert!(outcome.summary.failure_reason.is_some());
}

fn base_setup_config() -> SetupConfigView {
    use fedi_decentralized_service_liquidity_manager::{
        AdvertisementConfig, AttestationSummary, CapacityConfig, CapacityMode,
        ChainObserverBackendView, ChainObserverConfigView, DurationSecs, FundingPolicyConfig,
        GatewayConfigView, GatewayId, GatewayName, ReplenishmentConfig, RpcEndpointAddress,
        RpcEndpointConfig, RpcEndpointId, RpcProtocolName, RpcTransport, SourceType,
    };
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
            ready_advertisement_enabled: true,
        },
        provider_display: None,
        policy: ProviderPolicy {
            accepted_attester_policies: vec![],
            supported_networks: vec![BitcoinNetwork::Regtest],
        },
        attestation_summary: AttestationSummary::default(),
    }
}

fn test_data_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("flip-verification-{name}-{nanos}"))
}

#[tokio::test]
async fn happy_path_all_trusted_passes_with_duplicate_fman_seats() -> anyhow::Result<()> {
    let harness = Harness::new("happy-path", 3, 2, &[&[0, 1], &[2]]).await?;
    harness.program_trusted(0)?;
    harness.program_trusted(1)?;

    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;

    assert!(outcome.rejection.is_none(), "{:?}", outcome.summary);
    assert_eq!(
        outcome.summary.policy_result,
        VerificationCheckStatus::Passed
    );
    let policy = outcome
        .summary
        .accepted_attester_policy
        .expect("matched policy is recorded");
    assert_eq!(policy.attester_pubkey.0, harness.attester_hex);
    assert_eq!(outcome.summary.seat_checks.len(), 3);
    assert!(
        outcome
            .summary
            .revocation_checks
            .iter()
            .all(|check| check.status == VerificationCheckStatus::Passed)
    );
    Ok(())
}

/// Batching plus request-carried trust material took full validation of one
/// `RequestLiquidity` from `2N + 1` sequential Nostr round trips down to
/// two: the admission gate's endorsement-badge lookup, and stage 6's
/// batched lookup over every carried badge
/// ([`SPEC-flip-federation-trust`]).
///
/// Until this test, that number lived only in live-suite wall-clock. A
/// change reintroducing a per-identity lookup would have run slower without
/// failing anything. Widening the FMan set is the half that catches it: a
/// count that tracks the identity count is the regression, and asserting
/// two on one fixed set alone would not see it.
///
/// Both cases assert acceptance first. A rejection short-circuits the
/// stages, so an equal count between a rejected pair of runs would mean
/// nothing.
#[tokio::test]
async fn one_request_costs_two_revocation_round_trips() -> anyhow::Result<()> {
    let narrow = Harness::new("round-trips-narrow", 3, 2, &[&[0, 1], &[2]]).await?;
    narrow.program_trusted(0)?;
    narrow.program_trusted(1)?;

    let outcome = narrow.verify(VerificationRequirement::AllTrusted).await;
    assert!(outcome.rejection.is_none(), "{:?}", outcome.summary);
    assert_eq!(
        narrow.revocation.lookups().len(),
        2,
        "one admission-gate lookup, one batched stage-6 lookup: {:?}",
        narrow.revocation.lookups()
    );

    // Twice the identities, twice the seats, four distinct badges, one
    // issuer. Still two round trips.
    let wide = Harness::new("round-trips-wide", 6, 4, &[&[0, 1], &[2], &[3], &[4, 5]]).await?;
    for index in 0..wide.fmans.len() {
        wide.program_trusted(index)?;
    }

    let outcome = wide.verify(VerificationRequirement::AllTrusted).await;
    assert!(outcome.rejection.is_none(), "{:?}", outcome.summary);
    let lookups = wide.revocation.lookups();
    assert_eq!(
        lookups.len(),
        2,
        "the round-trip count must not grow with the identity count: {lookups:?}"
    );

    // Without this the count above could hold vacuously — four identities
    // that somehow shared one badge would also cost two round trips, and
    // the test would pass while proving nothing about batching. The gate
    // spends its trip on the endorsement badge alone; stage 6 carries all
    // four carried badges in one filter.
    assert_eq!(lookups[0].credential_digests.len(), 1, "{lookups:?}");
    assert_eq!(lookups[1].credential_digests.len(), 4, "{lookups:?}");
    Ok(())
}

/// A federation formed without the optional stability-pool module is an
/// ordinary valid federation, and its FMans can hold every credential this
/// pipeline checks. Nothing else in the pipeline reads the module map, so
/// without this gate such a request is accepted, funded through an ordinary
/// wallet peg-in, and fails only at the first stability-module lookup — which
/// happens *after* the peg-in is claimed, leaving provider e-cash in a target
/// client that can never deposit it.
#[tokio::test]
async fn a_stability_request_needs_the_module_in_the_previewed_config() -> anyhow::Result<()> {
    let mut harness = Harness::new("stability-module-gate", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;

    // Same federation, same credentials, same everything else.
    assert!(
        harness
            .verify_stability(VerificationRequirement::AllTrusted)
            .await
            .rejection
            .is_none(),
        "a target carrying the module is accepted"
    );

    harness.drop_stability_module();

    let outcome = harness
        .verify_stability(VerificationRequirement::AllTrusted)
        .await;
    assert_rejects(&outcome, PublicRejectionCode::UnsupportedSourceType);

    // A gateway-only request against the same module-less federation is
    // unaffected: the gate is about what the requested source needs, not
    // about the federation being unusable.
    assert!(
        harness
            .verify(VerificationRequirement::AllTrusted)
            .await
            .rejection
            .is_none(),
        "a gateway-only request needs no stability-pool module"
    );
    Ok(())
}

#[tokio::test]
async fn preview_unavailable_and_invalid_invite_map_to_codes() -> anyhow::Result<()> {
    let harness = Harness::new("preview-errors", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;

    // Unprogrammed invite code -> fake preview reports unavailable.
    harness
        .preview_provider
        .respond_invalid(&harness.invite, "bad invite");
    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidDetailsPayload);

    // A well-formed invite the fake preview has never heard of. The
    // endorsement names that federation too, so the gate passes and the
    // preview stage is what fails.
    let (other_invite, other_federation) = test_invite(0xcd);
    let mut other_preview = harness.preview.clone();
    other_preview.federation_id = FederationId(other_federation);
    let mut request = harness.request();
    request.federation_details.invite_code = InviteCode(other_invite);
    request.fman_endorsement = Some(endorsement_for(
        &harness.issuer,
        &harness.authority,
        &other_preview,
        &harness.fmans[0],
        "0",
        "guardian-0",
    )?);
    let outcome = harness
        .provider()
        .verify(
            &request,
            &harness.config(VerificationRequirement::AllTrusted),
        )
        .await;
    assert_rejects(&outcome, PublicRejectionCode::ProviderUnavailable);
    Ok(())
}

#[test]
fn endpoint_policy_rejection_is_classified_without_exposing_endpoint_details() {
    let (code, reason) = preview_error_rejection(FederationPreviewError::EndpointPolicyRejected);
    assert_eq!(code, PublicRejectionCode::InvalidDetailsPayload);
    assert_eq!(reason, "invite endpoint rejected by transport policy");
}

#[tokio::test]
async fn preview_hint_mismatches_reject_invalid_details() -> anyhow::Result<()> {
    let harness = Harness::new("hint-mismatch", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;
    let config = harness.config(VerificationRequirement::AllTrusted);

    let mut request = harness.request();
    request.federation_details.federation_config_hash = HashBytes(vec![9, 9, 9]);
    let outcome = harness.provider().verify(&request, &config).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidDetailsPayload);

    let mut request = harness.request();
    request.federation_details.fleet_seat_hints = vec![FleetSeat {
        seat_id: FleetSeatId("seat-1".to_owned()),
        peer_id: PeerId("0".to_owned()),
        guardian_identity: GuardianIdentity("guardian-wrong".to_owned()),
        fleet_manager_pubkey: Pubkey(harness.fmans[0].pubkey_hex.clone()),
        role_metadata: vec![],
    }];
    let outcome = harness.provider().verify(&request, &config).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidDetailsPayload);
    Ok(())
}

#[tokio::test]
async fn missing_or_malformed_directory_rejects_invalid_seat_binding() -> anyhow::Result<()> {
    // `SPEC-flip-federation-trust` maps malformed metadata to
    // `invalid_seat_binding`, not to the details-payload code the
    // superseded `fedi:fman_api_urls` path used.
    let harness = Harness::new("metadata", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;
    let config = harness.config(VerificationRequirement::AllTrusted);

    let mut preview = harness.preview.clone();
    preview.fman_seat_bindings_metadata = None;
    harness
        .preview_provider
        .respond_ok(&harness.invite, preview);
    let outcome = harness.provider().verify(&harness.request(), &config).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidSeatBinding);

    let mut preview = harness.preview.clone();
    preview.fman_seat_bindings_metadata = Some("{\"not\":\"canonical\"}".to_owned());
    harness
        .preview_provider
        .respond_ok(&harness.invite, preview);
    let outcome = harness.provider().verify(&harness.request(), &config).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidSeatBinding);
    Ok(())
}

/// Verify a request against a deliberately altered seat-binding directory.
async fn verify_with_directory(
    harness: &Harness,
    bindings: Vec<FmanPeerAttestation>,
) -> anyhow::Result<VerificationOutcome> {
    let mut preview = harness.preview.clone();
    preview.fman_seat_bindings_metadata = Some(
        FmanSeatBindings::new(bindings)
            .map_err(|error| anyhow::anyhow!("{error}"))?
            .canonical_string()
            .map_err(|error| anyhow::anyhow!("{error}"))?,
    );
    harness
        .preview_provider
        .respond_ok(&harness.invite, preview);

    Ok(harness
        .provider()
        .verify(
            &harness.request(),
            &harness.config(VerificationRequirement::AllTrusted),
        )
        .await)
}

#[tokio::test]
async fn seat_binding_failures_reject_invalid_seat_binding() -> anyhow::Result<()> {
    // The container's own validator owns these rules and tests them in the
    // domain crate; what is checked here is that the pipeline surfaces them
    // as `invalid_seat_binding` rather than any other code.
    let harness = Harness::new("seat-binding", 2, 2, &[&[0], &[1]]).await?;
    harness.program_trusted(0)?;
    harness.program_trusted(1)?;
    let (a, b) = (&harness.fmans[0], &harness.fmans[1]);

    // A binding naming the wrong guardian for its seat.
    let outcome = verify_with_directory(
        &harness,
        vec![
            attestation_for(&harness.preview, a, "0", "guardian-0"),
            attestation_for(&harness.preview, b, "1", "guardian-wrong"),
        ],
    )
    .await?;
    assert_rejects(&outcome, PublicRejectionCode::InvalidSeatBinding);

    // A binding for a peer the previewed config does not have.
    let outcome = verify_with_directory(
        &harness,
        vec![
            attestation_for(&harness.preview, a, "0", "guardian-0"),
            attestation_for(&harness.preview, b, "1", "guardian-1"),
            attestation_for(&harness.preview, b, "9", "guardian-9"),
        ],
    )
    .await?;
    assert_rejects(&outcome, PublicRejectionCode::InvalidSeatBinding);

    // A previewed seat with no binding at all.
    let outcome = verify_with_directory(
        &harness,
        vec![attestation_for(&harness.preview, a, "0", "guardian-0")],
    )
    .await?;
    assert_rejects(&outcome, PublicRejectionCode::InvalidSeatBinding);
    Ok(())
}

#[tokio::test]
async fn seat_binding_for_another_config_revision_is_rejected() -> anyhow::Result<()> {
    // An attestation signed against a different `federation_config_hash`
    // must not carry over, even though it is otherwise well formed and
    // names a real seat of this federation.
    let harness = Harness::new("wrong-config-hash", 2, 2, &[&[0], &[1]]).await?;
    harness.program_trusted(0)?;
    harness.program_trusted(1)?;
    let mut other_revision = harness.preview.clone();
    other_revision.federation_config_hash = HashBytes(vec![9, 9, 9]);

    let outcome = verify_with_directory(
        &harness,
        vec![
            attestation_for(&harness.preview, &harness.fmans[0], "0", "guardian-0"),
            attestation_for(&other_revision, &harness.fmans[1], "1", "guardian-1"),
        ],
    )
    .await?;

    assert_rejects(&outcome, PublicRejectionCode::InvalidSeatBinding);
    Ok(())
}

#[tokio::test]
async fn an_unanswered_identity_stays_untrusted() -> anyhow::Result<()> {
    // The request simply carries nothing for this identity. That is a trust
    // outcome, not an outage: the requester controls whether an identity is
    // answered for at all, and an unanswered one is untrusted, so it must
    // reach the policy stage rather than short-circuit as
    // `provider_unavailable`.
    let harness = Harness::new("missing-material", 2, 2, &[&[0], &[1]]).await?;
    harness.program_trusted(0)?;

    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::PolicyMismatch);
    assert!(outcome.summary.credential_checks.iter().any(|check| {
        check.name == "fman_trust_material" && check.status == VerificationCheckStatus::Failed
    }));
    Ok(())
}

#[tokio::test]
async fn a_request_carrying_no_trust_material_is_rejected() -> anyhow::Result<()> {
    // Absence is a rejection, never a bypass. `None` deserializes so the
    // request can be answered with a signed rejection instead of a bare
    // transport error.
    let harness = Harness::new("absent-material", 2, 2, &[&[0], &[1]]).await?;
    harness.program_trusted(0)?;
    harness.program_trusted(1)?;

    let mut request = harness.request();
    request.fman_trust_material = None;
    let outcome = harness
        .provider()
        .verify(
            &request,
            &harness.config(VerificationRequirement::AllTrusted),
        )
        .await;

    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

#[tokio::test]
async fn material_signed_for_another_federation_is_rejected() -> anyhow::Result<()> {
    // The material is bound to the previewed federation and config
    // revision, so material minted for a different federation cannot be
    // replayed into this request even though it verifies on its own.
    let harness = Harness::new("foreign-material", 2, 2, &[&[0], &[1]]).await?;
    harness.program_trusted(0)?;

    let mut foreign_preview = harness.preview.clone();
    foreign_preview.federation_id = FederationId("some-other-federation".to_owned());
    harness.push_material(material_for(&foreign_preview, &harness.fmans[1], vec![]));

    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

#[tokio::test]
async fn duplicate_material_for_one_identity_is_rejected() -> anyhow::Result<()> {
    // Resolving two entries for one identity by position would let the
    // ordering of a requester-supplied list decide a trust outcome.
    let harness = Harness::new("duplicate-material", 2, 2, &[&[0], &[1]]).await?;
    harness.program_trusted(0)?;
    harness.program_trusted(1)?;

    let mut request = harness.request();
    let material = request
        .fman_trust_material
        .as_mut()
        .expect("harness carries material");
    material.push(material[0].clone());

    let outcome = harness
        .provider()
        .verify(
            &request,
            &harness.config(VerificationRequirement::AllTrusted),
        )
        .await;

    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

#[tokio::test]
async fn material_contradicting_the_directory_is_rejected() -> anyhow::Result<()> {
    // Both the directory and the material say which seats an FMan runs. The
    // directory reached consensus among threshold guardians; the material
    // is one FMan's word. A disagreement means one of them is describing a
    // federation it is not in, and the directory is the one with a
    // federation behind it.
    let harness = Harness::new("contradicting-material", 2, 2, &[&[0], &[1]]).await?;
    harness.program_trusted(0)?;

    // FMan 1 claims peer 0, which the directory binds to FMan 0.
    let liar = Fman {
        pubkey_hex: harness.fmans[1].pubkey_hex.clone(),
        keys: harness.fmans[1].keys.clone(),
        peers: vec![0],
    };
    harness.push_material(material_for(&harness.preview, &liar, vec![]));

    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

/// Verify a request whose only deviation is its endorsement.
async fn verify_with_endorsement(
    harness: &Harness,
    endorsement: Option<FmanEndorsement>,
) -> VerificationOutcome {
    let mut request = harness.request();
    request.fman_endorsement = endorsement;
    harness
        .provider()
        .verify(
            &request,
            &harness.config(VerificationRequirement::AllTrusted),
        )
        .await
}

#[tokio::test]
async fn gate_rejects_a_request_with_no_endorsement() -> anyhow::Result<()> {
    let harness = Harness::new("gate-missing", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;

    // Absent is a rejection, not a bypass.
    let outcome = verify_with_endorsement(&harness, None).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

#[tokio::test]
async fn gate_rejects_an_unverifiable_attestation() -> anyhow::Result<()> {
    let harness = Harness::new("gate-bad-signature", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;

    let mut endorsement = harness.endorsement_from(0, "0", "guardian-0")?;
    endorsement.attestation.attestation.issued_at = Timestamp(1);
    let outcome = verify_with_endorsement(&harness, Some(endorsement)).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

#[tokio::test]
async fn gate_rejects_an_endorsement_for_another_federation() -> anyhow::Result<()> {
    // The federation comes from the invite code, not from the requester's
    // own `federation_details.federation_id` — otherwise a requester could
    // present any endorsement it holds and relabel the request to match.
    let harness = Harness::new("gate-foreign-federation", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;

    let (_, other_federation) = test_invite(0xcd);
    let mut other_preview = harness.preview.clone();
    other_preview.federation_id = FederationId(other_federation.clone());
    let endorsement = endorsement_for(
        &harness.issuer,
        &harness.authority,
        &other_preview,
        &harness.fmans[0],
        "0",
        "guardian-0",
    )?;

    let mut request = harness.request();
    request.fman_endorsement = Some(endorsement);
    // Relabelling the self-declared hint does not help.
    request.federation_details.federation_id = FederationId(other_federation);
    let outcome = harness
        .provider()
        .verify(
            &request,
            &harness.config(VerificationRequirement::AllTrusted),
        )
        .await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidSeatBinding);
    Ok(())
}

#[tokio::test]
async fn preview_for_a_different_federation_cannot_relabel_an_endorsed_invite() -> anyhow::Result<()>
{
    let harness = Harness::new("preview-federation-mismatch", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;

    // The endorsement binds the federation parsed from `invite`; a faulty
    // preview must not be able to replace that identity before persistence.
    let (_, other_federation) = test_invite(0xcd);
    let mut mismatched_preview = harness.preview.clone();
    mismatched_preview.federation_id = FederationId(other_federation);
    harness
        .preview_provider
        .respond_ok(&harness.invite, mismatched_preview);

    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidDetailsPayload);
    Ok(())
}

#[tokio::test]
async fn gate_rejects_a_badge_from_an_untrusted_issuer() -> anyhow::Result<()> {
    let harness = Harness::new("gate-foreign-issuer", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;

    let foreign_issuer = test_foreign_issuer_context();
    let foreign_authority = test_issuer_authority(&foreign_issuer, ISSUER_RELAY)?;
    let endorsement = endorsement_for(
        &foreign_issuer,
        &foreign_authority,
        &harness.preview,
        &harness.fmans[0],
        "0",
        "guardian-0",
    )?;

    let outcome = verify_with_endorsement(&harness, Some(endorsement)).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

#[tokio::test]
async fn gate_rejects_an_authentic_badge_below_the_profile_minimum() -> anyhow::Result<()> {
    let harness = Harness::new("gate-below-minimum", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;
    let mut endorsement = harness.endorsement_from(0, "0", "guardian-0")?;
    endorsement.trust = harness
        .trust_envelope_for_at_level(&harness.fmans[0], UNIT_TEST_PEER_BADGE_TRUST_LEVEL - 1)?;

    let outcome = verify_with_endorsement(&harness, Some(endorsement)).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    assert!(
        outcome
            .summary
            .failure_reason
            .as_deref()
            .is_some_and(|detail| detail.contains("below required minimum 9"))
    );
    Ok(())
}

#[tokio::test]
async fn gate_rejects_a_badge_bound_to_another_fman() -> anyhow::Result<()> {
    // The envelope's subject must be the identity that signed the seat
    // attestation, so an endorsement cannot pair one guardian's binding
    // with another guardian's badge.
    let harness = Harness::new("gate-subject-mismatch", 2, 2, &[&[0], &[1]]).await?;
    harness.program_trusted(0)?;

    let mut endorsement = harness.endorsement_from(0, "0", "guardian-0")?;
    let (holder_authorization, signed_credential) =
        envelope_for(&harness.issuer, &harness.authority, &harness.fmans[1])?;
    endorsement.trust = HolderAuthorizationEnvelope {
        holder_authorization,
        signed_credential,
    };

    let outcome = verify_with_endorsement(&harness, Some(endorsement)).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

#[tokio::test]
async fn gate_verifies_the_badge_before_spending_a_relay_lookup() -> anyhow::Result<()> {
    // Everything the gate checks before the badge cryptography is forgeable
    // by anyone holding the invite code and a trusted issuer's pubkey: the
    // attestation is self-signable, and the issuer-installed check reads
    // the credential's claimed, unverified issuer id. If the revocation
    // lookup ran first, such a forgery would cost a relay round trip before
    // anything rejected it.
    //
    // The fetcher is programmed to fail, so the ordering is observable: a
    // gate that looks up revocations first reports `provider_unavailable`,
    // and one that verifies first reports `invalid_credentials` without
    // ever reaching the relay.
    let harness = Harness::new("gate-verify-before-lookup", 2, 2, &[&[0], &[1]]).await?;
    harness.program_trusted(0)?;
    harness
        .revocation
        .respond_err(ISSUER_RELAY, "relay must not be reached");

    // A real badge from the installed issuer, bound to the wrong FMan: the
    // issuer-installed check passes and the cryptography is what fails.
    let mut endorsement = harness.endorsement_from(0, "0", "guardian-0")?;
    let (holder_authorization, signed_credential) =
        envelope_for(&harness.issuer, &harness.authority, &harness.fmans[1])?;
    endorsement.trust = HolderAuthorizationEnvelope {
        holder_authorization,
        signed_credential,
    };

    let outcome = verify_with_endorsement(&harness, Some(endorsement)).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    assert!(
        outcome.summary.revocation_checks.is_empty(),
        "a badge that fails verification must not have cost a revocation lookup"
    );
    Ok(())
}

#[tokio::test]
async fn gate_rejects_a_revoked_endorsement_badge() -> anyhow::Result<()> {
    let harness = Harness::new("gate-revoked", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;

    let revocation = harness
        .issuer
        .revoke_credential(&harness.endorsement.trust.signed_credential)?;
    let service_revocation = serde_json::from_value(serde_json::to_value(&revocation)?)?;
    harness
        .revocation
        .respond_ok(ISSUER_RELAY, vec![service_revocation]);

    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

#[tokio::test]
async fn gate_runs_before_the_preview() -> anyhow::Result<()> {
    // A gate failure must cost no preview. The preview is programmed to
    // fail with a different code, so the gate's code proves the ordering.
    let harness = Harness::new("gate-precedes-preview", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;
    harness
        .preview_provider
        .respond_invalid(&harness.invite, "preview must not be consulted");

    let outcome = verify_with_endorsement(&harness, None).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

#[tokio::test]
async fn required_revocation_lookup_failure_is_unavailable() -> anyhow::Result<()> {
    let harness = Harness::new("revocation-down", 1, 1, &[&[0]]).await?;
    harness.program_trusted(0)?;
    harness.revocation.respond_err(ISSUER_RELAY, "relay down");

    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::ProviderUnavailable);
    Ok(())
}

#[tokio::test]
async fn no_nostr_endorsement_authority_is_unavailable() -> anyhow::Result<()> {
    let harness = Harness::new("endorsement-no-nostr", 1, 1, &[&[0]]).await?;
    let authority = harness.issuer.issuer_authority(vec![
        fedi_credential_sdk_protocol::RevocationLocation {
            protocol: "https".to_owned(),
            location: "https://attester.example/revocations".to_owned(),
        },
    ])?;
    attestation_store::install(
        &harness.database,
        fedi_decentralized_service_liquidity_manager::AttestationInstallRequest {
            payload: attestation_payload(&authority)?,
        },
    )
    .await?;

    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::ProviderUnavailable);
    assert!(outcome.summary.revocation_checks.iter().any(|check| {
        check.name == "revocation_freshness"
            && check.status == VerificationCheckStatus::Failed
            && check.detail.as_deref()
                == Some("issuer authority lists no supported Nostr revocation locations")
    }));
    Ok(())
}

#[tokio::test]
async fn no_nostr_advertisement_authority_is_unavailable() -> anyhow::Result<()> {
    let harness = Harness::new("advertisement-no-nostr", 1, 1, &[&[0]]).await?;
    let issuer = test_foreign_issuer_context();
    let authority =
        issuer.issuer_authority(vec![fedi_credential_sdk_protocol::RevocationLocation {
            protocol: "https".to_owned(),
            location: "https://attester.example/revocations".to_owned(),
        }])?;
    let issuer_hex = authority.issuer.issuer_id_pubkey.0.to_string();
    attestation_store::install(
        &harness.database,
        fedi_decentralized_service_liquidity_manager::AttestationInstallRequest {
            payload: attestation_payload(&authority)?,
        },
    )
    .await?;
    let (holder_authorization, signed_credential) =
        envelope_for(&issuer, &authority, &harness.fmans[0])?;
    harness.push_material(material_for(
        &harness.preview,
        &harness.fmans[0],
        vec![HolderAuthorizationEnvelope {
            holder_authorization,
            signed_credential,
        }],
    ));
    let mut config = harness.config(VerificationRequirement::AllTrusted);
    config.policy.accepted_attester_policies[0].attester_pubkey = Pubkey(issuer_hex.clone());

    let outcome = harness.provider().verify(&harness.request(), &config).await;
    assert_rejects(&outcome, PublicRejectionCode::ProviderUnavailable);
    assert!(outcome.summary.revocation_checks.iter().any(|check| {
        check.name == "revocation_freshness"
            && check.status == VerificationCheckStatus::Failed
            && check.subject.as_deref() == Some(issuer_hex.as_str())
            && check.detail.as_deref()
                == Some("issuer authority lists no supported Nostr revocation locations")
    }));
    Ok(())
}

#[tokio::test]
async fn revoked_fman_credential_rejects_invalid_credentials() -> anyhow::Result<()> {
    let harness = Harness::new("revoked-credential", 1, 1, &[&[0]]).await?;
    let credential = harness.program_trusted(0)?;
    let revocation = harness.issuer.revoke_credential(&credential)?;
    let service_revocation = serde_json::from_value(serde_json::to_value(&revocation)?)?;
    harness
        .revocation
        .respond_ok(ISSUER_RELAY, vec![service_revocation]);

    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    Ok(())
}

#[tokio::test]
async fn below_minimum_fman_material_rejects_invalid_credentials() -> anyhow::Result<()> {
    let harness = Harness::new("material-below-minimum", 1, 1, &[&[0]]).await?;
    let fman = &harness.fmans[0];
    let envelope =
        harness.trust_envelope_for_at_level(fman, UNIT_TEST_PEER_BADGE_TRUST_LEVEL - 1)?;
    harness.push_material(material_for(&harness.preview, fman, vec![envelope]));

    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);
    assert!(outcome.summary.credential_checks.iter().any(|check| {
        check.name == "issuer_credential_policy"
            && check.status == VerificationCheckStatus::Failed
            && check
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("below required minimum 9"))
    }));
    Ok(())
}

#[tokio::test]
async fn a_badge_with_the_wrong_schema_rejects_invalid_credentials() -> anyhow::Result<()> {
    let harness = Harness::new("bad-badges", 1, 1, &[&[0]]).await?;
    let fman = &harness.fmans[0];

    // Credential under a different schema string.
    let holder = HolderContext::generate();
    let info = serde_json::json!({
        "schema": "fedi-other-schema-v1.0",
        "trust_level": 7,
    });
    let (issuance_request, pending) =
        fedi_credential_sdk_protocol::PendingIssuance::create_request(
            &harness.authority.issuer.issuance_key,
            harness.authority.issuer.issuer_id_pubkey.clone(),
            info.clone(),
            serde_json::json!(holder.public_key().to_string()),
        )?;
    let issuance_response = harness.issuer.issue_credential(info, &issuance_request)?;
    let bad_credential =
        pending.finalize(&harness.authority.issuer.issuance_key, &issuance_response)?;
    let authorization = holder_authorization_for_provider(
        &holder,
        &bad_credential,
        &Pubkey(fman.pubkey_hex.clone()),
    )?;
    harness.push_material(material_for(
        &harness.preview,
        fman,
        vec![HolderAuthorizationEnvelope {
            holder_authorization: authorization,
            signed_credential: bad_credential,
        }],
    ));
    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::InvalidCredentials);

    // The old transport could carry a holder authorization with no
    // matching backing credential; the envelope carries both together, so
    // that failure mode is now unrepresentable rather than merely untested.
    Ok(())
}

#[tokio::test]
async fn consensus_majority_counts_distinct_fman_identities_once() -> anyhow::Result<()> {
    // Four peers with threshold 3: one trusted FMan operating two peers
    // still counts once, so two trusted identities of three fail the
    // threshold and a third trusted identity satisfies it.
    let harness = Harness::new("distinct-count", 4, 3, &[&[0, 1], &[2], &[3]]).await?;
    harness.program_trusted(0)?;
    harness.program_trusted(1)?;
    harness.program_untrusted(2);

    let outcome = harness
        .verify(VerificationRequirement::ConsensusMajorityTrusted)
        .await;
    assert_rejects(&outcome, PublicRejectionCode::PolicyMismatch);

    harness.program_trusted(2)?;
    let outcome = harness
        .verify(VerificationRequirement::ConsensusMajorityTrusted)
        .await;
    assert!(outcome.rejection.is_none(), "{:?}", outcome.summary);
    assert_eq!(
        outcome.summary.policy_result,
        VerificationCheckStatus::Passed
    );
    Ok(())
}

#[tokio::test]
async fn all_trusted_fails_with_any_untrusted_identity() -> anyhow::Result<()> {
    let harness = Harness::new("all-trusted-gap", 2, 1, &[&[0], &[1]]).await?;
    harness.program_trusted(0)?;
    harness.program_untrusted(1);

    let outcome = harness.verify(VerificationRequirement::AllTrusted).await;
    assert_rejects(&outcome, PublicRejectionCode::PolicyMismatch);
    Ok(())
}
