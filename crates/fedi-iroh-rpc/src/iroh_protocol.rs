use std::{
    io::{self, Write},
    sync::Arc,
    time::Duration,
};

use crate::{
    RpcConnectionContext, RpcError, RpcServiceHandler, client::decode_frame, frame::ResponseFrame,
};
use iroh::{
    endpoint::{Connection, ReadToEndError},
    protocol::{AcceptError, ProtocolHandler},
};
use tokio::sync::Semaphore;

/// Iroh protocol adapter serving RPC streams for a generated service server.
pub struct IrohProtocol<H> {
    handler: Arc<H>,
    max_request_bytes: usize,
    max_response_bytes: usize,
    request_read_timeout: Duration,
    response_write_timeout: Duration,
    stream_permits: Arc<Semaphore>,
}

impl<H> IrohProtocol<H> {
    /// Creates an iroh protocol handler with default limits.
    ///
    /// Defaults to 1 MiB request and response frames, 10-second request-read
    /// and response-write deadlines, and 128 concurrent stream tasks.
    #[must_use]
    pub fn new(handler: H) -> Self {
        Self::with_resource_limits(
            handler,
            1024 * 1024,
            1024 * 1024,
            128,
            Duration::from_secs(10),
            Duration::from_secs(10),
        )
    }

    /// Creates an iroh protocol handler with explicit request size and concurrency limits.
    ///
    /// Panics if `max_concurrent_streams` is zero.
    #[must_use]
    pub fn with_limits(
        handler: H,
        max_request_bytes: usize,
        max_concurrent_streams: usize,
    ) -> Self {
        Self::with_resource_limits(
            handler,
            max_request_bytes,
            1024 * 1024,
            max_concurrent_streams,
            Duration::from_secs(10),
            Duration::from_secs(10),
        )
    }

    /// Creates an iroh protocol handler with explicit request, concurrency, and initial-frame
    /// limits.
    ///
    /// The initial-frame deadline applies only before a complete request frame
    /// is decoded. Decoded service handlers continue through client disconnects.
    ///
    /// Panics if `max_concurrent_streams` is zero.
    #[must_use]
    pub fn with_limits_and_request_read_timeout(
        handler: H,
        max_request_bytes: usize,
        max_concurrent_streams: usize,
        request_read_timeout: Duration,
    ) -> Self {
        assert!(
            0 < max_concurrent_streams,
            "max_concurrent_streams must be nonzero"
        );
        Self::with_resource_limits(
            handler,
            max_request_bytes,
            1024 * 1024,
            max_concurrent_streams,
            request_read_timeout,
            Duration::from_secs(10),
        )
    }

    /// Creates an iroh protocol handler with explicit resource limits.
    ///
    /// `max_response_bytes` limits the complete encoded response frame.
    /// `max_concurrent_streams` accounts for each detached stream task through
    /// response completion or abandonment.
    /// A response limit too small to encode a transport error can cause the
    /// server to close a rejected stream without a response frame.
    ///
    /// Panics if `max_concurrent_streams` is zero.
    #[must_use]
    pub fn with_resource_limits(
        handler: H,
        max_request_bytes: usize,
        max_response_bytes: usize,
        max_concurrent_streams: usize,
        request_read_timeout: Duration,
        response_write_timeout: Duration,
    ) -> Self {
        assert!(
            0 < max_concurrent_streams,
            "max_concurrent_streams must be nonzero"
        );
        Self {
            handler: Arc::new(handler),
            max_request_bytes,
            max_response_bytes,
            request_read_timeout,
            response_write_timeout,
            stream_permits: Arc::new(Semaphore::new(max_concurrent_streams)),
        }
    }

    /// Creates an iroh protocol handler with a custom request size limit.
    #[must_use]
    pub fn with_max_request_bytes(handler: H, max_request_bytes: usize) -> Self {
        Self::with_limits(handler, max_request_bytes, 128)
    }
}

