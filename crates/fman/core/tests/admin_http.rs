//! The browser-facing listener across both phases of a start.
//!
//! What these cover that `tests/admin.rs` cannot: `tests/admin.rs` builds the
//! router from an already-open `Fleet`, which is precisely the shape that made
//! first-run setup unreachable in a browser. These start where a fresh host
//! starts — a data root with no identity — and follow one listener through the
//! handover.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt as _;

use super::*;
use crate::admin::{AdminError, AdminErrorKind, AdminRequest, OperatorPhase};
use crate::directory::DirectoryPresence;
use crate::facts::PortBase;
use crate::fleet::FleetConfig;
use crate::onboarding::Onboarding;
use crate::seat_process::SeatProcessSpawner;
use crate::seat_process::fake::{block_forever, write_fake_fedimintd};
use crate::seat_process::{BitcoindConfig, RespawnPolicy, SeatProcessConfig};

/// A restore reaches no relay in these tests: the phase under test is the
/// listener, not the documents.
struct NoArchive;
struct NoHolderAuthorizations;

#[async_trait::async_trait]
impl crate::onboarding::HolderAuthorizationFetcher for NoHolderAuthorizations {
    async fn retained(
        &self,
        _identity: &crate::identity::RootMnemonic,
    ) -> anyhow::Result<Vec<fedi_decentralized_domain::HolderAuthorizationEnvelope>> {
        Ok(Vec::new())
    }

    async fn fetch(
        &self,
        _identity: &crate::identity::RootMnemonic,
    ) -> anyhow::Result<(Vec<crate::onboarding::FetchedHolderAuthorization>, u64)> {
        Ok((Vec::new(), u64::MAX))
    }
}

#[async_trait::async_trait]
impl crate::backup::BackupArchive for NoArchive {
    async fn recover(
        &self,
        _identity: &crate::identity::RootMnemonic,
    ) -> Result<crate::backup::RecoveredFleet, crate::backup::RecoverError> {
        Err(crate::backup::RecoverError::Other(anyhow::anyhow!(
            "no backup archive in this test"
        )))
    }
}

async fn process(temp: &TempDir) -> SeatProcessConfig {
    SeatProcessConfig {
        data_root: temp.path().to_owned(),
        fedimintd: write_fake_fedimintd(temp.path(), &block_forever()).await,
        bitcoin_network: bitcoin::Network::Regtest,
        iroh_dns: "https://dns.iroh.link/pkarr".parse().unwrap(),
        bitcoin_backend: crate::seat_process::BitcoinBackend::Bitcoind(BitcoindConfig {
            url: "http://127.0.0.1:18443".to_owned(),
            username: "user".to_owned(),
            password: "pass".to_owned(),
        }),
    }
}

async fn un_onboarded(temp: &TempDir) -> (Arc<Onboarding>, crate::db::Db) {
    let db = crate::db::Db::open(temp.path()).await.unwrap();
    assert!(
        db.load_identity().await.unwrap().is_none(),
        "the point of these tests is a data root with no identity"
    );
    let onboarding = Onboarding::new(
        db.clone(),
        process(temp).await,
        Arc::new(NoArchive),
        Arc::new(NoHolderAuthorizations),
        true,
    );
    (onboarding, db)
}

/// The fleet this host has once an operator has answered, opened the way the
/// daemon opens it: against an identity onboarding already chose.
async fn opened_fleet(temp: &TempDir, db: crate::db::Db) -> Arc<crate::fleet::Fleet> {
    Arc::new(
        crate::fleet::Fleet::open(
            db,
            FleetConfig {
                process_spawner: SeatProcessSpawner::Fake(Arc::new(
                    crate::seat_process::fake::FakeSeatProcessSpawner::default(),
                )),
                manifold_environment:
                    fedi_decentralized_manifold_environment::ManifoldEnvironment::Development,
                first_port_base: PortBase::new(31_500).unwrap(),
                setup_payments_configured: true,
                respawn: RespawnPolicy::default(),
                // Tests hold the relay down and watch the scan land; a production
                // cadence would only make them slow.
                backup_scan_interval: std::time::Duration::from_millis(10),
                push_gateway_origin: None,
                push_callback_retry_interval: std::time::Duration::from_millis(10),
                completion_callback_invoker: Arc::new(crate::push_callback::TestCallbackInvoker),
                process: process(temp).await,
            },
            Arc::new(crate::wallet::NoWallet),
        )
        .await
        .unwrap(),
    )
}

fn directory(fleet: &crate::fleet::Fleet) -> tokio::sync::watch::Receiver<DirectoryPresence> {
    let (tx, rx) = tokio::sync::watch::channel(DirectoryPresence {
        service_nostr_pubkey: fleet.identity().derive_service_nostr_keys().public_key(),
        onboarding: crate::directory::OnboardingStatus::Checking,
        latest_fman_version: None,
    });
    // The receiver reads what was last published; nothing publishes again here.
    std::mem::forget(tx);
    rx
}

/// One `POST /api/admin` over a real TCP connection, so what is asserted is
/// what a browser would receive.
async fn post_admin(
    addr: std::net::SocketAddr,
    request: &AdminRequest,
) -> Result<Value, AdminError> {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/api/admin"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_string(request).unwrap())
        .send()
        .await
        .expect("the operator listener answered");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    serde_json::from_str(&response.text().await.unwrap()).unwrap()
}

