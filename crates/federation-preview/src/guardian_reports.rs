//! Public health reads, keeping old status usable without the newer endpoint.

use std::collections::BTreeMap;
use std::future::Future;
use std::str::FromStr as _;
use std::time::Duration;

use fedi_decentralized_domain::{FederationId, GuardianReports, GuardianSessionReport, InviteCode};
use fedimint_api_client::api::{DynGlobalApi, IGlobalFederationApi, StatusResponse};
use fedimint_api_client::download_from_invite_code;
use fedimint_core::PeerId;
use fedimint_core::endpoint_constants::{PEER_AVAILABILITY_ENDPOINT, STATUS_ENDPOINT};
use fedimint_core::invite_code::InviteCode as FedimintInviteCode;
use fedimint_core::module::ApiRequestErased;
use fedimint_core::runtime::timeout;
use futures_util::future::join_all;
use serde_json::Value;

use crate::{ConnectorRegistry, FederationPreviewError};

const REPORT_TIMEOUT: Duration = Duration::from_secs(10);

/// Fetch fresh reports without joining the federation. A failed optional endpoint
/// does not discard that peer's legacy status, or another peer's answers.
///
/// # Errors
/// Returns an error when the invite or config cannot be read within 30 seconds.
pub async fn read_guardian_reports(
    connectors: &ConnectorRegistry,
    invite_code: &InviteCode,
) -> Result<GuardianReports, FederationPreviewError> {
    let invite = FedimintInviteCode::from_str(&invite_code.0)
        .map_err(|error| FederationPreviewError::InvalidInviteCode(error.to_string()))?;
    reports_after_setup(FederationId(invite.federation_id().to_string()), async {
        download_from_invite_code(connectors, &invite)
            .await
            .map(|(_, api)| api)
    })
    .await
}

async fn reports_after_setup(
    federation_id: FederationId,
    setup: impl Future<Output = anyhow::Result<DynGlobalApi>>,
) -> Result<GuardianReports, FederationPreviewError> {
    let api = timeout(Duration::from_secs(30), setup)
        .await
        .map_err(|_| {
            FederationPreviewError::Unavailable("guardian config read timed out".to_owned())
        })?
        .map_err(|error| FederationPreviewError::Unavailable(error.to_string()))?;
    Ok(read_guardian_reports_from_api(federation_id, api.as_ref()).await)
}

