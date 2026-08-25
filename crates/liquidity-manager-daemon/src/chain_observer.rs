//! Read-only chain access, over Esplora or Bitcoin Core.
//!
//! FLIP watches the chain to attribute wallet outflows to the operations that
//! caused them. The observer only reads: it never signs, broadcasts, or holds
//! keys. Only an Esplora observer can also serve a target Fedimint client,
//! because the Fedimint wallet client has no Bitcoin Core path.

use async_trait::async_trait;
use fedi_decentralized_service_liquidity_manager::{
    ChainObserverBackendView, ChainObserverConfigView,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChainObserverHealth {
    pub reachable: bool,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TxEvidence {
    pub txid: String,
    pub confirmations: u32,
    pub outputs: Vec<ChainOutputEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChainOutputEvidence {
    pub txid: String,
    pub vout: u32,
    pub address: Option<String>,
    pub script_pubkey: String,
    pub amount_sats: u64,
    pub confirmations: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AddressEvidence {
    pub address: String,
    pub outputs: Vec<ChainOutputEvidence>,
}

#[async_trait]
pub(crate) trait ChainObserver: Send + Sync {
    async fn health(&self) -> anyhow::Result<ChainObserverHealth>;
    async fn tx_evidence(&self, txid: &str) -> anyhow::Result<Option<TxEvidence>>;
    async fn address_evidence(&self, address: &str) -> anyhow::Result<AddressEvidence>;
}

pub(crate) enum ConfiguredChainObserver {
    Esplora(EsploraChainObserver),
    Bitcoind(BitcoindChainObserver),
}

impl ConfiguredChainObserver {
    pub(crate) fn from_config(config: &ChainObserverConfigView, password: Option<String>) -> Self {
        // Debug, not info: this is rebuilt from configuration on every worker
        // pass, so it says which backend a pass is about to use rather than
        // announcing a change. The URL is operator configuration and carries no
        // credential; the bitcoind password arrives separately.
        match &config.backend {
            ChainObserverBackendView::Esplora { url } => {
                tracing::debug!(backend = "esplora", url = %url.0, "using the chain observer");
                Self::Esplora(EsploraChainObserver::new(url.0.clone()))
            }
            ChainObserverBackendView::Bitcoind { url, username, .. } => {
                tracing::debug!(backend = "bitcoind", url = %url.0, "using the chain observer");
                Self::Bitcoind(BitcoindChainObserver::new(
                    url.0.clone(),
                    username.clone(),
                    password,
                ))
            }
        }
    }
}

#[async_trait]
impl ChainObserver for ConfiguredChainObserver {
    async fn health(&self) -> anyhow::Result<ChainObserverHealth> {
        match self {
            Self::Esplora(observer) => observer.health().await,
            Self::Bitcoind(observer) => observer.health().await,
        }
    }

    async fn tx_evidence(&self, txid: &str) -> anyhow::Result<Option<TxEvidence>> {
        match self {
            Self::Esplora(observer) => observer.tx_evidence(txid).await,
            Self::Bitcoind(observer) => observer.tx_evidence(txid).await,
        }
    }

    async fn address_evidence(&self, address: &str) -> anyhow::Result<AddressEvidence> {
        match self {
            Self::Esplora(observer) => observer.address_evidence(address).await,
            Self::Bitcoind(observer) => observer.address_evidence(address).await,
        }
    }
}

pub(crate) struct EsploraChainObserver {
    base_url: String,
    client: Client,
}

impl EsploraChainObserver {
    pub(crate) fn new(base_url: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            client: Client::new(),
        }
    }
}

#[async_trait]
impl ChainObserver for EsploraChainObserver {
    async fn health(&self) -> anyhow::Result<ChainObserverHealth> {
        let url = format!("{}/blocks/tip/height", self.base_url);
        let response = self.client.get(url).send().await?;
        Ok(ChainObserverHealth {
            reachable: response.status().is_success(),
            detail: Some(format!("esplora status={}", response.status())),
        })
    }

    async fn tx_evidence(&self, txid: &str) -> anyhow::Result<Option<TxEvidence>> {
        let url = format!("{}/tx/{txid}", self.base_url);
        let response = self.client.get(url).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let status = response
            .error_for_status()?
            .json::<EsploraTransaction>()
            .await?;
        if status.txid != txid {
            anyhow::bail!(
                "Esplora returned transaction {} for requested txid {txid}",
                status.txid
            );
        }
        let tip_height = self.tip_height().await.ok();
        let confirmations = confirmations_from_status(
            status.status.confirmed,
            status.status.block_height,
            tip_height,
        );
        Ok(Some(TxEvidence {
            txid: status.txid.clone(),
            confirmations,
            outputs: status
                .vout
                .into_iter()
                .enumerate()
                .map(|(vout, output)| ChainOutputEvidence {
                    txid: status.txid.clone(),
                    vout: vout as u32,
                    address: output.scriptpubkey_address,
                    script_pubkey: output.scriptpubkey,
                    amount_sats: output.value,
                    confirmations,
                })
                .collect(),
        }))
    }

    async fn address_evidence(&self, address: &str) -> anyhow::Result<AddressEvidence> {
        let url = format!("{}/address/{address}/txs", self.base_url);
        let txs = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<EsploraTransaction>>()
            .await?;
        let tip_height = self.tip_height().await.ok();
        let outputs = txs
            .into_iter()
            .flat_map(|tx| {
                let confirmations = confirmations_from_status(
                    tx.status.confirmed,
                    tx.status.block_height,
                    tip_height,
                );
                tx.vout
                    .into_iter()
                    .enumerate()
                    .filter(|(_, output)| output.scriptpubkey_address.as_deref() == Some(address))
                    .map(move |(vout, output)| ChainOutputEvidence {
                        txid: tx.txid.clone(),
                        vout: vout as u32,
                        address: output.scriptpubkey_address,
                        script_pubkey: output.scriptpubkey,
                        amount_sats: output.value,
                        confirmations,
                    })
            })
            .collect();
        Ok(AddressEvidence {
            address: address.to_owned(),
            outputs,
        })
    }
}

impl EsploraChainObserver {
    async fn tip_height(&self) -> anyhow::Result<u64> {
        let url = format!("{}/blocks/tip/height", self.base_url);
        Ok(self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?
            .parse::<u64>()?)
    }
}

#[derive(Deserialize)]
struct EsploraTxStatus {
    confirmed: bool,
    block_height: Option<u64>,
}

#[derive(Deserialize)]
struct EsploraTransaction {
    txid: String,
    status: EsploraTxStatus,
    vout: Vec<EsploraTxOutput>,
}

#[derive(Deserialize)]
struct EsploraTxOutput {
    scriptpubkey: String,
    scriptpubkey_address: Option<String>,
    value: u64,
}

/// bitcoind's RPC_INVALID_ADDRESS_OR_KEY, returned by `getrawtransaction`
/// for an unknown txid.
const BITCOIND_RPC_INVALID_ADDRESS_OR_KEY: i64 = -5;

pub(crate) struct BitcoindChainObserver {
    url: String,
    username: Option<String>,
    password: Option<String>,
    client: Client,
}

impl BitcoindChainObserver {
    pub(crate) fn new(url: String, username: Option<String>, password: Option<String>) -> Self {
        Self {
            url,
            username,
            password,
            client: Client::new(),
        }
    }

    async fn rpc<T>(&self, method: &str, params: serde_json::Value) -> anyhow::Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        match self.try_rpc(method, params).await? {
            Ok(result) => Ok(result),
            Err(error) => anyhow::bail!(
                "bitcoind RPC {method} failed with code {}: {}",
                error.code,
                error.message
            ),
        }
    }

    async fn try_rpc<T>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<Result<T, BitcoindRpcError>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut request = self.client.post(&self.url).json(&json!({
            "jsonrpc": "1.0",
            "id": "flip-liquidity-manager",
            "method": method,
            "params": params,
        }));
        if self.username.is_some() || self.password.is_some() {
            request = request.basic_auth(
                self.username.clone().unwrap_or_default(),
                self.password.clone(),
            );
        }
        let envelope = request
            .send()
            .await?
            .error_for_status()?
            .json::<BitcoindRpcEnvelope<T>>()
            .await?;
        if let Some(error) = envelope.error {
            return Ok(Err(error));
        }
        envelope
            .result
            .map(Ok)
            .ok_or_else(|| anyhow::anyhow!("bitcoind RPC {method} returned no result"))
    }
}

#[async_trait]
impl ChainObserver for BitcoindChainObserver {
    async fn health(&self) -> anyhow::Result<ChainObserverHealth> {
        let info: BlockchainInfo = self.rpc("getblockchaininfo", json!([])).await?;
        Ok(ChainObserverHealth {
            reachable: true,
            detail: Some(format!(
                "chain={}, blocks={}, verification_progress={}",
                info.chain, info.blocks, info.verificationprogress
            )),
        })
    }

    async fn tx_evidence(&self, txid: &str) -> anyhow::Result<Option<TxEvidence>> {
        let result: serde_json::Value = match self
            .try_rpc("getrawtransaction", json!([txid, true]))
            .await?
        {
            Ok(result) => result,
            // Unknown txid is the one expected miss; every other RPC error
            // (auth, loading, connectivity) must surface instead of looking
            // like "no evidence yet" to the sync task.
            Err(error) if error.code == BITCOIND_RPC_INVALID_ADDRESS_OR_KEY => return Ok(None),
            Err(error) => anyhow::bail!(
                "bitcoind RPC getrawtransaction failed with code {}: {}",
                error.code,
                error.message
            ),
        };
        let confirmations = confirmation_count(
            result
                .get("confirmations")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
        );
        let evidence_txid = result
            .get("txid")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(txid)
            .to_owned();
        if evidence_txid != txid {
            anyhow::bail!(
                "bitcoind returned transaction {evidence_txid} for requested txid {txid}"
            );
        }
        let outputs = result
            .get("vout")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .map(|output| bitcoind_output_evidence(&evidence_txid, confirmations, output))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Some(TxEvidence {
            txid: evidence_txid,
            confirmations,
            outputs,
        }))
    }

    async fn address_evidence(&self, address: &str) -> anyhow::Result<AddressEvidence> {
        let scantxoutset: serde_json::Value = self
            .rpc(
                "scantxoutset",
                json!(["start", [format!("addr({address})")]]),
            )
            .await?;
        let scan_height = scantxoutset
            .get("height")
            .and_then(serde_json::Value::as_u64);
        let outputs = scantxoutset
            .get("unspents")
            .and_then(serde_json::Value::as_array)
            .map(|unspents| {
                unspents
                    .iter()
                    .map(|unspent| {
                        let txid = required_str(unspent, "txid")?.to_owned();
                        let vout = required_u32(unspent, "vout")?;
                        let amount_sats =
                            btc_value_to_sats(unspent.get("amount").ok_or_else(|| {
                                anyhow::anyhow!("scantxoutset output has no amount")
                            })?)?;
                        let confirmations = match (
                            unspent.get("height").and_then(serde_json::Value::as_u64),
                            scan_height,
                        ) {
                            (Some(height), Some(tip)) if tip >= height => {
                                confirmation_count(tip - height + 1)
                            }
                            _ => 0,
                        };
                        Ok(ChainOutputEvidence {
                            txid,
                            vout,
                            address: Some(address.to_owned()),
                            script_pubkey: required_str(unspent, "scriptPubKey")?.to_owned(),
                            amount_sats,
                            confirmations,
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(AddressEvidence {
            address: address.to_owned(),
            outputs,
        })
    }
}

fn bitcoind_output_evidence(
    txid: &str,
    confirmations: u32,
    output: &serde_json::Value,
) -> anyhow::Result<ChainOutputEvidence> {
    let script = output
        .get("scriptPubKey")
        .ok_or_else(|| anyhow::anyhow!("getrawtransaction output has no scriptPubKey"))?;
    Ok(ChainOutputEvidence {
        txid: txid.to_owned(),
        vout: required_u32(output, "n")?,
        address: script
            .get("address")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        script_pubkey: required_str(script, "hex")?.to_owned(),
        amount_sats: btc_value_to_sats(
            output
                .get("value")
                .ok_or_else(|| anyhow::anyhow!("getrawtransaction output has no value"))?,
        )?,
        confirmations,
    })
}

fn required_str<'a>(value: &'a serde_json::Value, field: &str) -> anyhow::Result<&'a str> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("chain evidence field {field} is missing or not a string"))
}

fn required_u32(value: &serde_json::Value, field: &str) -> anyhow::Result<u32> {
    u32::try_from(
        value
            .get(field)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("chain evidence field {field} is missing or invalid"))?,
    )
    .map_err(Into::into)
}

/// Parses Bitcoin Core's JSON BTC decimal without passing through an f64.
fn btc_value_to_sats(value: &serde_json::Value) -> anyhow::Result<u64> {
    let decimal = value
        .as_number()
        .ok_or_else(|| anyhow::anyhow!("Bitcoin amount is not a JSON number"))?
        .to_string();
    let (mantissa, exponent) = decimal
        .split_once(['e', 'E'])
        .map_or(Ok((decimal.as_str(), 0_i32)), |(mantissa, exponent)| {
            Ok::<_, anyhow::Error>((mantissa, exponent.parse::<i32>()?))
        })?;
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.starts_with('-')
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        anyhow::bail!("invalid Bitcoin amount {decimal}");
    }
    let digits = format!("{whole}{fraction}").parse::<u64>()?;
    let scale = 8_i32 + exponent
        - i32::try_from(fraction.len()).map_err(|_| anyhow::anyhow!("Bitcoin amount overflow"))?;
    if scale >= 0 {
        return digits
            .checked_mul(
                10_u64
                    .checked_pow(scale as u32)
                    .ok_or_else(|| anyhow::anyhow!("Bitcoin amount overflow"))?,
            )
            .ok_or_else(|| anyhow::anyhow!("Bitcoin amount overflow"));
    }
    let divisor = 10_u64
        .checked_pow((-scale) as u32)
        .ok_or_else(|| anyhow::anyhow!("Bitcoin amount overflow"))?;
    if digits % divisor != 0 {
        anyhow::bail!("Bitcoin amount has sub-satoshi precision: {decimal}");
    }
    Ok(digits / divisor)
}

#[derive(Deserialize)]
struct BitcoindRpcEnvelope<T> {
    result: Option<T>,
    error: Option<BitcoindRpcError>,
}

/// JSON-RPC error object returned by bitcoind.
#[derive(Debug, Deserialize)]
struct BitcoindRpcError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct BlockchainInfo {
    chain: String,
    blocks: u64,
    verificationprogress: f64,
}

/// A chain-reported confirmation count, clamped to what the wire type carries.
///
/// The three places FLIP learns a confirmation count — a bitcoind
/// `confirmations` field, an Esplora scan height, and a block-height difference
/// — all arrive as `u64` and are reported as `u32`. A count that large means a
/// backend answered nonsense; saturating reports "very deeply confirmed", which
/// is the safe direction, where truncating could report a deeply confirmed
/// transaction as unconfirmed.
fn confirmation_count(count: u64) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn confirmations_from_status(
    confirmed: bool,
    block_height: Option<u64>,
    tip_height: Option<u64>,
) -> u32 {
    match (confirmed, block_height, tip_height) {
        (true, Some(block_height), Some(tip_height)) if tip_height >= block_height => {
            confirmation_count(tip_height - block_height + 1)
        }
        (true, Some(_), None) => 1,
        _ => 0,
    }
}

#[cfg(test)]
#[path = "../tests/chain_observer.rs"]
mod tests;