/// The fault this fixes: a host with no identity served no HTTP at all, so the
/// wizard that onboards it could never load, and the operator was never asked
/// to write down the phrase the whole fleet depends on.
///
/// The address is captured once and reused after the handover. That is the
/// assertion: an operator who finished the wizard must not have to reload
/// against a port that went away and came back.
#[tokio::test]
async fn the_operator_listener_serves_a_data_root_with_no_identity_and_survives_the_handover() {
    let temp = TempDir::new().unwrap();
    let (onboarding, db) = un_onboarded(&temp).await;
    let phase = OperatorPhase::onboarding(onboarding.clone());
    let (addr, _task) = serve(
        router(&phase, AdminHttpAuth::TrustedProxy),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();

    // Before an identity: the fleet vocabulary is refused, and the browser can
    // tell "not set up" from "not answering" because it got an answer.
    let refused = post_admin(addr, &AdminRequest::ListSeats)
        .await
        .unwrap_err();
    assert_eq!(refused.kind, AdminErrorKind::NotOnboarded);

    // ...and the two verbs that end the phase are served over the same listener.
    let onboarded = post_admin(addr, &AdminRequest::OnboardAsNew { if_needed: false })
        .await
        .unwrap();
    assert_eq!(onboarded["onboarded"], "new");

    // The remaining stages are the browser's: the relay fetch is shortcut at
    // the database, and the initial offer lands over this same listener.
    db.merge_holder_authorization_events(&[(vec![1; 32], 1, "{}".to_owned())], 1)
        .await
        .unwrap();
    let completed = post_admin(
        addr,
        &AdminRequest::ConfigureInitialOffer {
            max_seats: 1,
            price_msats: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(completed["onboarding"], "complete");

    // The daemon's startup observes durable completion, opens the fleet from
    // the database, and hands this already-serving listener over to it.
    tokio::time::timeout(std::time::Duration::from_secs(5), onboarding.completed())
        .await
        .expect("startup observes that the browser settled onboarding")
        .unwrap();
    let fleet = opened_fleet(&temp, db).await;
    phase.open_fleet(fleet.clone(), directory(&fleet));

    // Same address, no rebind, and now the full surface.
    assert_eq!(
        post_admin(addr, &AdminRequest::ShowPlans).await.unwrap()["plans"],
        serde_json::json!([])
    );
    assert_eq!(
        post_admin(addr, &AdminRequest::ShowMnemonic).await.unwrap()["mnemonic"],
        fleet.identity().phrase()
    );
    fleet.shutdown().await;
}

/// The refusal a browser reads is a value, not a sentence.
///
/// `AdminErrorKind` exists so the wizard need not sniff prose
/// (`crates/fman/core/src/admin.rs`), and this is the variant it opens on.
/// Nothing here asserts the message: it is the operator's sentence, and
/// rewording it is not a contract change.
#[tokio::test]
async fn the_pre_identity_refusal_carries_the_not_onboarded_discriminant() {
    let temp = TempDir::new().unwrap();
    let phase = OperatorPhase::onboarding(un_onboarded(&temp).await.0);
    let (addr, _task) = serve(
        router(&phase, AdminHttpAuth::TrustedProxy),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();

    for request in [
        AdminRequest::Onboarding,
        AdminRequest::ListSeats,
        AdminRequest::ShowPlans,
        AdminRequest::ShowMnemonic,
    ] {
        let refused = post_admin(addr, &request).await.unwrap_err();
        assert_eq!(refused.kind, AdminErrorKind::NotOnboarded, "{refused:?}");
    }
}

/// Password mode has to work before an identity exists, or the ordering change
/// would have quietly turned the un-onboarded window into an open one.
///
/// The password comes from a file the deployment wrote, so nothing about it
/// waits on a fleet (SPEC-operator-http, *Authentication modes*).
#[tokio::test]
async fn password_mode_protects_the_onboarding_phase_and_serves_its_login() {
    let temp = TempDir::new().unwrap();
    let phase = OperatorPhase::onboarding(un_onboarded(&temp).await.0);
    let app = router(
        &phase,
        AdminHttpAuth::Password("generated-password".to_owned()),
    );

    let unauthenticated = app
        .clone()
        .oneshot(
            Request::post("/api/admin")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&AdminRequest::OnboardAsNew { if_needed: false }).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        unauthenticated.status(),
        StatusCode::UNAUTHORIZED,
        "onboarding a host is an operator action like any other"
    );

    let signed_in = app
        .clone()
        .oneshot(
            Request::post("/api/auth")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"password":"generated-password"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(signed_in.status(), StatusCode::NO_CONTENT);
    let cookie = signed_in.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_owned();

    let refused = app
        .oneshot(
            Request::post("/api/admin")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(
                    serde_json::to_vec(&AdminRequest::ListSeats).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::OK);
    let body = to_bytes(refused.into_body(), 1024 * 1024).await.unwrap();
    let answered: Result<Value, AdminError> = serde_json::from_slice(&body).unwrap();
    assert_eq!(answered.unwrap_err().kind, AdminErrorKind::NotOnboarded);
}
