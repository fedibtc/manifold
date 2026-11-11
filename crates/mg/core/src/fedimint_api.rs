//! Native Fedimint API adapter for one seat's local `fedimintd`, and for the
//! one public read the daemon makes of that seat's federation peers
//! ([`FedimintApi::federation_session_count`]).
//!
//! The seat lifecycle owns state interpretation and error policy; request
//! encoding, endpoint routing, authentication envelopes, and transports are
//! delegated to Fedimint's `DynGlobalApi` and module API extension traits.

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use fedimint_api_client::api::{DynGlobalApi, FederationApiExt as _, FederationError, ServerError};
use fedimint_connectors::ConnectorRegistry;
use fedimint_core::config::ClientConfig;
use fedimint_core::core::ModuleInstanceId;
use fedimint_core::endpoint_constants::{
    CLIENT_CONFIG_ENDPOINT, SESSION_COUNT_ENDPOINT, SHUTDOWN_AFTER_CURRENT_SESSION_ENDPOINT,
};
use fedimint_core::module::{ApiAuth, ApiRequestErased};
use fedimint_core::peer_availability::ResetPeerAvailabilityRequest;
use fedimint_core::util::SafeUrl;
use fedimint_core::{NumPeersExt as _, PeerId};
use fedimint_lnv2_common::endpoint_constants::ADD_GATEWAY_ENDPOINT;
use fedimint_meta_client::api::MetaFederationApi as _;
use fedimint_meta_common::{MetaConsensusValue, MetaKey, MetaValue};
use thiserror::Error;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum FedimintApiError {
    /// Nothing is serving on the seat's API port (child down, still booting,
    /// or inside the DKG API gap).
    #[error("fedimintd unreachable: {0}")]
    Unreachable(String),

    /// The serving fedimintd rejected the request.
    #[error("fedimintd rejected {method}: {message}")]
    Rejected { method: String, message: String },

    /// The server answered something the native client could not decode.
    #[error("fedimintd returned an invalid {method} response: {detail}")]
    InvalidResponse { method: String, detail: String },
}

/// Why the federation's session count could not be learned from its peers;
/// a fixed code, never peer text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerQueryFailure {
    /// The seat's own config could not be read to find its peers.
    LocalConfig,
    /// Most peers that did not answer timed out.
    Timeout,
    /// Most peers that did not answer could not be connected to.
    Unreachable,
    /// Most peers that did not answer refused the request or answered
    /// something that is not a session count.
    Rejected,
}

impl PeerQueryFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalConfig => "local_config",
            Self::Timeout => "timeout",
            Self::Unreachable => "unreachable",
            Self::Rejected => "rejected",
        }
    }
}

/// Fewer peers answered than the count needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerSessionCountUnknown {
    pub failure: PeerQueryFailure,
    pub answered: usize,
    pub required: usize,
}

/// Native-client adapter for one local fedimintd, carrying its durable setup auth.
#[derive(Clone)]
pub struct FedimintApi {
    api: DynGlobalApi,
    auth: ApiAuth,
    connectors: ConnectorRegistry,
}

impl FedimintApi {
    pub fn new(connectors: ConnectorRegistry, api_port: u16, api_auth: &str) -> Self {
        let url = SafeUrl::parse(&format!("ws://127.0.0.1:{api_port}"))
            .expect("a loopback URL with a u16 port is valid");
        let api = DynGlobalApi::new_admin_setup(connectors.clone(), url)
            .expect("the local setup API endpoint is valid");
        Self {
            api,
            auth: ApiAuth::new(api_auth.to_owned()),
            connectors,
        }
    }

    async fn request<T>(
        &self,
        method: &'static str,
        request: impl Future<Output = Result<T, FederationError>>,
    ) -> Result<T, FedimintApiError> {
        tokio::time::timeout(REQUEST_TIMEOUT, request)
            .await
            .map_err(|_| FedimintApiError::Unreachable(format!("{method} request timed out")))?
            .map_err(map_federation_error)
    }

    /// Confirm that the formed federation's consensus API is serving, and
    /// return the seat's own session count once consensus runs.
    pub async fn probe(&self) -> Result<Option<u64>, FedimintApiError> {
        match tokio::time::timeout(REQUEST_TIMEOUT, self.api.clone().status()).await {
            Ok(Ok(status)) => Ok(status.federation.map(|federation| federation.session_count)),
            Ok(Err(error)) => Err(map_federation_error(error)),
            Err(_) => Err(FedimintApiError::Unreachable(
                "status request timed out".to_owned(),
            )),
        }
    }

    pub async fn invite_code(&self) -> Result<String, FedimintApiError> {
        self.request(
            "invite_code",
            self.api
                .clone()
                .request_admin_no_auth("invite_code", ApiRequestErased::default()),
        )
        .await
    }

