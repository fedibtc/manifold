//! Invite-code federation preview over the real Fedimint client API.
//!
//! Two components need the same consensus read and must not drift apart. FLIP
//! previews a federation to decide liquidity eligibility; the FI reads
//! consensus metadata back after writing the seat-binding directory. This
//! crate owns the transport for both, so the `fedimint-api-client` dependency
//! lives in exactly one place.
//!
//! [`read_consensus`] is the lower-level entry point, returning the downloaded
//! config and the raw metadata value without interpreting either. It exists
//! because `fi-client` must stay `wasm32-unknown-unknown`-compatible and
//! dependency-thin: its consumer performs this call and hands back the
//! undigested result, leaving every derivation and every signature check
//! inside `fi-client` ([`ARCH-fi-client`](../../fi-client/specs/ARCH-fi-client.md)).
//!
//! [`preview`] builds FLIP's [`FederationPreview`] on top of the same read.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr as _;

pub use fedi_decentralized_domain::federation_config_hash;
use fedi_decentralized_domain::{
    BitcoinNetwork, FMAN_SEAT_BINDINGS_META_FIELD_KEY, FederationId, FederationSeat,
    FederationSeats, GatewayApiUrl, GuardianIdentity, HashBytes, InviteCode, PeerId,
    federation_seats,
};
use fedimint_api_client::api::{DynGlobalApi, FederationApiExt as _};
use fedimint_api_client::download_from_invite_code;
use fedimint_api_client::query::FilterMapThreshold;
pub use fedimint_connectors::ConnectorRegistry;
use fedimint_core::NumPeersExt as _;
use fedimint_core::config::ClientConfig;
use fedimint_core::core::ModuleInstanceId;
use fedimint_core::encoding::{Decodable as _, DynRawFallback};
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::module::ApiRequestErased;
use fedimint_core::module::registry::ModuleDecoderRegistry;
use fedimint_core::util::SafeUrl;
use fedimint_meta_client::api::MetaFederationApi as _;
use fedimint_meta_client::common::DEFAULT_META_KEY;
use fedimint_wallet_client::config::WalletClientConfig;
use fedimint_walletv2_common::config::WalletClientConfig as WalletV2ClientConfig;
use serde::{Deserialize, Serialize};

/// Fedimint module kind holding the federation's Bitcoin wallet config.
const WALLET_MODULE_KIND: &str = "wallet";

/// Fedimint v2 module kind holding the federation's Bitcoin wallet config.
const WALLET_V2_MODULE_KIND: &str = "walletv2";

/// Fedimint module kind holding mutable consensus metadata.
const META_MODULE_KIND: &str = "meta";

/// Error returned by a federation preview attempt.
#[derive(Debug, thiserror::Error)]
pub enum FederationPreviewError {
    /// The invite code is malformed or the previewed config is inconsistent.
    #[error("invalid invite code: {0}")]
    InvalidInviteCode(String),

    /// A caller-owned transport policy refused an invite endpoint before dial.
    ///
    /// Deliberately carries no endpoint or resolver detail: this error can be
    /// returned in a signed public response, where exposing either would turn
    /// the policy into a network-information oracle.
    #[error("invite endpoint rejected by transport policy")]
    EndpointPolicyRejected,

    /// The preview could not complete for a temporary or deployment reason.
    #[error("federation preview unavailable: {0}")]
    Unavailable(String),
}

/// Undigested result of one consensus read.
///
/// Deliberately carries the config and the raw metadata value rather than
/// anything derived from them. A caller that cannot depend on this crate can
/// receive this across a capability boundary and still perform every
/// derivation and verification itself.
#[derive(Clone, Debug)]
pub struct FederationConsensusSnapshot {
    /// Final client config, as downloaded and threshold-agreed.
    pub config: ClientConfig,

    /// Raw `meta` module consensus value under [`DEFAULT_META_KEY`], when the
    /// federation has one. This is the whole JSON object of meta fields, not
    /// any single field within it.
    pub meta_value: Option<Vec<u8>>,