impl<H> std::fmt::Debug for IrohProtocol<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrohProtocol")
            .field("max_request_bytes", &self.max_request_bytes)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("request_read_timeout", &self.request_read_timeout)
            .field("response_write_timeout", &self.response_write_timeout)
            .field(
                "available_stream_permits",
                &self.stream_permits.available_permits(),
            )
            .finish_non_exhaustive()
    }
}

impl<H> Clone for IrohProtocol<H> {
    fn clone(&self) -> Self {
        Self {
            handler: Arc::clone(&self.handler),
            max_request_bytes: self.max_request_bytes,
            max_response_bytes: self.max_response_bytes,
            request_read_timeout: self.request_read_timeout,
            response_write_timeout: self.response_write_timeout,
            stream_permits: Arc::clone(&self.stream_permits),
        }
    }
}

impl<H> ProtocolHandler for IrohProtocol<H>
where
    H: RpcServiceHandler,
{
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let connection_context = RpcConnectionContext {
            remote_node_id: Some(connection.remote_id().to_string()),
        };
        loop {
            let Ok((mut send, mut recv)) = connection.accept_bi().await else {
                break;
            };

            let Ok(permit) = Arc::clone(&self.stream_permits).acquire_owned().await else {
                break;
            };
            let handler = Arc::clone(&self.handler);
            let max_request_bytes = self.max_request_bytes;
            let max_response_bytes = self.max_response_bytes;
            let request_read_timeout = self.request_read_timeout;
            let response_write_timeout = self.response_write_timeout;
            let context = connection_context.clone();
            // Deliberately detached: a handler runs to completion even if
            // the client disconnects (only the response write fails).
            // Services rely on this run-to-completion guarantee for their
            // multi-step sequences: fleet-manager's ceremony verbs assume
            // the only mid-sequence interruption is process death (see
            // `Seat` in fleet-manager). Do not abort in-flight handlers
            // on disconnect without auditing those dependents.
            tokio::spawn(async move {
                let _permit = permit;
                let response = {
                    match tokio::time::timeout(
                        request_read_timeout,
                        recv.read_to_end(max_request_bytes),
                    )
                    .await
                    {
                        Ok(Ok(bytes)) => match decode_frame(&bytes) {
                            Ok(frame) => {
                                handler
                                    .handle_rpc_with_context(context, &frame.method, &frame.body)
                                    .await
                            }
                            Err(err) => Err(err),
                        },
                        Ok(Err(err)) => Err(map_request_read_error(err)),
                        Err(_) => Err(RpcError::RequestTimedOut),
                    }
                };

                // Keep the handler's raw response only through the bounded
                // encoding attempt. In particular, drop an oversized response
                // before awaiting any fallback write.
                let response_bytes = match encode_response(response, max_response_bytes) {
                    Ok(bytes) => bytes,
                    Err(RpcError::ResponseTooLarge) => {
                        let too_large =
                            ResponseFrame::Transport(RpcError::ResponseTooLarge.to_string());
                        match encode_bounded(&too_large, max_response_bytes) {
                            Ok(bytes) => bytes,
                            Err(_) => return,
                        }
                    }
                    Err(_) => return,
                };

                let _ = tokio::time::timeout(response_write_timeout, async {
                    if send.write_all(&response_bytes).await.is_ok() {
                        let _ = send.finish();
                    }
                })
                .await;
            });
        }

        Ok(())
    }
}

fn encode_response(
    response: Result<Vec<u8>, RpcError>,
    max_bytes: usize,
) -> Result<Vec<u8>, RpcError> {
    let response_frame = match response {
        Ok(bytes) => ResponseFrame::Service(bytes),
        Err(err) => ResponseFrame::Transport(err.to_string()),
    };
    encode_bounded(&response_frame, max_bytes)
}

fn encode_bounded<T>(value: &T, max_bytes: usize) -> Result<Vec<u8>, RpcError>
where
    T: serde::Serialize,
{
    let mut writer = BoundedWriter::new(max_bytes);
    match ciborium::into_writer(value, &mut writer) {
        Ok(()) => Ok(writer.into_inner()),
        Err(_) if writer.exceeded_limit => Err(RpcError::ResponseTooLarge),
        Err(err) => Err(RpcError::Encode(err.to_string())),
    }
}

