//! Native Fedimint API adapter for one seat's local `fedimintd`.
//!
//! The seat lifecycle owns state interpretation and error policy; request
//! encoding, endpoint routing, authentication envelopes, and transports are
//! delegated to Fedimint's `DynGlobalApi` and module API extension traits.

use std::future::Future;
use std::time::Duration;

use fedimint_api_client::api::{DynGlobalApi, FederationApiExt as _, FederationError, ServerError};
use fedimint_connectors::ConnectorRegistry;
use fedimint_core::config::ClientConfig;
use fedimint_core::core::ModuleInstanceId;
use fedimint_core::endpoint_constants::CLIENT_CONFIG_ENDPOINT;
use fedimint_core::module::{ApiAuth, ApiRequestErased};
use fedimint_core::util::SafeUrl;
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

/// Native-client adapter for one local fedimintd, carrying its durable setup auth.
#[derive(Clone)]
pub struct FedimintApi {
    api: DynGlobalApi,
    auth: ApiAuth,
}

impl FedimintApi {
    pub fn new(connectors: ConnectorRegistry, api_port: u16, api_auth: &str) -> Self {
        let url = SafeUrl::parse(&format!("ws://127.0.0.1:{api_port}"))
            .expect("a loopback URL with a u16 port is valid");
        let api = DynGlobalApi::new_admin_setup(connectors, url)
            .expect("the local setup API endpoint is valid");
        Self {
            api,
            auth: ApiAuth::new(api_auth.to_owned()),
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

    /// Confirm that the formed federation's consensus API is serving.
    pub async fn probe(&self) -> Result<(), FedimintApiError> {
        match tokio::time::timeout(REQUEST_TIMEOUT, self.api.clone().status()).await {
            Ok(Ok(_)) => Ok(()),
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