    pub async fn client_config(&self) -> Result<ClientConfig, FedimintApiError> {
        self.request(
            CLIENT_CONFIG_ENDPOINT,
            self.api
                .clone()
                .request_admin_no_auth(CLIENT_CONFIG_ENDPOINT, ApiRequestErased::default()),
        )
        .await
    }

    /// Ask the guardian to exit once its current consensus session
    /// completes. The schedule lives only in the running process.
    pub async fn shutdown_after_current_session(&self) -> Result<(), FedimintApiError> {
        self.request(
            SHUTDOWN_AFTER_CURRENT_SESSION_ENDPOINT,
            self.api
                .clone()
                .shutdown_after_current_session(self.auth.clone()),
        )
        .await
    }

    pub async fn meta_get_consensus(
        &self,
        meta_id: ModuleInstanceId,
        key: MetaKey,
    ) -> Result<Option<MetaConsensusValue>, FedimintApiError> {
        self.request(
            "get_consensus",
            self.api.clone().with_module(meta_id).get_consensus(key),
        )
        .await
    }

    pub async fn reset_peer_availability(
        &self,
        reset: ResetPeerAvailabilityRequest,
    ) -> Result<(), FedimintApiError> {
        self.request(
            "reset_peer_availability",
            self.api
                .clone()
                .reset_peer_availability(reset, self.auth.clone()),
        )
        .await
    }

    pub async fn meta_submit(
        &self,
        meta_id: ModuleInstanceId,
        key: MetaKey,
        value: MetaValue,
    ) -> Result<(), FedimintApiError> {
        self.request(
            "submit",
            self.api
                .clone()
                .with_module(meta_id)
                .submit(key, value, self.auth.clone()),
        )
        .await
        .map(|_| ())
    }

    pub async fn add_gateway(&self, gateway_api: SafeUrl) -> Result<bool, FedimintApiError> {
        let config = self.client_config().await?;
        let lnv2_id = config
            .modules
            .iter()
            .find(|(_, module)| module.kind == fedimint_lnv2_common::KIND)
            .map(|(instance_id, _)| *instance_id)
            .ok_or_else(|| FedimintApiError::InvalidResponse {
                method: CLIENT_CONFIG_ENDPOINT.to_owned(),
                detail: "federation config carries no lnv2 module".to_owned(),
            })?;
        self.request(
            ADD_GATEWAY_ENDPOINT,
            self.api.clone().with_module(lnv2_id).request_admin::<bool>(
                ADD_GATEWAY_ENDPOINT,
                ApiRequestErased::new(gateway_api),
                self.auth.clone(),
            ),
        )
        .await
    }
}

impl FedimintApi {
    /// The federation's session count as its other peers report it: the
    /// `max_evil + 1`-th highest answer, so at least one honest peer has
    /// reached it and up to `max_evil` peers cannot inflate it. Peers come
    /// from the seat's own client config; `own` is left out.
    pub async fn federation_session_count(
        &self,
        own: PeerId,
    ) -> Result<u64, PeerSessionCountUnknown> {
        let config = self
            .client_config()
            .await
            .map_err(|_| PeerSessionCountUnknown {
                failure: PeerQueryFailure::LocalConfig,
                answered: 0,
                required: 1,
            })?;
        let endpoints = &config.global.api_endpoints;
        let required = endpoints.to_num_peers().max_evil() + 1;
        let peers = endpoints
            .iter()
            .filter(|(peer, _)| **peer != own)
            .map(|(peer, url)| (*peer, url.url.clone()))
            .collect::<BTreeMap<_, _>>();
        let unknown = |failure| PeerSessionCountUnknown {
            failure,
            answered: 0,
            required,
        };
        let api = DynGlobalApi::new(self.connectors.clone(), peers.clone(), None)
            .map_err(|_| unknown(PeerQueryFailure::LocalConfig))?;
        let mut queries = tokio::task::JoinSet::new();
        for peer in peers.into_keys() {
            let api = api.clone();
            queries.spawn(async move {
                match tokio::time::timeout(
                    REQUEST_TIMEOUT,
                    api.request_single_peer::<u64>(
                        SESSION_COUNT_ENDPOINT.to_owned(),
                        ApiRequestErased::default(),
                        peer,
                    ),
                )
                .await
                {
                    Ok(Ok(count)) => Ok(count),
                    Err(_) => Err(PeerQueryFailure::Timeout),
                    Ok(Err(
                        ServerError::Connection(_)
                        | ServerError::Transport(_)
                        | ServerError::InternalClientError(_),
                    )) => Err(PeerQueryFailure::Unreachable),
                    Ok(Err(_)) => Err(PeerQueryFailure::Rejected),
                }
            });
        }
        // Each query ends within its timeout, so this ends within one.
        let answers = queries.join_all().await;
        quorum_session_count(&answers, required)
    }
}

