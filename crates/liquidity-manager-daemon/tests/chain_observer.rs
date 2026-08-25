use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use super::*;

#[test]
fn confirmations_are_derived_from_chain_tip_when_available() {
    assert_eq!(confirmations_from_status(false, None, Some(100)), 0);
    assert_eq!(confirmations_from_status(true, Some(100), Some(105)), 6);
    assert_eq!(confirmations_from_status(true, Some(100), None), 1);
    assert_eq!(confirmations_from_status(true, Some(106), Some(105)), 0);
}

#[test]
fn a_confirmation_count_saturates_rather_than_truncating() {
    assert_eq!(confirmation_count(0), 0);
    assert_eq!(confirmation_count(6), 6);
    assert_eq!(confirmation_count(u64::from(u32::MAX)), u32::MAX);
    // Truncating would report this as 0 confirmations — unconfirmed — which is
    // the dangerous direction for a funding decision.
    assert_eq!(confirmation_count(u64::from(u32::MAX) + 1), u32::MAX);
    assert_eq!(confirmation_count(u64::MAX), u32::MAX);
}

#[test]
fn bitcoind_btc_amounts_are_parsed_exactly() -> anyhow::Result<()> {
    let amount = |json| serde_json::from_str::<Value>(json);
    assert_eq!(btc_value_to_sats(&amount("0.00012345")?)?, 12_345);
    assert_eq!(btc_value_to_sats(&amount("0.00000001")?)?, 1);
    assert_eq!(
        btc_value_to_sats(&amount("21000000.00000000")?)?,
        2_100_000_000_000_000
    );
    assert!(btc_value_to_sats(&amount("0.000000001")?).is_err());
    Ok(())
}

#[tokio::test]
async fn esplora_observer_reads_health_tx_and_address_evidence() -> anyhow::Result<()> {
    let base_url = start_esplora_mock().await?;
    let observer = EsploraChainObserver::new(base_url);

    let health = observer.health().await?;
    assert!(health.reachable);
    assert_eq!(health.detail.as_deref(), Some("esplora status=200 OK"));

    let tx = observer
        .tx_evidence("confirmed-tx")
        .await?
        .expect("confirmed tx evidence exists");
    assert_eq!(tx.txid, "confirmed-tx");
    assert_eq!(tx.confirmations, 6);
    assert_eq!(tx.outputs.len(), 2);
    assert_eq!(tx.outputs[0].vout, 0);
    assert_eq!(tx.outputs[0].address.as_deref(), Some("bcrt1qaddress"));
    assert_eq!(tx.outputs[0].amount_sats, 12_345);
    assert_eq!(tx.outputs[0].script_pubkey, "0014abcd");
    assert_eq!(observer.tx_evidence("missing-tx").await?, None);

    let address = observer.address_evidence("bcrt1qaddress").await?;
    assert_eq!(address.address, "bcrt1qaddress");
    assert_eq!(
        address.outputs,
        vec![
            ChainOutputEvidence {
                txid: "address-confirmed".to_owned(),
                vout: 1,
                address: Some("bcrt1qaddress".to_owned()),
                script_pubkey: "0014confirmed".to_owned(),
                amount_sats: 25_000,
                confirmations: 3,
            },
            ChainOutputEvidence {
                txid: "address-mempool".to_owned(),
                vout: 0,
                address: Some("bcrt1qaddress".to_owned()),
                script_pubkey: "0014mempool".to_owned(),
                amount_sats: 30_000,
                confirmations: 0,
            },
        ]
    );
    Ok(())
}

#[tokio::test]
async fn bitcoind_observer_reads_rpc_evidence_and_errors() -> anyhow::Result<()> {
    let base_url = start_bitcoind_mock().await?;
    let observer = BitcoindChainObserver::new(
        base_url.clone(),
        Some("bitcoin".to_owned()),
        Some("secret".to_owned()),
    );

    let health = observer.health().await?;
    assert!(health.reachable);
    assert_eq!(
        health.detail.as_deref(),
        Some("chain=regtest, blocks=105, verification_progress=1")
    );

    let tx = observer
        .tx_evidence("known-tx")
        .await?
        .expect("known tx evidence exists");
    assert_eq!(tx.confirmations, 7);
    assert_eq!(tx.outputs[0].vout, 2);
    assert_eq!(tx.outputs[0].address.as_deref(), Some("bcrt1qaddress"));
    assert_eq!(tx.outputs[0].amount_sats, 12_345);
    assert_eq!(tx.outputs[0].script_pubkey, "0014abcd");
    assert_eq!(observer.tx_evidence("missing-tx").await?, None);

    let address = observer.address_evidence("bcrt1qaddress").await?;
    assert_eq!(
        address.outputs,
        vec![ChainOutputEvidence {
            txid: "utxo-tx".to_owned(),
            vout: 3,
            address: Some("bcrt1qaddress".to_owned()),
            script_pubkey: "0014scan".to_owned(),
            amount_sats: 1,
            confirmations: 6,
        }]
    );

    let unavailable = BitcoindChainObserver::new(format!("{base_url}/error"), None, None);
    let error = unavailable
        .health()
        .await
        .expect_err("RPC error should make health fail");
    assert!(
        error
            .to_string()
            .contains("bitcoind RPC getblockchaininfo failed"),
        "{error}"
    );

    // Only the unknown-txid code (-5) maps to None; any other RPC error
    // must propagate instead of masquerading as missing evidence.
    let error = unavailable
        .tx_evidence("known-tx")
        .await
        .expect_err("non-missing-tx RPC error should propagate");
    assert!(error.to_string().contains("code -28"), "{error}");
    Ok(())
}

