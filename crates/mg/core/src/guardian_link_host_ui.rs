//! The host-UI forward: the linked device opens the web UI of the host this
//! MG runs on (the umbrelOS or StartOS dashboard) over Iroh
//! ([`SPEC-guardian-link`](../../specs/SPEC-guardian-link.md)).
//!
//! Each bidirectional stream on [`HOST_UI_ALPN`] is one TCP connection to the
//! single host-UI address the daemon was started with; bytes pass through
//! unchanged. The MG parses nothing: the caller speaks HTTP/1.1, and a
//! WebSocket upgrade continues on the same stream. Only the endpoint the
//! device linked from gets a stream, and only while the guardian link is on;
//! a connection that stops qualifying is closed.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fedi_decentralized_service_manifold_guardian::{HostUiRefusal, HostUiStreamError};
use iroh::endpoint::{Connection, RecvStream, SendStream, VarInt};
use iroh::protocol::{AcceptError, ProtocolHandler};
use tokio::net::TcpStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;

use crate::fleet::Fleet;

/// Streams open at once across every connection. A browser keeps about six
/// HTTP connections and a few WebSockets to one origin.
pub const MAX_HOST_UI_STREAMS: usize = 32;

/// How long the MG waits for the host UI to accept a TCP connection.
const HOST_UI_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The [`HOST_UI_ALPN`](fedi_decentralized_service_manifold_guardian::HOST_UI_ALPN)
/// protocol handler.
#[derive(Clone)]
pub struct HostUiForward {
    fleet: Arc<Fleet>,
    /// Where the host serves its web UI, as seen from the MG; `None` when the
    /// deployment has none to forward.
    host_ui: Option<SocketAddr>,
    streams: Arc<Semaphore>,
}

impl HostUiForward {
    #[must_use]
    pub fn new(fleet: Arc<Fleet>, host_ui: Option<SocketAddr>) -> Self {
        Self {
            fleet,
            host_ui,
            streams: Arc::new(Semaphore::new(MAX_HOST_UI_STREAMS)),
        }
    }

    async fn refusal(&self, caller: &iroh::EndpointId) -> Option<HostUiRefusal> {
        match self.fleet.host_ui_refusal(caller).await {
            Some(refusal) => Some(refusal),
            None if self.host_ui.is_none() => Some(HostUiRefusal::Unavailable),
            None => None,
        }
    }
}

impl std::fmt::Debug for HostUiForward {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostUiForward")
            .field("host_ui", &self.host_ui)
            .finish_non_exhaustive()
    }
}

fn close(connection: &Connection, refusal: HostUiRefusal) {
    connection.close(
        VarInt::from_u32(refusal.code()),
        refusal.reason().as_bytes(),
    );
}

fn reset(send: &mut SendStream, recv: &mut RecvStream, error: HostUiStreamError) {
    let code = VarInt::from_u32(error.code());
    let _ = send.reset(code);
    let _ = recv.stop(code);
}

impl ProtocolHandler for HostUiForward {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let caller = connection.remote_id();
        // Subscribe before the first check, so a change between the two is
        // seen as a change.
        let (mut linked_device, mut policy) = self.fleet.host_ui_gate_changes();
        if let Some(refusal) = self.refusal(&caller).await {
            close(&connection, refusal);
            return Ok(());
        }
        let host_ui = self.host_ui.expect("refusal() is None only with a host UI");
        tracing::info!("host UI forward opened for the linked device");
        let mut streams = JoinSet::new();
        loop {
            tokio::select! {
                accepted = connection.accept_bi() => {
                    let Ok((mut send, mut recv)) = accepted else {
                        break;
                    };
                    match Arc::clone(&self.streams).try_acquire_owned() {
                        Ok(permit) => {
                            streams.spawn(forward(host_ui, send, recv, permit));
                        }
                        Err(_) => reset(&mut send, &mut recv, HostUiStreamError::TooManyStreams),
                    }
                }
                changed = linked_device.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
                changed = async { policy.as_mut().expect("guarded").changed().await },
                    if policy.is_some() =>
                {
                    if changed.is_err() {
                        policy = None;
                    }
                }
                Some(_) = streams.join_next() => continue,
            }
            if let Some(refusal) = self.refusal(&caller).await {
                close(&connection, refusal);
                break;
            }
        }
        // Dropping the set aborts the streams still open.
        drop(streams);
        Ok(())
    }
}

/// Pipe one stream to a new TCP connection to the host UI until both sides
/// have finished, or either fails.
async fn forward(
    host_ui: SocketAddr,
    mut send: SendStream,
    mut recv: RecvStream,
    _permit: OwnedSemaphorePermit,
) {
    let mut tcp =
        match tokio::time::timeout(HOST_UI_CONNECT_TIMEOUT, TcpStream::connect(host_ui)).await {
            Ok(Ok(tcp)) => tcp,
            Ok(Err(error)) => {
                tracing::warn!(%error, "host UI is not reachable");
                reset(&mut send, &mut recv, HostUiStreamError::HostUnreachable);
                return;
            }
            Err(_) => {
                tracing::warn!("host UI did not accept a connection in time");
                reset(&mut send, &mut recv, HostUiStreamError::HostUnreachable);
                return;
            }
        };
    let mut stream = tokio::io::join(recv, send);
    if let Err(error) = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await {
        tracing::debug!(%error, "host UI stream ended with an error");
    }
}