    /// The meta module's monotone consensus revision for `meta_value`, from
    /// the same `get_consensus` read. `Some` exactly when `meta_value` is
    /// `Some`.
    pub meta_revision: Option<u64>,

    /// Bitcoin network decoded from the final wallet module config.
    pub network: BitcoinNetwork,
}

/// One peer from the previewed final federation config.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviewPeer {
    /// Fedimint peer id from the final config.
    pub peer_id: PeerId,

    /// Guardian consensus identity from the final config.
    pub guardian_identity: GuardianIdentity,
}

/// Authoritative federation facts derived from an invite code.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FederationPreview {
    /// Fedimint federation id.
    pub federation_id: FederationId,

    /// Canonical hash of the final federation config.
    pub federation_config_hash: HashBytes,

    /// Bitcoin network the federation runs on.
    pub network: BitcoinNetwork,

    /// Final config peer set.
    pub peers: Vec<PreviewPeer>,

    /// Consensus majority threshold for this peer set.
    pub consensus_threshold: u32,

    /// Raw canonical `fedi:fman_seat_bindings` consensus metadata value, when
    /// set.
    pub fman_seat_bindings_metadata: Option<String>,

    /// Fedimint module kinds the final config carries.
    ///
    /// A liquidity source can need a module the target federation does not
    /// have, and a federation id does not commit the module map — the config
    /// hash beside it does. Carrying the kinds lets that be refused while the
    /// answer is still bound to the hash the request is checked against,
    /// rather than discovered by a worker after value has moved.
    pub module_kinds: BTreeSet<String>,
}

impl FederationPreview {
    /// The federation facts a seat-binding check runs against.
    ///
    /// The preview is the authority for these values, so this hands them to
    /// the shared validator in the shape it expects rather than re-deriving
    /// anything.
    #[must_use]
    pub fn federation_seats(&self) -> FederationSeats {
        FederationSeats::from_parts(
            self.federation_id.clone(),
            self.federation_config_hash.clone(),
            self.consensus_threshold,
            self.peers
                .iter()
                .map(|peer| FederationSeat {
                    peer_id: peer.peer_id.clone(),
                    guardian_identity: peer.guardian_identity.clone(),
                })
                .collect(),
        )
    }
}

/// Bind a connector registry with the client-side defaults.
///
/// # Errors
///
/// Returns an error if the connector registry cannot be bound.
pub async fn bind_client_connectors() -> anyhow::Result<ConnectorRegistry> {
    ConnectorRegistry::build_from_client_defaults().bind().await
}

/// Download the final config and the raw `meta` consensus value.
///
/// It deliberately does not join the federation or open a database:
/// `download_from_invite_code` returns the config plus an API handle, and the
/// metadata is one unauthenticated consensus query away.
///
/// The federation id is checked against the invite code twice inside that
/// download — once against the peer that answered, then again over the
/// threshold-agreed config — so the peer set this reports cannot be
/// substituted by any single guardian.
///
/// # Errors
///
/// Returns an error if the invite code is malformed, the config download
/// fails, or the consensus query fails.
pub async fn read_consensus(
    connectors: &ConnectorRegistry,
    invite_code: &InviteCode,
) -> Result<FederationConsensusSnapshot, FederationPreviewError> {
    let invite = FedimintInviteCode::from_str(&invite_code.0).map_err(|error| {
        FederationPreviewError::InvalidInviteCode(format!("malformed invite code: {error}"))
    })?;
    let (config, api) = download_from_invite_code(connectors, &invite)
        .await
        .map_err(|error| {
            FederationPreviewError::Unavailable(format!(
                "could not download the federation config: {error}"
            ))
        })?;
    let (meta_revision, meta_value) = match read_meta_value(&api, &config).await? {
        Some((revision, value)) => (Some(revision), Some(value)),
        None => (None, None),
    };

    let network = network_of(&config)?;

    Ok(FederationConsensusSnapshot {
        config,
        meta_value,
        meta_revision,
        network,
    })
}

