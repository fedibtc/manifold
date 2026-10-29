//! Replacement decisions belong to the FA, using peer observations and signed TSP policy.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use fedi_decentralized_domain::{
    AdmittedSetupPaymentFederations, GetMgTrustMaterialRequest, GuardianReports, MgSeatBindings,
    ProtocolV1, Pubkey, federation_seats, parse_protocol_peer_id,
};
use fedi_decentralized_nostr_clients::FaNostrClient;
use fedi_decentralized_peer_badge_verifier::PeerBadgeVerificationError;
use fedi_decentralized_service_manifold_guardian::ManifoldGuardianService;
use fedimint_core::peer_availability::merge_peer_availability;
use fedimint_core::runtime::timeout;
use fedimint_core::{NumPeersExt as _, PeerId};
use futures::future::join_all;
use serde::Serialize;

use crate::formation::{PostFormedAuthority, PostFormedSeat, invite_federation_id, now_secs};
use crate::liquidity::liquidity_seat_bindings_field;
use crate::{
    FaClient, FaError, FaIdentity, FaPayments, FaResult, FederationConsensusReader, FederationId,
    FormationRunOptions, ManifoldGuardianConnector, SeatId,
};

// MG production configs use 3,600 broadcast rounds at 50 ms. This estimates
// missed participation; it is not a wall-clock measurement of an outage.
// These server-only defaults are not exposed in the client config:
// https://github.com/fedibtc/fedimint/blob/v0.12.0-fedi15/fedimint-server/src/config/mod.rs#L66
const SESSION_SECONDS: u64 = 180;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Evidence supporting replacement. Missing reports never become a reason.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplacementReason {
    Offline {
        last_seen_secs: u64,
    },
    MissedSessions {
        sessions: u64,
    },
    /// f+1 other peers report disconnection, but duration cannot be established.
    Disconnected,
    LowWeekScore {
        basis_points: u16,
    },
    LowMonthScore {
        basis_points: u16,
    },
    HolderRevoked,
}

/// A guardian that met at least one replacement condition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplacementCandidate {
    pub seat_index: u16,
    pub mg_id: nostr_sdk::PublicKey,
    pub seat_id: SeatId,
    pub reasons: Vec<ReplacementReason>,
}

/// One complete check. An empty list does not assert that every guardian is healthy.
#[derive(Clone, Debug, Serialize)]
pub struct ReplacementCheck {
    /// Internal context for binding receipts, not part of the displayed result.
    #[serde(skip)]
    pub(crate) formation_id: crate::FormationId,
    /// In-memory approvals are not serialized with display data.
    #[serde(skip)]
    pub(crate) receipts: BTreeMap<u16, crate::ReplacementReceipt>,
    pub federation_id: FederationId,
    pub checked_at: u64,
    pub candidates: Vec<ReplacementCandidate>,
}

