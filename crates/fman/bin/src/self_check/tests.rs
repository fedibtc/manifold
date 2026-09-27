use super::*;

#[tokio::test]
async fn dns_keeps_both_families_when_ipv6_answers_first() {
    let ipv4: IpAddr = "127.0.0.1".parse().unwrap();
    let ipv6: IpAddr = "::1".parse().unwrap();
    let addresses = collect_families(
        async {
            tokio::time::sleep(Duration::from_millis(15)).await;
            vec![ipv4]
        },
        async { vec![ipv6] },
        Duration::from_millis(100),
    )
    .await;
    assert!(addresses.contains(&ipv4));
    assert!(addresses.contains(&ipv6));
}

#[tokio::test]
async fn dns_retains_a_only_when_aaaa_stalls() {
    let ipv4: IpAddr = "127.0.0.1".parse().unwrap();
    let addresses = collect_families(
        async { vec![ipv4] },
        std::future::pending(),
        Duration::from_millis(15),
    )
    .await;
    assert_eq!(addresses, vec![ipv4]);
}

#[test]
fn core_chain_names_and_private_errors_are_not_serialized() {
    for (network, chain) in [
        (Network::Bitcoin, "main"),
        (Network::Testnet, "test"),
        (Network::Testnet4, "testnet4"),
        (Network::Signet, "signet"),
        (Network::Regtest, "regtest"),
    ] {
        let body = serde_json::json!({
            "id": "fman-self-check",
            "error": null,
            "result": { "chain": chain, "initialblockdownload": false,
                "secret": "private-host secret" }
        });
        assert_eq!(
            core_response(
                reqwest::StatusCode::OK,
                body.to_string().as_bytes(),
                network
            ),
            Reason::Reached
        );
    }
    let body = br#"{"id":"fman-self-check","result":null,"error":{"code":-28,"message":"private-host secret"}}"#;
    assert_eq!(
        core_response(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            body,
            Network::Bitcoin
        ),
        Reason::Starting
    );
}

#[tokio::test]
async fn bitcoin_reads_only_configured_local_rpc_and_forwards_auth_only_there() {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use fman_core::seat_process::BitcoindConfig;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let app = Router::new().route(
        "/rpc",
        post(
            move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                let tx = tx.clone();
                async move {
                    tx.send((headers, body)).await.unwrap();
                    Json(serde_json::json!({"id":"fman-self-check","error":null,
                    "result":{"chain":"main","initialblockdownload":false,
                    "ignored":"private-host secret"}}))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/rpc", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
        .bind()
        .await
        .unwrap();
    let config = SeatProcessConfig {
        data_root: std::path::PathBuf::new(),
        bitcoin_network: Network::Bitcoin,
        bitcoin_backend: BitcoinBackend::Bitcoind {
            primary: BitcoindConfig {
                url,
                username: "fixture-user".into(),
                password: "fixture-password".into(),
            },
            esplora_fallback: None,
        },
        iroh_dns: fedimint_core::util::SafeUrl::parse("http://127.0.0.1/").unwrap(),
    };
    let mut adapter = DaemonConnectivityChecks::new(&config, endpoint.clone(), true);
    adapter.proxy_configured = false;
    assert_eq!(
        adapter
            .bitcoin(Id::BitcoinPrimary, &config.bitcoin_backend)
            .await,
        check(Id::BitcoinPrimary, Status::Pass, Reason::Reached)
    );
    let (headers, body) = rx.recv().await.unwrap();
    assert_eq!(body["method"], "getblockchaininfo");
    assert_eq!(body["params"], serde_json::json!([]));
    assert_eq!(body["id"], "fman-self-check");
    assert_eq!(
        headers["authorization"],
        "Basic Zml4dHVyZS11c2VyOmZpeHR1cmUtcGFzc3dvcmQ="
    );
    server.abort();
    endpoint.close().await;
}

#[tokio::test]
async fn esplora_preserves_prefix_and_does_not_authenticate() {
    use axum::{Router, http::HeaderMap, routing::get};
    let (tx, mut rx) = tokio::sync::mpsc::channel(2);
    let app = Router::new()
        .route(
            "/prefix/blocks/tip/height",
            get({
                let tx = tx.clone();
                move |headers: HeaderMap| {
                    let tx = tx.clone();
                    async move {
                        tx.send(headers).await.unwrap();
                        "123"
                    }
                }
            }),
        )
        .route(
            "/prefix/block-height/0",
            get(move |headers: HeaderMap| {
                let tx = tx.clone();
                async move {
                    tx.send(headers).await.unwrap();
                    bitcoin::constants::genesis_block(Network::Bitcoin)
                        .block_hash()
                        .to_string()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}/prefix", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0DisableRelay)
        .bind()
        .await
        .unwrap();
    let config = SeatProcessConfig {
        data_root: std::path::PathBuf::new(),
        bitcoin_network: Network::Bitcoin,
        bitcoin_backend: BitcoinBackend::Esplora(base),
        iroh_dns: fedimint_core::util::SafeUrl::parse("http://127.0.0.1/").unwrap(),
    };
    let mut adapter = DaemonConnectivityChecks::new(&config, endpoint.clone(), true);
    adapter.proxy_configured = false;
    assert_eq!(
        adapter
            .bitcoin(Id::BitcoinPrimary, &config.bitcoin_backend)
            .await,
        check(Id::BitcoinPrimary, Status::Pass, Reason::Reached)
    );
    for _ in 0..2 {
        assert!(!rx.recv().await.unwrap().contains_key("authorization"));
    }
    server.abort();
    endpoint.close().await;
}
