//! Public health reads, keeping old status usable without the newer endpoint.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::str::FromStr as _;
use std::time::Duration;

use fedi_decentralized_domain::{FederationId, GuardianReports, GuardianSessionReport, InviteCode};
use fedimint_api_client::api::StatusResponse;
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
    timeout(Duration::from_secs(30), async {
        let invite = FedimintInviteCode::from_str(&invite_code.0)
            .map_err(|error| FederationPreviewError::InvalidInviteCode(error.to_string()))?;
        let (_, api) = download_from_invite_code(connectors, &invite)
            .await
            .map_err(|error| FederationPreviewError::Unavailable(error.to_string()))?;
        Ok(collect_reports(
            FederationId(invite.federation_id().to_string()),
            api.all_peers().clone(),
            |peer, method| {
                let api = &api;
                async move {
                    api.request_raw(peer, method, &ApiRequestErased::default())
                        .await
                        .ok()
                }
            },
        )
        .await)
    })
    .await
    .map_err(|_| FederationPreviewError::Unavailable("guardian report read timed out".to_owned()))?
}

async fn collect_reports<F, Fut>(
    federation_id: FederationId,
    peers: BTreeSet<PeerId>,
    read: F,
) -> GuardianReports
where
    F: Fn(PeerId, &'static str) -> Fut,
    Fut: Future<Output = Option<Value>>,
{
    let mut result = GuardianReports {
        federation_id,
        peers: peers.clone(),
        sessions: BTreeMap::new(),
        availability: BTreeMap::new(),
    };
    let answers = join_all(peers.into_iter().map(|peer| {
        let read = &read;
        async move {
            let (status, availability) = futures_util::join!(
                timeout(REPORT_TIMEOUT, read(peer, STATUS_ENDPOINT)),
                timeout(REPORT_TIMEOUT, read(peer, PEER_AVAILABILITY_ENDPOINT)),
            );
            (peer, status.ok().flatten(), availability.ok().flatten())
        }
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
    use fedimint_api_client::api::{
        LegacyFederationStatus, LegacyP2PConnectionStatus, LegacyPeerStatus,
    };
    use fedimint_core::admin_client::ServerStatusLegacy;
    use fedimint_core::peer_availability::PeerAvailability;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn guardian_reports_keep_partial_answers_and_bound_hung_peers() {
        let peers = (0..4).map(PeerId::from).collect();
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
        let start = tokio::time::Instant::now();
        let reports = collect_reports(FederationId("test".into()), peers, |peer, method| {
            let status = status.clone();
            async move {
                match (peer.to_usize(), method) {
                    (0 | 1, STATUS_ENDPOINT) => Some(status),
                    (1, PEER_AVAILABILITY_ENDPOINT) => {
                        Some(serde_json::to_value(PeerAvailability::default()).unwrap())
                    }
                    (2, _) => Some(serde_json::json!({"malformed": true})),
                    (3, _) => std::future::pending().await,
                    _ => None, // Older peer lacks peer_availability.
                }
            }
        })
        .await;
        assert_eq!(start.elapsed(), REPORT_TIMEOUT);
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
}