impl<I, P, N, F, C> FaClient<I, P, N, F, C>
where
    I: FaIdentity,
    P: FaPayments,
    N: FaNostrClient,
    F: ManifoldGuardianConnector,
    C: FederationConsensusReader,
{
    /// Check the formed federation now, using the authenticated TSP publication.
    /// Reports that cannot be fetched or verified provide no replacement evidence.
    pub(crate) async fn collect_replacement_candidates(&self) -> FaResult<ReplacementCheck> {
        // An idle or unreconciled client has no guardian set to judge.
        self.maintenance_authority(self.fa_id()?).await?;
        match self
            .admitted_setup_payment_federations(FormationRunOptions::default())
            .await
        {
            Ok(_) | Err(FaError::Timeout(_)) => {}
            Err(error) => return Err(error),
        }
        // A relay timeout does not erase an authenticated retained policy.
        // Missing or invalid stored policy still fails below.
        let policy = self.replacement_policy().await?;
        let _guard = self.inner.run_guard.try_lock().map_err(|_| FaError::Busy)?;
        let authority = self.maintenance_authority(self.fa_id()?).await?;
        let federation_id = FederationId(invite_federation_id(&authority.invite_code)?.to_string());
        // Without the seats' peer ids, peer reports say nothing about a seat.
        let seat_peers = self.seat_peers(&authority).await.unwrap_or_default();
        let peers = seat_peers.values().copied().collect::<BTreeSet<_>>();
        let reports = async {
            if peers.is_empty() {
                return None;
            }
            self.guardian_reports(&authority, &federation_id, &peers)
                .await
        };
        let revoked = join_all(authority.seats.iter().map(|seat| self.revoked_holder(seat)));
        let (reports, revoked) = futures::join!(reports, revoked);
        let reports = reports.unwrap_or_else(|| GuardianReports {
            federation_id: federation_id.clone(),
            peers: peers.clone(),
            sessions: BTreeMap::new(),
            availability: BTreeMap::new(),
        });
        let now = now_secs()?;
        let reasons = replacement_reasons(&reports, &policy, now);
        let candidates = authority
            .seats
            .iter()
            .zip(revoked)
            .filter_map(|(seat, revoked)| {
                let mut reasons = seat_peers
                    .get(&seat.index)
                    .and_then(|peer| reasons.get(peer))
                    .cloned()
                    .unwrap_or_default();
                if revoked {
                    reasons.push(ReplacementReason::HolderRevoked);
                }
                (!reasons.is_empty()).then(|| ReplacementCandidate {
                    seat_index: seat.index,
                    mg_id: seat.mg_id,
                    seat_id: seat.seat_id.clone(),
                    reasons,
                })
            })
            .collect();
        Ok(ReplacementCheck {
            formation_id: authority.formation_id,
            receipts: BTreeMap::new(),
            federation_id,
            checked_at: now,
            candidates,
        })
    }

    /// Each seat's Fedimint peer id, from the verified consensus seat
    /// binding that names the seat's MG. The seat index is the FA's order of
    /// the seats, not the peer id. `None` when the bindings cannot be read
    /// or verified, or do not name every seat's MG exactly once.
    async fn seat_peers(&self, authority: &PostFormedAuthority) -> Option<BTreeMap<u16, PeerId>> {
        let invite = self
            .dial_invite(&authority.invite_code, authority.seat_routes())
            .await;
        let consensus = timeout(
            REQUEST_TIMEOUT,
            self.inner.ports.consensus_reader.read_consensus(&invite),
        )
        .await
        .ok()?
        .ok()?;
        let federation = federation_seats(&consensus.config).ok()?;
        let directory = liquidity_seat_bindings_field(&consensus.meta_value).ok()??;
        let bindings = MgSeatBindings::parse_canonical(&directory)
            .and_then(|bindings| bindings.verify_for_federation(&federation))
            .ok()?;
        authority
            .seats
            .iter()
            .map(|seat| {
                let mut named = bindings
                    .iter()
                    .filter(|binding| binding.mg_pubkey.0 == seat.mg_id.to_string());
                let binding = named.next()?;
                if named.next().is_some() {
                    return None;
                }
                Some((seat.index, parse_protocol_peer_id(&binding.peer_id)?))
            })
            .collect()
    }

    async fn guardian_reports(
        &self,
        authority: &PostFormedAuthority,
        federation_id: &FederationId,
        peers: &BTreeSet<PeerId>,
    ) -> Option<GuardianReports> {
        let invite = self
            .dial_invite(&authority.invite_code, authority.seat_routes())
            .await;
        // The invite reader allows 30s for setup and 10s for reports.
        timeout(
            Duration::from_secs(45),
            self.inner
                .ports
                .consensus_reader
                .read_guardian_reports(&invite),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .filter(|report| {
            report.federation_id == *federation_id
                && report.peers == *peers
                && (!report.sessions.is_empty() || !report.availability.is_empty())
        })
    }

    async fn revoked_holder(&self, seat: &PostFormedSeat) -> bool {
        timeout(REQUEST_TIMEOUT, async {
            let client = self
                .inner
                .ports
                .mg_connector
                .connect(&seat.locator)
                .await
                .ok()?;
            let response = client
                .get_fman_trust_material(GetMgTrustMaterialRequest {
                    version: ProtocolV1,
                })
                .await
                .ok()?;
            let now = now_secs().ok()?;
            let material = response
                .verify_for_mg(
                    &Pubkey(seat.mg_id.to_string()),
                    fedi_decentralized_domain::Timestamp(now),
                    crate::liquidity::FA_LIQUIDITY_TRUST_MATERIAL_VALIDITY.as_secs(),
                )
                .ok()?;
            if material.holder_authorizations.is_empty() {
                return None;
            }
            // A failed lookup is not a revocation. Authenticate issuance first:
            // the verifier may find a revocation before checking envelope proofs.
            for envelope in &material.holder_authorizations {
                self.inner
                    .peer_badge_verifier
                    .verify_issuance_at(envelope, now)
                    .ok()?;
                if !matches!(
                    self.inner.peer_badge_verifier.verify(envelope).await,
                    Err(PeerBadgeVerificationError::CredentialRevoked)
                ) {
                    return Some(false);
                }
            }
            Some(true)
        })
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
    }
}

fn replacement_reasons(
    reports: &GuardianReports,
    policy: &AdmittedSetupPaymentFederations,
    now: u64,
) -> BTreeMap<PeerId, Vec<ReplacementReason>> {
    let num_peers = reports.peers.to_num_peers();
    let mut availability = reports.availability.clone();
    availability.retain(|reporter, _| reports.peers.contains(reporter));
    for report in availability.values_mut() {
        report
            .last_seen_secs
            .retain(|peer, seen| reports.peers.contains(peer) && *seen <= now);
        for window in &mut report.windows {
            if window.coverage_bps > 10_000 {
                window.peers.clear();
            }
            window
                .peers
                .retain(|peer, score| reports.peers.contains(peer) && *score <= 10_000);
        }
    }
    let merged = merge_peer_availability(&availability, num_peers);
    let min = policy.min_uptime_percent();
    reports
        .peers
        .iter()
        .map(|peer| {
            let mut reasons = Vec::new();
            if let Some(last_seen) = merged.last_seen_secs.get(peer) {
                if now.saturating_sub(*last_seen) > policy.max_guardian_offline_secs() {
                    reasons.push(ReplacementReason::Offline {
                        last_seen_secs: *last_seen,
                    });
                }
            } else {
                let mut gaps = reports
                    .sessions
                    .iter()
                    .filter(|(reporter, _)| *reporter != peer && reports.peers.contains(reporter))
                    .filter_map(|(_, report)| {
                        let last = (*report.last_contribution.get(peer)?)?;
                        // A contribution to the current session means no missed
                        // completed sessions, not missing history.
                        (last <= report.session_count)
                            .then(|| report.session_count.saturating_sub(last.saturating_add(1)))
                    })
                    .collect::<Vec<_>>();
                gaps.sort_unstable_by(|a, b| b.cmp(a));
                if let Some(gap) = gaps.get(num_peers.one_honest() - 1) {
                    if gap.saturating_mul(SESSION_SECONDS) > policy.max_guardian_offline_secs() {
                        reasons.push(ReplacementReason::MissedSessions { sessions: *gap });
                    }
                } else {
                    // Unknown history is not itself evidence. In this fallback,
                    // f+1 other peers must explicitly report disconnection now.
                    let disconnected = reports
                        .sessions
                        .iter()
                        .filter(|(reporter, report)| {
                            *reporter != peer
                                && reports.peers.contains(reporter)
                                && report.disconnected.contains(peer)
                        })
                        .count();
                    if disconnected >= num_peers.one_honest() {
                        reasons.push(ReplacementReason::Disconnected);
                    }
                }
            }
            for (index, minimum) in [(0, min.week), (1, min.month)] {
                if let Some(score) = merged
                    .windows
                    .get(index)
                    .and_then(|scores| scores.get(peer))
                    && *score < u16::from(minimum) * 100
                {
                    reasons.push(if index == 0 {
                        ReplacementReason::LowWeekScore {
                            basis_points: *score,
                        }
                    } else {
                        ReplacementReason::LowMonthScore {
                            basis_points: *score,
                        }
                    });
                }
            }
            (*peer, reasons)
        })
        .collect()
}

#[cfg(test)]
mod tests;