struct BoundedWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
    exceeded_limit: bool,
}

impl BoundedWriter {
    fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(max_bytes.min(8 * 1024)),
            max_bytes,
            exceeded_limit: false,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(new_len) = self.bytes.len().checked_add(buf.len()) else {
            self.exceeded_limit = true;
            return Err(io::Error::other("encoded response exceeds limit"));
        };
        if self.max_bytes < new_len {
            self.exceeded_limit = true;
            return Err(io::Error::other("encoded response exceeds limit"));
        }
        if self.bytes.capacity() < new_len {
            self.bytes
                .try_reserve_exact(new_len - self.bytes.len())
                .map_err(io::Error::other)?;
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn map_request_read_error(error: ReadToEndError) -> RpcError {
    match error {
        ReadToEndError::TooLong => RpcError::RequestTooLarge,
        ReadToEndError::Read(error) => RpcError::Iroh(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use iroh::{
        Endpoint, RelayMode,
        endpoint::{QuicTransportConfig, presets},
        protocol::Router,
    };
    use tokio::sync::{Notify, oneshot};

    use super::*;
    use crate::{RpcClient, RpcResult, frame::RequestFrame};

    const ALPN: &[u8] = b"fedi/test/context/1";

    #[test]
    fn bounded_response_encoding_accepts_exact_cap_and_rejects_cap_plus_one() {
        let frame = ResponseFrame::Service(vec![0x5a; 1024]);
        let encoded = crate::client::encode(&frame).expect("encode response fixture");

        assert_eq!(
            encode_bounded(&frame, encoded.len()).expect("exact cap"),
            encoded
        );
        assert!(matches!(
            encode_bounded(&frame, encoded.len() - 1),
            Err(RpcError::ResponseTooLarge)
        ));

        let mut writer = BoundedWriter::new(16);
        assert!(writer.write_all(&[0; 17]).is_err());
        assert!(writer.bytes.len() <= 16);

        let mut incrementally_grown = BoundedWriter::new(16 * 1024);
        incrementally_grown
            .write_all(&[0; 4 * 1024])
            .expect("partial initial capacity");
        incrementally_grown
            .write_all(&[0; 12 * 1024])
            .expect("cross initial capacity to exact cap");
        assert_eq!(incrementally_grown.bytes.len(), 16 * 1024);
        assert!(incrementally_grown.write_all(&[0]).is_err());
        assert_eq!(incrementally_grown.bytes.len(), 16 * 1024);
    }

    #[tokio::test]
    async fn passes_remote_node_id_to_handler_context() -> Result<(), Box<dyn std::error::Error>> {
        // Relay-free loopback endpoints: the client dials the router's direct
        // socket address, so the test runs in sandboxes without external
        // network. Waiting for `online()` with relays enabled hangs forever
        // there; the N0 preset is kept only for its crypto-provider wiring.
        let server_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let (sender, receiver) = oneshot::channel();
        let router = Router::builder(server_endpoint)
            .accept(
                ALPN,
                IrohProtocol::new(ContextRecordingHandler {
                    sender: Mutex::new(Some(sender)),
                }),
            )
            .spawn();

        let client_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let client_node_id = client_endpoint.id().to_string();
        let connection = client_endpoint
            .connect(router.endpoint().addr(), ALPN)
            .await?;
        let client = RpcClient::new(connection);

        let response: String = client.call("record_context", ()).await?;
        assert_eq!(response, "ok");
        let context = tokio::time::timeout(Duration::from_secs(10), receiver).await??;
        assert_eq!(
            context.remote_node_id.as_deref(),
            Some(client_node_id.as_str())
        );

        router.shutdown().await?;
        client_endpoint.close().await;

        Ok(())
    }

    #[tokio::test]
    async fn partial_streams_release_handler_permits_after_the_initial_frame_deadline()
    -> Result<(), Box<dyn std::error::Error>> {
        let server_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let protocol = IrohProtocol::with_limits_and_request_read_timeout(
            EchoHandler,
            1024 * 1024,
            2,
            Duration::from_millis(100),
        );
        let router = Router::builder(server_endpoint)
            .accept(ALPN, protocol.clone())
            .spawn();

        let attacker_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let attacker_connection = attacker_endpoint
            .connect(router.endpoint().addr(), ALPN)
            .await?;
        let mut partial_streams = Vec::new();
        for _ in 0..2 {
            let (mut send, recv) = attacker_connection.open_bi().await?;
            send.write_all(&[0]).await?;
            partial_streams.push((send, recv));
        }

        tokio::time::timeout(Duration::from_secs(1), async {
            while 0 < protocol.stream_permits.available_permits() {
                tokio::task::yield_now().await;
            }
        })
        .await?;

        let legitimate_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let legitimate_connection = legitimate_endpoint
            .connect(router.endpoint().addr(), ALPN)
            .await?;
        let response: String = tokio::time::timeout(
            Duration::from_secs(2),
            RpcClient::new(legitimate_connection).call("echo", ()),
        )
        .await??;
        assert_eq!(response, "ok");

        drop(partial_streams);
        router.shutdown().await?;
        attacker_endpoint.close().await;
        legitimate_endpoint.close().await;

        Ok(())
    }

    #[tokio::test]
    async fn decoded_handler_runs_after_client_disconnect() -> Result<(), Box<dyn std::error::Error>>
    {
        let handler = DisconnectResilientHandler {
            started: Arc::new(Notify::new()),
            resume: Arc::new(Notify::new()),
            completed: Arc::new(Notify::new()),
        };
        let server_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let router = Router::builder(server_endpoint)
            .accept(ALPN, IrohProtocol::new(handler.clone()))
            .spawn();

        let client_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let connection = client_endpoint
            .connect(router.endpoint().addr(), ALPN)
            .await?;
        let request_bytes =
            crate::client::encode(&RequestFrame::new("wait", crate::client::encode(&())?))?;
        let (mut send, _recv) = connection.open_bi().await?;
        send.write_all(&request_bytes).await?;
        send.finish()?;

        tokio::time::timeout(Duration::from_secs(1), handler.started.notified()).await?;
        connection.close(0u8.into(), b"client disconnected");
        handler.resume.notify_one();
        tokio::time::timeout(Duration::from_secs(1), handler.completed.notified()).await?;

        router.shutdown().await?;
        client_endpoint.close().await;

        Ok(())
    }

    #[tokio::test]
    async fn slow_readers_are_task_capped_and_response_timeout_releases_permits()
    -> Result<(), Box<dyn std::error::Error>> {
        let server_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let protocol = IrohProtocol::with_resource_limits(
            LargeResponseHandler { bytes: 32 * 1024 },
            1024,
            256 * 1024,
            2,
            Duration::from_secs(1),
            Duration::from_secs(1),
        );
        let router = Router::builder(server_endpoint)
            .accept(ALPN, protocol.clone())
            .spawn();

        let tiny_receive_window = QuicTransportConfig::builder()
            .receive_window(32_u8.into())
            .stream_receive_window(16_u8.into())
            .build();
        let attacker_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .transport_config(tiny_receive_window)
            .bind()
            .await?;
        let attacker_connection = attacker_endpoint
            .connect(router.endpoint().addr(), ALPN)
            .await?;
        let request_bytes =
            crate::client::encode(&RequestFrame::new("large", crate::client::encode(&())?))?;
        let mut unread_streams = Vec::new();
        for _ in 0..2 {
            let (mut send, recv) = attacker_connection.open_bi().await?;
            send.write_all(&request_bytes).await?;
            send.finish()?;
            unread_streams.push(recv);
        }

        tokio::time::timeout(Duration::from_secs(1), async {
            while 0 < protocol.stream_permits.available_permits() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert_eq!(protocol.stream_permits.available_permits(), 0);

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            protocol.stream_permits.available_permits(),
            0,
            "slow response tasks must retain both permits before their deadline"
        );

        let legitimate_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let legitimate_connection = legitimate_endpoint
            .connect(router.endpoint().addr(), ALPN)
            .await?;
        let mut legitimate_call = tokio::spawn(async move {
            RpcClient::new(legitimate_connection)
                .call::<_, Vec<u8>>("large", ())
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut legitimate_call)
                .await
                .is_err(),
            "a third task must remain queued before the response deadline"
        );
        let response = tokio::time::timeout(Duration::from_secs(2), legitimate_call).await???;
        assert_eq!(response.len(), 32 * 1024);

        drop(unread_streams);
        router.shutdown().await?;
        attacker_endpoint.close().await;
        legitimate_endpoint.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn oversized_response_writes_only_bounded_transport_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let server_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let router = Router::builder(server_endpoint)
            .accept(
                ALPN,
                IrohProtocol::with_resource_limits(
                    LargeResponseHandler { bytes: 1024 },
                    1024,
                    128,
                    1,
                    Duration::from_secs(1),
                    Duration::from_secs(1),
                ),
            )
            .spawn();
        let client_endpoint = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await?;
        let connection = client_endpoint
            .connect(router.endpoint().addr(), ALPN)
            .await?;

        let error = RpcClient::new(connection)
            .call::<_, Vec<u8>>("large", ())
            .await
            .expect_err("oversized response must be rejected");
        assert!(
            matches!(error, RpcError::Remote(message) if message == RpcError::ResponseTooLarge.to_string())
        );

        router.shutdown().await?;
        client_endpoint.close().await;
        Ok(())
    }

    struct ContextRecordingHandler {
        sender: Mutex<Option<oneshot::Sender<RpcConnectionContext>>>,
    }

    #[async_trait::async_trait]
    impl RpcServiceHandler for ContextRecordingHandler {
        async fn handle_rpc(&self, method: &str, _body: &[u8]) -> RpcResult<Vec<u8>> {
            Err(RpcError::UnknownMethod(method.to_owned()))
        }

        async fn handle_rpc_with_context(
            &self,
            context: RpcConnectionContext,
            _method: &str,
            body: &[u8],
        ) -> RpcResult<Vec<u8>> {
            if let Some(sender) = self.sender.lock().expect("context sender mutex").take() {
                let _ = sender.send(context);
            }
            crate::__private::decode_call::<(), String, _>(body, |_| async { "ok".to_owned() })
                .await
        }
    }

    struct EchoHandler;

    #[async_trait::async_trait]
    impl RpcServiceHandler for EchoHandler {
        async fn handle_rpc(&self, _method: &str, body: &[u8]) -> RpcResult<Vec<u8>> {
            crate::__private::decode_call::<(), String, _>(body, |_| async { "ok".to_owned() })
                .await
        }
    }

    struct LargeResponseHandler {
        bytes: usize,
    }

    #[async_trait::async_trait]
    impl RpcServiceHandler for LargeResponseHandler {
        async fn handle_rpc(&self, _method: &str, body: &[u8]) -> RpcResult<Vec<u8>> {
            crate::__private::decode_call::<(), Vec<u8>, _>(body, |_| async {
                vec![0x5a; self.bytes]
            })
            .await
        }
    }

    #[derive(Clone)]
    struct DisconnectResilientHandler {
        started: Arc<Notify>,
        resume: Arc<Notify>,
        completed: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl RpcServiceHandler for DisconnectResilientHandler {
        async fn handle_rpc(&self, _method: &str, body: &[u8]) -> RpcResult<Vec<u8>> {
            self.started.notify_one();
            self.resume.notified().await;
            self.completed.notify_one();
            crate::__private::decode_call::<(), String, _>(body, |_| async { "ok".to_owned() })
                .await
        }
    }
}
