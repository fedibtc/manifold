//! Test-support helpers for crate-level integration tests.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerifier;
use fedi_decentralized_service_liquidity_manager::{
    AllocationItemTarget, AttestationInstallRequest, FederationId, FederationName, GatewayId,
    GatewayName, InviteCode, ItemAllocationStatus, PayloadProof, Pubkey, PublicRpcPayloadDomain,
    RequestLiquidityRequest, Sats, SetupConfigView, Sha256Digest, Signature, Signed, SourceType,
    Url, VerificationCheck, VerificationCheckStatus, VerificationSummary, public_rpc_payload_hash,
};
use nostr_sdk::Keys;
use nostr_sdk::secp256k1::Message;
use serde::Serialize;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::allocation_store::{self, FundingTargetRecord};
use crate::auth::SchnorrAuthProvider;
use crate::config::{DaemonArgs, DaemonPaths};
use crate::daemon::{DaemonContext, DaemonState};
use crate::database::Database;
use crate::holder_authorization::HolderAuthorizationFetcher;
use crate::identity;
use crate::nostr::RelayPublisher;
use crate::secret_store::SecretStore;
use crate::target_fedimint::TargetFedimintClients;
use crate::verification::{
    VerificationModeInfo, VerificationOutcome, VerificationProvider as VerificationProviderTrait,
};

/// The advertised endpoint address used by `ready_setup_config`; test contexts
/// stamp it as the local public Iroh node id so the endpoint-binding readiness
/// check passes without a running Iroh endpoint.
pub(crate) const TEST_IROH_NODE_ID: &str = "iroh-node-id";

/// Build a daemon context on the production auth flow: a generated provider
/// key is imported as the production identity and Schnorr-signs everything.
/// A unique SQLite path under the shared test directory.
///
/// Keyed by test name, process id and nanosecond clock, so tests running
/// concurrently in one binary and across binaries never share a database.
pub(crate) fn test_sqlite_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_nanos();
    std::env::temp_dir()
        .join("fedi-flip-tests")
        .join(format!("{name}-{}-{nanos}.sqlite", std::process::id()))
}

pub(crate) async fn production_test_context(
    name: &str,
    relay_publisher: Arc<dyn RelayPublisher>,
    verification_provider: Arc<dyn VerificationProviderTrait>,
) -> anyhow::Result<DaemonContext> {
    let provider_keys = Keys::generate();
    let provider_secret_hex = provider_keys.secret_key().to_secret_hex();
    let data_dir = test_data_dir(name);
    let sqlite_path = data_dir.join("flip.sqlite");
    let args = DaemonArgs {
        manifold_environment: ManifoldEnvironment::Development,
        data_dir: data_dir.clone(),
        sqlite_path: sqlite_path.clone(),
        admin_bind_address: "127.0.0.1:0".parse()?,
        public_bind_address: "127.0.0.1:0".parse()?,
        bootstrap_admin_token: Some("test-admin-token".to_owned()),
        secret_store_key: Some(SecretStore::generate_hex_key()),
        allow_bootstrap_token_fallback: false,
        mode: crate::config::DaemonMode::Normal,
        provider_nostr_secret_key: Some(provider_secret_hex.clone()),
        trust_fixtures_dir: None,
        max_open_target_clients: crate::target_fedimint::DEFAULT_MAX_OPEN_TARGET_CLIENTS,
        allow_private_federation_endpoints: false,
    };
    let paths = DaemonPaths {
        data_dir,
        sqlite_path,
        secret_store_key: args.data_dir.join("secret-store.key"),
        federations_dir: args.data_dir.join("federations"),
        lock_file: args.data_dir.join("flip.lock"),
    };
    tokio::fs::create_dir_all(&paths.data_dir).await?;
    let database = Database::connect(&paths.sqlite_path).await?;
    let secret_store =
        SecretStore::load_or_create(&paths.secret_store_key, args.secret_store_key.as_deref())?;
    let identity = identity::load_or_import_production_provider_identity(
        &database,
        &secret_store,
        Some(&provider_secret_hex),
    )
    .await?
    .expect("generated provider key imports as production identity");

    Ok(DaemonContext {
        args,
        paths,
        daemon_state: Arc::new(RwLock::new(DaemonState::default())),
        database,
        secret_store,
        auth_provider_slot: Arc::new(RwLock::new(Arc::new(SchnorrAuthProvider::new(identity)?))),
        identity_installed: Arc::new(tokio::sync::watch::channel(true).0),
        verification_provider,
        peer_badge_verifier: PeerBadgeVerifier::try_from_profile(
            &ManifoldEnvironment::Development
                .profile()
                .expect("development profile resolves"),
        )
        .expect("development PeerBadge verifier"),
        relay_publisher,
        holder_authorization_read: Arc::new(RwLock::new(
            crate::holder_authorization::LastRelayRead::NotYet,
        )),
        holder_authorization_fetcher: Arc::new(StaticHolderAuthorizationFetcher::default()),
        target_fedimint_clients: TargetFedimintClients::default(),
        verification_budget: std::sync::Arc::new(
            crate::verification_budget::VerificationBudget::default(),
        ),
        worker_health: Arc::new(RwLock::new(crate::daemon::WorkerHealthMap::new())),
        allocation_admission: Arc::new(RwLock::new(crate::daemon::AllocationAdmission::Open)),
        work_quiescence: crate::daemon::WorkQuiescence::default(),
        shutdown: CancellationToken::new(),
        background_tasks: TaskTracker::new(),
    })
}

