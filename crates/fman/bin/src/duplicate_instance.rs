//! Best-effort pre-activation check for a second FMan with this identity.
//!
//! This is not a distributed lock: a partition or two simultaneous startups
//! can both miss each other. Never bind the persistent key here: that could
//! replace the running instance's discovery or relay registration.

use std::future::Future;
use std::time::Duration;

use fedi_decentralized_service_fleet_manager::FLEET_MANAGER_ALPN;
use iroh::address_lookup::{DnsAddressLookup, PkarrResolver};
use iroh::endpoint::{Builder, PortmapperConfig, presets};
use iroh::{Endpoint, EndpointAddr};

pub(super) const PROBE_BUDGET: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    DuplicateDetected,
    Unconfirmed,
    Shutdown,
}

/// Dial only with a fresh, unpublished client identity and the existing FMan
/// ALPN. An authenticated handshake confirms ownership; errors do not prove
/// uniqueness. Cleanup follows the timed attempt and may take additional time.
pub(super) async fn probe(
    target: EndpointAddr,
    budget: Duration,
    shutdown: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<Outcome> {
    // N0 normally includes a publisher. Retain its resolvers and relay
    // support, but never publish an address or request port mappings.
    let builder = Endpoint::builder(presets::N0)
        .clear_address_lookup()
        .address_lookup(PkarrResolver::n0_dns())
        .address_lookup(DnsAddressLookup::n0_dns())
        .portmapper_config(PortmapperConfig::Disabled);
    probe_with_builder(target, budget, shutdown, builder).await
}

async fn probe_with_builder(
    target: EndpointAddr,
    budget: Duration,
    shutdown: impl Future<Output = anyhow::Result<()>>,
    builder: Builder,
) -> anyhow::Result<Outcome> {
    tokio::pin!(shutdown);
    let deadline = tokio::time::Instant::now() + budget;
    let endpoint = tokio::select! {
        result = tokio::time::timeout_at(deadline, builder.bind()) => match result {
            Ok(Ok(endpoint)) => endpoint,
            _ => return Ok(Outcome::Unconfirmed),
        },
        result = &mut shutdown => return result.map(|()| Outcome::Shutdown),
    };
    debug_assert_ne!(
        endpoint.id(),
        target.id,
        "probe must never reuse the fleet identity"
    );

    let outcome = tokio::select! {
        result = tokio::time::timeout_at(deadline, endpoint.connect(target, FLEET_MANAGER_ALPN)) => {
            match result {
                Ok(Ok(connection)) => {
                    tracing::error!(
                        safe_to_share = true,
                        "Another instance using this Fleet Manager identity is reachable. \
                         Fleet startup is blocked. Stop the duplicate instance, verify that only \
                         one host will run this identity, then manually restart this process."
                    );
                    connection.close(0u32.into(), b"");
                    Outcome::DuplicateDetected
                }
                _ => Outcome::Unconfirmed,
            }
        },
        result = &mut shutdown => {
            // Still close the endpoint before returning from an interrupted probe.
            endpoint.close().await;
            return result.map(|()| Outcome::Shutdown);
        }
    };
    // Keep the same signal registration alive while closing. A signal consumed
    // during cleanup must not be lost before main starts the fleet. Never drop
    // a started close future: Iroh marks itself closing before its actors drain.
    if close_or_shutdown(endpoint.close(), &mut shutdown).await? {
        return Ok(Outcome::Shutdown);
    }
    Ok(outcome)
}

async fn close_or_shutdown(
    close: impl Future<Output = ()>,
    shutdown: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<bool> {
    tokio::pin!(close);
    tokio::pin!(shutdown);
    let interrupted = tokio::select! {
        biased;
        result = &mut shutdown => Some(result),
        _ = &mut close => None,
    };
    if let Some(result) = interrupted {
        close.await;
        result?;
        return Ok(true);
    }
    Ok(false)
}

/// Local E2E instances use explicit loopback routes and skip public discovery.
pub(super) fn should_probe(local_e2e: bool) -> bool {
    !local_e2e
}

/// Decide whether fleet activation may follow the one probe attempt.
/// Confirmed duplicates and shutdown can never resume activation.
pub(super) async fn allow_activation(
    outcome: Outcome,
    admin_task: tokio::task::JoinHandle<()>,
    admin_http_task: Option<tokio::task::JoinHandle<()>>,
    shutdown: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<bool> {
    match outcome {
        Outcome::DuplicateDetected => {
            park_duplicate(admin_task, admin_http_task, shutdown).await?;
            Ok(false)
        }
        Outcome::Unconfirmed => {
            tracing::warn!(
                safe_to_share = true,
                "Duplicate-instance check did not confirm another running instance; continuing \
                 startup. This best-effort check does not establish that this identity is unique."
            );
            Ok(true)
        }
        Outcome::Shutdown => Ok(false),
    }
}

/// Stop the operator listeners and do not activate fleet work until shutdown.
/// Already accepted onboarding requests may finish answering.
/// The caller retains its data-root lock while awaiting this function.
pub(super) async fn park_duplicate(
    admin_task: tokio::task::JoinHandle<()>,
    admin_http_task: Option<tokio::task::JoinHandle<()>>,
    shutdown: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<()> {
    admin_task.abort();
    let _ = admin_task.await;
    if let Some(task) = admin_http_task {
        task.abort();
        let _ = task.await;
    }
    shutdown.await
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;
    use iroh::endpoint::presets;
    use std::future::pending;

    async fn bind_server(alpn: &[u8]) -> Endpoint {
        Endpoint::builder(presets::N0DisableRelay)
            .alpns(vec![alpn.to_vec()])
            .bind()
            .await
            .unwrap()
    }

    async fn local_probe(
        target: EndpointAddr,
        budget: Duration,
        shutdown: impl Future<Output = anyhow::Result<()>>,
    ) -> anyhow::Result<Outcome> {
        probe_with_builder(
            target,
            budget,
            shutdown,
            Endpoint::builder(presets::N0DisableRelay)
                .clear_address_lookup()
                .portmapper_config(PortmapperConfig::Disabled),
        )
        .await
    }

    #[tokio::test]
    async fn detects_only_authenticated_fman_handshake() {
        let server = bind_server(FLEET_MANAGER_ALPN).await;
        let accept = tokio::spawn({
            let server = server.clone();
            async move { server.accept().await.unwrap().await.unwrap() }
        });
        assert_eq!(
            local_probe(server.addr(), Duration::from_secs(5), pending())
                .await
                .unwrap(),
            Outcome::DuplicateDetected
        );
        let connection = accept.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), connection.closed())
            .await
            .unwrap();
        server.close().await;
    }

    #[tokio::test]
    async fn mismatched_identity_and_alpn_are_unconfirmed() {
        let server = bind_server(FLEET_MANAGER_ALPN).await;
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let accept = tokio::spawn({
            let server = server.clone();
            async move {
                let connecting = server.accept().await.unwrap();
                let _ = reached_tx.send(());
                let _ = connecting.await;
            }
        });
        let wrong_id = SecretKey::generate().public();
        let wrong_route = EndpointAddr::new(wrong_id).with_addrs(
            server
                .addr()
                .ip_addrs()
                .copied()
                .map(iroh::TransportAddr::Ip),
        );
        assert_eq!(
            local_probe(wrong_route, Duration::from_secs(5), pending())
                .await
                .unwrap(),
            Outcome::Unconfirmed
        );
        tokio::time::timeout(Duration::from_secs(2), reached_rx)
            .await
            .unwrap()
            .unwrap();
        accept.await.unwrap();
        server.close().await;

        let server = bind_server(b"not/fman").await;
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let accept = tokio::spawn({
            let server = server.clone();
            async move {
                let connecting = server.accept().await.unwrap();
                let _ = reached_tx.send(());
                let _ = connecting.await;
            }
        });
        assert_eq!(
            local_probe(server.addr(), Duration::from_secs(5), pending())
                .await
                .unwrap(),
            Outcome::Unconfirmed
        );
        tokio::time::timeout(Duration::from_secs(2), reached_rx)
            .await
            .unwrap()
            .unwrap();
        accept.await.unwrap();
        server.close().await;
    }

    #[tokio::test]
    async fn shutdown_interrupts_probe_and_e2e_skips_it() {
        assert!(!should_probe(true));
        assert!(should_probe(false));
        let target = SecretKey::generate().public();
        assert_eq!(
            local_probe(target.into(), Duration::from_secs(5), async { Ok(()) })
                .await
                .unwrap(),
            Outcome::Shutdown
        );
    }

    #[tokio::test]
    async fn expired_budget_is_unconfirmed() {
        let target = SecretKey::generate().public();
        assert_eq!(
            local_probe(target.into(), Duration::ZERO, pending())
                .await
                .unwrap(),
            Outcome::Unconfirmed
        );
    }

    #[tokio::test]
    async fn blocked_instance_stops_listeners_and_waits_for_shutdown() {
        let server = bind_server(FLEET_MANAGER_ALPN).await;
        let accept = tokio::spawn({
            let server = server.clone();
            async move { server.accept().await.unwrap().await.unwrap() }
        });
        assert_eq!(
            local_probe(server.addr(), Duration::from_secs(5), pending())
                .await
                .unwrap(),
            Outcome::DuplicateDetected
        );
        drop(accept.await.unwrap());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (admin_stopped_tx, admin_stopped_rx) = tokio::sync::oneshot::channel();
        let admin = tokio::spawn(async move {
            struct OnDrop(Option<tokio::sync::oneshot::Sender<()>>);
            impl Drop for OnDrop {
                fn drop(&mut self) {
                    let _ = self.0.take().unwrap().send(());
                }
            }
            let _on_drop = OnDrop(Some(admin_stopped_tx));
            started_tx.send(()).unwrap();
            pending::<()>().await;
        });
        started_rx.await.unwrap();
        let http = tokio::spawn(pending::<()>());
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let parked = tokio::spawn(allow_activation(
            Outcome::DuplicateDetected,
            admin,
            Some(http),
            async move {
                shutdown_rx.await?;
                Ok(())
            },
        ));
        tokio::time::timeout(Duration::from_secs(1), admin_stopped_rx)
            .await
            .unwrap()
            .unwrap();
        // Even the peer disappearing cannot resume this process.
        server.close().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!parked.is_finished());
        shutdown_tx.send(()).unwrap();
        assert!(!parked.await.unwrap().unwrap());
        assert!(
            allow_activation(
                Outcome::Unconfirmed,
                tokio::spawn(async {}),
                None,
                pending(),
            )
            .await
            .unwrap(),
            "only an unconfirmed check can activate the fleet"
        );
    }

    #[tokio::test]
    async fn shutdown_during_cleanup_is_not_lost_or_cancelling_close() {
        let (close_tx, close_rx) = tokio::sync::oneshot::channel();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(close_or_shutdown(
            async { close_rx.await.unwrap() },
            async {
                shutdown_rx.await.unwrap();
                Ok(())
            },
        ));
        tokio::task::yield_now().await;
        shutdown_tx.send(()).unwrap();
        tokio::task::yield_now().await;
        assert!(!task.is_finished(), "must finish an already started close");
        close_tx.send(()).unwrap();
        assert!(task.await.unwrap().unwrap());
    }
}