/// The `required`-th highest of the peers' answers, or why too few answered:
/// the failure most peers hit, the first in [`PeerQueryFailure`] order on a
/// tie.
fn quorum_session_count(
    answers: &[Result<u64, PeerQueryFailure>],
    required: usize,
) -> Result<u64, PeerSessionCountUnknown> {
    let mut counts = answers
        .iter()
        .filter_map(|answer| answer.ok())
        .collect::<Vec<_>>();
    counts.sort_unstable_by(|a, b| b.cmp(a));
    if let Some(count) = counts.get(required - 1) {
        return Ok(*count);
    }
    let failure = [
        PeerQueryFailure::Timeout,
        PeerQueryFailure::Unreachable,
        PeerQueryFailure::Rejected,
    ]
    .into_iter()
    .max_by_key(|failure| {
        let hits = answers
            .iter()
            .filter(|answer| answer.err() == Some(*failure))
            .count();
        // `max_by_key` keeps the last maximum; rank earlier kinds higher on a tie.
        (hits, std::cmp::Reverse(*failure as u8))
    })
    .expect("the list is not empty");
    Err(PeerSessionCountUnknown {
        failure,
        answered: counts.len(),
        required,
    })
}

fn map_federation_error(error: FederationError) -> FedimintApiError {
    let method = error.method.clone();
    if let Some((
        _,
        ServerError::ResponseDeserialization(detail) | ServerError::InvalidResponse(detail),
    )) = error.get_peer_errors().find(|(_, error)| {
        matches!(
            error,
            ServerError::ResponseDeserialization(_) | ServerError::InvalidResponse(_)
        )
    }) {
        return FedimintApiError::InvalidResponse {
            method,
            detail: detail.to_string(),
        };
    }

    let peer_errors = error.get_peer_errors().collect::<Vec<_>>();
    if !peer_errors.is_empty()
        && peer_errors.iter().all(|(_, error)| {
            matches!(
                error,
                ServerError::Connection(_)
                    | ServerError::Transport(_)
                    | ServerError::InternalClientError(_)
            )
        })
    {
        return FedimintApiError::Unreachable(peer_errors.first().map_or_else(
            || "connection failed".to_owned(),
            |(_, error)| error.to_string(),
        ));
    }

    let message = peer_errors
        .first()
        .map(|(_, error)| error.to_string())
        .or_else(|| error.get_general_error().map(ToString::to_string))
        .unwrap_or_else(|| "request failed".to_owned());
    FedimintApiError::Rejected { method, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMEOUT: Result<u64, PeerQueryFailure> = Err(PeerQueryFailure::Timeout);
    const UNREACHABLE: Result<u64, PeerQueryFailure> = Err(PeerQueryFailure::Unreachable);
    const REJECTED: Result<u64, PeerQueryFailure> = Err(PeerQueryFailure::Rejected);

    /// With four guardians one may lie, so the count is the second highest
    /// of the other three: a single inflated answer cannot fake a lag.
    #[test]
    fn one_inflated_peer_cannot_raise_the_federation_count() {
        assert_eq!(quorum_session_count(&[Ok(1000), Ok(90), Ok(91)], 2), Ok(91));
        assert_eq!(
            quorum_session_count(&[Ok(91), Ok(1000), TIMEOUT], 2),
            Ok(91)
        );
        // Seven guardians tolerate two liars: the third highest.
        assert_eq!(
            quorum_session_count(&[Ok(9), Ok(9), Ok(5), Ok(4), Ok(3), Ok(2)], 3),
            Ok(5)
        );
    }

    #[test]
    fn too_few_answers_name_the_failure_most_peers_hit() {
        assert_eq!(
            quorum_session_count(&[Ok(90), UNREACHABLE, UNREACHABLE], 2),
            Err(PeerSessionCountUnknown {
                failure: PeerQueryFailure::Unreachable,
                answered: 1,
                required: 2,
            })
        );
        assert_eq!(
            quorum_session_count(&[REJECTED, TIMEOUT, REJECTED], 2),
            Err(PeerSessionCountUnknown {
                failure: PeerQueryFailure::Rejected,
                answered: 0,
                required: 2,
            })
        );
        assert_eq!(
            quorum_session_count(&[REJECTED, TIMEOUT, Ok(3)], 2)
                .unwrap_err()
                .failure,
            PeerQueryFailure::Timeout,
            "a tie names the earlier kind"
        );
    }
}