/// Build a daemon context that booted with no provider signing key, the way a
/// fresh deployment does before its operator installs one.
///
/// Returns the context and the secret key the caller can install through
/// [`DaemonContext::install_provider_signing_identity`].
pub(crate) async fn unconfigured_identity_test_context(
    name: &str,
    relay_publisher: Arc<dyn RelayPublisher>,
    verification_provider: Arc<dyn VerificationProviderTrait>,
) -> anyhow::Result<(DaemonContext, String)> {
    let context = production_test_context(name, relay_publisher, verification_provider).await?;
    let provider_secret_hex = context
        .args
        .provider_nostr_secret_key
        .clone()
        .expect("production test context generates a provider key");

    // Roll the context back to its pre-install state: drop the identity rows
    // the builder seeded and restore the fail-closed provider.
    sqlx::query("DELETE FROM secret_records WHERE name = ?")
        .bind(identity::PROVIDER_NOSTR_SECRET)
        .execute(context.database.pool())
        .await?;
    sqlx::query("DELETE FROM provider_identity")
        .execute(context.database.pool())
        .await?;
    *context.auth_provider_slot.write().await = Arc::new(crate::auth::UnconfiguredAuthProvider);
    context.identity_installed.send_replace(false);

    Ok((context, provider_secret_hex))
}

/// Test-only pass-all verification double.
///
/// This is a unit/integration test stand-in for the verification pipeline,
/// not a runtime mode: the daemon itself always constructs the real pipeline.
#[derive(Clone, Debug, Default)]
pub(crate) struct StaticVerificationProvider;

#[async_trait::async_trait]
impl VerificationProviderTrait for StaticVerificationProvider {
    fn mode(&self) -> VerificationModeInfo {
        VerificationModeInfo {
            mode: "test_static_pass",
            inputs_available: true,
            fixtures: true,
            detail: "test-only pass-all verification double",
        }
    }

    async fn verify(
        &self,
        request: &RequestLiquidityRequest,
        config: &SetupConfigView,
    ) -> VerificationOutcome {
        VerificationOutcome {
            summary: VerificationSummary {
                federation_id: request.federation_details.federation_id.clone(),
                policy_result: VerificationCheckStatus::Passed,
                seat_checks: vec![static_check("test_static_seat_binding")],
                credential_checks: vec![static_check("test_static_credentials")],
                revocation_checks: vec![static_check("test_static_revocations")],
                accepted_attester_policy: config.policy.accepted_attester_policies.first().cloned(),
                failure_reason: None,
            },
            rejection: None,
        }
    }
}

pub(crate) fn static_verification_provider() -> Arc<dyn VerificationProviderTrait> {
    Arc::new(StaticVerificationProvider)
}

fn static_check(name: &str) -> VerificationCheck {
    VerificationCheck {
        name: name.to_owned(),
        status: VerificationCheckStatus::NotRun,
        subject: None,
        detail: Some("test-only static verification double".to_owned()),
    }
}

