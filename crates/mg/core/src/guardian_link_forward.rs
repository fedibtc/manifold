//! The guardian-link forward: the linked device opens web UIs on the host
//! this MG runs on, such as the umbrelOS or StartOS dashboard, over Iroh
//! ([`SPEC-guardian-link`](../../specs/SPEC-guardian-link.md)).
//!
//! The daemon holds a map from [`ForwardTarget`] to an address. Each
//! bidirectional stream on [`GUARDIAN_LINK_FORWARD_ALPN`](fedi_decentralized_service_manifold_guardian::GUARDIAN_LINK_FORWARD_ALPN) names one target in
//! its header and is then one TCP connection to that target's address; bytes
//! pass through unchanged. The MG parses nothing past the header: the caller
//! speaks HTTP/1.1, and a WebSocket upgrade continues on the same stream.
//! Only the endpoint the device linked from gets a stream, and only while the
//! guardian link is on; a connection that stops qualifying is closed.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fedi_decentralized_service_manifold_guardian::{
    AnnouncedForwardTarget, ForwardRefusal, ForwardStreamError, ForwardTarget, ForwardTargets,
    MAX_FORWARD_STREAMS,
};
use iroh::endpoint::{Connection, RecvStream, SendStream, VarInt};
use iroh::protocol::{AcceptError, ProtocolHandler};
use tokio::net::TcpStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;

use crate::fleet::Fleet;

/// How long the MG waits for a target to accept a TCP connection, and for a
/// stream's header.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The [`GUARDIAN_LINK_FORWARD_ALPN`](fedi_decentralized_service_manifold_guardian::GUARDIAN_LINK_FORWARD_ALPN) protocol handler.
#[derive(Clone)]
pub struct GuardianLinkForward {
    fleet: Arc<Fleet>,
    /// Where each target is, as seen from the MG. Empty when the deployment
    /// has nothing to forward.
    targets: Arc<BTreeMap<ForwardTarget, SocketAddr>>,
    streams: Arc<Semaphore>,
}

impl GuardianLinkForward {
    #[must_use]
    pub fn new(fleet: Arc<Fleet>, targets: BTreeMap<ForwardTarget, SocketAddr>) -> Self {
        Self {
            fleet,
            targets: Arc::new(targets),
            streams: Arc::new(Semaphore::new(MAX_FORWARD_STREAMS)),
        }
    }

    async fn refusal(&self, caller: &iroh::EndpointId) -> Option<ForwardRefusal> {
        match self.fleet.forward_refusal(caller).await {
            Some(refusal) => Some(refusal),
            None if self.targets.is_empty() => Some(ForwardRefusal::Unavailable),
            None => None,
        }
    }

    fn announcement(&self) -> Vec<u8> {
        let targets = ForwardTargets {
            targets: self
                .targets
                .iter()
                .map(|(target, address)| AnnouncedForwardTarget {
                    name: target.name().to_owned(),
                    port: address.port(),
                })
                .collect(),
        };
        serde_json::to_vec(&targets).expect("serializes")
    }
}

impl std::fmt::Debug for GuardianLinkForward {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuardianLinkForward")
            .field("targets", &self.targets)
            .finish_non_exhaustive()
    }
}

fn close(connection: &Connection, refusal: ForwardRefusal) {
    connection.close(
        VarInt::from_u32(refusal.code()),
        refusal.reason().as_bytes(),
    );
}

fn reset(send: &mut SendStream, recv: &mut RecvStream, error: ForwardStreamError) {
    let code = VarInt::from_u32(error.code());
    let _ = send.reset(code);
    let _ = recv.stop(code);
}

impl ProtocolHandler for GuardianLinkForward {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let caller = connection.remote_id();
        // Subscribe before the first check, so a change between the two is
        // seen as a change.
        let (mut linked_device, mut policy) = self.fleet.forward_gate_changes();
        if let Some(refusal) = self.refusal(&caller).await {
            close(&connection, refusal);
            return Ok(());
        }
        tracing::info!("guardian link forward opened for the linked device");
        let mut streams = JoinSet::new();
        streams.spawn(announce(connection.clone(), self.announcement()));
        loop {
            tokio::select! {
                accepted = connection.accept_bi() => {
                    let Ok((mut send, mut recv)) = accepted else {
                        break;
                    };
                    match Arc::clone(&self.streams).try_acquire_owned() {
                        Ok(permit) => {
                            streams.spawn(forward(Arc::clone(&self.targets), send, recv, permit));
                        }
                        Err(_) => reset(&mut send, &mut recv, ForwardStreamError::TooManyStreams),
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

/// Tell the caller which targets this MG serves.
async fn announce(connection: Connection, announcement: Vec<u8>) {
    let Ok(mut send) = connection.open_uni().await else {
        return;
    };
    if send.write_all(&announcement).await.is_ok() {
        let _ = send.finish();
    }
}

/// The target a stream's header names, if this MG serves it.
async fn read_target(
    recv: &mut RecvStream,
    targets: &BTreeMap<ForwardTarget, SocketAddr>,
) -> Option<SocketAddr> {
    let mut length = [0u8; 1];
    recv.read_exact(&mut length).await.ok()?;
    let length = usize::from(length[0]);
    if length == 0 || ForwardTarget::MAX_NAME_BYTES < length {
        return None;
    }
    let mut name = vec![0u8; length];
    recv.read_exact(&mut name).await.ok()?;
    let target: ForwardTarget = std::str::from_utf8(&name).ok()?.parse().ok()?;
    targets.get(&target).copied()
}

/// Pipe one stream to a new TCP connection to the target its header names
/// until both sides have finished, or either fails.
async fn forward(
    targets: Arc<BTreeMap<ForwardTarget, SocketAddr>>,
    mut send: SendStream,
    mut recv: RecvStream,
    _permit: OwnedSemaphorePermit,
) {
    let Ok(Some(address)) =
        tokio::time::timeout(CONNECT_TIMEOUT, read_target(&mut recv, &targets)).await
    else {
        reset(&mut send, &mut recv, ForwardStreamError::UnknownTarget);
        return;
    };
    let mut tcp = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
        Ok(Ok(tcp)) => tcp,
        Ok(Err(error)) => {
            tracing::warn!(%error, "forward target is not reachable");
            reset(&mut send, &mut recv, ForwardStreamError::TargetUnreachable);
            return;
        }
        Err(_) => {
            tracing::warn!("forward target did not accept a connection in time");
            reset(&mut send, &mut recv, ForwardStreamError::TargetUnreachable);
            return;
        }
    };
    let mut stream = tokio::io::join(recv, send);
    if let Err(error) = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await {
        tracing::debug!(%error, "forward stream ended with an error");
    }
}
