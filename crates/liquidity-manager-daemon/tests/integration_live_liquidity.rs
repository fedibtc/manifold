mod common;
mod test_support;

/// Module kinds the previewed config claims for an ordinary target federation.
///
/// These fixtures substitute the preview, so they also decide what acceptance
/// believes the target can do. A gateway-only stack deliberately claims no
/// stability-pool module: its federation does not have one, and a fixture that
/// pretended otherwise would hide the capability gate rather than exercise it.
const GATEWAY_TARGET_MODULE_KINDS: &[&str] = &["wallet", "mint", "ln"];

/// The same, for the stability-pool stack, whose federation really does carry
/// the module.
const STABILITY_TARGET_MODULE_KINDS: &[&str] =
    &["wallet", "mint", "ln", STABILITY_POOL_MODULE_KIND];

use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use common::defe::DefeResources;
use common::live_liquidity::bitcoin::BitcoinFixture;
use common::live_liquidity::daemon::{
    DaemonLaunch, DaemonProcess, TestDataDir, TestPorts, admin_post, admin_post_when_available,
    wait_for_endpoint_addr, wait_for_health,
};
use common::live_liquidity::esplora::EsploraFixture;
use common::live_liquidity::fedimint::FedimintFixture;
use common::live_liquidity::gateway::GatewayFixture;
use common::live_liquidity::trust::{self, LiveTrust};
use common::live_liquidity::unique_test_id;
use common::nostr_relay::{find_advertisement_event, query_advertisement_event};
use fedi_decentralized_liquidity_manager_daemon::{
    Database, FLIP_PROVIDER_ADVERTISEMENT_D_TAG, FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
    FLIP_PROVIDER_ADVERTISEMENT_HASHTAG, STABILITY_POOL_MODULE_KIND,
};
use fedi_decentralized_service_liquidity_manager::{
    AllocationItemTarget, BitcoinNetwork, FederationId, FederationLiquidityDetails, FederationName,
    FmanEndorsement, GetAllocationStatusRequest, GetAllocationStatusResponse,
    GetFmanTrustMaterialResponse, HashBytes, InviteCode as ServiceInviteCode, ItemAllocationStatus,
    LiquidityAmountBounds, LiquidityProviderAdvertisement, PayloadProof, ProtocolVersion, Pubkey,
    PublicLiquidityApi, PublicLiquidityApiClient, PublicRejectionCode, PublicRpcPayloadDomain,
    RequestLiquidityOutcome, RequestLiquidityRequest, Sats, Sha256Digest, Signature, Signed,
    Timestamp, Url, public_rpc_payload_hash, request_liquidity_details_hash_for_request,
};
use fedi_iroh_rpc::iroh::{Endpoint, EndpointAddr, TransportAddr, endpoint::presets};
use nostr_sdk::secp256k1::Message;
use reqwest::{Client, StatusCode};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::Row;
use tracing_subscriber::EnvFilter;

/// Data-directory name for the target federation a test stands up.
///
/// Named per federation rather than fixed inside the fixture: a test that needs
/// two of them needs two directories, and two `fedimintd` processes sharing one
/// is a corrupt federation rather than a failed test.
const TARGET_FEDIMINT_LABEL: &str = "target-fedimint";

/// The same, for the second federation in the two-federation test.
const SECOND_TARGET_FEDIMINT_LABEL: &str = "second-target-fedimint";

const GATEWAY_AMOUNT: u64 = 1_000_000;
const STABILITY_AMOUNT: u64 = 1_000_000;
const GATEWAY_FEE_RESERVE: u64 = 200_000;
const TOP_UP_BTC: f64 = 1.0;

/// Deposit that covers one gateway allocation and no more: the item's amount
/// plus the fee reserve the wallet withholds, with a margin the second
/// allocation cannot fit into.
const ONE_ALLOCATION_BTC: f64 = 0.015;
const FEDIMINT_FINALITY_BLOCKS: u32 = 11;
const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("fedi_decentralized_liquidity_manager_daemon=debug,info")
        }))
        .with_test_writer()
        .try_init();
}

struct LiveWalletStack {
    _resources: DefeResources,
    bitcoin: BitcoinFixture,
    gateway: GatewayFixture,
    relay_url: String,
    ports: TestPorts,
    data_dir: TestDataDir,
    launch: DaemonLaunch,
    trust: LiveTrust,
    admin_url: String,
    http: Client,
    daemon: Option<DaemonProcess>,

    /// Endpoint identity from the first boot; every later boot must match it.
    first_endpoint_addr: Option<EndpointAddr>,
}

impl LiveWalletStack {
    async fn start(test_name: &str) -> anyhow::Result<Self> {
        let test_id = unique_test_id(test_name);
        Self::start_with_test_id(test_name, &test_id).await
    }

    async fn start_with_test_id(test_name: &str, test_id: &str) -> anyhow::Result<Self> {
        let data_dir = TestDataDir::new(test_name)?;
        let resources = DefeResources::live_liquidity().await?;
        let relay_url = resources.relay().url.clone();
        let bitcoin =
            BitcoinFixture::new(resources.bitcoind().clone()).context("bitcoin fixture")?;
        let gateway = GatewayFixture::start(test_id, &bitcoin, data_dir.path())
            .await
            .context("gateway fixture")?;
        let ports = TestPorts::allocate()?;
        let mut launch = DaemonLaunch::new(data_dir.path())?;
        launch.holder_authorization_relay_url = Some(relay_url.clone());
        let trust = trust::live_trust(&launch, &relay_url)?;
        let admin_url = format!("http://{}", ports.admin_bind_address);
        let http = Client::new();
        let mut stack = Self {
            _resources: resources,
            bitcoin,
            gateway,
            relay_url,
            ports,
            data_dir,
            launch,
            trust,
            admin_url,
            http,
            daemon: None,
            first_endpoint_addr: None,
        };
        let endpoint_addr = stack.start_daemon().await?;
        trust::install_issuer_authority(&stack.http, &stack.admin_url, &stack.trust).await?;
        // Enrollment reconciles against the configured relays, so it can only
        // run once setup has named them.
        stack.apply_setup(&endpoint_addr).await?;
        trust::enrol_provider_authorization(
            &stack.http,
            &stack.admin_url,
            &stack.trust,
            &stack.relay_url,
        )
        .await?;
        Ok(stack)
    }

    async fn start_daemon(&mut self) -> anyhow::Result<EndpointAddr> {
        ensure!(self.daemon.is_none(), "daemon is already running");
        let mut daemon = DaemonProcess::start(self.data_dir.path(), &self.ports, &self.launch)?;
        wait_for_health(&self.http, &self.admin_url, &mut daemon).await?;
        let endpoint_addr = wait_for_endpoint_addr(self.data_dir.path(), &mut daemon).await?;
        self.daemon = Some(daemon);

        // The public transport key is derived from the provider identity, not
        // generated per boot, so every restart in this suite must come back on
        // the same node id. If it ever does not, the advertised address is
        // stale and the deployment silently stops being reachable.
        match &self.first_endpoint_addr {
            Some(first) => ensure!(
                endpoint_addr.id == first.id,
                "public endpoint identity changed across a restart: {:?} -> {:?}",
                first.id,
                endpoint_addr.id
            ),
            None => self.first_endpoint_addr = Some(endpoint_addr.clone()),
        }
        Ok(endpoint_addr)
    }

    fn stop_daemon(&mut self) -> anyhow::Result<()> {
        if let Some(mut daemon) = self.daemon.take() {
            daemon.stop()?;
        }
        Ok(())
    }

    async fn apply_setup(&self, endpoint_addr: &EndpointAddr) -> anyhow::Result<()> {
        apply_live_setup(
            &self.http,
            &self.admin_url,
            &self.gateway,
            &self.bitcoin,
            &self.relay_url,
            endpoint_addr,
            &self.trust.attester_pubkey_hex,
            None,
        )
        .await
    }

    async fn configure_publish_and_connect(
        &mut self,
    ) -> anyhow::Result<(
        EndpointAddr,
        Signed<LiquidityProviderAdvertisement>,
        PublicRpcHarness,
    )> {
        let Self {
            bitcoin,
            gateway,
            relay_url,
            data_dir,
            trust,
            admin_url,
            http,
            daemon,
            ..
        } = self;
        let daemon = daemon.as_mut().context("daemon is not running")?;
        configure_publish_and_connect(
            http,
            admin_url,
            gateway,
            bitcoin,
            relay_url,
            data_dir.path(),
            daemon,
            trust,
            None,
        )
        .await
    }

    async fn fund_gateway_wallet(&self) -> anyhow::Result<()> {
        fund_gateway_wallet(&self.http, &self.admin_url, &self.bitcoin).await
    }

    async fn create_deposit_address(&self, label: &str) -> anyhow::Result<(String, String)> {
        let response = admin_post(
            &self.http,
            &self.admin_url,
            "create_deposit_address",
            &json!({ "label": label }),
        )
        .await?;
        let address = response["address"]
            .as_str()
            .context("create_deposit_address returned address")?
            .to_owned();
        let operation_id = response["operation_id"]
            .as_str()
            .context("create_deposit_address returned operation_id")?
            .to_owned();
        Ok((address, operation_id))
    }
}

struct LiveLiquidityStack {
    wallet: LiveWalletStack,
    target_fedimint: FedimintFixture,
    endorsement: FmanEndorsement,
    trust_material: Vec<GetFmanTrustMaterialResponse>,
}

impl LiveLiquidityStack {
    async fn start(test_name: &str) -> anyhow::Result<Self> {
        let test_id = unique_test_id(test_name);
        let wallet = LiveWalletStack::start_with_test_id(test_name, &test_id).await?;
        let target_fedimint = FedimintFixture::start(
            TARGET_FEDIMINT_LABEL,
            &wallet.bitcoin,
            wallet.data_dir.path(),
        )
        .await
        .context("target fedimint fixture")?;
        // The target federation (and thus its invite code) only exists now,
        // after the daemon booted: fixture files are read lazily per call.
        let artifacts = trust::write_trust_fixtures(
            &wallet.trust,
            &wallet.launch.trust_fixtures_dir,
            &target_fedimint.invite_code,
            &live_config_hash(),
            GATEWAY_TARGET_MODULE_KINDS,
        )?;
        // The FMan advertisement transport is real, so every operating
        // identity must be resolvable on the leased relay before a request
        // can be accepted.
        Ok(Self {
            wallet,
            target_fedimint,
            endorsement: artifacts.endorsement,
            trust_material: artifacts.trust_material,
        })
    }

    async fn request_gateway_liquidity(
        &mut self,
    ) -> anyhow::Result<Signed<RequestLiquidityRequest>> {
        let (_endpoint_addr, advertisement, rpc) =
            self.wallet.configure_publish_and_connect().await?;
        let liquidity_request = live_liquidity_request(
            &advertisement.payload.provider_pubkey,
            &self.target_fedimint.invite_code,
            &self.endorsement,
            &self.trust_material,
        )?;
        let signed_request = sign_public_rpc(
            PublicRpcPayloadDomain::RequestLiquidityRequest,
            liquidity_request,
        )?;
        let response = rpc
            .client
            .request_liquidity(signed_request.clone())
            .await
            .context("request_liquidity over real Iroh")?;
        match &response.payload.outcome {
            RequestLiquidityOutcome::Accepted(status) => {
                assert_eq!(status.item_statuses.len(), 1);
            }
            RequestLiquidityOutcome::Rejected(rejection) => {
                anyhow::bail!("live liquidity request rejected: {rejection:?}");
            }
        }
        Ok(signed_request)
    }
}