/// Schnorr-sign a public RPC payload with the given keys.
///
/// Provisional requester-side signing: the canonical FI signing byte layout
/// remains unpinned (open-items item 2); this mirrors the
/// provider-side domain-tagged canonical CBOR hash the daemon verifies today.
pub(crate) fn sign_public_rpc_with_keys<T>(
    domain: PublicRpcPayloadDomain,
    payload: T,
    keys: &Keys,
) -> anyhow::Result<Signed<T>>
where
    T: Serialize,
{
    let hash = public_rpc_payload_hash(domain, &payload)?;
    let digest: [u8; 32] = hash
        .0
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("payload hash must be 32 bytes"))?;
    let message = Message::from_digest(digest);
    Ok(Signed {
        payload,
        proof: PayloadProof {
            signature: Signature(keys.sign_schnorr(&message).serialize().to_vec()),
        },
    })
}

/// Deterministic test keypair derived from a non-zero tag byte.
pub(crate) fn fixed_test_keys(tag: u8) -> Keys {
    assert_ne!(tag, 0, "tag 0 is not a valid secret key");
    Keys::parse(&format!("{tag:02x}").repeat(32)).expect("fixed tag secret key parses")
}

/// The trust-envelope pair enrolled by [`enroll_provider_trust_envelope`].
pub(crate) struct InstalledProviderEnvelope {
    pub authorization: fedi_credential_sdk_protocol::HolderAuthorization,
    pub credential: fedi_credential_sdk_protocol::SignedCredential,
}

/// Relay URL the static enrollment fetcher answers for.
pub(crate) const UNIT_TEST_AUTHORIZATION_RELAY: &str = "wss://authorization.example";

/// A [`HolderAuthorizationFetcher`] serving fixed answers, so ingest tests run
/// the real reconciliation without a relay.
#[derive(Default)]
pub(crate) struct StaticHolderAuthorizationFetcher {
    answers: Vec<(String, Result<Vec<nostr_sdk::Event>, String>)>,
    wildcard: Option<Vec<nostr_sdk::Event>>,
}

impl StaticHolderAuthorizationFetcher {
    /// Serve `events` from whichever relay is asked.
    pub(crate) fn serving(events: Vec<nostr_sdk::Event>) -> Self {
        Self {
            answers: Vec::new(),
            wildcard: Some(events),
        }
    }
}

#[async_trait::async_trait]
impl HolderAuthorizationFetcher for StaticHolderAuthorizationFetcher {
    async fn fetch_candidates(
        &self,
        relay_url: &Url,
        _provider_pubkey: nostr_sdk::PublicKey,
    ) -> Result<Vec<nostr_sdk::Event>, String> {
        if let Some((_, answer)) = self.answers.iter().find(|(url, _)| url == &relay_url.0) {
            return answer.clone();
        }
        self.wildcard
            .clone()
            .ok_or_else(|| "no answer configured".to_owned())
    }
}

/// Install a trusted issuer authority, then enroll a complete provider trust
/// envelope the way production does: a Holder publishes a kind-37705
/// authorization and the daemon reconciles it off a relay.
///
/// The authority still installs through the admin path — it is
/// operator-configured trust policy, not holder-published material — while the
/// authorization and its backing badge arrive together in the event.
pub(crate) async fn enroll_provider_trust_envelope(
    database: &Database,
    provider_pubkey: &Pubkey,
) -> anyhow::Result<InstalledProviderEnvelope> {
    let issuer = credentials::test_issuer_context();
    let authority =
        credentials::test_issuer_authority(&issuer, credentials::UNIT_TEST_ISSUER_RELAY)?;
    let holder = fedi_credential_sdk_protocol::HolderContext::generate();
    let credential = credentials::issue_credential_for_holder(&issuer, &authority, &holder)?;
    let authorization =
        credentials::holder_authorization_for_provider(&holder, &credential, provider_pubkey)?;
    crate::attestation_store::install(
        database,
        AttestationInstallRequest {
            payload: credentials::attestation_payload(&authority)?,
        },
    )
    .await?;

    let event = credentials::flip_authorization_event(
        &holder,
        &authorization,
        &credential,
        provider_pubkey,
    )?;
    let outcome = crate::holder_authorization::refresh(
        database,
        &StaticHolderAuthorizationFetcher::serving(vec![event]),
        provider_pubkey,
        &[Url(UNIT_TEST_AUTHORIZATION_RELAY.to_owned())],
    )
    .await?;
    anyhow::ensure!(
        outcome.candidates_verified == 1,
        "test enrollment did not verify: {outcome:?}"
    );

    Ok(InstalledProviderEnvelope {
        authorization,
        credential,
    })
}