/// Read the LNv2 gateway view aggregated by the upstream client strategy.
///
/// Values outside [`GatewayApiUrl`]'s public client-endpoint policy are
/// ignored individually. An unrelated legacy or operator-managed entry must
/// not hide an exact valid gateway that the caller is checking for.
///
/// # Errors
///
/// Returns an error if the invite or final config is invalid, the federation
/// has no LNv2 module, or a threshold gateway query cannot complete.
pub async fn read_lnv2_gateways(
    connectors: &ConnectorRegistry,
    invite_code: &InviteCode,
) -> Result<Vec<fedi_decentralized_domain::GatewayApiUrl>, FederationPreviewError> {
    let invite = FedimintInviteCode::from_str(&invite_code.0).map_err(|error| {
        FederationPreviewError::InvalidInviteCode(format!("malformed invite code: {error}"))
    })?;
    let (config, api) = download_from_invite_code(connectors, &invite)
        .await
        .map_err(|error| FederationPreviewError::Unavailable(error.to_string()))?;
    let (lnv2_id, _) =
        module_config(&config, fedimint_lnv2_common::KIND.as_str()).ok_or_else(|| {
            FederationPreviewError::InvalidInviteCode("federation has no lnv2 module".to_owned())
        })?;
    let api = api.with_module(lnv2_id);
    let gateways: BTreeMap<fedimint_core::PeerId, Vec<fedimint_core::util::SafeUrl>> = api
        .request_with_strategy(
            FilterMapThreshold::new(|_, gateways| Ok(gateways), api.all_peers().to_num_peers()),
            fedimint_lnv2_common::endpoint_constants::GATEWAYS_ENDPOINT.to_owned(),
            ApiRequestErased::default(),
        )
        .await
        .map_err(|error| FederationPreviewError::Unavailable(error.to_string()))?;
    Ok(admitted_lnv2_gateway_view(&gateways))
}

fn admitted_lnv2_gateway_view(
    gateways: &BTreeMap<fedimint_core::PeerId, Vec<SafeUrl>>,
) -> Vec<GatewayApiUrl> {
    gateways
        .values()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|url| GatewayApiUrl::try_from(url.as_str()).ok())
        .collect()
}

/// Preview the federation identified by `invite_code`.
///
/// # Errors
///
/// Returns an error if the consensus read fails, the config is unusable, or
/// the federation runs on an unsupported Bitcoin network.
pub async fn preview(
    connectors: &ConnectorRegistry,
    invite_code: &InviteCode,
) -> Result<FederationPreview, FederationPreviewError> {
    let snapshot = read_consensus(connectors, invite_code).await?;

    // Phase 0's derivation, never a local re-derivation: the FMan signs
    // this same config hash into every attestation.
    let seats = federation_seats(&snapshot.config).map_err(|error| {
        FederationPreviewError::InvalidInviteCode(format!(
            "federation config is not usable: {error}"
        ))
    })?;

    Ok(FederationPreview {
        federation_id: seats.federation_id().clone(),
        federation_config_hash: seats.federation_config_hash().clone(),
        network: snapshot.network,
        peers: seats
            .seats()
            .iter()
            .map(|seat| PreviewPeer {
                peer_id: seat.peer_id.clone(),
                guardian_identity: seat.guardian_identity.clone(),
            })
            .collect(),
        consensus_threshold: seats.consensus_threshold(),
        fman_seat_bindings_metadata: seat_bindings_field(snapshot.meta_value.as_deref())?,
        module_kinds: module_kinds(&snapshot.config),
    })
}

/// Every Fedimint module kind a final client config carries.
///
/// Read from the config's own module map rather than from a client's
/// registered modules: the two can disagree, and only the config is what the
/// federation reached consensus on and hashed.
#[must_use]
pub fn module_kinds(config: &ClientConfig) -> BTreeSet<String> {
    config
        .modules
        .values()
        .map(|module| module.kind.as_str().to_owned())
        .collect()
}

/// Find one module instance by kind and return its undecoded config bytes.
fn module_config<'a>(config: &'a ClientConfig, kind: &str) -> Option<(ModuleInstanceId, &'a [u8])> {
    config.modules.iter().find_map(|(instance_id, module)| {
        if module.kind.as_str() != kind {
            return None;
        }
        match &module.config {
            DynRawFallback::Raw { raw, .. } => Some((*instance_id, raw.as_slice())),
            DynRawFallback::Decoded(_) => None,
        }
    })
}

