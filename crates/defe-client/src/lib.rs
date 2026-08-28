//! Client library for talking to a local `defe` server.
//!
//! The crate exposes [`AsyncDefeClient`] for async code.
//!
//! Tests normally create a client with [`AsyncDefeClient::connect_from_env`] while running under
//! `defe exec <cmd...>` or with a
//! persistent development server socket exported in the environment.
//!
//! Resource allocation methods return a [`ResourceLease`] containing a connection-local
//! [`ResourceHandleId`]. Dropping the lease value in the client process does **not** release the
//! remote resource; call `release`, or drop the client connection to let the server clean up all
//! handles owned by that connection. If an async request future is cancelled mid-request, drop the
//! async client instead of reusing it: the request/response stream may be out of sync.

use std::env;
use std::fmt;
use std::os::fd::{AsFd as _, OwnedFd};
use std::path::PathBuf;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream as TokioUnixStream;

pub use defe_api::{
    ApiError, ApiErrorKind, DEV_DEFE_SOCKET_PATH, FlipInfo, FlipRequest, FmanInfo, FmanRequest,
    FrameError, GatewaydInfo, GatewaydRequest, MAX_FRAME_SIZE, NostrRelayInfo, NostrRelayRequest,
    PushGatewayInfo, PushGatewayRequest, Request, ResourceDescriptor, ResourceHandleId,
    ResourceLease, ResourceRequest, Response, RestartMode, SharingMode,
};
pub use defe_api::{BitcoindInfo, BitcoindRequest};

/// Async client for a local `defe` server started by `defe exec <cmd...>` or a persistent dev server.
#[derive(Debug)]
pub struct AsyncDefeClient {
    /// Connected Unix socket used for async request/response RPCs.
    stream: TokioUnixStream,
}

impl AsyncDefeClient {
    /// Duplicates the connection solely to keep its server-side leases alive.
    ///
    /// The holder must never read from or write to this descriptor because the
    /// active client owns the sequential request/response protocol.
    pub fn duplicate_lifetime_guard(&self) -> std::io::Result<OwnedFd> {
        self.stream.as_fd().try_clone_to_owned()
    }

    /// Connect to the `defe` server named by [`DEV_DEFE_SOCKET_PATH`].
    pub async fn connect_from_env() -> Result<Self, DefeClientError> {
        let socket_path = env::var_os(DEV_DEFE_SOCKET_PATH).ok_or(DefeClientError::MissingEnv)?;
        if socket_path.is_empty() {
            return Err(DefeClientError::EmptyEnv);
        }

        Self::connect(socket_path).await
    }

    /// Connect to a `defe` server at an explicit Unix socket path.
    pub async fn connect(socket_path: impl Into<PathBuf>) -> Result<Self, DefeClientError> {
        let socket_path = socket_path.into();
        let stream = TokioUnixStream::connect(&socket_path)
            .await
            .map_err(|source| DefeClientError::Connect {
                socket_path,
                source,
            })?;
        Ok(Self { stream })
    }

    /// Send a raw RPC request and return the raw response.
    pub async fn request(&mut self, request: &Request) -> Result<Response, DefeClientError> {
        async_write_request(&mut self.stream, request).await?;
        async_read_frame::<Response>(&mut self.stream).await
    }

    /// Send a ping request and expect a pong response.
    pub async fn ping(&mut self) -> Result<(), DefeClientError> {
        match self.request(&Request::Ping).await? {
            Response::Pong => Ok(()),
            Response::Error(err) => Err(DefeClientError::Server(err)),
            response => Err(DefeClientError::UnexpectedResponse {
                operation: "ping",
                response: Box::new(response),
            }),
        }
    }

    /// Allocate a resource through the server.
    pub async fn allocate(
        &mut self,
        request: ResourceRequest,
    ) -> Result<ResourceLease, DefeClientError> {
        match self.request(&Request::Allocate(request)).await? {
            Response::Resource(lease) => Ok(lease),
            Response::Error(err) => Err(DefeClientError::Server(err)),
            response => Err(DefeClientError::UnexpectedResponse {
                operation: "allocate",
                response: Box::new(response),
            }),
        }
    }

