//! Staged rollouts of MG features
//! (`specs/SPEC-setup-payment-federations.md#staged-rollouts`).
//!
//! Each MG holds one private random [`RolloutSeed`]. A seat's draw for a
//! feature hashes the seed with the seat id and the feature name, so draws are
//! uniform, stable across restarts, and independent between seats, features
//! and MGs; the publisher cannot choose which seats a percentage selects. The
//! code that defines a feature chooses its [`RolloutUnit`]: per seat by
//! default, per MG for a host-level feature.
//!
//! [`spawn_rollout_log`] follows every published schedule for every live seat
//! and logs each change, before any feature reads its state, so a rollout of a
//! name no code knows can be watched in telemetry.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fedi_decentralized_domain::{
    AdmittedSetupPaymentFederations, RolloutDraw, RolloutFeature, Rollouts,
};
use fedi_decentralized_service_manifold_guardian::SeatId;
use rand::RngCore as _;
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;

use crate::db::Db;
use crate::fleet::Fleet;

#[cfg(test)]
#[path = "../tests/rollout.rs"]
mod tests;

/// How often the log re-evaluates schedules between publications.
const EVALUATE_INTERVAL: Duration = Duration::from_secs(30);

/// Domain separation for [`RolloutSeed::draw`]; a new version draws anew.
const DRAW_DOMAIN: &[u8] = b"fedi-manifold-guardian/rollout-draw/v1";

/// This MG's private rollout seed: 32 random bytes from the OS RNG, stored
/// once in the database. It is not derived from the root mnemonic, so no
/// emitted rollout state depends on an identity secret. Never log it or a
/// draw: knowing them tells which seats a percentage selects.
pub struct RolloutSeed([u8; 32]);

/// What one draw is for: one seat, or the whole MG.
#[derive(Clone, Copy)]
pub enum RolloutUnit<'a> {
    Seat(&'a SeatId),
    Mg,
}

impl RolloutSeed {
    /// The stored seed, created on the first call for this data root.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn load(db: &Db) -> anyhow::Result<Self> {
        let mut fresh = [0; 32];
        rand::rngs::OsRng.fill_bytes(&mut fresh);
        Ok(Self(db.rollout_seed(fresh).await?))
    }

    /// The uniform draw of `unit` for `feature`: the first four bytes of
    /// SHA-256 over the domain, the seed, the unit and the feature name.
    #[must_use]
    pub fn draw(&self, unit: RolloutUnit<'_>, feature: &RolloutFeature) -> RolloutDraw {
        let mut hash = Sha256::new();
        hash.update(DRAW_DOMAIN);
        hash.update(self.0);
        match unit {
            RolloutUnit::Seat(seat_id) => {
                hash.update([1]);
                hash.update(seat_id.as_bytes());
            }
            RolloutUnit::Mg => hash.update([2]),
        }
        hash.update(feature.as_str());
        let digest = hash.finalize();
        RolloutDraw::new(u32::from_be_bytes(
            digest[..4].try_into().expect("SHA-256 has four bytes"),
        ))
    }
}

/// One change of a seat's state for a feature, or its first state.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Flip {
    seat_id: SeatId,
    feature: RolloutFeature,
    enabled: bool,
}

/// The last state logged for each live seat and published feature.
#[derive(Default)]
struct RolloutLog(BTreeMap<(SeatId, RolloutFeature), bool>);

impl RolloutLog {
    /// Evaluate every published feature for every seat in `seats` and return
    /// the states that differ from the last ones: all of them the first time,
    /// and `false` for a seat that ran a feature the publication dropped. A
    /// dropped seat leaves the log silently.
    fn update(
        &mut self,
        seed: &RolloutSeed,
        rollouts: &Rollouts,
        seats: &[SeatId],
        now_secs: u64,
    ) -> Vec<Flip> {
        let mut next = BTreeMap::new();
        for seat_id in seats {
            for (feature, schedule) in rollouts.iter() {
                let draw = seed.draw(RolloutUnit::Seat(seat_id), feature);
                next.insert(
                    (seat_id.clone(), feature.clone()),
                    schedule.includes(draw, now_secs),
                );
            }
        }
        let mut flips = Vec::new();
        for (key, &enabled) in &next {
            if self.0.get(key) != Some(&enabled) {
                flips.push(Flip {
                    seat_id: key.0.clone(),
                    feature: key.1.clone(),
                    enabled,
                });
            }
        }
        for ((seat_id, feature), enabled) in &self.0 {
            if *enabled && rollouts.get(feature).is_none() && seats.contains(seat_id) {
                flips.push(Flip {
                    seat_id: seat_id.clone(),
                    feature: feature.clone(),
                    enabled: false,
                });
            }
        }
        self.0 = next;
        flips
    }
}

/// Log every seat's rollout state until the runtime shuts down: its first
/// state and each change, evaluated on every newly admitted publication and
/// every [`EVALUATE_INTERVAL`] against the wall clock. Without a publication
/// no feature runs and nothing is logged.
pub fn spawn_rollout_log(
    fleet: Arc<Fleet>,
    mut policy: watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
    seed: RolloutSeed,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut log = RolloutLog::default();
        loop {
            let rollouts = policy
                .borrow_and_update()
                .as_ref()
                .map(|policy| policy.rollout().clone())
                .unwrap_or_default();
            let now_secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_secs());
            for flip in log.update(&seed, &rollouts, &fleet.live_seat_ids(), now_secs) {
                tracing::info!(
                    safe_to_share = true,
                    seat_id = %flip.seat_id,
                    feature = %flip.feature,
                    enabled = flip.enabled,
                    "seat rollout feature state"
                );
            }
            tokio::select! {
                changed = policy.changed() => {
                    if changed.is_err() {
                        // The publication runtime ended; keep following the
                        // schedules held.
                        tokio::time::sleep(EVALUATE_INTERVAL).await;
                    }
                }
                () = tokio::time::sleep(EVALUATE_INTERVAL) => {}
            }
        }
    })
}