/// Directly seeded `allocations` row with its items, bypassing the public
/// acceptance path. This is the one test-side definition of the seed SQL;
/// the one production writer is `allocation_store::insert_allocation`. The
/// funding target and item ids are derived from `federation_id`, so seeded
/// rows cannot carry the column/`target_json` disagreement that the
/// production writer makes impossible.
pub(crate) struct AllocationSeed {
    pub federation_id: FederationId,
    pub requester_pubkey: Pubkey,
    pub provider_pubkey: Pubkey,
    pub network: String,
    /// `None` derives a hash from the federation id bytes, so distinct
    /// seeded federations never collide on the unique details-hash index.
    pub details_payload_hash: Option<Sha256Digest>,
    pub committed_amount: Sats,
    pub reserved_amount: Sats,
    pub items: Vec<ItemSeed>,
}

impl Default for AllocationSeed {
    fn default() -> Self {
        Self {
            federation_id: FederationId("federation-1".to_owned()),
            requester_pubkey: Pubkey("requester-1".to_owned()),
            provider_pubkey: Pubkey("provider-1".to_owned()),
            network: "regtest".to_owned(),
            details_payload_hash: None,
            committed_amount: Sats(10_000),
            reserved_amount: Sats(10_000),
            items: Vec::new(),
        }
    }
}

/// One seeded allocation item. The item id is derived from the seed's
/// federation id and this source type, as in production planning.
pub(crate) struct ItemSeed {
    pub source_type: SourceType,
    pub status: ItemAllocationStatus,
    pub committed_amount: Sats,
    pub reserved_amount: Sats,
    /// `None` derives a target of the matching variant over this item's
    /// amount, with the `ready_setup_config` gateway identity.
    pub item_target: Option<AllocationItemTarget>,
    pub step_json: Option<String>,
    pub failure_json: Option<String>,
}

impl Default for ItemSeed {
    fn default() -> Self {
        Self {
            source_type: SourceType::Gateway,
            status: ItemAllocationStatus::Pending,
            committed_amount: Sats(10_000),
            reserved_amount: Sats(10_000),
            item_target: None,
            step_json: None,
            failure_json: None,
        }
    }
}

/// A parseable invite code naming one loopback guardian endpoint.
///
/// The endpoint policy runs before the gateway is asked to attach a
/// federation, so a seed carrying an unparseable placeholder would make every
/// gateway test refuse at the policy instead of exercising the path under
/// test. Loopback is what a local harness actually offers, and the tests that
/// use it pass `EndpointPolicy::AllowPrivate` for the same reason a local
/// deployment does.
pub(crate) fn loopback_invite_code() -> String {
    use fedimint_core::PeerId;
    use fedimint_core::config::FederationId as FedimintFederationId;
    use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
    use fedimint_core::util::SafeUrl;

    FedimintInviteCode::new(
        SafeUrl::parse("ws://127.0.0.1:18173/").expect("static test url"),
        PeerId::from(0),
        FedimintFederationId::dummy(),
        None,
    )
    .to_string()
}

impl AllocationSeed {
    pub(crate) async fn insert(&self, database: &Database) -> anyhow::Result<()> {
        let target = FundingTargetRecord {
            federation_id: self.federation_id.clone(),
            federation_name: FederationName("Federation One".to_owned()),
            invite_code: InviteCode(loopback_invite_code()),
            federation_config_hash: "01020304".to_owned(),
        };
        let details_payload_hash = self.details_payload_hash.unwrap_or_else(|| {
            let mut hash = [0u8; 32];
            let bytes = self.federation_id.0.as_bytes();
            let len = bytes.len().min(hash.len());
            hash[..len].copy_from_slice(&bytes[..len]);
            Sha256Digest(hash)
        });
        sqlx::query(
            "INSERT INTO allocations \
             (federation_id, requester_pubkey, provider_pubkey, network, details_payload_hash, \
              request_json, verification_json, target_json, \
              committed_amount_sats, reserved_amount_sats) \
             VALUES (?, ?, ?, ?, ?, '{}', '{}', ?, ?, ?)",
        )
        .bind(&self.federation_id.0)
        .bind(&self.requester_pubkey.0)
        .bind(&self.provider_pubkey.0)
        .bind(&self.network)
        .bind(details_payload_hash.0.to_vec())
        .bind(serde_json::to_string(&target)?)
        .bind(self.committed_amount.0 as i64)
        .bind(self.reserved_amount.0 as i64)
        .execute(database.pool())
        .await?;

        for item in &self.items {
            let item_id = allocation_store::item_id(&self.federation_id, item.source_type);
            let item_target = item
                .item_target
                .clone()
                .unwrap_or_else(|| match item.source_type {
                    SourceType::Gateway => AllocationItemTarget::Gateway {
                        item_id: item_id.clone(),
                        gateway_id: GatewayId("gateway-1".to_owned()),
                        gateway_name: GatewayName("Gateway One".to_owned()),
                        amount: item.committed_amount,
                    },
                    SourceType::StabilityPool => AllocationItemTarget::StabilityPool {
                        item_id: item_id.clone(),
                        amount: item.committed_amount,
                    },
                });
            sqlx::query(
                "INSERT INTO allocation_items \
                 (item_id, federation_id, source_type, status, committed_amount_sats, \
                  reserved_amount_sats, item_json, step_json, failure_json, \
                  created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch())",
            )
            .bind(&item_id.0)
            .bind(&self.federation_id.0)
            .bind(item.source_type.to_string())
            .bind(item.status.to_string())
            .bind(item.committed_amount.0 as i64)
            .bind(item.reserved_amount.0 as i64)
            .bind(serde_json::to_string(&item_target)?)
            .bind(item.step_json.as_deref())
            .bind(item.failure_json.as_deref())
            .execute(database.pool())
            .await?;
        }
        Ok(())
    }
}

