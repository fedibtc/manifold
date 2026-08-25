//! Focused FLIP setup harness shared by the foreground staging composer.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use defe_client::{BitcoindInfo, FlipInfo, GatewaydInfo};
use fedi_credential_sdk_protocol::{
    HolderAuthorizationRequest, HolderContext, IssuerAuthority, IssuerContext, IssuerSecretKeys,
    PendingIssuance, RevocationLocation, SubjectPubkey,
};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_service_liquidity_manager::{
    AttestationInstallRequest, AttestationPayload, Pubkey,
};
use nostr_sdk::Keys;
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};

const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Configure FLIP, enroll its provider authorization, publish an advertisement,
/// and return the advertised endpoint identifier.
pub async fn configure_and_publish(
    flip: &FlipInfo,
    gateway: &GatewaydInfo,
    bitcoin: &BitcoindInfo,
    relay_url: &str,
) -> Result<String> {
    let http = Client::builder()
        // apply_setup_config may legitimately spend more than 18 seconds in its
        // sequential dependency probes before answering.
        .timeout(Duration::from_secs(30))
        .build()
        .context("build bounded FLIP Admin HTTP client")?;
    let endpoint_id = wait_for_endpoint_id(&flip.data_dir).await?;
    let issuer = test_issuer();
    let authority = issuer.issuer_authority(vec![RevocationLocation {
        protocol: "nostr".to_owned(),
        location: relay_url.to_owned(),
    }])?;
    let attester_pubkey = authority.issuer.issuer_id_pubkey.0.to_string();
    admin_post_available(
        &http,
        flip,
        "attestation_install",
        &serde_json::to_value(AttestationInstallRequest {
            payload: AttestationPayload(serde_json::to_vec(&authority)?),
        })?,
    )
    .await?;
    for (secret, value) in [
        ("gateway_admin_credential", gateway.password.as_str()),
        ("chain_observer_password", bitcoin.rpc_password.as_str()),
    ] {
        admin_post_available(
            &http,
            flip,
            "set_config_secret",
            &json!({"secret": secret, "update": {"action": "set", "value": value}}),
        )
        .await?;
    }
    let probed = admin_post_available(
        &http,
        flip,
        "probe_gateway",
        &json!({"admin_url": gateway.api_url}),
    )
    .await?;
    let gateway_id = probed["gateway_id"]
        .as_str()
        .context("FLIP gateway probe returned no gateway_id")?;
    let setup = setup_config(
        gateway,
        bitcoin,
        relay_url,
        &endpoint_id,
        &attester_pubkey,
        gateway_id,
    );
    let mut last = Value::Null;
    for _ in 0..600 {
        last = admin_post_available(&http, flip, "apply_setup_config", &setup).await?;
        if last["status"] == "ready" {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    ensure!(
        last["status"] == "ready",
        "FLIP setup did not become ready: {last}"
    );
    enroll_provider(&http, flip, relay_url, &issuer, &authority).await?;
    let published = admin_post_available(
        &http,
        flip,
        "republish_advertisement",
        &json!({"force": true}),
    )
    .await?;
    ensure!(
        published["publication_status"] == "published",
        "FLIP advertisement did not publish: {published}"
    );
    let state = admin_post_available(&http, flip, "get_advertisement_state", &json!({})).await?;
    ensure!(
        state["ready"] == true && state["publication_status"] == "published",
        "FLIP advertisement is not ready and published: {state}"
    );
    Ok(endpoint_id)
}

async fn enroll_provider(
    http: &Client,
    flip: &FlipInfo,
    relay_url: &str,
    issuer: &IssuerContext,
    authority: &IssuerAuthority,
) -> Result<()> {
    let holder = HolderContext::generate();
    let minimum_trust = ManifoldEnvironment::Development
        .profile()?
        .minimum_peer_badge_trust_level();
    let info = json!({"schema": "fedi-trust-score-v1.0", "trust_level": minimum_trust});
    let (request, pending) = PendingIssuance::create_request(
        &authority.issuer.issuance_key,
        authority.issuer.issuer_id_pubkey.clone(),
        info.clone(),
        json!(holder.public_key().to_string()),
    )?;
    let credential = pending.finalize(
        &authority.issuer.issuance_key,
        &issuer.issue_credential(info, &request)?,
    )?;
    let provider = Pubkey(flip.provider_pubkey_hex.clone());
    let authorization = holder.authorize_credential_use(
        HolderAuthorizationRequest {
            subject_pubkey: provider.0.parse::<SubjectPubkey>()?,
        },
        &credential,
    )?;
    let digest = serde_json::to_value(&authorization.authorization.credential_digest)?
        .as_str()
        .context("credential digest is not a string")?
        .to_owned();
    let content = serde_json_canonicalizer::to_string(&json!({
        "version": 1,
        "holder_id_pubkey": holder.public_key().to_string(),
        "holder_authorization": authorization,
        "signed_credential": credential,
    }))?;
    let client = nostr_sdk::Client::new(Keys::parse(&holder.export_secret_key())?);
    client.add_relay(relay_url).await?;
    client.connect().await;
    use fedi_decentralized_nostr::flip::{
        FLIP_AUTHORIZATION_HASHTAG, HOLDER_AUTHORIZATION_EVENT_KIND, flip_authorization_d_tag,
    };
    client
        .send_event_builder(
            nostr_sdk::EventBuilder::new(
                nostr_sdk::Kind::Custom(HOLDER_AUTHORIZATION_EVENT_KIND),
                content,
            )
            .tags([
                nostr_sdk::Tag::parse([
                    "d",
                    flip_authorization_d_tag(&provider.0, &digest).as_str(),
                ])?,
                nostr_sdk::Tag::parse(["t", FLIP_AUTHORIZATION_HASHTAG])?,
                nostr_sdk::Tag::parse(["p", provider.0.as_str()])?,
            ]),
        )
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    client.disconnect().await;
    let refreshed =
        admin_post_available(http, flip, "refresh_holder_authorizations", &json!({})).await?;
    ensure!(
        refreshed["candidates_verified"]
            .as_u64()
            .unwrap_or_default()
            >= 1,
        "FLIP provider authorization did not enroll: {refreshed}"
    );
    Ok(())
}

fn setup_config(
    gateway: &GatewaydInfo,
    bitcoin: &BitcoindInfo,
    relay_url: &str,
    endpoint_id: &str,
    attester_pubkey: &str,
    gateway_id: &str,
) -> Value {
    json!({"config": {
        "network": "regtest",
        "gateway": {
            "gateway_id": gateway_id,
            "gateway_name": "primary",
            "admin_url": gateway.api_url,
            "identity_metadata": []
        },
        "chain_observer": {"backend": {
            "type": "bitcoind",
            "url": bitcoin.rpc_url,
            "username": bitcoin.rpc_username
        }},
        "relays": [relay_url],
        "capacity": {
            "mode": "available_funds",
            "explicit_cap": null,
            "supported_sources": ["gateway", "stability_pool"]
        },
        "funding_policy": {
            "fee_reserve": 200000,
            "confirmations": 11,
            "stability_pool_min_fee_rate_ppb": 0
        },
        "replenishment": {"warning_threshold": 1000, "critical_threshold": 500},
        "advertised_endpoint": {
            "endpoint_id": "defe-staging-iroh-endpoint",
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
            "accepted_attester_policies": [{
                "attester_pubkey": attester_pubkey,
                "verification_requirement": "all_trusted"
            }],
            "supported_networks": ["regtest"]
        }
    }})
}

async fn admin_post_available(
    http: &Client,
    flip: &FlipInfo,
    method: &str,
    body: &Value,
) -> Result<Value> {
    for _ in 0..600 {
        let response = http
            .post(format!("{}/admin/v1/{method}", flip.admin_url))
            .bearer_auth(&flip.admin_token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("send FLIP admin request {method}"))?;
        if response.status() != StatusCode::SERVICE_UNAVAILABLE {
            return response
                .error_for_status()
                .with_context(|| format!("FLIP admin request {method} failed"))?
                .json()
                .await
                .with_context(|| format!("decode FLIP admin response {method}"));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("FLIP admin request {method} stayed unavailable")
}

async fn wait_for_endpoint_id(data_dir: &Path) -> Result<String> {
    let path = data_dir.join("public-iroh-endpoint-addr.json");
    for _ in 0..300 {
        if let Ok(bytes) = tokio::fs::read(&path).await {
            let value: Value = serde_json::from_slice(&bytes)?;
            return value["id"]
                .as_str()
                .map(ToOwned::to_owned)
                .with_context(|| format!("{} has no endpoint id", path.display()));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!(
        "FLIP public endpoint file did not appear at {}",
        path.display()
    )
}

fn test_issuer() -> IssuerContext {
    let keys: IssuerSecretKeys = serde_json::from_str(include_str!(
        "../../liquidity-manager-daemon/tests/fixtures/issuer-secret-keys.json"
    ))
    .expect("fixed integration issuer keys deserialize");
    IssuerContext::import_secret_key(&keys).expect("fixed integration issuer key imports")
}