/// Read the Bitcoin network from the wallet module config.
///
/// Decoded from the module's own bytes rather than through a decoder registry:
/// the downloaded config arrives undecoded, and this needs one field from one
/// module.
fn network_of(config: &ClientConfig) -> Result<BitcoinNetwork, FederationPreviewError> {
    let network = if let Some((_, raw)) = module_config(config, WALLET_V2_MODULE_KIND) {
        WalletV2ClientConfig::consensus_decode_whole(raw, &ModuleDecoderRegistry::default())
            .map_err(|error| {
                FederationPreviewError::InvalidInviteCode(format!(
                    "walletv2 module config could not be decoded: {error}"
                ))
            })?
            .network
    } else if let Some((_, raw)) = module_config(config, WALLET_MODULE_KIND) {
        WalletClientConfig::consensus_decode_whole(raw, &ModuleDecoderRegistry::default())
            .map_err(|error| {
                FederationPreviewError::InvalidInviteCode(format!(
                    "wallet module config could not be decoded: {error}"
                ))
            })?
            .network
            .0
    } else {
        return Err(FederationPreviewError::InvalidInviteCode(
            "federation config has no wallet or walletv2 module".to_owned(),
        ));
    };

    match network {
        bitcoin::Network::Bitcoin => Ok(BitcoinNetwork::Bitcoin),
        bitcoin::Network::Testnet => Ok(BitcoinNetwork::Testnet),
        bitcoin::Network::Signet => Ok(BitcoinNetwork::Signet),
        bitcoin::Network::Regtest => Ok(BitcoinNetwork::Regtest),
        other => Err(FederationPreviewError::InvalidInviteCode(format!(
            "federation runs on an unsupported Bitcoin network: {other}"
        ))),
    }
}

/// Read the raw `meta` module consensus value with its consensus revision.
///
/// The directory lives in the `meta` module's consensus state, not in the
/// client config's `global.meta`: that is what lets the config hash the
/// guardians signed survive the FI's write. The module holds a JSON object of
/// meta fields under [`DEFAULT_META_KEY`], matching Fedimint's own
/// `ClientConfig::meta` convention. The revision comes from the same
/// `get_consensus` response as the bytes, so the pair names exactly one
/// consensus occurrence.
async fn read_meta_value(
    api: &DynGlobalApi,
    config: &ClientConfig,
) -> Result<Option<(u64, Vec<u8>)>, FederationPreviewError> {
    let Some((meta_id, _)) = module_config(config, META_MODULE_KIND) else {
        return Ok(None);
    };
    let Some(consensus) = api
        .with_module(meta_id)
        .get_consensus(DEFAULT_META_KEY)
        .await
        .map_err(|error| {
            FederationPreviewError::Unavailable(format!(
                "could not read consensus metadata: {error}"
            ))
        })?
    else {
        return Ok(None);
    };

    Ok(Some((
        consensus.revision,
        consensus.value.as_slice().to_vec(),
    )))
}

/// Extract the `fedi:fman_seat_bindings` string from a raw meta object.
///
/// `Ok(None)` means the federation published no directory. That is a
/// verification failure for the caller to map, not a preview failure.
///
/// # Errors
///
/// Returns an error if the metadata is not a JSON object or the directory
/// field is present but not a string.
pub fn seat_bindings_field(
    meta_value: Option<&[u8]>,
) -> Result<Option<String>, FederationPreviewError> {
    let Some(meta_value) = meta_value else {
        return Ok(None);
    };
    let fields: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(meta_value).map_err(|error| {
            FederationPreviewError::InvalidInviteCode(format!(
                "consensus metadata is not a JSON object: {error}"
            ))
        })?;

    match fields.get(FMAN_SEAT_BINDINGS_META_FIELD_KEY) {
        None => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(FederationPreviewError::InvalidInviteCode(format!(
            "{FMAN_SEAT_BINDINGS_META_FIELD_KEY} consensus metadata is not a string"
        ))),
    }
}