/// A coherent archive from before acceptance cannot erase the allocation
/// identity that makes an exact FI replay idempotent.
#[tokio::test(flavor = "multi_thread")]
async fn live_restore_rejects_allocation_rollback_and_replay_stays_idempotent() -> anyhow::Result<()>
{
    init_logging();

    let mut stack = LiveLiquidityStack::start("live-restore-allocation-rollback").await?;
    stack.wallet.fund_gateway_wallet().await?;
    let (_endpoint_addr, advertisement, rpc) = stack.wallet.configure_publish_and_connect().await?;

    let backup = admin_post(
        &stack.wallet.http,
        &stack.wallet.admin_url,
        "create_backup",
        &json!({}),
    )
    .await?;
    let archive = backup["archive"]
        .as_str()
        .context("create_backup returned archive")?;

    let request = sign_public_rpc(
        PublicRpcPayloadDomain::RequestLiquidityRequest,
        live_liquidity_request(
            &advertisement.payload.provider_pubkey,
            &stack.target_fedimint.invite_code,
            &stack.endorsement,
            &stack.trust_material,
        )?,
    )?;
    let first = rpc
        .client
        .request_liquidity(request.clone())
        .await
        .context("first request_liquidity over real Iroh")?;
    ensure!(
        matches!(first.payload.outcome, RequestLiquidityOutcome::Accepted(_)),
        "first liquidity request was not accepted"
    );

    let restore = stack
        .wallet
        .http
        .post(format!(
            "{}/admin/v1/restore_backup",
            stack.wallet.admin_url
        ))
        .bearer_auth(common::live_liquidity::daemon::ADMIN_TOKEN)
        .json(&json!({ "archive": archive }))
        .send()
        .await
        .context("request live restore")?;
    assert_eq!(restore.status(), StatusCode::PRECONDITION_FAILED);
    let restore_error: Value = restore.json().await.context("decode restore refusal")?;
    assert_eq!(restore_error["code"], "failed_precondition");
    assert!(
        restore_error["message"]
            .as_str()
            .is_some_and(|message| message.contains("archive predates the accepted allocation")),
        "restore refusal should identify the allocation rollback: {restore_error}"
    );

    let replay = rpc
        .client
        .request_liquidity(request)
        .await
        .context("replayed request_liquidity over real Iroh")?;
    ensure!(
        matches!(replay.payload.outcome, RequestLiquidityOutcome::Accepted(_)),
        "exact replay should answer from the unchanged existing allocation"
    );
    let allocations = admin_post(
        &stack.wallet.http,
        &stack.wallet.admin_url,
        "list_allocations",
        &json!({
            "page": { "cursor": null, "limit": 10 },
            "time_range": null
        }),
    )
    .await?;
    assert_eq!(
        allocations["allocations"]["items"].as_array().map(Vec::len),
        Some(1)
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_liquidity_happy_path_publishes_nostr_and_allocates_over_iroh() -> anyhow::Result<()> {
    init_logging();

    let test_id = unique_test_id("live_liquidity");
    let data_dir = TestDataDir::new("integration-live-liquidity")?;
    let _resources = DefeResources::live_liquidity().await?;
    let relay_url = _resources.relay().url.clone();
    let bitcoin = BitcoinFixture::new(_resources.bitcoind().clone()).context("bitcoin fixture")?;
    let gateway = GatewayFixture::start(&test_id, &bitcoin, data_dir.path())
        .await
        .context("gateway fixture")?;
    let target_fedimint = FedimintFixture::start(TARGET_FEDIMINT_LABEL, &bitcoin, data_dir.path())
        .await
        .context("target fedimint fixture")?;

    let ports = TestPorts::allocate()?;
    let mut launch = DaemonLaunch::new(data_dir.path())?;
    launch.holder_authorization_relay_url = Some(relay_url.clone());
    let trust = trust::live_trust(&launch, &relay_url)?;
    let artifacts = trust::write_trust_fixtures(
        &trust,
        &launch.trust_fixtures_dir,
        &target_fedimint.invite_code,
        &live_config_hash(),
        GATEWAY_TARGET_MODULE_KINDS,
    )?;
    let admin_url = format!("http://{}", ports.admin_bind_address);
    let http = Client::new();
    let mut daemon = DaemonProcess::start(data_dir.path(), &ports, &launch)?;
    wait_for_health(&http, &admin_url, &mut daemon).await?;
    trust::install_issuer_authority(&http, &admin_url, &trust).await?;
    let endpoint_addr = wait_for_endpoint_addr(data_dir.path(), &mut daemon).await?;

    apply_live_setup(
        &http,
        &admin_url,
        &gateway,
        &bitcoin,
        &relay_url,
        &endpoint_addr,
        &trust.attester_pubkey_hex,
        None,
    )
    .await?;
    fund_gateway_wallet(&http, &admin_url, &bitcoin).await?;

    let (endpoint_addr, advertisement, rpc) = configure_publish_and_connect(
        &http,
        &admin_url,
        &gateway,
        &bitcoin,
        &relay_url,
        data_dir.path(),
        &mut daemon,
        &trust,
        None,
    )
    .await?;
    let liquidity_request = live_liquidity_request(
        &advertisement.payload.provider_pubkey,
        &target_fedimint.invite_code,
        &artifacts.endorsement,
        &artifacts.trust_material,
    )?;
    let signed_request = sign_public_rpc(
        PublicRpcPayloadDomain::RequestLiquidityRequest,
        liquidity_request,
    )?;
    // Two first submissions race on the real Iroh surface. Both callers must
    // observe the one durable allocation, never a transient conflict or two
    // independently funded rows.
    let (first, second) = tokio::join!(
        rpc.client.request_liquidity(signed_request.clone()),
        rpc.client.request_liquidity(signed_request.clone()),
    );
    let response = first.context("first concurrent request_liquidity over real Iroh")?;
    let concurrent = second.context("second concurrent request_liquidity over real Iroh")?;
    let status = match &response.payload.outcome {
        RequestLiquidityOutcome::Accepted(status) => status.clone(),
        RequestLiquidityOutcome::Rejected(rejection) => {
            anyhow::bail!("first live liquidity request rejected: {rejection:?}");
        }
    };
    assert_eq!(status.item_statuses.len(), 1);
    ensure!(
        matches!(
            concurrent.payload.outcome,
            RequestLiquidityOutcome::Accepted(_)
        ),
        "concurrent exact request did not resolve to the accepted allocation: {concurrent:?}"
    );

    let exact_replay = rpc
        .client
        .request_liquidity(signed_request.clone())
        .await
        .context("exact request replay over real Iroh")?;
    ensure!(
        matches!(
            exact_replay.payload.outcome,
            RequestLiquidityOutcome::Accepted(_)
        ),
        "exact replay did not return the accepted allocation: {exact_replay:?}"
    );
    let allocations = admin_post(
        &http,
        &admin_url,
        "list_allocations",
        &json!({ "page": { "cursor": null, "limit": 10 }, "time_range": null }),
    )
    .await?;
    assert_eq!(
        allocations["allocations"]["items"].as_array().map(Vec::len),
        Some(1),
        "concurrent and replayed requests must commit one allocation"
    );

    // The accepted request ran the full verification pipeline: policy passed
    // and every seat, credential, and revocation stage passed for real.
    assert_live_verification_passed(
        &http,
        &admin_url,
        &signed_request.payload.federation_details.federation_id,
    )
    .await?;

    // The federation is the allocation's identity: a later request for the
    // same federation with different details conflicts before verification
    // runs, so even a freshly published revocation cannot disturb the
    // existing allocation. (Revocation gating of new allocations is covered
    // by the verification pipeline's unit tests.) The amount bump makes the
    // details commitment differ deterministically; a rebuilt request could
    // otherwise land in the same wall-clock second as the original and hash
    // identically, which is the idempotent-repeat case, not a conflict.
    trust::publish_revocation(&relay_url, &trust, &artifacts.fman_credentials[0]).await?;
    let mut conflicting_details = live_liquidity_request(
        &advertisement.payload.provider_pubkey,
        &target_fedimint.invite_code,
        &artifacts.endorsement,
        &artifacts.trust_material,
    )?;
    conflicting_details.amounts.gateway_min_amount = Sats(GATEWAY_AMOUNT + 1);
    conflicting_details.details_payload_hash =
        request_liquidity_details_hash_for_request(&conflicting_details)
            .context("recompute conflicting details_payload_hash")?;
    let conflicting_request = sign_public_rpc(
        PublicRpcPayloadDomain::RequestLiquidityRequest,
        conflicting_details,
    )?;
    let conflicting_response = rpc
        .client
        .request_liquidity(conflicting_request)
        .await
        .context("post-allocation request over real Iroh")?;
    match &conflicting_response.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => {
            assert_eq!(rejection.code, PublicRejectionCode::RequestConflict);
        }
        RequestLiquidityOutcome::Accepted(_) => {
            anyhow::bail!("conflicting request for an allocated federation must be rejected");
        }
    }

    let federation_id = wait_for_any_admin_allocation(&http, &admin_url).await?;
    wait_for_active_wallet_operations(&http, &admin_url, &federation_id).await?;
    daemon.stop()?;
    mine_and_sync(&bitcoin, FEDIMINT_FINALITY_BLOCKS).await?;

    let mut daemon = DaemonProcess::start(data_dir.path(), &ports, &launch)?;
    wait_for_health(&http, &admin_url, &mut daemon).await?;
    // No reconfiguration: the restarted daemon keeps its endpoint identity.
    let rpc = reconnect_after_restart(data_dir.path(), &mut daemon, &endpoint_addr).await?;
    let final_status = wait_for_public_completion(
        &rpc,
        &signed_request.payload,
        &bitcoin,
        Duration::from_secs(900),
    )
    .await?;
    assert!(
        final_status
            .payload
            .status
            .item_statuses
            .iter()
            .all(|item| item.status == ItemAllocationStatus::Completed)
    );

    let health = admin_post(&http, &admin_url, "get_health", &json!({})).await?;
    ensure!(
        health.to_string().contains("\"component\":\"relays\"")
            && health.to_string().contains("\"status\":\"healthy\""),
        "relay health was not healthy after live publication: {health}"
    );

    daemon.stop()?;

    let mut restarted = DaemonProcess::start(data_dir.path(), &ports, &launch)?;
    wait_for_health(&http, &admin_url, &mut restarted).await?;
    let restarted_rpc =
        reconnect_after_restart(data_dir.path(), &mut restarted, &endpoint_addr).await?;

    let restarted_allocation = get_admin_allocation(&http, &admin_url, &federation_id).await?;
    ensure!(
        restarted_allocation["allocation"]["status"]["item_statuses"]
            .as_array()
            .is_some_and(|items| items.iter().all(|item| item["status"] == "completed")),
        "completed items should remain terminal after restart: {restarted_allocation}"
    );
    ensure!(
        allocation_wallet_operations_completed(&restarted_allocation)?,
        "completed wallet operations should remain durable after restart: {restarted_allocation}"
    );

    let restarted_status = get_public_status(&restarted_rpc, &signed_request.payload).await?;
    assert!(
        restarted_status
            .payload
            .status
            .item_statuses
            .iter()
            .all(|item| item.status == ItemAllocationStatus::Completed)
    );

    restarted.stop()?;
    Ok(())
}

/// The operator's replenishment path, end to end: a request the wallet cannot
/// cover is refused, the operator tops the wallet up, and the same signed
/// request is admitted and funded.
///
/// A capacity refusal persists nothing, so the repeat is not a retry of stored
/// work. Acceptance re-plans the same bytes against the balance the top-up left
/// behind. That is what makes this an operator path rather than an idempotency
/// one: the requester re-sends a signature it already holds, and the provider
/// remembers nothing about having said no.
#[tokio::test(flavor = "multi_thread")]
async fn live_top_up_admits_the_request_capacity_first_refused() -> anyhow::Result<()> {
    init_logging();

    // `LiveLiquidityStack::start` funds nothing, and the leased bitcoind is
    // exclusive, so its chain is fresh and the gatewayd wallet below has never
    // held a satoshi.
    let mut stack = LiveLiquidityStack::start("live-capacity-top-up").await?;
    let (_endpoint_addr, advertisement, rpc) = stack.wallet.configure_publish_and_connect().await?;
    let http = stack.wallet.http.clone();
    let admin_url = stack.wallet.admin_url.clone();

    // `get_funds` reads gatewayd and persists the balance observation that
    // acceptance plans against. Without it the refusal below could be the
    // absence of any observation rather than a decision about a real balance,
    // and the two answer with the same rejection code.
    let empty = admin_post(&http, &admin_url, "get_funds", &json!({})).await?;
    assert_eq!(empty["balance"]["spendable"], 0);
    assert_eq!(empty["balance"]["available_balance"], 0);
    assert_eq!(
        empty["replenishment"], "critical",
        "an empty wallet must show the operator a critical replenishment state: {empty}"
    );

    let signed_request = sign_public_rpc(
        PublicRpcPayloadDomain::RequestLiquidityRequest,
        live_liquidity_request(
            &advertisement.payload.provider_pubkey,
            &stack.target_fedimint.invite_code,
            &stack.endorsement,
            &stack.trust_material,
        )?,
    )?;
    let refused = rpc
        .client
        .request_liquidity(signed_request.clone())
        .await
        .context("request_liquidity over real Iroh before the top-up")?;
    match &refused.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => assert_eq!(
            rejection.code,
            PublicRejectionCode::InsufficientCapacity,
            "an unfunded wallet must refuse on capacity: {rejection:?}"
        ),
        RequestLiquidityOutcome::Accepted(_) => {
            anyhow::bail!("an unfunded provider must not accept a gateway allocation")
        }
    }
    let after_refusal = admin_post(
        &http,
        &admin_url,
        "list_allocations",
        &json!({ "page": { "cursor": null, "limit": 10 }, "time_range": null }),
    )
    .await?;
    assert_eq!(
        after_refusal["allocations"]["items"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "a refused request must leave no allocation behind"
    );

    // The operator tops the wallet up the way the runbook says: a deposit
    // address from the Admin API, a real on-chain send to it, and the
    // confirmations the daemon requires before it will spend.
    stack.wallet.fund_gateway_wallet().await?;
    let funded = admin_post(&http, &admin_url, "get_funds", &json!({})).await?;
    ensure!(
        funded["balance"]["available_balance"]
            .as_u64()
            .is_some_and(|available| available >= GATEWAY_AMOUNT + GATEWAY_FEE_RESERVE),
        "the top-up left too little available balance to plan the request: {funded}"
    );
    assert_eq!(funded["replenishment"], "ok");

    // The same request bytes, unchanged and re-sent.
    let accepted = rpc
        .client
        .request_liquidity(signed_request.clone())
        .await
        .context("request_liquidity over real Iroh after the top-up")?;
    match &accepted.payload.outcome {
        RequestLiquidityOutcome::Accepted(status) => assert_eq!(status.item_statuses.len(), 1),
        RequestLiquidityOutcome::Rejected(rejection) => {
            anyhow::bail!("the topped-up provider rejected the repeated request: {rejection:?}")
        }
    }
    // The refusal short-circuited nothing that acceptance now skips: the
    // admitted request ran the whole verification pipeline for real.
    assert_live_verification_passed(
        &http,
        &admin_url,
        &signed_request.payload.federation_details.federation_id,
    )
    .await?;

    let federation_id = wait_for_any_admin_allocation(&http, &admin_url).await?;
    let final_status = wait_for_public_completion(
        &rpc,
        &signed_request.payload,
        &stack.wallet.bitcoin,
        Duration::from_secs(900),
    )
    .await?;
    assert!(
        final_status
            .payload
            .status
            .item_statuses
            .iter()
            .all(|item| item.status == ItemAllocationStatus::Completed)
    );

    // The admitted repeat funds once, out of the money the top-up added.
    let operation =
        wait_for_gateway_funding_operation(&http, &admin_url, &federation_id, &["completed"], true)
            .await?;
    let operation_id = operation["operation_id"]
        .as_str()
        .context("gateway funding operation id")?
        .to_owned();
    let txid = operation["txid"]
        .as_str()
        .context("gateway funding operation txid")?
        .to_owned();
    let allocation = get_admin_allocation(&http, &admin_url, &federation_id).await?;
    assert_single_completed_gateway_operation(&allocation, &operation_id, &txid)?;
    assert_gateway_completion_evidence(&allocation, &operation_id, &txid)?;

    stack.wallet.stop_daemon()?;
    Ok(())
}

/// Two federations, one wallet, and the operator in between.
///
/// The first request is admitted and funded. The second arrives against a
/// wallet whose free balance the first has already reserved, so it is refused
/// rather than queued: FLIP keeps no pending-request state, and the refusal
/// persists nothing. The operator tops the wallet up, the second requester asks
/// again — the only thing that can restart it — and both federations end funded
/// out of their own wallet operations.
///
/// This is the shape of a working day rather than of a single mechanism, so it
/// is where an interaction between capacity accounting, the funding worker and
/// settlement attribution would show up: two allocations that share one wallet
/// must not share an operation, a txid, or each other's money.
#[tokio::test(flavor = "multi_thread")]
async fn live_second_federation_is_funded_after_the_operator_tops_up() -> anyhow::Result<()> {
    init_logging();

    let mut stack = LiveLiquidityStack::start("live-two-federations").await?;

    // A second real target federation on the same chain, in its own data
    // directory. Its preview joins the fixture map rather than replacing the
    // first federation's.
    let second_fedimint = FedimintFixture::start(
        SECOND_TARGET_FEDIMINT_LABEL,
        &stack.wallet.bitcoin,
        stack.wallet.data_dir.path(),
    )
    .await
    .context("second target fedimint fixture")?;
    let second = trust::write_trust_fixtures(
        &stack.wallet.trust,
        &stack.wallet.launch.trust_fixtures_dir,
        &second_fedimint.invite_code,
        &live_config_hash(),
        GATEWAY_TARGET_MODULE_KINDS,
    )?;

    // Funded for one allocation and not two. A wallet holding the suite's usual
    // whole bitcoin would admit both requests and prove nothing about capacity.
    fund_gateway_wallet_amount(
        &stack.wallet.http,
        &stack.wallet.admin_url,
        &stack.wallet.bitcoin,
        ONE_ALLOCATION_BTC,
        GATEWAY_AMOUNT + GATEWAY_FEE_RESERVE,
    )
    .await?;

    let (_endpoint_addr, advertisement, rpc) = stack.wallet.configure_publish_and_connect().await?;
    let http = stack.wallet.http.clone();
    let admin_url = stack.wallet.admin_url.clone();

    let first_request = sign_public_rpc(
        PublicRpcPayloadDomain::RequestLiquidityRequest,
        live_liquidity_request(
            &advertisement.payload.provider_pubkey,
            &stack.target_fedimint.invite_code,
            &stack.endorsement,
            &stack.trust_material,
        )?,
    )?;
    let second_request = sign_public_rpc(
        PublicRpcPayloadDomain::RequestLiquidityRequest,
        live_liquidity_request(
            &advertisement.payload.provider_pubkey,
            &second_fedimint.invite_code,
            &second.endorsement,
            &second.trust_material,
        )?,
    )?;
    let first_federation_id = first_request
        .payload
        .federation_details
        .federation_id
        .0
        .clone();
    let second_federation_id = second_request
        .payload
        .federation_details
        .federation_id
        .0
        .clone();
    ensure!(
        first_federation_id != second_federation_id,
        "the two fixtures must be two federations"
    );

    let first_outcome = rpc
        .client
        .request_liquidity(first_request.clone())
        .await
        .context("first federation request_liquidity over real Iroh")?;
    match &first_outcome.payload.outcome {
        RequestLiquidityOutcome::Accepted(status) => assert_eq!(status.item_statuses.len(), 1),
        RequestLiquidityOutcome::Rejected(rejection) => {
            anyhow::bail!("the first federation was rejected: {rejection:?}")
        }
    }

    // The second federation meets a wallet whose free balance is committed to
    // the first. Reserved, not spent: nothing has left the wallet yet.
    let reserved_refusal = rpc
        .client
        .request_liquidity(second_request.clone())
        .await
        .context("second federation request_liquidity while the first is in flight")?;
    match &reserved_refusal.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => assert_eq!(
            rejection.code,
            PublicRejectionCode::InsufficientCapacity,
            "an amount already reserved must not be offered twice: {rejection:?}"
        ),
        RequestLiquidityOutcome::Accepted(_) => {
            anyhow::bail!("a second allocation must not be admitted against reserved capacity")
        }
    }
    let after_refusal = admin_post(
        &http,
        &admin_url,
        "list_allocations",
        &json!({ "page": { "cursor": null, "limit": 10 }, "time_range": null }),
    )
    .await?;
    assert_eq!(
        after_refusal["allocations"]["items"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "the refused federation must hold no allocation"
    );

    let first_status = wait_for_public_completion(
        &rpc,
        &first_request.payload,
        &stack.wallet.bitcoin,
        Duration::from_secs(900),
    )
    .await?;
    assert!(
        first_status
            .payload
            .status
            .item_statuses
            .iter()
            .all(|item| item.status == ItemAllocationStatus::Completed)
    );

    // Still refused once the money has actually left: the shortfall is the
    // wallet's, not an artifact of holding a reservation.
    let settled_refusal = rpc
        .client
        .request_liquidity(second_request.clone())
        .await
        .context("second federation request_liquidity after the first settled")?;
    match &settled_refusal.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => assert_eq!(
            rejection.code,
            PublicRejectionCode::InsufficientCapacity,
            "a spent wallet must still refuse: {rejection:?}"
        ),
        RequestLiquidityOutcome::Accepted(_) => {
            anyhow::bail!("a wallet that funded the first allocation cannot afford the second")
        }
    }

    // The operator tops up, and the second requester asks again.
    stack.wallet.fund_gateway_wallet().await?;
    let admitted = rpc
        .client
        .request_liquidity(second_request.clone())
        .await
        .context("second federation request_liquidity after the top-up")?;
    match &admitted.payload.outcome {
        RequestLiquidityOutcome::Accepted(status) => assert_eq!(status.item_statuses.len(), 1),
        RequestLiquidityOutcome::Rejected(rejection) => {
            anyhow::bail!("the topped-up provider rejected the second federation: {rejection:?}")
        }
    }
    let second_status = wait_for_public_completion(
        &rpc,
        &second_request.payload,
        &stack.wallet.bitcoin,
        Duration::from_secs(900),
    )
    .await?;
    assert!(
        second_status
            .payload
            .status
            .item_statuses
            .iter()
            .all(|item| item.status == ItemAllocationStatus::Completed)
    );

    // Two federations, two allocations, and two sends that share nothing.
    let allocations = admin_post(
        &http,
        &admin_url,
        "list_allocations",
        &json!({ "page": { "cursor": null, "limit": 10 }, "time_range": null }),
    )
    .await?;
    assert_eq!(
        allocations["allocations"]["items"].as_array().map(Vec::len),
        Some(2),
        "each funded federation must hold exactly one allocation"
    );

    let mut txids = Vec::new();
    for federation_id in [&first_federation_id, &second_federation_id] {
        let operation = wait_for_gateway_funding_operation(
            &http,
            &admin_url,
            federation_id,
            &["completed"],
            true,
        )
        .await?;
        let operation_id = operation["operation_id"]
            .as_str()
            .context("gateway funding operation id")?
            .to_owned();
        let txid = operation["txid"]
            .as_str()
            .context("gateway funding operation txid")?
            .to_owned();
        let allocation = get_admin_allocation(&http, &admin_url, federation_id).await?;
        assert_single_completed_gateway_operation(&allocation, &operation_id, &txid)?;
        assert_gateway_completion_evidence(&allocation, &operation_id, &txid)?;
        txids.push(txid);
    }
    ensure!(
        txids[0] != txids[1],
        "two allocations must not settle on one transaction: {txids:?}"
    );

    stack.wallet.stop_daemon()?;
    Ok(())
}

