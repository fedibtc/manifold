//! In-memory ordering for the one writer of a provider's relay coordinate.

use std::time::Duration;

use fedi_decentralized_service_liquidity_manager::{ServiceError, ServiceResult, Timestamp};
use tokio::time::Instant;

/// Bound clock pacing independently of queueing behind relay I/O.
pub(crate) const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Held under the daemon's advertisement mutex through the complete relay I/O.
///
/// Restart deliberately forgets event history. The two-second startup guard
/// outlives the previous writer's now+1 timestamps only with one writer and a
/// nondecreasing wall clock across restart; this is not durable total ordering.
pub(crate) struct AdvertisementWriter {
    /// All relay mutations, including admin calls, wait through this startup gate.
    first_write_at: Instant,
    /// Startup second+1 initially, then the latest attempted event timestamp.
    last_attempt: Timestamp,
}

impl Default for AdvertisementWriter {
    fn default() -> Self {
        Self::new(crate::now_timestamp())
    }
}

impl AdvertisementWriter {
    /// Starts a fresh process-local writer without reading persisted timestamps.
    pub(crate) fn new(startup: Timestamp) -> Self {
        Self {
            first_write_at: Instant::now() + Duration::from_secs(2),
            last_attempt: Timestamp(startup.0.saturating_add(1)),
        }
    }

    /// Samples a strictly newer wall-clock second after waiting, before relay I/O.
    ///
    /// Ambiguous sends and cancellation after this returns consume their slot.
    /// A stalled/backward clock fails within the same monotonic deadline rather
    /// than inventing a future timestamp or waiting indefinitely.
    pub(crate) async fn next_timestamp(
        &mut self,
        deadline: Instant,
        clock: impl Fn() -> Timestamp,
    ) -> ServiceResult<Timestamp> {
        tokio::time::timeout_at(deadline, async {
            tokio::time::sleep_until(self.first_write_at).await;
            loop {
                let now = clock();
                if now > self.last_attempt {
                    self.last_attempt = now;
                    return now;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
        .await
        .map_err(|_| wait_error())
    }
}

/// Reports bounded clock-pacing failure.
pub(crate) fn wait_error() -> ServiceError {
    crate::failed_precondition("advertisement writer could not obtain a newer clock second in time")
}

#[cfg(test)]
#[path = "../tests/advertisement_writer.rs"]
mod tests;