    /// Allocate a Nostr relay resource with the requested sharing mode.
    pub async fn request_nostr_relay(
        &mut self,
        sharing: SharingMode,
    ) -> Result<ResourceLease, DefeClientError> {
        let lease = self
            .allocate(ResourceRequest::NostrRelay(NostrRelayRequest { sharing }))
            .await?;
        match lease.descriptor {
            ResourceDescriptor::NostrRelay(_) => Ok(lease),
            descriptor => Err(DefeClientError::UnexpectedDescriptor {
                operation: "request_nostr_relay",
                descriptor: Box::new(descriptor),
            }),
        }
    }

    /// Allocate a push gateway resource with the requested sharing mode.
    pub async fn request_push_gateway(
        &mut self,
        sharing: SharingMode,
    ) -> Result<ResourceLease, DefeClientError> {
        let lease = self
            .allocate(ResourceRequest::PushGateway(PushGatewayRequest { sharing }))
            .await?;
        match lease.descriptor {
            ResourceDescriptor::PushGateway(_) => Ok(lease),
            descriptor => Err(DefeClientError::UnexpectedDescriptor {
                operation: "request_push_gateway",
                descriptor: Box::new(descriptor),
            }),
        }
    }

    /// Allocate a Bitcoin Core regtest resource with the requested sharing
    /// mode.
    pub async fn request_bitcoind(
        &mut self,
        sharing: SharingMode,
    ) -> Result<ResourceLease, DefeClientError> {
        let lease = self
            .allocate(ResourceRequest::Bitcoind(BitcoindRequest { sharing }))
            .await?;
        match lease.descriptor {
            ResourceDescriptor::Bitcoind(_) => Ok(lease),
            descriptor => Err(DefeClientError::UnexpectedDescriptor {
                operation: "request_bitcoind",
                descriptor: Box::new(descriptor),
            }),
        }
    }

    /// Allocate one exclusive Fleet Manager resource.
    pub async fn request_fman(
        &mut self,
        request: FmanRequest,
    ) -> Result<ResourceLease, DefeClientError> {
        let lease = self.allocate(ResourceRequest::Fman(request)).await?;
        match lease.descriptor {
            ResourceDescriptor::Fman(_) => Ok(lease),
            descriptor => Err(DefeClientError::UnexpectedDescriptor {
                operation: "request_fman",
                descriptor: Box::new(descriptor),
            }),
        }
    }

    /// Allocate one exclusive FLIP stack.
    pub async fn request_flip(
        &mut self,
        request: FlipRequest,
    ) -> Result<ResourceLease, DefeClientError> {
        let lease = self.allocate(ResourceRequest::Flip(request)).await?;
        match lease.descriptor {
            ResourceDescriptor::Flip(_) => Ok(lease),
            descriptor => Err(DefeClientError::UnexpectedDescriptor {
                operation: "request_flip",
                descriptor: Box::new(descriptor),
            }),
        }
    }

    /// Allocate one local Fedimint gateway daemon resource.
    pub async fn request_gatewayd(
        &mut self,
        request: GatewaydRequest,
    ) -> Result<ResourceLease, DefeClientError> {
        let lease = self.allocate(ResourceRequest::Gatewayd(request)).await?;
        match lease.descriptor {
            ResourceDescriptor::Gatewayd(_) => Ok(lease),
            descriptor => Err(DefeClientError::UnexpectedDescriptor {
                operation: "request_gatewayd",
                descriptor: Box::new(descriptor),
            }),
        }
    }

    /// Release a resource handle owned by this client connection.
    pub async fn release(&mut self, handle_id: ResourceHandleId) -> Result<(), DefeClientError> {
        match self.request(&Request::Release(handle_id)).await? {
            Response::Released => Ok(()),
            Response::Error(err) => Err(DefeClientError::Server(err)),
            response => Err(DefeClientError::UnexpectedResponse {
                operation: "release",
                response: Box::new(response),
            }),
        }
    }

    /// Restart a resource handle owned by this client connection.
    pub async fn restart(
        &mut self,
        handle_id: ResourceHandleId,
        mode: RestartMode,
    ) -> Result<ResourceLease, DefeClientError> {
        match self.request(&Request::Restart { handle_id, mode }).await? {
            Response::Resource(lease) if lease.handle_id == handle_id => Ok(lease),
            Response::Resource(lease) => Err(DefeClientError::RestartHandleMismatch {
                requested: handle_id,
                returned: lease.handle_id,
            }),
            Response::Error(err) => Err(DefeClientError::Server(err)),
            response => Err(DefeClientError::UnexpectedResponse {
                operation: "restart",
                response: Box::new(response),
            }),
        }
    }
}