/// Cancelling a wedged allocation hands its capacity to the next federation.
///
/// A federation that goes away after its request is admitted leaves an
/// allocation nothing can finish. Acceptance never touches the target
/// federation -- it reads a preview, a seat-binding directory and the trust
/// material the request carries -- so FLIP admits the request and reserves the
/// money for it, and only then does the funding worker discover that there is
/// no federation for a gateway to join.
///
/// The reservation is what makes that more than untidy. It is money the
/// provider cannot offer anyone else, so the next federation in line is refused
/// for capacity FLIP is holding against work that will never happen.
///
/// `cancel_allocation` is the operator's answer, and this is the property worth
/// a live test: not that a row changes state, which unit tests already show,
/// but that the money comes back and the next requester is admitted with it.
#[tokio::test(flavor = "multi_thread")]
async fn live_cancelling_a_wedged_allocation_frees_capacity_for_the_next_federation()
-> anyhow::Result<()> {
    init_logging();

    let mut stack = LiveLiquidityStack::start("live-cancel-frees-capacity").await?;
    let second_fedimint = FedimintFixture::start(
        SECOND_TARGET_FEDIMINT_LABEL,
        &stack.wallet.bitcoin,
        stack.wallet.data_dir.path(),
    )
    .await
    .context("second target fedimint fixture")?;
    let second = trust::write_trust_fixtures(
        &stack.wallet.trust,
        &stack.wallet.launch.trust_fixtures_dir,
        &second_fedimint.invite_code,
        &live_config_hash(),
        GATEWAY_TARGET_MODULE_KINDS,
    )?;
    // Funded for one allocation, so the wedged one is genuinely in the second
    // federation's way rather than merely untidy.
    fund_gateway_wallet_amount(
        &stack.wallet.http,
        &stack.wallet.admin_url,
        &stack.wallet.bitcoin,
        ONE_ALLOCATION_BTC,
        GATEWAY_AMOUNT + GATEWAY_FEE_RESERVE,
    )
    .await?;
    let (_endpoint_addr, advertisement, rpc) = stack.wallet.configure_publish_and_connect().await?;
    let http = stack.wallet.http.clone();
    let admin_url = stack.wallet.admin_url.clone();

    let wedged_request = sign_public_rpc(
        PublicRpcPayloadDomain::RequestLiquidityRequest,
        live_liquidity_request(
            &advertisement.payload.provider_pubkey,
            &stack.target_fedimint.invite_code,
            &stack.endorsement,
            &stack.trust_material,
        )?,
    )?;
    let next_request = sign_public_rpc(
        PublicRpcPayloadDomain::RequestLiquidityRequest,
        live_liquidity_request(
            &advertisement.payload.provider_pubkey,
            &second_fedimint.invite_code,
            &second.endorsement,
            &second.trust_material,
        )?,
    )?;
    let wedged_federation_id = wedged_request
        .payload
        .federation_details
        .federation_id
        .0
        .clone();
    let next_federation_id = next_request
        .payload
        .federation_details
        .federation_id
        .0
        .clone();

    // The first federation goes away, with its invite code and its trust
    // material still perfectly valid.
    stack.target_fedimint.stop()?;

    let admitted = rpc
        .client
        .request_liquidity(wedged_request.clone())
        .await
        .context("request_liquidity for the departed federation")?;
    match &admitted.payload.outcome {
        RequestLiquidityOutcome::Accepted(status) => assert_eq!(status.item_statuses.len(), 1),
        RequestLiquidityOutcome::Rejected(rejection) => anyhow::bail!(
            "admission reads the request, not the federation, so this must still be accepted: \
             {rejection:?}"
        ),
    }

    // The money is now committed to work that cannot proceed.
    let held = admin_post(&http, &admin_url, "get_funds", &json!({})).await?;
    ensure!(
        held["balance"]["in_flight_allocations"]
            .as_u64()
            .is_some_and(|in_flight| in_flight >= GATEWAY_AMOUNT + GATEWAY_FEE_RESERVE),
        "the wedged allocation must be holding its reservation: {held}"
    );
    let refused = rpc
        .client
        .request_liquidity(next_request.clone())
        .await
        .context("request_liquidity for the next federation while the first is wedged")?;
    match &refused.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => assert_eq!(
            rejection.code,
            PublicRejectionCode::InsufficientCapacity,
            "the next federation must be refused by the wedged reservation: {rejection:?}"
        ),
        RequestLiquidityOutcome::Accepted(_) => {
            anyhow::bail!("capacity held by the wedged allocation must not be offered again")
        }
    }

    // The operator ends the allocation nobody can finish.
    let cancelled = admin_post(
        &http,
        &admin_url,
        "cancel_allocation",
        &json!({
            "federation_id": wedged_federation_id,
            "reason": "the target federation is gone and this can never fund"
        }),
    )
    .await?;
    assert_eq!(
        cancelled["status"], "accepted",
        "an allocation that has submitted nothing must be cancellable: {cancelled}"
    );
    assert_eq!(
        cancelled["allocation_status"]["item_statuses"][0]["status"],
        "cancelled"
    );

    // The money is back, and this is the assertion the whole test exists for.
    let released = admin_post(&http, &admin_url, "get_funds", &json!({})).await?;
    assert_eq!(
        released["balance"]["in_flight_allocations"], 0,
        "cancellation must release the reservation: {released}"
    );
    ensure!(
        released["balance"]["available_balance"]
            .as_u64()
            .is_some_and(|available| available >= GATEWAY_AMOUNT + GATEWAY_FEE_RESERVE),
        "the released capacity must be enough for the waiting federation: {released}"
    );

    let now_admitted = rpc
        .client
        .request_liquidity(next_request.clone())
        .await
        .context("request_liquidity for the next federation after the cancellation")?;
    match &now_admitted.payload.outcome {
        RequestLiquidityOutcome::Accepted(status) => assert_eq!(status.item_statuses.len(), 1),
        RequestLiquidityOutcome::Rejected(rejection) => {
            anyhow::bail!("released capacity must admit the waiting federation: {rejection:?}")
        }
    }
    let completed = wait_for_public_completion(
        &rpc,
        &next_request.payload,
        &stack.wallet.bitcoin,
        Duration::from_secs(900),
    )
    .await?;
    assert!(
        completed
            .payload
            .status
            .item_statuses
            .iter()
            .all(|item| item.status == ItemAllocationStatus::Completed)
    );

    // The cancelled allocation stays cancelled: releasing its money is not a
    // way of quietly retrying it.
    let wedged = get_admin_allocation(&http, &admin_url, &wedged_federation_id).await?;
    assert_eq!(
        wedged["allocation"]["status"]["item_statuses"][0]["status"],
        "cancelled"
    );
    let funded = get_admin_allocation(&http, &admin_url, &next_federation_id).await?;
    assert_eq!(
        funded["allocation"]["status"]["item_statuses"][0]["status"],
        "completed"
    );

    stack.wallet.stop_daemon()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_trust_reject_matrix_maps_each_failure_to_its_code() -> anyhow::Result<()> {
    // The reject matrix runs against one stack. Rejections persist nothing and
    // are re-evaluated from scratch, so the cases do not interfere and none of
    // them leaves an allocation behind for the next one to collide with. No
    // case is allowed to be accepted, so the stack never needs funding.
    init_logging();

    let test_id = unique_test_id("live_reject_matrix");
    let data_dir = TestDataDir::new("integration-live-reject-matrix")?;
    let _resources = DefeResources::live_liquidity().await?;
    let relay_url = _resources.relay().url.clone();
    let bitcoin = BitcoinFixture::new(_resources.bitcoind().clone()).context("bitcoin fixture")?;
    let gateway = GatewayFixture::start(&test_id, &bitcoin, data_dir.path())
        .await
        .context("gateway fixture")?;
    let target_fedimint = FedimintFixture::start(TARGET_FEDIMINT_LABEL, &bitcoin, data_dir.path())
        .await
        .context("target fedimint fixture")?;

    let ports = TestPorts::allocate()?;
    let mut launch = DaemonLaunch::new(data_dir.path())?;
    launch.holder_authorization_relay_url = Some(relay_url.clone());
    let trust = trust::live_trust(&launch, &relay_url)?;
    let artifacts = trust::write_trust_fixtures(
        &trust,
        &launch.trust_fixtures_dir,
        &target_fedimint.invite_code,
        &live_config_hash(),
        GATEWAY_TARGET_MODULE_KINDS,
    )?;
    // Guardian 1's advertisement is deliberately withheld for the first case
    // and published later, so the same stack covers both sides of it.

    let admin_url = format!("http://{}", ports.admin_bind_address);
    let http = Client::new();
    let mut daemon = DaemonProcess::start(data_dir.path(), &ports, &launch)?;
    wait_for_health(&http, &admin_url, &mut daemon).await?;
    trust::install_issuer_authority(&http, &admin_url, &trust).await?;
    let (_endpoint_addr, advertisement, rpc) = configure_publish_and_connect(
        &http,
        &admin_url,
        &gateway,
        &bitcoin,
        &relay_url,
        data_dir.path(),
        &mut daemon,
        &trust,
        None,
    )
    .await?;
    let provider_pubkey = advertisement.payload.provider_pubkey.clone();

    let reject = async |request: RequestLiquidityRequest| -> anyhow::Result<PublicRejectionCode> {
        let signed = sign_public_rpc(PublicRpcPayloadDomain::RequestLiquidityRequest, request)?;
        let response = rpc
            .client
            .request_liquidity(signed)
            .await
            .context("request_liquidity over real Iroh")?;
        match &response.payload.outcome {
            RequestLiquidityOutcome::Rejected(rejection) => Ok(rejection.code),
            RequestLiquidityOutcome::Accepted(_) => {
                anyhow::bail!("request was accepted but the matrix expects a rejection")
            }
        }
    };
    let request = || {
        live_liquidity_request(
            &provider_pubkey,
            &target_fedimint.invite_code,
            &artifacts.endorsement,
            &artifacts.trust_material,
        )
    };

    // A request with no endorsement never reaches the preview.
    let mut ungated = request()?;
    ungated.fman_endorsement = None;
    assert_eq!(
        reject(ungated).await?,
        PublicRejectionCode::InvalidCredentials,
        "missing endorsement"
    );

    // A request carrying no trust material at all is a rejection, never a
    // bypass: it deserializes so it can be answered with a signed rejection.
    let mut unmaterialized = request()?;
    unmaterialized.fman_trust_material = None;
    assert_eq!(
        reject(unmaterialized).await?,
        PublicRejectionCode::InvalidCredentials,
        "no trust material"
    );

    // One guardian's material is withheld, so its identity is unanswered and
    // therefore untrusted, and `all_trusted` cannot be satisfied. Nothing is
    // malformed, so this is a policy outcome rather than a credential failure.
    let mut withheld = request()?;
    withheld.fman_trust_material = Some(vec![artifacts.trust_material[0].clone()]);
    assert_eq!(
        reject(withheld).await?,
        PublicRejectionCode::PolicyMismatch,
        "withheld trust material"
    );

    // Material whose signature has been tampered with. The requester carries
    // it, so a forged entry must fail rather than be ignored.
    let mut forged = request()?;
    let mut forged_material = artifacts.trust_material.clone();
    forged_material[1].material.issued_at = fedi_decentralized_service_liquidity_manager::Timestamp(
        forged_material[1].material.issued_at.0 + 1,
    );
    forged.fman_trust_material = Some(forged_material);
    assert_eq!(
        reject(forged).await?,
        PublicRejectionCode::InvalidCredentials,
        "trust material signature does not cover the payload"
    );

    // Two entries for one identity: resolving that by position would let the
    // ordering of a requester-supplied list decide a trust outcome.
    let mut duplicated = request()?;
    let mut duplicated_material = artifacts.trust_material.clone();
    duplicated_material.push(artifacts.trust_material[0].clone());
    duplicated.fman_trust_material = Some(duplicated_material);
    assert_eq!(
        reject(duplicated).await?,
        PublicRejectionCode::InvalidCredentials,
        "duplicate trust material for one identity"
    );

    // With complete, coherent material the same request gets past trust
    // resolution, which is what makes the remaining cases meaningful.

    // A previewed guardian identity that the (correctly signed) seat binding
    // does not name. Only the preview changes, so the identities whose ads are
    // on the relay stay the same.
    let mut mismatched = artifacts.preview.clone();
    mismatched.peers[0].guardian_identity =
        fedi_decentralized_service_liquidity_manager::GuardianIdentity(
            "guardian-impostor".to_owned(),
        );
    trust::rewrite_preview_fixture(
        &launch.trust_fixtures_dir,
        &target_fedimint.invite_code,
        &mismatched,
    )?;
    assert_eq!(
        reject(request()?).await?,
        PublicRejectionCode::InvalidSeatBinding,
        "seat binding does not match the preview"
    );

    // Restore the coherent preview, then revoke one guardian's badge on the
    // real relay: the fresh request-time lookup must find it.
    trust::rewrite_preview_fixture(
        &launch.trust_fixtures_dir,
        &target_fedimint.invite_code,
        &artifacts.preview,
    )?;
    trust::publish_revocation(&relay_url, &trust, &artifacts.fman_credentials[1]).await?;
    assert_eq!(
        reject(request()?).await?,
        PublicRejectionCode::InvalidCredentials,
        "revoked badge"
    );

    // Nothing above was accepted, so nothing was persisted.
    let allocations = admin_post(
        &http,
        &admin_url,
        "list_allocations",
        &json!({ "page": { "cursor": null, "limit": 10 }, "time_range": null }),
    )
    .await?;
    assert_eq!(
        allocations["allocations"]["items"].as_array().map(Vec::len),
        Some(0),
        "a rejected request must persist no allocation"
    );

    daemon.stop()?;
    Ok(())
}