static NEXT_TEST_DATA_DIR: AtomicU64 = AtomicU64::new(0);

fn test_data_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_nanos();
    let sequence = NEXT_TEST_DATA_DIR.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join("fedi-flip-tests")
        .join(format!("{name}-{}-{nanos}-{sequence}", std::process::id()))
}

/// Credential SDK fixtures for crate unit tests.
///
/// The containing module is compiled only under `cfg(test)`, so the hardcoded
/// test issuer keys stay out of shipped binaries.
pub(crate) mod credentials {
    use fedi_credential_sdk_protocol::{
        HolderAuthorization, HolderAuthorizationRequest, HolderContext, IssuerAuthority,
        IssuerContext, IssuerSecretKeys, PendingIssuance, SignedCredential, SubjectPubkey,
    };
    use fedi_decentralized_service_liquidity_manager::{
        AttestationPayload, HolderAuthorization as ServiceHolderAuthorization, Pubkey,
        SignedCredential as ServiceCredential,
    };
    use serde::Serialize;
    use serde_json::json;

    pub(crate) fn test_issuer_context() -> IssuerContext {
        IssuerContext::import_secret_key(&test_issuer_secret_keys())
            .expect("fixed test issuer secret keys import")
    }

    pub(crate) fn test_foreign_issuer_context() -> IssuerContext {
        let mut keys = test_issuer_secret_keys();
        keys.issuer_id_secret_key =
            "0000000000000000000000000000000000000000000000000000000000000001".to_owned();
        IssuerContext::import_secret_key(&keys).expect("fixed foreign test issuer keys import")
    }

    /// Relay URL used by unit tests that never perform live revocation
    /// lookups (the fake fetcher is keyed by this URL).
    pub(crate) const UNIT_TEST_ISSUER_RELAY: &str = "wss://relay.example";
    pub(crate) const UNIT_TEST_PEER_BADGE_TRUST_LEVEL: u64 = 9;

    pub(crate) fn test_issuer_authority(
        issuer: &IssuerContext,
        revocation_relay_url: &str,
    ) -> anyhow::Result<IssuerAuthority> {
        Ok(
            issuer.issuer_authority(vec![fedi_credential_sdk_protocol::RevocationLocation {
                protocol: "nostr".to_owned(),
                location: revocation_relay_url.to_owned(),
            }])?,
        )
    }

    pub(crate) fn issue_credential_for_holder(
        issuer: &IssuerContext,
        authority: &IssuerAuthority,
        holder: &HolderContext,
    ) -> anyhow::Result<SignedCredential> {
        issue_credential_for_holder_with_trust_level(
            issuer,
            authority,
            holder,
            UNIT_TEST_PEER_BADGE_TRUST_LEVEL,
        )
    }

