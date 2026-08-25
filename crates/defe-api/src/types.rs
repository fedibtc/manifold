use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A request sent by a client to the local `defe` server.
///
/// Serialized as an externally tagged CBOR map with PascalCase variant names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Request {
    /// Check that the server is reachable and can decode requests.
    Ping,
    /// Allocate a new resource lease matching the supplied resource request.
    Allocate(ResourceRequest),
    /// Release a resource handle previously returned to this client connection.
    Release(ResourceHandleId),
    /// Restart a leased resource and return an updated descriptor for it.
    Restart {
        /// Connection-local resource handle to restart.
        handle_id: ResourceHandleId,
        /// Restart policy controlling whether a running resource may be stopped.
        mode: RestartMode,
    },
}

/// Resource allocation requests supported by the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ResourceRequest {
    /// Allocate a local Nostr relay process.
    NostrRelay(NostrRelayRequest),
    /// Allocate a local push gateway HTTP server process.
    PushGateway(PushGatewayRequest),
    /// Allocate a local Bitcoin Core regtest node.
    Bitcoind(BitcoindRequest),
    /// Allocate one local Fleet Manager process.
    Fman(FmanRequest),
    /// Allocate one local FLIP daemon process.
    Flip(FlipRequest),
    /// Allocate one local Fedimint gateway daemon process.
    Gatewayd(GatewaydRequest),
}

/// Inputs required to start one local Fleet Manager resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FmanRequest {
    /// Whether Defe may reuse an identical manager slot.
    pub sharing: SharingMode,
    /// Regtest Bitcoin Core instance the Fleet Manager uses for federation setup.
    ///
    /// This is a non-owning launch dependency. The client must keep its lease
    /// alive for the FMan resource lifetime.
    pub bitcoind: BitcoindInfo,
    /// Nostr relay on which the Fleet Manager publishes its advertisement.
    ///
    /// This is a non-owning launch dependency. The client must keep its lease
    /// alive for the FMan resource lifetime.
    pub nostr_relay_url: String,
    /// First port in the manager's dedicated four-port federation-seat grid.
    pub first_port_base: u16,
    /// Complete direct Iroh route map used by this formation's managers and FI.
    pub iroh_connect_overrides: String,
}

/// Inputs required to start one local FLIP stack.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FlipRequest {
    /// Whether Defe may reuse an identically configured FLIP daemon slot.
    pub sharing: SharingMode,
    /// Optional direct Iroh route map for a locally formed federation.
    pub iroh_connect_overrides: Option<String>,
}

/// Inputs required to start one local Fedimint gateway daemon.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GatewaydRequest {
    /// Whether Defe may reuse an identical gateway slot.
    pub sharing: SharingMode,
    /// Regtest Bitcoin Core instance used by the gateway.
    ///
    /// This is a non-owning launch dependency. The client must keep its lease
    /// alive for the gateway resource lifetime.
    pub bitcoind: BitcoindInfo,
    /// Optional direct Iroh route map for a locally formed federation.
    pub iroh_connect_overrides: Option<String>,
}
/// Request parameters for a Nostr relay resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NostrRelayRequest {
    /// Whether the relay may be shared with other compatible allocations.
    pub sharing: SharingMode,
}

impl NostrRelayRequest {
    /// Build a request for the default shared Nostr relay resource.
    #[must_use]
    pub const fn shared() -> Self {
        Self {
            sharing: SharingMode::Shared,
        }
    }

    /// Build a request for a private Nostr relay resource.
    #[must_use]
    pub const fn exclusive() -> Self {
        Self {
            sharing: SharingMode::Exclusive,
        }
    }
}

/// Request parameters for a push gateway resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PushGatewayRequest {
    /// Whether the gateway may be shared with other compatible allocations.
    pub sharing: SharingMode,
}

impl PushGatewayRequest {
    /// Build a request for the default shared push gateway resource.
    #[must_use]
    pub const fn shared() -> Self {
        Self {
            sharing: SharingMode::Shared,
        }
    }

    /// Build a request for a private push gateway resource.
    #[must_use]
    pub const fn exclusive() -> Self {
        Self {
            sharing: SharingMode::Exclusive,
        }
    }
}

/// Request parameters for a Bitcoin Core regtest resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BitcoindRequest {
    /// Whether the node may be shared with other compatible allocations.
    pub sharing: SharingMode,
}

impl BitcoindRequest {
    /// Build a request for the default shared regtest Bitcoin Core resource.
    #[must_use]
    pub const fn shared() -> Self {
        Self {
            sharing: SharingMode::Shared,
        }
    }

    /// Build a request for a private regtest Bitcoin Core resource.
    #[must_use]
    pub const fn exclusive() -> Self {
        Self {
            sharing: SharingMode::Exclusive,
        }
    }
}

/// Whether a resource can be reused by compatible requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SharingMode {
    /// Reuse an existing compatible resource when one is available.
    Shared,
    /// Allocate a resource slot dedicated to this lease.
    Exclusive,
}

/// Client-requested restart behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum RestartMode {
    /// Restart only when the underlying resource process has already exited.
    IfExited,
    /// Stop the resource if needed and start a fresh generation.
    Force,
}

/// A resource handle scoped to one client connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceHandleId(
    /// Numeric handle value assigned by the server for one client connection.
    pub u64,
);