/// A dependency outage takes the provider off the market, and only the operator
/// puts it back.
///
/// Three transitions, in the order an operator meets them. Validation fails, so
/// the advertisement leaves the relay and requests are refused with
/// `provider_unavailable`. The dependency returns, so requests are admitted
/// again — but the advertisement is not, because
/// [`SPEC-flip-advertisement`](../specs/SPEC-flip-advertisement.md) makes a
/// withdrawal durable and lets no automatic pass undo one. The operator's
/// explicit republish is what returns it.
///
/// The middle transition is the one worth having a live test for: an FI holding
/// an advertisement can still transact against a provider that no relay is
/// offering, and an operator reading only the relay would conclude the opposite.
#[tokio::test(flavor = "multi_thread")]
async fn live_dependency_outage_withdraws_the_advertisement_until_the_operator_republishes()
-> anyhow::Result<()> {
    init_logging();

    let mut stack = LiveLiquidityStack::start("live-dependency-outage").await?;
    stack.wallet.fund_gateway_wallet().await?;
    let (endpoint_addr, advertisement, rpc) = stack.wallet.configure_publish_and_connect().await?;
    let http = stack.wallet.http.clone();
    let admin_url = stack.wallet.admin_url.clone();
    let relay_url = stack.wallet.relay_url.clone();
    let provider_pubkey_hex = advertisement.payload.provider_pubkey.0.clone();

    let signed_request = sign_public_rpc(
        PublicRpcPayloadDomain::RequestLiquidityRequest,
        live_liquidity_request(
            &advertisement.payload.provider_pubkey,
            &stack.target_fedimint.invite_code,
            &stack.endorsement,
            &stack.trust_material,
        )?,
    )?;

    // The outage: the same configuration, with the chain observer moved to a
    // port nothing listens on. Port 1 refuses immediately, so this is a
    // dependency that is definitely gone rather than one that is slow.
    let mut broken = live_setup_config(
        &stack.wallet.gateway,
        &stack.wallet.bitcoin,
        &relay_url,
        &endpoint_addr.id.to_string(),
        &stack.wallet.trust.attester_pubkey_hex,
        None,
    );
    broken["config"]["chain_observer"] = json!({
        "backend": {
            "type": "bitcoind",
            "url": "http://127.0.0.1:1",
            "username": stack.wallet.bitcoin.rpc_username()
        }
    });
    let broken_setup = admin_post(&http, &admin_url, "apply_setup_config", &broken).await?;
    ensure!(
        broken_setup["status"] != "ready",
        "an unreachable chain observer must not validate as ready: {broken_setup}"
    );
    ensure!(
        broken_setup["validation"]["checks"]
            .as_array()
            .is_some_and(|checks| checks.iter().any(|check| {
                check["name"] == "chain_observer_reachable" && check["status"] == "failed"
            })),
        "setup validation did not name the unreachable chain observer: {broken_setup}"
    );

    // Applying config reconciles the advertisement, so the withdrawal has
    // already run by the time the verb answers; the poll is for the relay's
    // asynchronous indexing of it, not for a worker tick.
    wait_for_withdrawn_advertisement(&relay_url, &provider_pubkey_hex).await?;
    let withdrawn_state =
        admin_post(&http, &admin_url, "get_advertisement_state", &json!({})).await?;
    assert_eq!(withdrawn_state["ready"], false);
    assert_eq!(withdrawn_state["publication_status"], "not_ready");
    ensure!(
        withdrawn_state["withdrawn_at"].is_number(),
        "a withdrawal must be recorded where the operator can read it: {withdrawn_state}"
    );

    let refused = rpc
        .client
        .request_liquidity(signed_request.clone())
        .await
        .context("request_liquidity over real Iroh during the outage")?;
    match &refused.payload.outcome {
        RequestLiquidityOutcome::Rejected(rejection) => assert_eq!(
            rejection.code,
            PublicRejectionCode::ProviderUnavailable,
            "a deployment whose dependency is gone must refuse as unavailable: {rejection:?}"
        ),
        RequestLiquidityOutcome::Accepted(_) => {
            anyhow::bail!("a provider that failed its own validation must not accept a request")
        }
    }

    // The dependency comes back.
    stack.wallet.apply_setup(&endpoint_addr).await?;

    // Readiness recovers, and the standing withdrawal outlives it. This is the
    // durable-withdrawal rule, seen from the outside: the deployment is fit to
    // advertise and is still not advertised.
    let repaired_state =
        admin_post(&http, &admin_url, "get_advertisement_state", &json!({})).await?;
    assert_eq!(repaired_state["ready"], true);
    ensure!(
        repaired_state["withdrawn_at"].is_number(),
        "repairing the dependency must not clear the standing withdrawal: {repaired_state}"
    );
    ensure!(
        find_advertisement_event(
            &relay_url,
            &provider_pubkey_hex,
            FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
            FLIP_PROVIDER_ADVERTISEMENT_D_TAG,
            FLIP_PROVIDER_ADVERTISEMENT_HASHTAG,
        )
        .await?
        .is_none(),
        "a repaired deployment must not re-advertise itself"
    );

    // The RPC surface does not wait for the advertisement. An FI that already
    // holds one transacts again the moment validation passes.
    let accepted = rpc
        .client
        .request_liquidity(signed_request.clone())
        .await
        .context("request_liquidity over real Iroh after the repair")?;
    match &accepted.payload.outcome {
        RequestLiquidityOutcome::Accepted(status) => assert_eq!(status.item_statuses.len(), 1),
        RequestLiquidityOutcome::Rejected(rejection) => {
            anyhow::bail!("the repaired provider rejected the request: {rejection:?}")
        }
    }

    // The operator says otherwise, which is the only thing that can.
    let republished =
        publish_and_assert_advertisement(&http, &admin_url, &relay_url, &endpoint_addr).await?;
    assert_eq!(
        republished.payload.provider_pubkey.0, provider_pubkey_hex,
        "a republish must not change the advertised provider identity"
    );
    let published_state =
        admin_post(&http, &admin_url, "get_advertisement_state", &json!({})).await?;
    assert_eq!(published_state["publication_status"], "published");
    assert_eq!(
        published_state["withdrawn_at"],
        Value::Null,
        "publishing must clear the standing withdrawal: {published_state}"
    );

    stack.wallet.stop_daemon()?;
    Ok(())
}