/// Read fresh reports through an existing API, without downloading config.
/// The caller must supply the complete peer set for the named federation, from
/// a previously verified config. Missing answers stay absent. Each endpoint
/// gets ten seconds, independently of any earlier connection setup.
pub async fn read_guardian_reports_from_api(
    federation_id: FederationId,
    api: &dyn IGlobalFederationApi,
) -> GuardianReports {
    let peers = api.all_peers();
    let mut result = GuardianReports {
        federation_id,
        peers: peers.clone(),
        sessions: BTreeMap::new(),
        availability: BTreeMap::new(),
    };
    let answers = join_all(peers.iter().copied().map(|peer| async move {
        let params = ApiRequestErased::default();
        let (status, availability) = futures_util::join!(
            timeout(
                REPORT_TIMEOUT,
                api.request_raw(peer, STATUS_ENDPOINT, &params)
            ),
            timeout(
                REPORT_TIMEOUT,
                api.request_raw(peer, PEER_AVAILABILITY_ENDPOINT, &params)
            ),
        );
        (
            peer,
            status.ok().and_then(Result::ok),
            availability.ok().and_then(Result::ok),
        )
    }))
    .await;
    for (peer, status, availability) in answers {
        if let Some(status) = status
            .and_then(|value| serde_json::from_value::<StatusResponse>(value).ok())
            .and_then(|status| status.federation)
        {
            result.sessions.insert(
                peer,
                GuardianSessionReport {
                    session_count: status.session_count,
                    last_contribution: status
                        .status_by_peer
                        .into_iter()
                        .map(|(peer, status)| (peer, status.last_contribution))
                        .collect(),
                },
            );
        }
        if let Some(report) = availability.and_then(|value| serde_json::from_value(value).ok()) {
            result.availability.insert(peer, report);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use fedimint_api_client::api::global_api::with_cache::GlobalFederationApiWithCacheExt as _;
    use fedimint_api_client::api::{
        DynModuleApi, IRawFederationApi, LegacyFederationStatus, LegacyP2PConnectionStatus,
        LegacyPeerStatus,
    };
    use fedimint_connectors::{DynGuaridianConnection, PeerStatus, ServerResult};
    use fedimint_core::admin_client::ServerStatusLegacy;
    use fedimint_core::core::ModuleInstanceId;
    use fedimint_core::peer_availability::PeerAvailability;
    use fedimint_core::{apply, async_trait_maybe_send};
    use futures_util::stream::BoxStream;

    use super::*;

    #[derive(Debug)]
    struct TestApi {
        peers: BTreeSet<PeerId>,
        status: Value,
    }

    #[apply(async_trait_maybe_send!)]
    impl IRawFederationApi for TestApi {
        fn all_peers(&self) -> &BTreeSet<PeerId> {
            &self.peers
        }
        fn self_peer(&self) -> Option<PeerId> {
            None
        }
        fn with_module(&self, _: ModuleInstanceId) -> DynModuleApi {
            unreachable!("no module reads")
        }
        async fn request_raw(
            &self,
            peer: PeerId,
            method: &str,
            _: &ApiRequestErased,
        ) -> ServerResult<Value> {
            assert!(
                matches!(method, STATUS_ENDPOINT | PEER_AVAILABILITY_ENDPOINT),
                "existing API must not download config"
            );
            Ok(match (peer.to_usize(), method) {
                (0 | 1, STATUS_ENDPOINT) => self.status.clone(),
                (1, PEER_AVAILABILITY_ENDPOINT) => {
                    serde_json::to_value(PeerAvailability::default()).unwrap()
                }
                (2, _) => serde_json::json!({"malformed": true}),
                (3, _) => std::future::pending().await,
                _ => Value::Null, // Older peer lacks peer_availability.
            })
        }
        fn connection_status_stream(&self) -> BoxStream<'static, BTreeMap<PeerId, PeerStatus>> {
            Box::pin(futures_util::stream::empty())
        }
        async fn wait_for_initialized_connections(&self) {}
        async fn get_peer_connection(&self, _: PeerId) -> ServerResult<DynGuaridianConnection> {
            unreachable!("raw reads only")
        }
    }

    fn api() -> DynGlobalApi {
        let status = serde_json::to_value(StatusResponse {
            server: ServerStatusLegacy::ConsensusRunning,
            federation: Some(LegacyFederationStatus {
                session_count: 600,
                status_by_peer: BTreeMap::from([(
                    PeerId::from(3),
                    LegacyPeerStatus {
                        connection_status: LegacyP2PConnectionStatus::Disconnected,
                        last_contribution: Some(100),
                        flagged: true,
                    },
                )]),
                ..Default::default()
            }),
        })
        .unwrap();
        TestApi {
            peers: (0..4).map(PeerId::from).collect(),
            status,
        }
        .with_cache()
        .into()
    }

    fn assert_partial_reports(reports: &GuardianReports) {
        assert_eq!(reports.peers.len(), 4);
        assert_eq!(reports.sessions.len(), 2);
        assert_eq!(
            reports.sessions[&PeerId::from(0)].last_contribution[&PeerId::from(3)],
            Some(100)
        );
        assert_eq!(
            reports.availability.keys().copied().collect::<Vec<_>>(),
            vec![PeerId::from(1)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn guardian_reports_from_existing_api_keep_partial_answers() {
        let start = tokio::time::Instant::now();
        let reports =
            read_guardian_reports_from_api(FederationId("test".into()), api().as_ref()).await;
        assert_eq!(start.elapsed(), REPORT_TIMEOUT);
        assert_partial_reports(&reports);
    }

    #[tokio::test(start_paused = true)]
    async fn guardian_reports_survive_slow_config_and_hung_optional_endpoints() {
        let start = tokio::time::Instant::now();
        let reports = reports_after_setup(FederationId("test".into()), async {
            tokio::time::sleep(Duration::from_secs(25)).await;
            Ok(api())
        })
        .await
        .unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(35));
        assert_partial_reports(&reports);
    }

    #[tokio::test(start_paused = true)]
    async fn guardian_reports_stop_when_config_never_arrives() {
        let start = tokio::time::Instant::now();
        let result = reports_after_setup(FederationId("test".into()), std::future::pending()).await;
        assert!(matches!(
            result,
            Err(FederationPreviewError::Unavailable(_))
        ));
        assert_eq!(start.elapsed(), Duration::from_secs(30));
    }
}