/// Errors returned by [`AsyncDefeClient`].
#[derive(thiserror::Error)]
pub enum DefeClientError {
    /// The client process has no environment variable pointing at a `defe` socket.
    #[error(
        "{DEV_DEFE_SOCKET_PATH} is not set. This client expects being run as a child of a `defe` process; run it inside `defe exec <cmd...>` or set {DEV_DEFE_SOCKET_PATH} to a defe server Unix socket path."
    )]
    MissingEnv,

    /// The client process has an empty socket path environment variable.
    #[error(
        "{DEV_DEFE_SOCKET_PATH} is set but empty. Set it to a defe server Unix socket path, or run the client inside `defe exec <cmd...>`."
    )]
    EmptyEnv,

    /// Opening the configured Unix socket failed.
    #[error("failed to connect to defe socket {}: {source}", socket_path.display())]
    Connect {
        /// Socket path the client attempted to connect to.
        socket_path: PathBuf,
        /// Underlying operating-system connection error.
        #[source]
        source: std::io::Error,
    },

    /// Writing the encoded request frame to the socket failed.
    #[error("failed to write request: {0}")]
    Write(#[source] std::io::Error),

    /// Reading the four-byte response frame length failed.
    #[error("failed to read frame length: {0}")]
    ReadFrameLength(#[source] std::io::Error),

    /// Reading the response frame payload failed.
    #[error("failed to read frame payload: {0}")]
    ReadFramePayload(#[source] std::io::Error),

    /// The server declared a response payload larger than the client accepts.
    #[error("frame payload is too large: {payload_len} bytes exceeds {MAX_FRAME_SIZE} byte limit")]
    FrameTooLarge {
        /// Declared response payload size in bytes.
        payload_len: usize,
    },

    /// The response frame was malformed or could not be decoded.
    #[error(transparent)]
    Frame(#[from] FrameError),

    /// The server returned a structured API error response.
    #[error("server returned error {:?}: {}", .0.kind, .0.message)]
    Server(ApiError),

    /// The restart response contained a lease for a different handle than requested.
    #[error("restart returned handle {returned:?}, but requested {requested:?}")]
    RestartHandleMismatch {
        /// Handle id sent in the restart request.
        requested: ResourceHandleId,
        /// Handle id returned in the restart lease.
        returned: ResourceHandleId,
    },

    /// The server returned a resource descriptor that does not match the client operation.
    #[error("unexpected {operation} resource descriptor: {descriptor:?}")]
    UnexpectedDescriptor {
        /// High-level client operation that was in progress.
        operation: &'static str,
        /// Descriptor variant received from the server.
        descriptor: Box<ResourceDescriptor>,
    },

    /// The server returned a well-formed response variant that does not match the request.
    #[error("unexpected {operation} response: {response:?}")]
    UnexpectedResponse {
        /// High-level client operation that was in progress.
        operation: &'static str,
        /// Response variant received from the server.
        response: Box<Response>,
    },
}

impl fmt::Debug for DefeClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

async fn async_write_request(
    stream: &mut TokioUnixStream,
    request: &Request,
) -> Result<(), DefeClientError> {
    let frame = defe_api::encode_frame(request)?;
    stream
        .write_all(&frame)
        .await
        .map_err(DefeClientError::Write)
}

async fn async_read_frame<T>(stream: &mut TokioUnixStream) -> Result<T, DefeClientError>
where
    T: serde::de::DeserializeOwned,
{
    let mut len_buf = [0_u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .map_err(DefeClientError::ReadFrameLength)?;

    let payload_len = u32::from_be_bytes(len_buf) as usize;
    if MAX_FRAME_SIZE < payload_len {
        return Err(DefeClientError::FrameTooLarge { payload_len });
    }

    let mut frame = Vec::with_capacity(4 + payload_len);
    frame.extend_from_slice(&len_buf);
    frame.resize(4 + payload_len, 0);
    stream
        .read_exact(&mut frame[4..])
        .await
        .map_err(DefeClientError::ReadFramePayload)?;

    defe_api::decode_frame(&frame).map_err(DefeClientError::Frame)
}
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
