//! Transparent relay proxy counting revocation requests on the real wire.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::tungstenite::Message;

/// Forward real Nostr traffic while observing expensive verification reads.
pub(super) struct RelayProbe {
    /// Local address supplied to the isolated verifier process.
    pub url: String,
    /// Number of revocation subscriptions sent to the real relay.
    pub revocation_reads: Arc<AtomicUsize>,
    /// Own every proxy connection; aborting drops their task set too.
    task: JoinHandle<()>,
}

impl RelayProbe {
    /// Start a transparent local proxy to one defe relay.
    pub async fn start(upstream: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let revocation_reads = Arc::new(AtomicUsize::new(0));
        let count = revocation_reads.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        let upstream = upstream.clone();
                        let count = count.clone();
                        connections.spawn(async move {
                            let downstream = tokio_tungstenite::accept_async(stream).await.unwrap();
                            let (upstream, _) = tokio_tungstenite::connect_async(upstream).await.unwrap();
                            let (mut to_client, mut from_client) = downstream.split();
                            let (mut to_relay, mut from_relay) = upstream.split();
                            tokio::select! {
                                _ = async {
                                    while let Some(Ok(message)) = from_client.next().await {
                                        if let Message::Text(text) = &message
                                            && let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
                                            && let Some(fields) = value.as_array()
                                            && fields.first().and_then(|v| v.as_str()) == Some("REQ")
                                            && fields.iter().skip(2).any(|filter| {
                                                filter["kinds"].as_array().is_some_and(|kinds| {
                                                    kinds.iter().any(|kind| kind.as_u64() == Some(37704))
                                                })
                                            })
                                        {
                                            count.fetch_add(1, Ordering::SeqCst);
                                        }
                                        if to_relay.send(message).await.is_err() {
                                            break;
                                        }
                                    }
                                } => {}
                                _ = async {
                                    while let Some(Ok(message)) = from_relay.next().await {
                                        if to_client.send(message).await.is_err() {
                                            break;
                                        }
                                    }
                                } => {}
                            }
                        });
                    }
                    Some(result) = connections.join_next() => { result.unwrap(); }
                }
            }
        });
        Self {
            url,
            revocation_reads,
            task,
        }
    }
}

impl Drop for RelayProbe {
    fn drop(&mut self) {
        self.task.abort();
    }
}
