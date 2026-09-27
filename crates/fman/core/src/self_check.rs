//! Bounded, operator-initiated observations. Only closed codes cross the admin boundary.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{Semaphore, watch};
use tokio::time::Instant;

use crate::directory::OnboardingStatus;

/// The eight fixed observations, in display order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum CheckId {
    DiscoveryDns,
    DiscoveryHttps,
    FmanRelay,
    BitcoinDns,
    BitcoinPrimary,
    BitcoinFallback,
    GuardianHealth,
    DirectoryObservation,
}

/// Coarse outcome, never a remote response or error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warning,
    Failure,
    Unknown,
    NotApplicable,
}

/// Fixed explanation code. Arbitrary strings are intentionally impossible.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    Reached,
    NoRecords,
    Timeout,
    Unreachable,
    NumericHost,
    NotHttps,
    UnsupportedConfiguration,
    UnsupportedProxyConfiguration,
    RunDeadline,
    Connected,
    Disconnected,
    NotSelected,
    Disabled,
    HttpAccess,
    HttpService,
    InvalidResponse,
    WrongNetwork,
    Synchronizing,
    Starting,
    RequestRejected,
    NotConfigured,
    CachedHealthy,
    CachedUnavailable,
    NoFormedSeats,
    RetainedAuthorization,
    Checking,
    NotObserved,
    PreviousRelayError,
}

/// One observation, with no free-form detail field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct Check {
    /// Fixed observation identity.
    pub check_id: CheckId,
    /// Fixed coarse status.
    pub status: CheckStatus,
    /// Fixed explanation code.
    pub reason_code: ReasonCode,
}

impl Check {
    /// Construct a closed observation.
    pub fn new(check_id: CheckId, status: CheckStatus, reason_code: ReasonCode) -> Self {
        Self {
            check_id,
            status,
            reason_code,
        }
    }
}

/// Versioned, deliberately small operator report.
#[derive(serde::Serialize, ts_rs::TS)]
pub struct SelfCheckReport {
    /// Wire schema version.
    pub schema_version: u8,
    /// Exactly eight observations in fixed order.
    pub checks: [Check; 8],
}

/// Admission or completed report, with no request-supplied inputs.
#[derive(serde::Serialize, ts_rs::TS)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SelfCheckResponse {
    /// Observations completed (unfinished probes are marked unknown).
    Completed { report: SelfCheckReport },
    /// Another run is active.
    Busy,
    /// A run was recently started.
    Cooldown,
    /// Daemon is stopping or adapter cannot run.
    Unavailable,
}

/// Network observations implemented by the daemon using its configured dependencies.
#[async_trait::async_trait]
pub trait ConnectivityChecks: Send + Sync {
    /// Fill the first six slots as bounded operations complete; never put raw data in slots.
    async fn run(&self, slots: Arc<Mutex<[Check; 6]>>);
}

struct Admission {
    stopped: bool,
    last_start: Option<Instant>,
}

/// Global admission, deadline, and shutdown owner for operator self-checks.
pub struct SelfCheck {
    checks: Arc<dyn ConnectivityChecks>,
    active: Arc<Semaphore>,
    admission: Mutex<Admission>,
    stop: watch::Sender<bool>,
}

impl SelfCheck {
    /// Create a controller with one global active run.
    pub fn new(checks: Arc<dyn ConnectivityChecks>) -> Self {
        let (stop, _) = watch::channel(false);
        Self {
            checks,
            active: Arc::new(Semaphore::new(1)),
            admission: Mutex::new(Admission {
                stopped: false,
                last_start: None,
            }),
            stop,
        }
    }

    /// Run cached observations and bounded network work without retaining the report.
    pub async fn run(&self, guardian: Check, directory: Check) -> SelfCheckResponse {
        let (permit, mut stop) = {
            let mut admission = self.admission.lock().expect("admission mutex");
            if admission.stopped {
                return SelfCheckResponse::Unavailable;
            }
            let Ok(permit) = self.active.clone().try_acquire_owned() else {
                return SelfCheckResponse::Busy;
            };
            let now = Instant::now();
            if admission
                .last_start
                .is_some_and(|start| now.duration_since(start) < Duration::from_secs(30))
            {
                return SelfCheckResponse::Cooldown;
            }
            admission.last_start = Some(now);
            (permit, self.stop.subscribe())
        };
        let slots = Arc::new(Mutex::new([
            Check::new(
                CheckId::DiscoveryDns,
                CheckStatus::Unknown,
                ReasonCode::RunDeadline,
            ),
            Check::new(
                CheckId::DiscoveryHttps,
                CheckStatus::Unknown,
                ReasonCode::RunDeadline,
            ),
            Check::new(
                CheckId::FmanRelay,
                CheckStatus::Unknown,
                ReasonCode::RunDeadline,
            ),
            Check::new(
                CheckId::BitcoinDns,
                CheckStatus::Unknown,
                ReasonCode::RunDeadline,
            ),
            Check::new(
                CheckId::BitcoinPrimary,
                CheckStatus::Unknown,
                ReasonCode::RunDeadline,
            ),
            Check::new(
                CheckId::BitcoinFallback,
                CheckStatus::Unknown,
                ReasonCode::RunDeadline,
            ),
        ]));
        tokio::select! {
            _ = self.checks.run(slots.clone()) => {}
            _ = tokio::time::sleep(Duration::from_secs(12)) => {}
            _ = stop.changed() => {
                return SelfCheckResponse::Unavailable;
            }
        }
        let network = *slots.lock().expect("network slots mutex");
        drop(permit);
        SelfCheckResponse::Completed {
            report: SelfCheckReport {
                schema_version: 1,
                checks: [
                    network[0], network[1], network[2], network[3], network[4], network[5],
                    guardian, directory,
                ],
            },
        }
    }

    /// Refuse admission, cancel work, then wait for the active run to release its permit.
    pub async fn shutdown(&self) {
        {
            let mut admission = self.admission.lock().expect("admission mutex");
            admission.stopped = true;
            self.stop.send_replace(true);
        }
        if let Ok(permit) = self.active.acquire().await {
            drop(permit);
        }
    }
}

#[cfg(test)]
#[path = "../tests/self_check.rs"]
mod tests;

/// Project retained enrollment without copying holder or relay details.
pub fn directory_check(state: &OnboardingStatus) -> Check {
    let (status, reason) = match state {
        OnboardingStatus::AuthorizationObserved { .. } => {
            (CheckStatus::Pass, ReasonCode::RetainedAuthorization)
        }
        OnboardingStatus::Checking => (CheckStatus::Unknown, ReasonCode::Checking),
        OnboardingStatus::NotObserved { .. } => (CheckStatus::Warning, ReasonCode::NotObserved),
        OnboardingStatus::RelayError { .. } => {
            (CheckStatus::Unknown, ReasonCode::PreviousRelayError)
        }
    };
    Check::new(CheckId::DirectoryObservation, status, reason)
}

#[cfg(test)]
struct NoNetworkChecks;

#[cfg(test)]
#[async_trait::async_trait]
impl ConnectivityChecks for NoNetworkChecks {
    async fn run(&self, _slots: Arc<Mutex<[Check; 6]>>) {}
}

#[cfg(test)]
impl SelfCheck {
    /// Construct an inert adapter for admin transport tests.
    pub(crate) fn test_new() -> Arc<Self> {
        Arc::new(Self::new(Arc::new(NoNetworkChecks)))
    }
}