/// A response sent by the local `defe` server.
///
/// Serialized as an externally tagged CBOR map with PascalCase variant names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Response {
    /// Successful response to [`Request::Ping`].
    Pong,
    /// Successful response containing a resource lease.
    Resource(ResourceLease),
    /// Successful response to [`Request::Release`].
    Released,
    /// Structured error response for a request the server could not fulfill.
    Error(ApiError),
}

/// An active resource lease owned by the client connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResourceLease {
    /// Connection-local handle used for later release or restart operations.
    pub handle_id: ResourceHandleId,
    /// Resource-specific information needed by the test process.
    pub descriptor: ResourceDescriptor,
}

/// Resource information returned to clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ResourceDescriptor {
    /// Connection details for a local Nostr relay.
    NostrRelay(NostrRelayInfo),
    /// Connection details for a local push gateway.
    PushGateway(PushGatewayInfo),
    /// Connection details for a local Bitcoin Core regtest node.
    Bitcoind(BitcoindInfo),
    /// Connection details for one local Fleet Manager.
    Fman(FmanInfo),
    /// Connection details for one local FLIP stack.
    Flip(FlipInfo),
    /// Connection details for one local Fedimint gateway daemon.
    Gatewayd(GatewaydInfo),
}

/// Connection details for a local Fleet Manager resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FmanInfo {
    /// Locator passed to FI formation.
    pub locator: String,
    /// Persistent data directory for this manager slot.
    pub data_dir: PathBuf,
    /// Direct Iroh routes for the manager's prospective federation seat.
    pub iroh_connect_overrides: String,
    /// Browser-facing operator API URL (`SPEC-operator-http`).
    pub admin_url: String,
    /// Operator password accepted by that API's `POST /api/auth`.
    pub admin_password: String,
}

/// Connection details for one local FLIP stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FlipInfo {
    /// FLIP administrative API URL.
    pub admin_url: String,
    /// Bootstrap token accepted by the administrative API.
    pub admin_token: String,
    /// Persistent FLIP data directory.
    pub data_dir: PathBuf,
    /// Directory from which FLIP reads test trust fixtures.
    pub trust_fixtures_dir: PathBuf,
    /// Provider Nostr public key imported by the FLIP daemon.
    pub provider_pubkey_hex: String,
}

/// Connection details for one local Fedimint gateway daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GatewaydInfo {
    /// Gateway administrative API URL.
    pub api_url: String,
    /// Gateway administrative credential.
    pub password: String,
}
/// Connection details for a local Nostr relay resource.
///
/// `data_dir` is serialized with Rust's `PathBuf` serde support. The local
/// protocol is intended for same-version clients and servers on the same
/// machine; paths are expected to be valid UTF-8 for portable CBOR exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NostrRelayInfo {
    /// WebSocket URL clients can use to connect to the relay.
    pub url: String,
    /// Host name or IP address on which the relay is listening.
    pub host: String,
    /// TCP port on which the relay is listening.
    pub port: u16,
    /// Persistent data directory used by this relay slot.
    pub data_dir: PathBuf,
}

/// Connection details for a local push gateway resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PushGatewayInfo {
    /// Base HTTP URL clients can use to connect to the gateway.
    pub url: String,
    /// Host name or IP address on which the gateway is listening.
    pub host: String,
    /// TCP port on which the gateway is listening.
    pub port: u16,
    /// App id configured on this gateway instance.
    pub app_id: String,
    /// SQLite database path used by this gateway slot.
    pub database_path: PathBuf,
}

/// Connection details for a local Bitcoin Core regtest resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BitcoindInfo {
    /// HTTP JSON-RPC URL clients can use to connect to bitcoind.
    pub rpc_url: String,
    /// Host name or IP address on which bitcoind RPC is listening.
    pub rpc_host: String,
    /// TCP port on which bitcoind RPC is listening.
    pub rpc_port: u16,
    /// TCP port on which bitcoind's regtest P2P listener is bound.
    pub p2p_port: u16,
    /// RPC username configured for this node.
    pub rpc_username: String,
    /// RPC password configured for this node.
    pub rpc_password: String,
    /// Persistent data directory used by this bitcoind slot.
    pub data_dir: PathBuf,
}

/// Structured API error returned in [`Response::Error`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ApiError {
    /// Stable error category suitable for client-side matching.
    pub kind: ApiErrorKind,
    /// Human-readable explanation intended for logs and test failures.
    pub message: String,
}

impl ApiError {
    /// Create a new API error with a stable kind and human-readable message.
    #[must_use]
    pub fn new(kind: ApiErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// Stable error categories clients can match on without string parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ApiErrorKind {
    /// The request is syntactically valid but semantically unsupported.
    InvalidRequest,
    /// The resource handle is not known to the current connection.
    UnknownHandle,
    /// Reserved for a future protocol where handles can be globally visible but
    /// rejected when they are not owned by the current client connection.
    HandleNotOwned,
    /// No driver is available for the requested resource kind.
    ResourceKindUnavailable,
    /// The server could not start the requested resource.
    ResourceStartFailed,
    /// The requested restart policy refused to restart the resource.
    ResourceRestartRefused,
    /// The server could not decode the client's request frame.
    ProtocolDecodeError,
    /// An unexpected server-side invariant failed.
    InternalServerError,
}