/// Polls until the relay serves no advertisement for this provider.
///
/// A withdrawal supersedes the event and then asks the relay to delete it, and
/// the relay indexes both asynchronously, so the absence the caller wants to
/// assert arrives shortly after the Admin API call that caused it returns.
async fn wait_for_withdrawn_advertisement(
    relay_url: &str,
    provider_pubkey_hex: &str,
) -> anyhow::Result<()> {
    for _ in 0..50 {
        let served = find_advertisement_event(
            relay_url,
            provider_pubkey_hex,
            FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
            FLIP_PROVIDER_ADVERTISEMENT_D_TAG,
            FLIP_PROVIDER_ADVERTISEMENT_HASHTAG,
        )
        .await?;
        if served.is_none() {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("the relay kept serving the advertisement after the provider withdrew it")
}

/// A send nobody can resolve escalates itself, and only the operator ends it.
///
/// An `in_doubt` send refuses guarded retry and cancellation both, because
/// nobody has established what happened to the money. That is safe and, on its
/// own, permanent: an operation whose evidence never arrives would sit there
/// for good. FLIP escalates it to `manual_review_required` once the operator's
/// configured threshold has passed, which is what puts a person in a position
/// to conclude it.
///
/// The escalation had no live coverage. Unit tests drive the transition
/// directly, and the operator end-to-end test resolves reviews it seeded
/// already under review, so a running daemon deciding to escalate a real send
/// was tested nowhere.
#[tokio::test(flavor = "multi_thread")]
async fn live_unresolvable_send_escalates_to_review_and_waits_for_the_operator()
-> anyhow::Result<()> {
    init_logging();

    let mut stack = LiveLiquidityStack::start("live-manual-review").await?;
    stack.wallet.fund_gateway_wallet().await?;
    let (endpoint_addr, advertisement, rpc) = stack.wallet.configure_publish_and_connect().await?;
    let http = stack.wallet.http.clone();
    let admin_url = stack.wallet.admin_url.clone();

    // A review threshold this test can cross. The shipped default is
    // deliberately long, and the funding policy cannot be changed once
    // operations are active, so it has to go in before the request.
    let mut prompt_review = live_setup_config(
        &stack.wallet.gateway,
        &stack.wallet.bitcoin,
        &stack.wallet.relay_url,
        &endpoint_addr.id.to_string(),
        &stack.wallet.trust.attester_pubkey_hex,
        None,
    );
    prompt_review["config"]["funding_policy"]["in_doubt_review_after_secs"] = json!(1);
    let applied = admin_post(&http, &admin_url, "apply_setup_config", &prompt_review).await?;
    assert_eq!(
        applied["status"], "ready",
        "the review threshold must apply cleanly: {applied}"
    );

    let signed_request = sign_public_rpc(
        PublicRpcPayloadDomain::RequestLiquidityRequest,
        live_liquidity_request(
            &advertisement.payload.provider_pubkey,
            &stack.target_fedimint.invite_code,
            &stack.endorsement,
            &stack.trust_material,
        )?,
    )?;
    let accepted = rpc
        .client
        .request_liquidity(signed_request.clone())
        .await
        .context("request_liquidity over real Iroh")?;
    match &accepted.payload.outcome {
        RequestLiquidityOutcome::Accepted(status) => assert_eq!(status.item_statuses.len(), 1),
        RequestLiquidityOutcome::Rejected(rejection) => {
            anyhow::bail!("live liquidity request rejected: {rejection:?}")
        }
    }

    // Caught before it settles. Nothing mines from here on, so the send stays
    // short of the confirmation depth the funding policy requires.
    let federation_id = wait_for_any_admin_allocation(&http, &admin_url).await?;
    let operation = wait_for_gateway_funding_operation(
        &http,
        &admin_url,
        &federation_id,
        &["broadcast", "confirmed"],
        true,
    )
    .await?;
    let operation_id = operation["operation_id"]
        .as_str()
        .context("gateway funding operation id")?
        .to_owned();

    // The send becomes one nobody can resolve: back in doubt, naming no
    // transaction, and pointing at an address the chain will never show a
    // payment to. That is the shape of a lost gatewayd response, which is the
    // situation reviewed operations really arise from, and it cannot be
    // produced from outside a gatewayd that is answering correctly.
    stack.wallet.stop_daemon()?;
    let unpaid_address = stack.wallet.bitcoin.new_address()?;
    rewind_operation_to_unresolvable_in_doubt(
        stack.wallet.data_dir.path(),
        &operation_id,
        &unpaid_address,
    )
    .await?;
    stack.wallet.start_daemon().await?;

    let reviewed = wait_for_wallet_operation_status(
        &http,
        &admin_url,
        &operation_id,
        "manual_review_required",
    )
    .await?;
    assert_eq!(
        reviewed["operation"]["failure"]["code"],
        "manual_review_required"
    );
    ensure!(
        reviewed["operation"]["failure"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("review threshold")),
        "an escalation must say why it gave up waiting: {reviewed}"
    );

    // Under review both escapes stay shut, which is the whole reason escalation
    // has to exist: an operator who could simply retry would not need it.
    //
    // The guarded retry answers that nothing matched rather than refusing. It
    // acts only on an action-required *safe* funding step, and a send under
    // review is not one, so from that verb's side there is no outstanding step
    // to repeat. What matters here is what it did not do: it did not resubmit.
    let retry = admin_post(
        &http,
        &admin_url,
        "retry_funding_step",
        &json!({
            "federation_id": federation_id,
            "item_id": null,
            "operation_id": operation_id
        }),
    )
    .await?;
    assert_eq!(
        retry["status"], "already_applied",
        "a reviewed send must not be retried by the guarded verb: {retry}"
    );
    ensure!(
        retry["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("no action-required safe funding step")),
        "the retry refusal must say no step matched: {retry}"
    );
    let unchanged = admin_post(
        &http,
        &admin_url,
        "get_wallet_operation",
        &json!({ "operation_id": operation_id }),
    )
    .await?;
    assert_eq!(
        unchanged["operation"]["status"], "manual_review_required",
        "the retry must leave the reviewed send exactly where it was: {unchanged}"
    );
    let cancel = admin_post(
        &http,
        &admin_url,
        "cancel_allocation",
        &json!({
            "federation_id": federation_id,
            "reason": "the operator would rather walk away"
        }),
    )
    .await?;
    assert_eq!(
        cancel["status"], "rejected",
        "an allocation holding a reviewed send must not be cancelled: {cancel}"
    );

    // Until the operator concludes it. `failed` is the conclusion that no
    // payment happened, and it is theirs to make: FLIP has no evidence either
    // way, which is why the operation reached them at all.
    let resolved = admin_post(
        &http,
        &admin_url,
        "resolve_manual_review",
        &json!({
            "operation_id": operation_id,
            "resolution": "failed",
            "txid": null,
            "reason": "external reconciliation found no payment"
        }),
    )
    .await?;
    assert_eq!(resolved["status"], "accepted");
    assert_eq!(resolved["operation"]["status"], "failed");

    // Read back rather than trusted: the response is a report, the row is the
    // outcome.
    let durable = admin_post(
        &http,
        &admin_url,
        "get_wallet_operation",
        &json!({ "operation_id": operation_id }),
    )
    .await?;
    assert_eq!(durable["operation"]["status"], "failed");
    // The conclusion was a person's, not evidence, so the record of who said
    // what has to outlive the response that reported it.
    let audited: i64 = {
        let database = Database::connect(stack.wallet.data_dir.path().join("flip.sqlite")).await?;
        let count = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log \
             WHERE action = 'resolve_manual_review' \
               AND detail_json LIKE '%\"outcome\":\"accepted\"%' \
               AND detail_json LIKE '%external reconciliation found no payment%'",
        )
        .fetch_one(database.pool())
        .await?;
        database.pool().close().await;
        count
    };
    assert_eq!(
        audited, 1,
        "the operator's conclusion must leave exactly one audit row carrying their reason"
    );

    stack.wallet.stop_daemon()?;
    Ok(())
}

/// Rewrites one wallet operation into a send whose outcome nobody can
/// establish: back in doubt, naming no transaction, and addressed to an output
/// the chain will never carry.
///
/// Written straight into the daemon's SQLite while it is stopped, the way this
/// suite's stability-item rewind is, because a gatewayd that answers correctly
/// cannot be made to lose its own response from outside.
async fn rewind_operation_to_unresolvable_in_doubt(
    data_dir: &Path,
    operation_id: &str,
    unpaid_address: &str,
) -> anyhow::Result<()> {
    let database = Database::connect(data_dir.join("flip.sqlite")).await?;
    let submitted_at = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock before UNIX epoch")?
            .as_secs(),
    )
    .context("submission timestamp does not fit")?
        - 3_600;
    let affected = sqlx::query(
        "UPDATE wallet_operations SET status = 'in_doubt', txid = NULL, tx_vout = NULL, \
         confirmation_count = NULL, completed_at = NULL, settled_tick = NULL, \
         sync_after = NULL, address = ?, submitted_at = ? WHERE operation_id = ?",
    )
    .bind(unpaid_address)
    .bind(submitted_at)
    .bind(operation_id)
    .execute(database.pool())
    .await?
    .rows_affected();
    database.pool().close().await;
    ensure!(
        affected == 1,
        "expected to rewind exactly one wallet operation, rewound {affected}"
    );
    Ok(())
}

/// Polls the Admin API until one wallet operation reports `status`.
async fn wait_for_wallet_operation_status(
    http: &Client,
    admin_url: &str,
    operation_id: &str,
    status: &str,
) -> anyhow::Result<Value> {
    let mut last = Value::Null;
    for _ in 0..300 {
        last = admin_post(
            http,
            admin_url,
            "get_wallet_operation",
            &json!({ "operation_id": operation_id }),
        )
        .await?;
        if last["operation"]["status"] == status {
            return Ok(last);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("wallet operation {operation_id} never reached {status}: {last}")
}

#[tokio::test(flavor = "multi_thread")]
async fn live_deposit_monitoring_restart_claims_funded_top_up() -> anyhow::Result<()> {
    init_logging();

    let mut stack = LiveWalletStack::start("live-deposit-monitoring-restart").await?;
    let (address, operation_id) = stack
        .create_deposit_address("deposit-monitoring-restart")
        .await?;
    let initial = load_wallet_operation_from_db(stack.data_dir.path(), &operation_id).await?;
    assert_eq!(initial.operation_id, operation_id);
    assert_eq!(initial.operation_type, "deposit");
    assert_eq!(initial.status, "pending");
    assert_eq!(initial.address.as_deref(), Some(address.as_str()));

    stack.stop_daemon()?;
    stack.bitcoin.send_to_address(&address, TOP_UP_BTC).await?;
    mine_and_sync(&stack.bitcoin, FEDIMINT_FINALITY_BLOCKS).await?;

    let endpoint_addr = stack.start_daemon().await?;
    stack.apply_setup(&endpoint_addr).await?;
    let completed = wait_for_db_wallet_operation_completed(
        stack.data_dir.path(),
        &operation_id,
        FEDIMINT_FINALITY_BLOCKS,
    )
    .await?;
    assert_eq!(completed.operation_id, operation_id);
    assert_eq!(completed.operation_type, "deposit");
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.address.as_deref(), Some(address.as_str()));
    ensure!(
        completed.txid.is_some(),
        "completed deposit should include observed txid: {completed:?}"
    );

    wait_for_spendable_funds(
        &stack.http,
        &stack.admin_url,
        &stack.bitcoin,
        GATEWAY_AMOUNT,
    )
    .await?;
    let health = admin_post(&stack.http, &stack.admin_url, "get_health", &json!({})).await?;
    assert_health_component(&health, "wallet")?;
    assert_health_component(&health, "gateway")?;
    assert_health_component(&health, "chain_observer")?;

    stack.stop_daemon()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_operator_withdrawal_intent_replays_once_and_rejects_conflicts() -> anyhow::Result<()>
{
    init_logging();

    let mut stack = LiveWalletStack::start("live-operator-withdrawal-intent").await?;
    stack.fund_gateway_wallet().await?;
    let address = stack.bitcoin.new_address()?;
    let request = json!({
        "withdrawal_intent_id": "e2e-withdrawal-intent",
        "address": address,
        "amount": 10_000,
        "fee_rate_sat_per_vbyte": 1
    });

    let (first, second) = tokio::join!(
        admin_post(
            &stack.http,
            &stack.admin_url,
            "request_withdrawal",
            &request
        ),
        admin_post(
            &stack.http,
            &stack.admin_url,
            "request_withdrawal",
            &request
        ),
    );
    let first = first?;
    let second = second?;
    assert_eq!(
        second["operation"]["operation_id"], first["operation"]["operation_id"],
        "concurrent identical intents must return one durable operation"
    );

    let operation_id = first["operation"]["operation_id"]
        .as_str()
        .context("withdrawal response operation id")?
        .to_owned();
    let database = Database::connect(stack.data_dir.path().join("flip.sqlite")).await?;
    let txid = wait_for_operator_withdrawal_broadcast(&database, &operation_id).await?;

    let replay = admin_post(
        &stack.http,
        &stack.admin_url,
        "request_withdrawal",
        &request,
    )
    .await?;
    assert_eq!(replay["operation"]["operation_id"], operation_id);
    assert_eq!(replay["operation"]["txid"], txid);

    let conflict = stack
        .http
        .post(format!("{}/admin/v1/request_withdrawal", stack.admin_url))
        .bearer_auth(common::live_liquidity::daemon::ADMIN_TOKEN)
        .json(&json!({
            "withdrawal_intent_id": "e2e-withdrawal-intent",
            "address": address,
            "amount": 10_001,
            "fee_rate_sat_per_vbyte": 1
        }))
        .send()
        .await?;
    assert_eq!(conflict.status(), StatusCode::PRECONDITION_FAILED);
    let error: Value = conflict.json().await?;
    assert_eq!(error["code"], "failed_precondition");

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wallet_operations WHERE operation_id = ? AND withdrawal_intent_id = ?",
    )
    .bind(&operation_id)
    .bind("e2e-withdrawal-intent")
    .fetch_one(database.pool())
    .await?;
    assert_eq!(count, 1, "replay and conflict must not create another send");
    database.pool().close().await;

    stack.stop_daemon()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_gateway_withdrawal_restart_resumes_without_duplicate_funding() -> anyhow::Result<()> {
    init_logging();

    let mut stack = LiveLiquidityStack::start("live-gateway-withdrawal-restart").await?;
    stack.wallet.fund_gateway_wallet().await?;
    let signed_request = stack.request_gateway_liquidity().await?;
    let federation_id =
        wait_for_any_admin_allocation(&stack.wallet.http, &stack.wallet.admin_url).await?;
    let broadcast = wait_for_gateway_funding_operation(
        &stack.wallet.http,
        &stack.wallet.admin_url,
        &federation_id,
        &["broadcast"],
        true,
    )
    .await?;
    let operation_id = required_str(&broadcast, "operation_id")?.to_owned();
    let txid = required_str(&broadcast, "txid")?.to_owned();
    let cancellation = admin_post(
        &stack.wallet.http,
        &stack.wallet.admin_url,
        "cancel_allocation",
        &json!({
            "federation_id": federation_id,
            "reason": "broadcast funding must be fenced from cancellation"
        }),
    )
    .await?;
    assert_eq!(cancellation["status"], "rejected");
    ensure!(
        cancellation["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains(&operation_id)
                && detail.contains("broadcast")
                && detail.contains("cannot be cancelled")),
        "broadcast cancellation rejection did not identify the fenced operation: {cancellation}"
    );

    stack.wallet.stop_daemon()?;
    mine_and_sync(&stack.wallet.bitcoin, FEDIMINT_FINALITY_BLOCKS).await?;

    stack.wallet.start_daemon().await?;
    let (_endpoint_addr, _advertisement, rpc) =
        stack.wallet.configure_publish_and_connect().await?;
    let final_status = wait_for_public_completion(
        &rpc,
        &signed_request.payload,
        &stack.wallet.bitcoin,
        Duration::from_secs(900),
    )
    .await?;
    assert!(
        final_status
            .payload
            .status
            .item_statuses
            .iter()
            .all(|item| item.status == ItemAllocationStatus::Completed)
    );

    let allocation =
        get_admin_allocation(&stack.wallet.http, &stack.wallet.admin_url, &federation_id).await?;
    assert_single_completed_gateway_operation(&allocation, &operation_id, &txid)?;
    assert_gateway_completion_evidence(&allocation, &operation_id, &txid)?;

    stack.wallet.stop_daemon()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_target_peg_in_restart_completes_after_wallet_operation_finality() -> anyhow::Result<()>
{
    init_logging();

    let mut stack = LiveLiquidityStack::start("live-target-peg-in-restart").await?;
    stack.wallet.fund_gateway_wallet().await?;
    let signed_request = stack.request_gateway_liquidity().await?;
    let federation_id =
        wait_for_any_admin_allocation(&stack.wallet.http, &stack.wallet.admin_url).await?;
    let broadcast = wait_for_gateway_funding_operation(
        &stack.wallet.http,
        &stack.wallet.admin_url,
        &federation_id,
        &["broadcast"],
        true,
    )
    .await?;
    let operation_id = required_str(&broadcast, "operation_id")?.to_owned();
    let txid = required_str(&broadcast, "txid")?.to_owned();
    let item_id = required_str(&broadcast, "item_id")?.to_owned();

    stack.wallet.stop_daemon()?;
    mine_and_sync(&stack.wallet.bitcoin, FEDIMINT_FINALITY_BLOCKS).await?;
    mark_wallet_operation_completed_for_restart(
        stack.wallet.data_dir.path(),
        &operation_id,
        &item_id,
        FEDIMINT_FINALITY_BLOCKS,
    )
    .await?;

    stack.wallet.start_daemon().await?;
    let (_endpoint_addr, _advertisement, rpc) =
        stack.wallet.configure_publish_and_connect().await?;
    let final_status = wait_for_public_completion(
        &rpc,
        &signed_request.payload,
        &stack.wallet.bitcoin,
        Duration::from_secs(900),
    )
    .await?;
    assert!(
        final_status
            .payload
            .status
            .item_statuses
            .iter()
            .all(|item| item.status == ItemAllocationStatus::Completed)
    );

    let allocation =
        get_admin_allocation(&stack.wallet.http, &stack.wallet.admin_url, &federation_id).await?;
    assert_single_completed_gateway_operation(&allocation, &operation_id, &txid)?;
    assert_gateway_completion_evidence(&allocation, &operation_id, &txid)?;

    stack.wallet.stop_daemon()?;
    Ok(())
}

/// Live stability-pool provisioning end to end: a real stability-pool-v2
/// federation, funded by FLIP through peg-in -> `deposit_to_provide`, observed
/// as provided provider liquidity. This exercises the current-state observer
/// (balance-delta peg-in claim + provider-report completion gate) against a
/// real Fedimint stability pool — the path that had no live coverage before.
/// The stability-pool stack: every fixture the two stability tests need, plus a
/// daemon already configured, advertised, and connected.
///
/// Standing this up costs a real gatewayd, a stability-pool-v2 federation, an
/// electrs and a relay, so the two tests share one builder rather than each
/// carrying its own copy of ninety lines of setup.
struct LiveStabilityStack {
    _resources: DefeResources,
    _gateway: GatewayFixture,
    _target_fedimint: FedimintFixture,
    /// Kept alive rather than read: dropping it stops the electrs the target
    /// client watches the chain through.
    _esplora: EsploraFixture,
    bitcoin: BitcoinFixture,
    data_dir: TestDataDir,
    ports: TestPorts,
    launch: DaemonLaunch,
    artifacts: trust::TrustFixtureArtifacts,
    /// The target federation's own config hash, shared by the preview fixture
    /// and every request built against it. Both must carry the same value the
    /// real client reports, or acceptance refuses the request before the
    /// stability worker ever sees it.
    target_config_hash: HashBytes,
    http: Client,
    admin_url: String,
    daemon: DaemonProcess,
    advertisement: Signed<LiquidityProviderAdvertisement>,
    rpc: PublicRpcHarness,
}

impl LiveStabilityStack {
    async fn start(name: &str) -> anyhow::Result<Self> {
        let test_id = unique_test_id(name);
        let data_dir = TestDataDir::new(name)?;
        let resources = DefeResources::live_liquidity().await?;
        let relay_url = resources.relay().url.clone();
        let bitcoin =
            BitcoinFixture::new(resources.bitcoind().clone()).context("bitcoin fixture")?;
        let gateway = GatewayFixture::start(&test_id, &bitcoin, data_dir.path())
            .await
            .context("gateway fixture")?;
        let target_fedimint = FedimintFixture::start_with_stability_pool(
            TARGET_FEDIMINT_LABEL,
            &bitcoin,
            data_dir.path(),
        )
        .await
        .context("stability-pool target fedimint fixture")?;
        // The target-federation wallet client watches the chain only through an
        // esplora; keep it alive for the whole test so the peg-in can be
        // claimed. These tests reach it the way production must — as the
        // daemon's own configured chain observer, handed to the target client —
        // rather than through `FM_FORCE_BITCOIN_RPC_*`, which would prove
        // nothing about the daemon since it overrides the backend from outside.
        let esplora = EsploraFixture::start(&bitcoin, data_dir.path())
            .await
            .context("esplora fixture")?;

        let ports = TestPorts::allocate()?;
        let mut launch = DaemonLaunch::new(data_dir.path())?;
        launch.holder_authorization_relay_url = Some(relay_url.clone());
        let trust = trust::live_trust(&launch, &relay_url)?;
        let target_config_hash = live_target_config_hash(&target_fedimint.invite_code).await?;
        let artifacts = trust::write_trust_fixtures(
            &trust,
            &launch.trust_fixtures_dir,
            &target_fedimint.invite_code,
            &target_config_hash,
            STABILITY_TARGET_MODULE_KINDS,
        )?;
        let admin_url = format!("http://{}", ports.admin_bind_address);
        let http = Client::new();
        let mut daemon = DaemonProcess::start(data_dir.path(), &ports, &launch)?;
        wait_for_health(&http, &admin_url, &mut daemon).await?;
        trust::install_issuer_authority(&http, &admin_url, &trust).await?;
        let endpoint_addr = wait_for_endpoint_addr(data_dir.path(), &mut daemon).await?;

        apply_live_setup(
            &http,
            &admin_url,
            &gateway,
            &bitcoin,
            &relay_url,
            &endpoint_addr,
            &trust.attester_pubkey_hex,
            Some(&esplora.http_url),
        )
        .await?;
        fund_gateway_wallet(&http, &admin_url, &bitcoin).await?;

        let (_endpoint_addr, advertisement, rpc) = configure_publish_and_connect(
            &http,
            &admin_url,
            &gateway,
            &bitcoin,
            &relay_url,
            data_dir.path(),
            &mut daemon,
            &trust,
            Some(&esplora.http_url),
        )
        .await?;

        Ok(Self {
            _resources: resources,
            _gateway: gateway,
            _target_fedimint: target_fedimint,
            _esplora: esplora,
            bitcoin,
            data_dir,
            ports,
            launch,
            artifacts,
            target_config_hash,
            http,
            admin_url,
            daemon,
            advertisement,
            rpc,
        })
    }

    /// Restarts the daemon and reconnects, without reconfiguring it.
    async fn restart(&mut self) -> anyhow::Result<()> {
        self.daemon = DaemonProcess::start(self.data_dir.path(), &self.ports, &self.launch)?;
        wait_for_health(&self.http, &self.admin_url, &mut self.daemon).await?;
        Ok(())
    }

    /// Requests stability liquidity and waits for the item to complete,
    /// returning the allocation's federation id.
    async fn drive_stability_allocation_to_completion(&mut self) -> anyhow::Result<String> {
        let liquidity_request = live_stability_request(
            &self.advertisement.payload.provider_pubkey,
            &self._target_fedimint.invite_code,
            &self.artifacts.endorsement,
            &self.artifacts.trust_material,
            &self.target_config_hash,
        )?;
        let signed_request = sign_public_rpc(
            PublicRpcPayloadDomain::RequestLiquidityRequest,
            liquidity_request,
        )?;
        let response = self
            .rpc
            .client
            .request_liquidity(signed_request.clone())
            .await
            .context("stability request_liquidity over real Iroh")?;
        let status = match &response.payload.outcome {
            RequestLiquidityOutcome::Accepted(status) => status.clone(),
            RequestLiquidityOutcome::Rejected(rejection) => {
                anyhow::bail!("live stability request rejected: {rejection:?}");
            }
        };
        // Stability-only request commits exactly one stability-pool item.
        assert_eq!(status.item_statuses.len(), 1);

        assert_live_verification_passed(
            &self.http,
            &self.admin_url,
            &signed_request.payload.federation_details.federation_id,
        )
        .await?;

        let federation_id = wait_for_any_admin_allocation(&self.http, &self.admin_url).await?;
        wait_for_active_wallet_operations(&self.http, &self.admin_url, &federation_id).await?;

        // Peg-in finality + the stability-pool provide cycle take longer than a
        // gateway send, so allow a generous budget while mining/syncing.
        let final_status = wait_for_public_completion(
            &self.rpc,
            &signed_request.payload,
            &self.bitcoin,
            Duration::from_secs(1200),
        )
        .await?;
        assert!(
            final_status
                .payload
                .status
                .item_statuses
                .iter()
                .all(|item| item.status == ItemAllocationStatus::Completed)
        );
        Ok(federation_id)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn live_stability_pool_happy_path_provides_into_pool() -> anyhow::Result<()> {
    init_logging();

    let mut stack = LiveStabilityStack::start("integration-live-stability").await?;
    let federation_id = stack.drive_stability_allocation_to_completion().await?;
    let (http, admin_url) = (stack.http.clone(), stack.admin_url.clone());
    let data_dir = &stack.data_dir;

    let allocation = get_admin_allocation(&http, &admin_url, &federation_id).await?;
    assert_stability_completion_evidence(&allocation)?;

    // The allocation above is what opens a target-federation client, so this is
    // the only place in the suite where eviction can be exercised against a
    // real one. It rides on this test rather than getting its own because a
    // separate one would have to stand up the whole stack and drive an
    // allocation again just to reach this state.
    assert_eviction_releases_the_client_lock(&http, &admin_url, data_dir.path(), &federation_id)
        .await?;

    stack.daemon.stop()?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn live_combined_request_concurrent_replay_creates_one_allocation() -> anyhow::Result<()> {
    init_logging();

    let mut stack = LiveStabilityStack::start("integration-live-combined-replay").await?;
    let request = live_request_with_amounts(
        &stack.advertisement.payload.provider_pubkey,
        &stack._target_fedimint.invite_code,
        &stack.artifacts.endorsement,
        &stack.artifacts.trust_material,
        &stack.target_config_hash,
        LiquidityAmountBounds {
            gateway_min_amount: Sats(GATEWAY_AMOUNT),
            gateway_max_amount: None,
            stability_min_amount: Sats(STABILITY_AMOUNT),
            stability_max_amount: None,
        },
    )?;
    let signed = sign_public_rpc(PublicRpcPayloadDomain::RequestLiquidityRequest, request)?;
    let (first, second) = tokio::join!(
        stack.rpc.client.request_liquidity(signed.clone()),
        stack.rpc.client.request_liquidity(signed.clone()),
    );
    let responses = [
        first?,
        second?,
        stack.rpc.client.request_liquidity(signed.clone()).await?,
    ];
    let mut accepted = Vec::new();
    for response in responses {
        match response.payload.outcome {
            RequestLiquidityOutcome::Accepted(status) => accepted.push(status),
            RequestLiquidityOutcome::Rejected(rejection) => {
                anyhow::bail!("combined-source exact replay rejected: {rejection:?}")
            }
        }
    }
    ensure!(
        accepted
            .iter()
            .all(|status| status.item_statuses.len() == 2)
    );
    ensure!(
        accepted[0]
            .item_statuses
            .iter()
            .any(|item| { matches!(item.target, AllocationItemTarget::Gateway { .. }) })
    );
    ensure!(
        accepted[0]
            .item_statuses
            .iter()
            .any(|item| { matches!(item.target, AllocationItemTarget::StabilityPool { .. }) })
    );

    let federation_id = &signed.payload.federation_details.federation_id.0;
    let allocations = admin_post(
        &stack.http,
        &stack.admin_url,
        "list_allocations",
        &json!({ "page": { "cursor": null, "limit": 10 }, "time_range": null }),
    )
    .await?;
    assert_eq!(
        allocations["allocations"]["items"].as_array().map(Vec::len),
        Some(1)
    );
    assert_live_verification_passed(
        &stack.http,
        &stack.admin_url,
        &signed.payload.federation_details.federation_id,
    )
    .await?;
    let allocation = get_admin_allocation(&stack.http, &stack.admin_url, federation_id).await?;
    assert_eq!(
        allocation["allocation"]["status"]["item_statuses"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    let database = Database::connect(stack.data_dir.path().join("flip.sqlite")).await?;
    let allocation_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM allocations WHERE federation_id = ?")
            .bind(federation_id)
            .fetch_one(database.pool())
            .await?;
    let source_counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT source_type, COUNT(*) FROM allocation_items WHERE federation_id = ? GROUP BY source_type ORDER BY source_type",
    )
    .bind(federation_id)
    .fetch_all(database.pool())
    .await?;
    assert_eq!(allocation_count, 1);
    assert_eq!(
        source_counts,
        vec![("gateway".to_owned(), 1), ("stability_pool".to_owned(), 1)]
    );
    database.pool().close().await;

    stack.daemon.stop()?;
    Ok(())
}

/// Restart recovery reuses the caller-owned operation ID already committed by
/// the real target client rather than creating another deposit.
///
/// The fixture preserves the complete persisted request and rewinds only local
/// observation state. Exact global operation-log lookup must find the receipt.
#[tokio::test(flavor = "multi_thread")]
async fn live_stability_pre_submit_restart_adopts_the_deposit_already_made() -> anyhow::Result<()> {
    init_logging();

    let mut stack = LiveStabilityStack::start("integration-live-stability-presubmit").await?;
    let federation_id = stack.drive_stability_allocation_to_completion().await?;

    // Rewind local observation while leaving the complete request and target receipt intact.
    stack.daemon.stop()?;
    let submitted_operation_id =
        rewind_stability_item_to_pre_submit(stack.data_dir.path(), &federation_id).await?;
    stack.restart().await?;

    // Assert exact-ID recovery before completion.
    let step = wait_for_recovered_deposit_operation(
        stack.data_dir.path(),
        &federation_id,
        Duration::from_secs(90),
    )
    .await?;
    // Recovery keeps the caller-owned ID; another submission would violate it.
    assert_eq!(
        step["sp_deposit_operation_id"].as_str(),
        Some(submitted_operation_id.as_str()),
        "FLIP should have recovered the existing deposit, not made another: {step}"
    );

    let item = wait_for_stability_item_status(
        &stack.http,
        &stack.admin_url,
        &federation_id,
        "completed",
        Duration::from_secs(300),
    )
    .await?;
    assert_eq!(
        item["completion_evidence"]["stability_pool"]["stability_pool_deposit_operation_id"]
            .as_str(),
        Some(submitted_operation_id.as_str()),
        "completion evidence should name the recovered deposit: {item}"
    );

    stack.daemon.stop()?;
    Ok(())
}

/// Proves the remediation route does what it claims: closing a target
/// federation's client releases its database lock, so the next use can reopen
/// it.
///
/// The route's own return value cannot show this — it reports that the map
/// entry was removed, and `evict` falls back to `ClientHandle`'s `Drop` when a
/// caller still holds a clone, which is not a synchronization point. So the
/// lock is probed directly: held while the client is open, free afterwards.
/// Without that, an operator can be told the client was closed while the lock
/// is still held, and their next reopen fails.
///
/// The probe opens the database with RocksDB rather than locking the `LOCK`
/// file, because RocksDB takes an fcntl record lock while `File::try_lock`
/// takes a BSD flock. On Linux those are independent lock spaces, so the
/// simpler probe succeeds even while the daemon holds the database.
async fn assert_eviction_releases_the_client_lock(
    http: &Client,
    admin_url: &str,
    data_dir: &Path,
    federation_id: &str,
) -> anyhow::Result<()> {
    let client_db = data_dir
        .join("federations")
        .join(federation_id)
        .join("client.db");
    ensure!(
        client_db.join("LOCK").is_file(),
        "expected an opened client database at {}",
        client_db.display()
    );
    ensure!(
        !client_db_is_openable(&client_db),
        "the client should still be open, holding {}",
        client_db.display()
    );

    let closed = admin_post(
        http,
        admin_url,
        "reopen_federation_client",
        &json!({ "federation_id": federation_id }),
    )
    .await?;
    ensure!(
        closed["closed"] == Value::Bool(true),
        "reopen_federation_client should report an open client was closed: {closed}"
    );

    // Shutdown is awaited inside the route when it holds the only reference,
    // which it does here — the allocation that opened the client is complete,
    // so no worker pass is mid-use.
    ensure!(
        client_db_is_openable(&client_db),
        "eviction reported success but {} is still locked",
        client_db.display()
    );
    Ok(())
}

/// Whether this process can take the client database, which it can only do
/// once the daemon has released it.
fn client_db_is_openable(client_db: &Path) -> bool {
    match fedimint_rocksdb::rocksdb::DB::open_default(client_db) {
        Ok(db) => {
            drop(db);
            true
        }
        Err(_) => false,
    }
}

fn assert_stability_completion_evidence(allocation: &Value) -> anyhow::Result<()> {
    let item = allocation["allocation"]["status"]["item_statuses"]
        .as_array()
        .and_then(|items| items.first())
        .context("allocation has item status")?;
    assert_eq!(item["status"], "completed");
    let evidence = item["completion_evidence"]["stability_pool"]
        .as_object()
        .context("stability-pool completion evidence exists")?;
    let fulfilled = evidence
        .get("fulfilled_amount")
        .and_then(Value::as_u64)
        .context("stability evidence fulfilled_amount")?;
    ensure!(
        fulfilled >= STABILITY_AMOUNT,
        "stability fulfilled below committed: {evidence:?}"
    );
    let observed = evidence
        .get("observed_provided_amount")
        .and_then(Value::as_u64)
        .context("stability evidence observed_provided_amount")?;
    ensure!(
        observed >= STABILITY_AMOUNT,
        "observed provided below committed: {evidence:?}"
    );
    ensure!(
        evidence
            .get("stability_pool_deposit_operation_id")
            .and_then(Value::as_str)
            .is_some(),
        "stability evidence carries a deposit operation id: {evidence:?}"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn apply_live_setup(
    http: &Client,
    admin_url: &str,
    gateway: &GatewayFixture,
    bitcoin: &BitcoinFixture,
    relay_url: &str,
    endpoint_addr: &EndpointAddr,
    attester_pubkey_hex: &str,
    esplora_url: Option<&str>,
) -> anyhow::Result<()> {
    let setup = live_setup_config(
        gateway,
        bitcoin,
        relay_url,
        &endpoint_addr.id.to_string(),
        attester_pubkey_hex,
        esplora_url,
    );
    // Secrets are written by name, so a config write carries none and
    // `apply_setup_config` requires the gateway credential to be stored first.
    // Both go in before the poll below, which retries the config write only.
    let mut secrets = vec![json!({
        "secret": "gateway_admin_credential",
        "update": { "action": "set", "value": gateway.password.clone() }
    })];
    if esplora_url.is_none() {
        secrets.push(json!({
            "secret": "chain_observer_password",
            "update": { "action": "set", "value": bitcoin.rpc_password() }
        }));
    }
    for secret in secrets {
        admin_post_when_available(http, admin_url, "set_config_secret", &secret).await?;
    }

    // Setup readiness includes the daemon's `gateway_wallet_api` check, which
    // requires gatewayd's lightning node to be *connected* — not just its admin
    // API up (all `GatewayFixture` waits for). Under a full parallel suite,
    // gatewayd's LDK node can take tens of seconds to connect, so poll generously
    // rather than the ~1s a serial run needs. Early-returns on ready, so the
    // happy path stays fast.
    //
    // Do not shorten this to gain suite time: it buys none. The loop returns the
    // instant setup reports ready, so a green run costs the same at any ceiling.
    // A 6s ceiling was measured against the parallel layout and failed all four
    // flip runners on `gateway_wallet_api: gatewayd lightning node is not
    // connected`.
    let mut last_response = None;
    for _ in 0..600 {
        let response =
            admin_post_when_available(http, admin_url, "apply_setup_config", &setup).await?;
        if response["status"] == "ready" {
            return Ok(());
        }
        last_response = Some(response);
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    anyhow::bail!(
        "live setup did not validate as ready: {}",
        last_response.unwrap_or_else(|| json!({ "error": "no setup response" }))
    )
}

#[allow(clippy::too_many_arguments)]
async fn configure_publish_and_connect(
    http: &Client,
    admin_url: &str,
    gateway: &GatewayFixture,
    bitcoin: &BitcoinFixture,
    relay_url: &str,
    data_dir: &Path,
    daemon: &mut DaemonProcess,
    trust: &trust::LiveTrust,
    esplora_url: Option<&str>,
) -> anyhow::Result<(
    EndpointAddr,
    Signed<LiquidityProviderAdvertisement>,
    PublicRpcHarness,
)> {
    let mut endpoint_addr = wait_for_endpoint_addr(data_dir, daemon).await?;
    let mut last_error = None;

    for _ in 0..6 {
        apply_live_setup(
            http,
            admin_url,
            gateway,
            bitcoin,
            relay_url,
            &endpoint_addr,
            &trust.attester_pubkey_hex,
            esplora_url,
        )
        .await?;
        trust::enrol_provider_authorization(&http, &admin_url, &trust, &relay_url).await?;

        let after_setup_addr = wait_for_endpoint_addr(data_dir, daemon).await?;
        if after_setup_addr != endpoint_addr {
            endpoint_addr = after_setup_addr;
            continue;
        }

        let advertisement =
            publish_and_assert_advertisement(http, admin_url, relay_url, &endpoint_addr).await?;
        match PublicRpcHarness::connect(direct_endpoint_addr(&endpoint_addr)?).await {
            Ok(rpc) => return Ok((endpoint_addr, advertisement, rpc)),
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(POLL_INTERVAL).await;
                endpoint_addr = wait_for_endpoint_addr(data_dir, daemon).await?;
            }
        }
    }

    anyhow::bail!(
        "Public Liquidity API endpoint did not stay stable/connectable: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "endpoint kept changing".to_owned())
    )
}

/// Reconnects to a restarted daemon without reconfiguring it.
///
/// This is the acceptance check for the derived Iroh transport key. An endpoint
/// identity regenerated on every boot invalidates the advertised address, so
/// every restart in this suite would have to re-run
/// `configure_publish_and_connect` to repair it. Deriving the key from the
/// provider identity makes the address survive, so a restart needs nothing but
/// a reconnect — and asserting the address is *unchanged* is what keeps that
/// true.
async fn reconnect_after_restart(
    data_dir: &Path,
    daemon: &mut DaemonProcess,
    expected_endpoint_addr: &EndpointAddr,
) -> anyhow::Result<PublicRpcHarness> {
    let endpoint_addr = wait_for_endpoint_addr(data_dir, daemon).await?;
    ensure!(
        endpoint_addr.id == expected_endpoint_addr.id,
        "the public endpoint identity changed across a restart: {:?} -> {:?}",
        expected_endpoint_addr.id,
        endpoint_addr.id
    );

    let mut last_error = None;
    for _ in 0..6 {
        match PublicRpcHarness::connect(direct_endpoint_addr(&endpoint_addr)?).await {
            Ok(rpc) => return Ok(rpc),
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }

    anyhow::bail!(
        "restarted Public Liquidity API endpoint did not become connectable: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown".to_owned())
    )
}

async fn publish_and_assert_advertisement(
    http: &Client,
    admin_url: &str,
    relay_url: &str,
    endpoint_addr: &EndpointAddr,
) -> anyhow::Result<Signed<LiquidityProviderAdvertisement>> {
    let republished = admin_post(
        http,
        admin_url,
        "republish_advertisement",
        &json!({ "force": true }),
    )
    .await?;
    ensure!(
        republished["publication_status"] == "published",
        "advertisement did not publish: {republished}"
    );

    let state = admin_post(http, admin_url, "get_advertisement_state", &json!({})).await?;
    ensure!(
        state["ready"] == true,
        "advertisement state not ready: {state}"
    );
    ensure!(
        state["publication_status"] == "published",
        "advertisement state not published: {state}"
    );
    let admin_advertisement: Signed<LiquidityProviderAdvertisement> =
        serde_json::from_value(state["advertisement"].clone())
            .context("deserialize admin advertisement")?;

    // Setup reconciliation may have published an earlier advertisement moments
    // before this forced republish; the relay replaces the
    // parameterized-replaceable event asynchronously, so poll briefly until it
    // serves the latest publication.
    let mut relay_advertisement: Option<Signed<LiquidityProviderAdvertisement>> = None;
    for _ in 0..10 {
        let event = query_advertisement_event(
            relay_url,
            &admin_advertisement.payload.provider_pubkey.0,
            FLIP_PROVIDER_ADVERTISEMENT_EVENT_KIND,
            FLIP_PROVIDER_ADVERTISEMENT_D_TAG,
            FLIP_PROVIDER_ADVERTISEMENT_HASHTAG,
        )
        .await?;
        let content = event
            .get("content")
            .and_then(serde_json::Value::as_str)
            .context("relay advertisement content is string")?;
        let candidate: Signed<LiquidityProviderAdvertisement> =
            serde_json::from_str(content).context("deserialize relay advertisement")?;
        let matches = candidate == admin_advertisement;
        relay_advertisement = Some(candidate);
        if matches {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let relay_advertisement =
        relay_advertisement.context("relay never served the advertisement")?;
    assert_eq!(relay_advertisement, admin_advertisement);
    assert_eq!(
        relay_advertisement.payload.relay_hints,
        vec![Url(relay_url.to_owned())]
    );
    assert_eq!(
        relay_advertisement.payload.api_endpoints,
        vec![Url(format!(
            "iroh://{}?alpn=fedi%2Fflip%2Fpublic-liquidity%2F1",
            endpoint_addr.id
        ))]
    );
    Ok(admin_advertisement)
}

async fn fund_gateway_wallet(
    http: &Client,
    admin_url: &str,
    bitcoin: &BitcoinFixture,
) -> anyhow::Result<()> {
    fund_gateway_wallet_amount(
        http,
        admin_url,
        bitcoin,
        TOP_UP_BTC,
        GATEWAY_AMOUNT + GATEWAY_FEE_RESERVE,
    )
    .await
}

/// Tops the provider wallet up by `btc` and waits until gatewayd reports at
/// least `min_spendable_sats` spendable.
///
/// The amount is a parameter because a capacity test has to fund a wallet that
/// cannot cover everything it will be asked for, which the suite's usual whole
/// bitcoin can always cover.
async fn fund_gateway_wallet_amount(
    http: &Client,
    admin_url: &str,
    bitcoin: &BitcoinFixture,
    btc: f64,
    min_spendable_sats: u64,
) -> anyhow::Result<()> {
    let deposit = admin_post(
        http,
        admin_url,
        "create_deposit_address",
        &json!({ "label": "live-liquidity-top-up" }),
    )
    .await?;
    let address = deposit["address"]
        .as_str()
        .context("create_deposit_address returned address")?;
    bitcoin.send_to_address(address, btc).await?;
    mine_and_sync(bitcoin, FEDIMINT_FINALITY_BLOCKS).await?;
    wait_for_spendable_funds(http, admin_url, bitcoin, min_spendable_sats).await?;
    Ok(())
}

async fn wait_for_public_completion(
    rpc: &PublicRpcHarness,
    request: &RequestLiquidityRequest,
    bitcoin: &BitcoinFixture,
    timeout: Duration,
) -> anyhow::Result<Signed<GetAllocationStatusResponse>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let status = get_public_status(rpc, request).await?;
        if status
            .payload
            .status
            .item_statuses
            .iter()
            .all(|item| item.status == ItemAllocationStatus::Completed)
        {
            return Ok(status);
        }
        if status.payload.status.item_statuses.iter().any(|item| {
            matches!(
                item.status,
                ItemAllocationStatus::Failed
                    | ItemAllocationStatus::Cancelled
                    | ItemAllocationStatus::ActionRequired
            )
        }) {
            anyhow::bail!(
                "allocation item stopped without completion: {:?}",
                status.payload.status
            );
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for allocation completion: {:?}",
                status.payload.status
            );
        }
        mine_and_sync(bitcoin, 1).await?;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn get_public_status(
    rpc: &PublicRpcHarness,
    request: &RequestLiquidityRequest,
) -> anyhow::Result<Signed<GetAllocationStatusResponse>> {
    let signed = sign_public_rpc(
        PublicRpcPayloadDomain::GetAllocationStatusRequest,
        GetAllocationStatusRequest {
            version: ProtocolVersion(1),
            requester_pubkey: request.requester_pubkey.clone(),
            details_payload_hash: request.details_payload_hash,
            provider_pubkey: request.provider_pubkey.clone(),
            issued_at: now_timestamp(),
        },
    )?;
    rpc.client
        .get_allocation_status(signed)
        .await
        .context("get_allocation_status over real Iroh")
}

async fn wait_for_any_admin_allocation(http: &Client, admin_url: &str) -> anyhow::Result<String> {
    for _ in 0..60 {
        let list = admin_post(
            http,
            admin_url,
            "list_allocations",
            &json!({
                "page": { "cursor": null, "limit": 10 },
                "time_range": null
            }),
        )
        .await?;
        let items = list["allocations"]["items"]
            .as_array()
            .context("allocation list items")?;
        if let Some(item) = items.first() {
            return item["federation_id"]
                .as_str()
                .map(ToOwned::to_owned)
                .context("allocation has federation_id");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("admin allocation list did not show any allocation")
}

async fn wait_for_active_wallet_operations(
    http: &Client,
    admin_url: &str,
    federation_id: &str,
) -> anyhow::Result<()> {
    for _ in 0..120 {
        let allocation = get_admin_allocation(http, admin_url, federation_id).await?;
        let operations = allocation["allocation"]["wallet_operations"]
            .as_array()
            .context("allocation wallet operations")?;
        if operations.iter().any(|operation| {
            matches!(
                operation["status"].as_str(),
                Some("pending" | "broadcast" | "confirmed")
            )
        }) {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("allocation did not create active wallet operations before restart")
}

fn allocation_wallet_operations_completed(allocation: &Value) -> anyhow::Result<bool> {
    let operations = allocation["allocation"]["wallet_operations"]
        .as_array()
        .context("allocation wallet operations")?;
    Ok(!operations.is_empty()
        && operations
            .iter()
            .all(|operation| operation["status"] == "completed"))
}

async fn wait_for_operator_withdrawal_broadcast(
    database: &Database,
    operation_id: &str,
) -> anyhow::Result<String> {
    for _ in 0..180 {
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT status, txid FROM wallet_operations WHERE operation_id = ?")
                .bind(operation_id)
                .fetch_optional(database.pool())
                .await?;
        if let Some((status, Some(txid))) = row {
            if status == "broadcast" {
                return Ok(txid);
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("operator withdrawal {operation_id} did not reach broadcast with a txid")
}

async fn wait_for_gateway_funding_operation(
    http: &Client,
    admin_url: &str,
    federation_id: &str,
    statuses: &[&str],
    require_txid: bool,
) -> anyhow::Result<Value> {
    for _ in 0..180 {
        let allocation = get_admin_allocation(http, admin_url, federation_id).await?;
        let operations = gateway_funding_operations(&allocation)?;
        ensure!(
            operations.len() <= 1,
            "expected at most one gateway funding operation: {allocation}"
        );
        if let Some(operation) = operations.into_iter().next() {
            let status_matches = operation["status"]
                .as_str()
                .is_some_and(|status| statuses.contains(&status));
            let txid_matches = !require_txid || operation["txid"].as_str().is_some();
            if status_matches && txid_matches {
                return Ok(operation);
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!(
        "gateway funding operation did not reach {:?} for {federation_id}",
        statuses
    )
}

fn gateway_funding_operations(allocation: &Value) -> anyhow::Result<Vec<Value>> {
    let operations = allocation["allocation"]["wallet_operations"]
        .as_array()
        .context("allocation wallet operations")?;
    Ok(operations
        .iter()
        .filter(|operation| operation["operation_type"] == "gateway_funding")
        .cloned()
        .collect())
}

fn assert_single_completed_gateway_operation(
    allocation: &Value,
    operation_id: &str,
    txid: &str,
) -> anyhow::Result<()> {
    let operations = gateway_funding_operations(allocation)?;
    ensure!(
        operations.len() == 1,
        "expected exactly one gateway funding operation: {allocation}"
    );
    let operation = &operations[0];
    assert_eq!(operation["operation_id"], operation_id);
    assert_eq!(operation["txid"], txid);
    assert_eq!(operation["status"], "completed");
    let confirmations = operation["confirmation_count"]
        .as_u64()
        .context("completed operation has confirmation_count")?;
    ensure!(
        confirmations >= u64::from(FEDIMINT_FINALITY_BLOCKS),
        "gateway funding operation did not keep finality depth: {operation}"
    );
    Ok(())
}

fn assert_gateway_completion_evidence(
    allocation: &Value,
    operation_id: &str,
    txid: &str,
) -> anyhow::Result<()> {
    let item = allocation["allocation"]["status"]["item_statuses"]
        .as_array()
        .and_then(|items| items.first())
        .context("allocation has item status")?;
    assert_eq!(item["status"], "completed");
    assert_eq!(item["fulfilled_amount"], GATEWAY_AMOUNT);
    let evidence = item["completion_evidence"]["gateway"]
        .as_object()
        .context("gateway completion evidence exists")?;
    assert_eq!(
        evidence
            .get("wallet_operation_id")
            .and_then(Value::as_str)
            .context("gateway evidence wallet_operation_id")?,
        operation_id
    );
    assert_eq!(
        evidence
            .get("withdrawal_txid")
            .and_then(Value::as_str)
            .context("gateway evidence withdrawal_txid")?,
        txid
    );
    assert_eq!(
        evidence
            .get("fulfilled_amount")
            .and_then(Value::as_u64)
            .context("gateway evidence fulfilled_amount")?,
        GATEWAY_AMOUNT
    );
    Ok(())
}

async fn get_admin_allocation(
    http: &Client,
    admin_url: &str,
    federation_id: &str,
) -> anyhow::Result<Value> {
    admin_post(
        http,
        admin_url,
        "get_allocation",
        &json!({ "federation_id": federation_id }),
    )
    .await
}

async fn wait_for_spendable_funds(
    http: &Client,
    admin_url: &str,
    bitcoin: &BitcoinFixture,
    min_sats: u64,
) -> anyhow::Result<Value> {
    for _ in 0..60 {
        let funds = admin_post(http, admin_url, "get_funds", &json!({})).await?;
        if funds["balance"]["spendable"].as_u64().unwrap_or(0) >= min_sats {
            return Ok(funds);
        }
        mine_and_sync(bitcoin, 1).await?;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("gateway wallet did not report at least {min_sats} spendable sats")
}

async fn mine_and_sync(bitcoin: &BitcoinFixture, blocks: u32) -> anyhow::Result<()> {
    let current_height = bitcoin.get_block_height().await.unwrap_or(0);
    let expected_height = current_height + u64::from(blocks);
    bitcoin.mine_blocks(blocks).await?;
    bitcoin.wait_for_block_height(expected_height).await?;
    Ok(())
}

#[derive(Debug)]
struct DbWalletOperation {
    operation_id: String,
    operation_type: String,
    status: String,
    address: Option<String>,
    txid: Option<String>,
    confirmation_count: Option<u32>,
}

async fn load_wallet_operation_from_db(
    data_dir: &Path,
    operation_id: &str,
) -> anyhow::Result<DbWalletOperation> {
    let database = Database::connect(data_dir.join("flip.sqlite")).await?;
    let row = sqlx::query(
        "SELECT operation_id, operation_type, status, address, txid, confirmation_count \
         FROM wallet_operations WHERE operation_id = ?",
    )
    .bind(operation_id)
    .fetch_one(database.pool())
    .await
    .with_context(|| format!("load wallet operation {operation_id} from sqlite"))?;
    let operation = DbWalletOperation {
        operation_id: row.get("operation_id"),
        operation_type: row.get("operation_type"),
        status: row.get("status"),
        address: row.get("address"),
        txid: row.get("txid"),
        confirmation_count: row
            .get::<Option<i64>, _>("confirmation_count")
            .map(|count| count.max(0) as u32),
    };
    database.pool().close().await;
    Ok(operation)
}

async fn wait_for_db_wallet_operation_completed(
    data_dir: &Path,
    operation_id: &str,
    min_confirmations: u32,
) -> anyhow::Result<DbWalletOperation> {
    for _ in 0..90 {
        let operation = load_wallet_operation_from_db(data_dir, operation_id).await?;
        if operation.status == "completed"
            && operation
                .confirmation_count
                .is_some_and(|count| count >= min_confirmations)
        {
            return Ok(operation);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("wallet operation {operation_id} did not complete after restart")
}

/// Rewinds a completed item to `submitting` while preserving its complete
/// caller-owned ID, amount, and fee tuple and the real target-client receipt.
async fn rewind_stability_item_to_pre_submit(
    data_dir: &Path,
    federation_id: &str,
) -> anyhow::Result<String> {
    let database = Database::connect(data_dir.join("flip.sqlite")).await?;
    let row: (String, String) = sqlx::query_as(
        "SELECT item_id, step_json FROM allocation_items \
         WHERE federation_id = ? AND source_type = 'stability_pool'",
    )
    .bind(federation_id)
    .fetch_one(database.pool())
    .await
    .context("load the stability item to rewind")?;
    let (item_id, step_json) = row;

    let mut step: Value = serde_json::from_str(&step_json)?;
    let submitted_operation_id = step["sp_deposit_operation_id"]
        .as_str()
        .context("the completed item recorded a deposit operation id")?
        .to_owned();
    ensure!(
        step["peg_in_status"].as_str() == Some("claimed"),
        "the rewind expects a claimed peg-in, found {step}"
    );
    step["sp_deposit_status"] = Value::String("submitting".to_owned());

    let result = sqlx::query(
        "UPDATE allocation_items \
         SET status = 'running', step_json = ?, failure_json = NULL, updated_at = unixepoch() \
         WHERE item_id = ?",
    )
    .bind(serde_json::to_string(&step)?)
    .bind(&item_id)
    .execute(database.pool())
    .await
    .context("rewind the stability item")?;
    ensure!(
        result.rows_affected() == 1,
        "expected to rewind exactly one item, affected {}",
        result.rows_affected()
    );
    database.pool().close().await;
    Ok(submitted_operation_id)
}

/// Waits for the daemon to record a deposit operation id on the item.
///
/// Returns as soon as one appears, whatever it is, so the caller can compare it
/// against the one that was rewound away and report both.
async fn wait_for_recovered_deposit_operation(
    data_dir: &Path,
    federation_id: &str,
    timeout: Duration,
) -> anyhow::Result<Value> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut step = Value::Null;
    while tokio::time::Instant::now() < deadline {
        step = load_stability_step(data_dir, federation_id).await?;
        if !step["sp_deposit_operation_id"].is_null() {
            return Ok(step);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    anyhow::bail!("no deposit operation id was recovered onto the item; last step {step}")
}

/// Reads a stability item's persisted step straight from SQLite.
async fn load_stability_step(data_dir: &Path, federation_id: &str) -> anyhow::Result<Value> {
    let database = Database::connect(data_dir.join("flip.sqlite")).await?;
    let step_json: String = sqlx::query_scalar(
        "SELECT step_json FROM allocation_items \
         WHERE federation_id = ? AND source_type = 'stability_pool'",
    )
    .bind(federation_id)
    .fetch_one(database.pool())
    .await
    .context("load the stability item step")?;
    database.pool().close().await;
    Ok(serde_json::from_str(&step_json)?)
}

/// Waits for the allocation's single stability item to reach one status.
async fn wait_for_stability_item_status(
    http: &Client,
    admin_url: &str,
    federation_id: &str,
    expected: &str,
    timeout: Duration,
) -> anyhow::Result<Value> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last = Value::Null;
    while tokio::time::Instant::now() < deadline {
        let allocation = get_admin_allocation(http, admin_url, federation_id).await?;
        if let Some(item) = allocation["allocation"]["status"]["item_statuses"]
            .as_array()
            .and_then(|items| items.first())
        {
            last = item.clone();
            if last["status"].as_str() == Some(expected) {
                return Ok(last);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    anyhow::bail!("stability item did not reach {expected}; last seen {last}")
}

async fn mark_wallet_operation_completed_for_restart(
    data_dir: &Path,
    operation_id: &str,
    item_id: &str,
    confirmations: u32,
) -> anyhow::Result<()> {
    let database = Database::connect(data_dir.join("flip.sqlite")).await?;
    let result = sqlx::query(
        "UPDATE wallet_operations \
         SET status = 'completed', confirmation_count = ?, \
             completed_at = COALESCE(completed_at, unixepoch()), updated_at = unixepoch() \
         WHERE operation_id = ?",
    )
    .bind(i64::from(confirmations))
    .bind(operation_id)
    .execute(database.pool())
    .await
    .with_context(|| format!("mark wallet operation {operation_id} completed"))?;
    ensure!(
        result.rows_affected() == 1,
        "expected to mark one wallet operation completed, affected {}",
        result.rows_affected()
    );

    let item_status: String =
        sqlx::query_scalar("SELECT status FROM allocation_items WHERE item_id = ?")
            .bind(item_id)
            .fetch_one(database.pool())
            .await
            .with_context(|| format!("load allocation item {item_id} status"))?;
    ensure!(
        item_status != "completed",
        "target peg-in restart fixture should leave allocation item incomplete"
    );
    database.pool().close().await;
    Ok(())
}

fn required_str<'a>(value: &'a Value, field: &str) -> anyhow::Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("field {field} missing from {value}"))
}

async fn assert_live_verification_passed(
    http: &Client,
    admin_url: &str,
    federation_id: &FederationId,
) -> anyhow::Result<()> {
    let response = admin_post(
        http,
        admin_url,
        "get_verification_summary",
        &json!({ "federation_id": federation_id.0 }),
    )
    .await?;
    let summary = &response["summary"];
    ensure!(
        summary["policy_result"] == "passed",
        "verification policy did not pass: {summary}"
    );
    for stage in ["seat_checks", "credential_checks", "revocation_checks"] {
        let checks = summary[stage]
            .as_array()
            .with_context(|| format!("verification summary has {stage}"))?;
        ensure!(!checks.is_empty(), "verification {stage} is empty");
        for check in checks {
            ensure!(
                check["status"] == "passed",
                "verification {stage} check did not pass: {check}"
            );
        }
    }
    Ok(())
}

fn assert_health_component(health: &Value, component: &str) -> anyhow::Result<()> {
    let components = health["components"]
        .as_array()
        .context("health components")?;
    let entry = components
        .iter()
        .find(|entry| entry["component"].as_str() == Some(component))
        .with_context(|| format!("health component {component} missing: {health}"))?;
    ensure!(
        entry["status"] == "healthy",
        "health component {component} was not healthy: {entry}"
    );
    Ok(())
}

/// Fabricated federation config hash shared by the request details and the
/// fixture preview so the preview cross-check passes; the real canonical
/// config-hash derivation is a tracked shared open item.
fn live_config_hash() -> HashBytes {
    HashBytes(vec![9; 32])
}

/// The target federation's own canonical config hash, read from consensus.
///
/// Stability allocations need the real one. The worker compares the opened
/// target client's config hash against the hash the allocation was accepted
/// for and refuses to fund a mismatch, so a fixture hash that is not the
/// federation's own is correctly rejected — and a stability fixture claiming
/// one would be testing the fixture rather than the daemon.
///
/// Gateway stacks keep [`live_config_hash`]: nothing on that path compares a
/// client's configuration, so an arbitrary hash costs them nothing and spares
/// them a consensus read they do not need.
async fn live_target_config_hash(invite_code: &str) -> anyhow::Result<HashBytes> {
    let connectors = fedi_decentralized_federation_preview::bind_client_connectors()
        .await
        .context("bind client connectors for target preview")?;
    let preview = fedi_decentralized_federation_preview::preview(
        &connectors,
        &ServiceInviteCode(invite_code.to_owned()),
    )
    .await
    .context("preview target federation for its config hash")?;
    Ok(preview.federation_config_hash)
}

/// Stable requester (FI-side) Schnorr keypair for the live tests.
fn requester_keys() -> nostr_sdk::Keys {
    nostr_sdk::Keys::parse(&"07".repeat(32)).expect("fixed requester secret key parses")
}

fn live_liquidity_request(
    provider_pubkey: &Pubkey,
    invite_code: &str,
    endorsement: &FmanEndorsement,
    trust_material: &[GetFmanTrustMaterialResponse],
) -> anyhow::Result<RequestLiquidityRequest> {
    live_request_with_amounts(
        provider_pubkey,
        invite_code,
        endorsement,
        trust_material,
        &live_config_hash(),
        LiquidityAmountBounds {
            gateway_min_amount: Sats(GATEWAY_AMOUNT),
            gateway_max_amount: None,
            stability_min_amount: Sats(0),
            stability_max_amount: None,
        },
    )
}

/// Stability-pool-only request: no gateway item, one stability-pool item.
fn live_stability_request(
    provider_pubkey: &Pubkey,
    invite_code: &str,
    endorsement: &FmanEndorsement,
    trust_material: &[GetFmanTrustMaterialResponse],
    config_hash: &HashBytes,
) -> anyhow::Result<RequestLiquidityRequest> {
    live_request_with_amounts(
        provider_pubkey,
        invite_code,
        endorsement,
        trust_material,
        config_hash,
        LiquidityAmountBounds {
            gateway_min_amount: Sats(0),
            gateway_max_amount: None,
            stability_min_amount: Sats(STABILITY_AMOUNT),
            stability_max_amount: None,
        },
    )
}

fn live_request_with_amounts(
    provider_pubkey: &Pubkey,
    invite_code: &str,
    endorsement: &FmanEndorsement,
    trust_material: &[GetFmanTrustMaterialResponse],
    config_hash: &HashBytes,
    amounts: LiquidityAmountBounds,
) -> anyhow::Result<RequestLiquidityRequest> {
    let parsed_invite = fedimint_core::invite_code::InviteCode::from_str(invite_code)
        .context("parse target invite code")?;
    let mut request = RequestLiquidityRequest {
        version: ProtocolVersion(1),
        requester_pubkey: Pubkey(requester_keys().public_key().to_hex()),
        provider_pubkey: provider_pubkey.clone(),
        issued_at: now_timestamp(),
        network: BitcoinNetwork::Regtest,
        amounts,
        details_payload_hash: Sha256Digest([0; 32]),
        fman_endorsement: Some(endorsement.clone()),
        fman_trust_material: Some(trust_material.to_vec()),
        federation_details: FederationLiquidityDetails {
            invite_code: ServiceInviteCode(invite_code.to_owned()),
            federation_id: FederationId(parsed_invite.federation_id().to_string()),
            federation_name: FederationName("Live Target Federation".to_owned()),
            federation_config_hash: config_hash.clone(),
            fleet_seat_hints: Vec::new(),
            revocation_locations: Vec::new(),
        },
        // Inside the provider's request-lifetime ceiling, which is the FI
        // client's own request validity. A live request has to respect it like
        // any other.
        expires_at: Timestamp(now_timestamp().0 + 1_800),
    };
    request.details_payload_hash = request_liquidity_details_hash_for_request(&request)
        .context("compute details_payload_hash")?;
    Ok(request)
}

fn sign_public_rpc<T>(domain: PublicRpcPayloadDomain, payload: T) -> anyhow::Result<Signed<T>>
where
    T: Serialize,
{
    let hash = public_rpc_payload_hash(domain, &payload)?;
    let digest: [u8; 32] = hash
        .0
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("payload hash must be 32 bytes"))?;
    let signature = requester_keys()
        .sign_schnorr(&Message::from_digest(digest))
        .serialize()
        .to_vec();
    Ok(Signed {
        payload,
        proof: PayloadProof {
            signature: Signature(signature),
        },
    })
}

fn now_timestamp() -> Timestamp {
    Timestamp(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
}

fn live_setup_config(
    gateway: &GatewayFixture,
    bitcoin: &BitcoinFixture,
    relay_url: &str,
    endpoint_id: &str,
    attester_pubkey_hex: &str,
    esplora_url: Option<&str>,
) -> Value {
    // An esplora-configured FLIP is the deployment shape that can serve its
    // target clients a chain backend of its own; bitcoind is the one that
    // cannot, and leaves them on whatever the target federation advertises.
    let chain_observer = match esplora_url {
        Some(url) => json!({ "backend": { "type": "esplora", "url": url } }),
        None => json!({
            "backend": {
                "type": "bitcoind",
                "url": bitcoin.host_rpc_url(),
                "username": bitcoin.rpc_username()
            }
        }),
    };
    json!({
        "config": {
            "network": "regtest",
            "gateway": {
                "gateway_id": "gateway-1",
                "gateway_name": "primary",
                "admin_url": gateway.api_url.clone(),
                "identity_metadata": []
            },
            "chain_observer": chain_observer,
            "relays": [relay_url],
            "capacity": {
                "mode": "available_funds",
                "explicit_cap": null,
                "supported_sources": ["gateway", "stability_pool"]
            },
            "funding_policy": {
                "fee_reserve": GATEWAY_FEE_RESERVE,
                "confirmations": FEDIMINT_FINALITY_BLOCKS,
                "stability_pool_min_fee_rate_ppb": 0
            },
            "replenishment": {
                "warning_threshold": 1000,
                "critical_threshold": 500
            },
            "advertised_endpoint": {
                "endpoint_id": "live-iroh-endpoint",
                "transport": "iroh",
                "address": endpoint_id,
                "discovery_hints": [],
                "rpc_protocol_name": "fedi/flip/public-liquidity/1"
            },
            "advertisement": {
                "republish_interval": 600,
                "ready_advertisement_enabled": true
            },
            "provider_display": null,
            "policy": {
                "accepted_attester_policies": [
                    {
                        "attester_pubkey": attester_pubkey_hex,
                        "verification_requirement": "all_trusted"
                    }
                ],
                "supported_networks": ["regtest"]
            }
        }
    })
}

fn direct_endpoint_addr(endpoint_addr: &EndpointAddr) -> anyhow::Result<EndpointAddr> {
    let ip_addr = endpoint_addr
        .addrs
        .iter()
        .find_map(|addr| match addr {
            TransportAddr::Ip(addr) => Some(*addr),
            _ => None,
        })
        .context("Public Liquidity API endpoint address did not include an IP address")?;
    Ok(EndpointAddr::from_parts(
        endpoint_addr.id,
        [TransportAddr::Ip(ip_addr)],
    ))
}

struct PublicRpcHarness {
    _endpoint: Endpoint,
    client: PublicLiquidityApiClient,
}

impl PublicRpcHarness {
    async fn connect(endpoint_addr: EndpointAddr) -> anyhow::Result<Self> {
        let endpoint = Endpoint::builder(presets::N0DisableRelay)
            .bind()
            .await
            .context("bind test Iroh endpoint")?;
        let connection = endpoint
            .connect(
                endpoint_addr,
                fedi_decentralized_service_liquidity_manager::PUBLIC_LIQUIDITY_API_ALPN,
            )
            .await
            .context("connect to Public Liquidity API over Iroh")?;
        Ok(Self {
            _endpoint: endpoint,
            client: PublicLiquidityApiClient::new(connection),
        })
    }
}