    pub(crate) fn issue_credential_for_holder_with_trust_level(
        issuer: &IssuerContext,
        authority: &IssuerAuthority,
        holder: &HolderContext,
        trust_level: u64,
    ) -> anyhow::Result<SignedCredential> {
        let credential_info = json!({
            "schema": "fedi-trust-score-v1.0",
            "trust_level": trust_level,
        });
        let (request, pending) = PendingIssuance::create_request(
            &authority.issuer.issuance_key,
            authority.issuer.issuer_id_pubkey.clone(),
            credential_info.clone(),
            json!(holder.public_key().to_string()),
        )?;
        let response = issuer.issue_credential(credential_info, &request)?;
        Ok(pending.finalize(&authority.issuer.issuance_key, &response)?)
    }

    pub(crate) fn holder_authorization_for_provider(
        holder: &HolderContext,
        credential: &SignedCredential,
        provider_pubkey: &Pubkey,
    ) -> anyhow::Result<HolderAuthorization> {
        Ok(holder.authorize_credential_use(
            HolderAuthorizationRequest {
                subject_pubkey: provider_pubkey.0.parse::<SubjectPubkey>()?,
            },
            credential,
        )?)
    }

    pub(crate) fn attestation_payload<T: Serialize>(
        value: &T,
    ) -> anyhow::Result<AttestationPayload> {
        Ok(AttestationPayload(serde_json::to_vec(value)?))
    }

    /// Nostr keys for a Holder, matching its credential-SDK identity.
    ///
    /// The kind-37705 admission checks bind the signed statement's holder to
    /// the event author, so a test that signs with an unrelated key is testing
    /// the rejection path whether or not it means to.
    pub(crate) fn holder_nostr_keys(holder: &HolderContext) -> anyhow::Result<nostr_sdk::Keys> {
        Ok(nostr_sdk::Keys::parse(&holder.export_secret_key())?)
    }

    /// Build the signed kind-37705 event a Holder app publishes to authorize a
    /// provider, tagged exactly as the FLIP variant specifies.
    ///
    /// Tests go through the real event so they exercise the bytes ingest
    /// parses, rather than a hand-built struct that cannot fail the way a relay
    /// answer can.
    pub(crate) fn flip_authorization_event(
        holder: &HolderContext,
        authorization: &HolderAuthorization,
        credential: &SignedCredential,
        provider_pubkey: &Pubkey,
    ) -> anyhow::Result<nostr_sdk::Event> {
        let credential_digest =
            serde_json::to_value(&authorization.authorization.credential_digest)?
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("credential digest serializes as a string"))?
                .to_owned();
        let content = serde_json_canonicalizer::to_string(&json!({
            "version": 1,
            "holder_id_pubkey": holder.public_key().to_string(),
            "holder_authorization": authorization,
            "signed_credential": credential,
        }))?;
        let issuer_pubkey = credential.credential.issuer_id_pubkey.0.to_string();
        let d_tag = fedi_decentralized_nostr::flip::flip_authorization_d_tag(
            &provider_pubkey.0,
            &credential_digest,
        );
        let builder = nostr_sdk::EventBuilder::new(
            nostr_sdk::Kind::Custom(
                fedi_decentralized_nostr::flip::HOLDER_AUTHORIZATION_EVENT_KIND,
            ),
            content,
        )
        .tags([
            nostr_sdk::Tag::parse(["d", d_tag.as_str()])?,
            nostr_sdk::Tag::parse([
                "t",
                fedi_decentralized_nostr::flip::FLIP_AUTHORIZATION_HASHTAG,
            ])?,
            nostr_sdk::Tag::parse(["p", provider_pubkey.0.as_str()])?,
            nostr_sdk::Tag::parse(["issuer", issuer_pubkey.as_str()])?,
            nostr_sdk::Tag::parse(["credential", credential_digest.as_str()])?,
            nostr_sdk::Tag::parse(["schema", "fedi-trust-score-v1.0"])?,
        ]);
        Ok(builder.sign_with_keys(&holder_nostr_keys(holder)?)?)
    }

    pub(crate) fn service_credential(
        credential: &SignedCredential,
    ) -> anyhow::Result<ServiceCredential> {
        Ok(serde_json::from_value(serde_json::to_value(credential)?)?)
    }

    pub(crate) fn service_holder_authorization(
        authorization: &HolderAuthorization,
    ) -> anyhow::Result<ServiceHolderAuthorization> {
        Ok(serde_json::from_value(serde_json::to_value(
            authorization,
        )?)?)
    }

    fn test_issuer_secret_keys() -> IssuerSecretKeys {
        serde_json::from_str(include_str!("fixtures/issuer-secret-keys.json"))
            .expect("fixed test issuer keys deserialize")
    }
}
