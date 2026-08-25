//! The operator-facing enrollment flow, end to end.
//!
//! The in-crate tests drive ingest through a fake fetcher and the relay
//! integration test drives the real fetcher against a real relay. Neither runs
//! the Admin API path an operator console actually calls. This does: a real
//! daemon process, a real relay, and the two enrollment routes over HTTP, in
//! the order the console will call them.

mod common;
mod test_support;

use std::time::Duration;

use common::nostr_relay::NostrRelayFixture;
use fedi_credential_sdk_protocol::HolderContext;
use fedi_decentralized_service_liquidity_manager::Pubkey;
use reqwest::Client;
use serde_json::{Value, json};
use test_support::{ADMIN_TOKEN, DaemonProcess, TestDataDir, TestPorts, wait_for_admin_ready};
use tracing_subscriber::EnvFilter;

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("fedi_decentralized_liquidity_manager_daemon=debug,info")
        }))
        .with_test_writer()
        .try_init();
}

#[tokio::test(flavor = "multi_thread")]
async fn operator_enrols_a_holder_authorization_over_the_admin_api() -> anyhow::Result<()> {
    init_logging();

    let relay = NostrRelayFixture::start().await?;
    let ports = TestPorts::allocate()?;
    let data_dir = TestDataDir::new("integration-holder-authorization")?;
    let admin_url = format!("http://{}", ports.admin_bind_address);
    let client = Client::new();

    // Enrollment reads the environment-pinned relays, so point the
    // development routing at this test's relay.
    let mut daemon = DaemonProcess::start_with_relay(data_dir.path(), &ports, relay.ws_url())?;
    wait_for_admin_ready(&client, &admin_url, &mut daemon).await?;

    // 1. Before an identity exists there is nothing to put in a QR. The route
    //    still answers, because "no identity yet" is a stage of setup rather
    //    than a fault, and a console that polls it must not see a 5xx.
    let state = enrollment_state(&client, &admin_url).await?;
    assert!(
        state["provider_pubkey"].is_null(),
        "no provider identity is installed yet: {state}"
    );
    assert_eq!(state["status"]["state"], "checking");

    // 2. The operator installs the provider identity.
    let provider_keys = nostr_sdk::Keys::generate();
    let installed: Value = client
        .post(format!("{admin_url}/admin/v1/install_provider_identity"))
        .bearer_auth(ADMIN_TOKEN)
        .json(&json!({ "nostr_secret_key": provider_keys.secret_key().to_secret_hex() }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let provider_pubkey = Pubkey(provider_keys.public_key().to_hex());
    assert_eq!(installed["provider_pubkey"], provider_pubkey.0);
    assert_eq!(installed["installed"], true);

    // 3. The console can now build the authorization request. The QR carries
    //    exactly this pubkey as the SDK `HolderAuthorizationRequest` subject.
    let state = enrollment_state(&client, &admin_url).await?;
    assert_eq!(state["provider_pubkey"], provider_pubkey.0);
    // The startup read fires once an identity exists, so this is `checking`
    // until it lands and `not_observed` after. Both mean "no Holder yet"; only
    // the second claims to have looked.
    let awaiting = state["status"]["state"]
        .as_str()
        .expect("status carries a state");
    assert!(
        matches!(awaiting, "checking" | "not_observed"),
        "unexpected pre-authorization state: {state}"
    );

    // 4. Enrollment does not wait on the operator's advertisement relay
    //    config: it reads where the Holder app publishes.
    let before_setup = refresh(&client, &admin_url).await?;
    assert_eq!(before_setup["relays_answered"], 1);
    assert_eq!(before_setup["candidates_verified"], 0);

    // Secrets are written by name; a config write carries none, and
    // `apply_setup_config` requires the gateway credential to be stored first.
    for (secret, value) in [
        ("gateway_admin_credential", "gateway-secret"),
        ("chain_observer_password", "bitcoind-secret"),
    ] {
        client
            .post(format!("{admin_url}/admin/v1/set_config_secret"))
            .bearer_auth(ADMIN_TOKEN)
            .json(&json!({ "secret": secret, "update": { "action": "set", "value": value } }))
            .send()
            .await?
            .error_for_status()?;
    }

    client
        .post(format!("{admin_url}/admin/v1/apply_setup_config"))
        .bearer_auth(ADMIN_TOKEN)
        .json(&setup_config(&admin_url, relay.ws_url()))
        .send()
        .await?
        .error_for_status()?;

    // 5. Reconciling against a reachable relay that carries nothing is a
    //    success reporting an answering relay and no candidates. This is the
    //    state the console shows while waiting for the Holder to scan.
    let empty = refresh(&client, &admin_url).await?;
    assert_eq!(empty["relays_answered"], 1);
    assert_eq!(
        empty["relays_failed"].as_array().map(Vec::len),
        Some(0),
        "the configured relay answered: {empty}"
    );
    assert_eq!(empty["candidates_verified"], 0);
    assert_eq!(
        empty["status"]["state"], "not_observed",
        "a relay that answered with nothing is a completed read, not an unread one"
    );

    // 6. The Holder app signs and publishes.
    let issuer = common::credentials::test_issuer_context();
    let authority = common::credentials::test_issuer_authority(&issuer, relay.ws_url())?;
    let holder = HolderContext::generate();
    let credential =
        common::credentials::issue_credential_for_holder(&issuer, &authority, &holder)?;
    let authorization = common::credentials::holder_authorization_for_provider(
        &holder,
        &credential,
        &provider_pubkey,
    )?;
    publish_authorization(
        relay.ws_url(),
        &holder,
        &authorization,
        &credential,
        &provider_pubkey,
    )
    .await?;

    // 7. The operator presses refresh again and the authorization is enrolled.
    let enrolled = refresh(&client, &admin_url).await?;
    assert_eq!(enrolled["relays_answered"], 1);
    assert_eq!(enrolled["candidates_verified"], 1);
    assert_eq!(enrolled["status"]["state"], "authorization_observed");
    assert_eq!(enrolled["status"]["authorizations"], 1);
    assert_eq!(
        enrolled["status"]["holders"],
        json!([holder.public_key().to_string()])
    );

    // 8. Enrollment is durable, and reading it back re-verifies rather than
    //    trusting the stored row.
    let state = enrollment_state(&client, &admin_url).await?;
    assert_eq!(state["status"]["state"], "authorization_observed");
    assert_eq!(state["status"]["authorizations"], 1);

    // 9. Reconciling again is idempotent: the relay still serves the same
    //    event, and an equal-dated authorization must not multiply the count.
    let again = refresh(&client, &admin_url).await?;
    assert_eq!(again["candidates_verified"], 1);
    assert_eq!(again["status"]["authorizations"], 1);

    daemon.stop()?;
    relay.close().await?;
    Ok(())
}

async fn enrollment_state(client: &Client, admin_url: &str) -> anyhow::Result<Value> {
    Ok(client
        .post(format!(
            "{admin_url}/admin/v1/get_holder_authorization_state"
        ))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn refresh(client: &Client, admin_url: &str) -> anyhow::Result<Value> {
    Ok(client
        .post(format!(
            "{admin_url}/admin/v1/refresh_holder_authorizations"
        ))
        .bearer_auth(ADMIN_TOKEN)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

/// Publish the kind-37705 event the Holder app publishes, with the FLIP tags.
async fn publish_authorization(
    relay_url: &str,
    holder: &HolderContext,
    authorization: &fedi_credential_sdk_protocol::HolderAuthorization,
    credential: &fedi_credential_sdk_protocol::SignedCredential,
    provider_pubkey: &Pubkey,
) -> anyhow::Result<()> {
    use fedi_decentralized_nostr::flip::{
        FLIP_AUTHORIZATION_HASHTAG, HOLDER_AUTHORIZATION_EVENT_KIND, flip_authorization_d_tag,
    };

    let credential_digest = serde_json::to_value(&authorization.authorization.credential_digest)?
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("credential digest serializes as a string"))?
        .to_owned();
    let content = serde_json_canonicalizer::to_string(&json!({
        "version": 1,
        "holder_id_pubkey": holder.public_key().to_string(),
        "holder_authorization": authorization,
        "signed_credential": credential,
    }))?;

    let keys = nostr_sdk::Keys::parse(&holder.export_secret_key())?;
    let client = nostr_sdk::Client::new(keys);
    client.add_relay(relay_url).await?;
    client.connect().await;
    client
        .send_event_builder(
            nostr_sdk::EventBuilder::new(
                nostr_sdk::Kind::Custom(HOLDER_AUTHORIZATION_EVENT_KIND),
                content,
            )
            .tags([
                nostr_sdk::Tag::parse([
                    "d",
                    flip_authorization_d_tag(&provider_pubkey.0, &credential_digest).as_str(),
                ])?,
                nostr_sdk::Tag::parse(["t", FLIP_AUTHORIZATION_HASHTAG])?,
                nostr_sdk::Tag::parse(["p", provider_pubkey.0.as_str()])?,
            ]),
        )
        .await?;
    // The relay accepts and indexes asynchronously; the daemon fetch that
    // follows is a separate connection and must not race the write.
    tokio::time::sleep(Duration::from_millis(500)).await;
    client.disconnect().await;
    Ok(())
}

fn setup_config(admin_url: &str, relay_url: &str) -> Value {
    json!({
        "config": {
            "network": "regtest",
            "gateway": {
                "gateway_id": "gateway-1",
                "gateway_name": "primary",
                "admin_url": admin_url,
                "identity_metadata": []
            },
            "chain_observer": {
                "backend": {
                    "type": "bitcoind",
                    "url": admin_url,
                    "username": "bitcoin"
                }
            },
            "relays": [relay_url],
            "capacity": {
                "mode": "explicit_cap",
                "explicit_cap": 10000,
                "supported_sources": ["gateway", "stability_pool"]
            },
            "funding_policy": {
                "fee_reserve": 0,
                "confirmations": 1,
                "stability_pool_min_fee_rate_ppb": 0
            },
            "replenishment": {
                "warning_threshold": 1000,
                "critical_threshold": 500
            },
            "advertised_endpoint": {
                "endpoint_id": null,
                "transport": "iroh",
                "address": "iroh-node-id",
                "discovery_hints": [],
                "rpc_protocol_name": "fedi/flip/public-liquidity/1"
            },
            "advertisement": {
                "republish_interval": 600,
                "ready_advertisement_enabled": false
            },
            "provider_display": null,
            "policy": {
                "accepted_attester_policies": [],
                "supported_networks": ["regtest"]
            }
        }
    })
}