async fn start_esplora_mock() -> anyhow::Result<String> {
    let app = Router::new()
        .route("/blocks/tip/height", get(|| async { "105" }))
        .route("/tx/{txid}", get(esplora_tx))
        .route("/address/{address}/txs", get(esplora_address_txs));
    serve_mock(app).await
}

async fn esplora_tx(Path(txid): Path<String>) -> impl IntoResponse {
    if txid == "missing-tx" {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(json!({
        "txid": txid,
        "status": { "confirmed": true, "block_height": 100 },
        "vout": [
            {
                "scriptpubkey": "0014abcd",
                "scriptpubkey_address": "bcrt1qaddress",
                "value": 12345
            },
            {
                "scriptpubkey": "6a",
                "scriptpubkey_address": null,
                "value": 0
            }
        ]
    }))
    .into_response()
}

async fn esplora_address_txs(Path(_address): Path<String>) -> impl IntoResponse {
    Json(json!([
        {
            "txid": "address-confirmed",
            "status": {
                "confirmed": true,
                "block_height": 103
            },
            "vout": [
                { "scriptpubkey": "6a", "scriptpubkey_address": null, "value": 0 },
                {
                    "scriptpubkey": "0014confirmed",
                    "scriptpubkey_address": "bcrt1qaddress",
                    "value": 25000
                }
            ]
        },
        {
            "txid": "address-mempool",
            "status": {
                "confirmed": false,
                "block_height": null
            },
            "vout": [{
                "scriptpubkey": "0014mempool",
                "scriptpubkey_address": "bcrt1qaddress",
                "value": 30000
            }]
        }
    ]))
}

async fn start_bitcoind_mock() -> anyhow::Result<String> {
    let app = Router::new()
        .route("/", post(bitcoind_rpc))
        .route("/error", post(bitcoind_rpc_error))
        .with_state(());
    serve_mock(app).await
}

async fn bitcoind_rpc(State(()): State<()>, Json(request): Json<Value>) -> impl IntoResponse {
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = request
        .get("params")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Json(match method {
        "getblockchaininfo" => json!({
            "result": {
                "chain": "regtest",
                "blocks": 105,
                "verificationprogress": 1.0
            },
            "error": null
        }),
        "getrawtransaction" => {
            if params.first().and_then(Value::as_str) == Some("missing-tx") {
                json!({
                    "result": null,
                    "error": { "code": -5, "message": "No such mempool or blockchain transaction" }
                })
            } else {
                json!({
                    "result": {
                        "txid": params.first().and_then(Value::as_str).unwrap_or("known-tx"),
                        "confirmations": 7,
                        "vout": [{
                            "n": 2,
                            "value": 0.00012345,
                            "scriptPubKey": {
                                "hex": "0014abcd",
                                "address": "bcrt1qaddress"
                            }
                        }]
                    },
                    "error": null
                })
            }
        }
        "scantxoutset" => json!({
            "result": {
                "success": true,
                "height": 105,
                "unspents": [
                    {
                        "txid": "utxo-tx",
                        "vout": 3,
                        "scriptPubKey": "0014scan",
                        "amount": 0.00000001,
                        "height": 100
                    }
                ]
            },
            "error": null
        }),
        _ => json!({
            "result": null,
            "error": { "code": -32601, "message": "method not found" }
        }),
    })
}

async fn bitcoind_rpc_error(Json(_request): Json<Value>) -> impl IntoResponse {
    Json(json!({
        "result": null,
        "error": { "code": -28, "message": "Loading block index" }
    }))
}

async fn serve_mock(app: Router) -> anyhow::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(format!("http://{addr}"))
}
