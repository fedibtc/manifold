//! Local daily observations of the seat's rolling 30-day backup count.
use std::time::Duration;

use fedi_decentralized_service_manifold_guardian::{
    ActiveWalletHistory, SeatHealth, SeatId, WindowedFigure,
};
use tokio::sync::{Mutex, watch};

use super::{SeatRuntimeFacts, SeatVerbError, module_id};
use crate::db::{Db, now_ms};
use crate::fedimint_api::FedimintApi;
use crate::fedimint_stats::{held_for_backup_window, held_from_formation};

const DAY_MS: i64 = 86_400_000;

pub(super) struct DailyWalletSamples {
    db: Db,
    seat: SeatId,
    client: FedimintApi,
    // A first RPC of the day can race the periodic sampler on the same seat.
    sampling: Mutex<()>,
}

impl DailyWalletSamples {
    pub(super) fn new(db: Db, seat: SeatId, client: FedimintApi) -> Self {
        Self {
            db,
            seat,
            client,
            sampling: Mutex::new(()),
        }
    }

    pub(super) async fn run(&self, mut runtime: watch::Receiver<SeatRuntimeFacts>) {
        loop {
            let ready = |facts: &SeatRuntimeFacts| {
                facts.formed_invite.is_some()
                    && facts.decommissioned_at_ms.is_none()
                    && facts.transfer_status().is_none()
                    && facts.health == SeatHealth::Healthy
            };
            if runtime.wait_for(ready).await.is_err() {
                return;
            }
            let delay = match self.today().await {
                Ok((day, _)) => {
                    Duration::from_millis(((day + 1) * DAY_MS - now_ms()).max(0) as u64)
                }
                Err(error) => {
                    tracing::warn!(seat_id = %self.seat, %error, "could not sample active wallets");
                    Duration::from_secs(300)
                }
            };
            tokio::time::sleep(delay).await;
        }
    }

    pub(super) async fn report(
        &self,
    ) -> Result<(WindowedFigure, ActiveWalletHistory), SeatVerbError> {
        let (day, figure) = self.today().await?;
        let previous_30d = self
            .db
            .active_wallet_sample(
                &self.seat,
                day - i64::try_from(ActiveWalletHistory::LOOKBACK_DAYS)
                    .expect("lookback fits in i64"),
            )
            .await
            .map_err(SeatVerbError::internal)?;
        Ok((
            figure,
            ActiveWalletHistory {
                utc_day: day as u64,
                previous_30d,
            },
        ))
    }

    async fn today(&self) -> Result<(i64, WindowedFigure), SeatVerbError> {
        let _sampling = self.sampling.lock().await;
        let day = now_ms() / DAY_MS;
        if let Some(sample) = self
            .db
            .active_wallet_sample(&self.seat, day)
            .await
            .map_err(SeatVerbError::internal)?
        {
            return Ok((day, sample));
        }
        let formed_at_ms = self
            .db
            .formed_at_ms(&self.seat)
            .await
            .map_err(SeatVerbError::internal)?
            .ok_or_else(|| {
                SeatVerbError::internal(anyhow::anyhow!("a running seat has no formed row"))
            })?;
        let backups = self
            .client
            .backup_statistics()
            .await
            .map_err(SeatVerbError::child_needed)?;
        let observed = now_ms();
        let day = observed / DAY_MS;
        // Only a young seat needs session zero to distinguish formation from
        // a transfer. Date the count at observation, not after other RPCs.
        let complete = if held_for_backup_window(formed_at_ms, observed) {
            true
        } else {
            let config = self
                .client
                .client_config()
                .await
                .map_err(SeatVerbError::child_needed)?;
            let lnv2 = module_id(&config, &fedimint_lnv2_common::KIND)?;
            let start = self
                .client
                .federation_start_unix_time(lnv2)
                .await
                .map_err(SeatVerbError::child_needed)?;
            held_from_formation(formed_at_ms, start)
        };
        let figure = WindowedFigure {
            value: u64::try_from(backups.refreshed_1m).map_err(SeatVerbError::internal)?,
            complete,
        };
        self.db
            .record_active_wallet_sample(&self.seat, day, figure)
            .await
            .map_err(SeatVerbError::internal)?;
        Ok((day, figure))
    }
}
