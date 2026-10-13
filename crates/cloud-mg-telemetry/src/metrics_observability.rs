//! Bounded process-local observability for guardian metrics admission.

use std::{
    fmt::Write as _,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const DIAGNOSTIC_INTERVAL: Duration = Duration::from_secs(300);

/// One fixed guardian metrics admission outcome.
#[derive(Clone, Copy)]
pub(crate) enum AdmissionOutcome {
    /// The response produced a safe projection.
    Admitted,
    /// At least one reviewed-deny family was discarded.
    KnownDenyDiscarded,
    /// At least one unknown family was discarded.
    UnknownDiscarded,
    /// At least one invalid admitted family was discarded.
    InvalidAdmittedDiscarded,
    /// The complete response failed the admission policy.
    Rejected,
    /// The seat lacked a valid current federation invite.
    InvalidFederationInvite,
}

impl AdmissionOutcome {
    fn index(self) -> usize {
        match self {
            Self::Admitted => 0,
            Self::KnownDenyDiscarded => 1,
            Self::UnknownDiscarded => 2,
            Self::InvalidAdmittedDiscarded => 3,
            Self::Rejected => 4,
            Self::InvalidFederationInvite => 5,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::KnownDenyDiscarded => "known_deny_discarded",
            Self::UnknownDiscarded => "unknown_discarded",
            Self::InvalidAdmittedDiscarded => "invalid_admitted_discarded",
            Self::Rejected => "rejected",
            Self::InvalidFederationInvite => "invalid_federation_invite",
        }
    }
}

/// Process-local fixed-cardinality admission counters and diagnostic limiter.
#[derive(Clone, Default)]
pub(crate) struct MetricsObservability {
    inner: Arc<MetricsObservabilityInner>,
}

#[derive(Default)]
struct MetricsObservabilityInner {
    counts: [AtomicU64; 6],
    last_rejection_diagnostic: Mutex<Option<Instant>>,
}

impl MetricsObservability {
    /// Record one admission outcome and emit at most one fixed rejection diagnostic per interval.
    pub(crate) fn record(&self, outcome: AdmissionOutcome) {
        self.inner.counts[outcome.index()].fetch_add(1, Ordering::Relaxed);
        if matches!(
            outcome,
            AdmissionOutcome::UnknownDiscarded
                | AdmissionOutcome::InvalidAdmittedDiscarded
                | AdmissionOutcome::Rejected
                | AdmissionOutcome::InvalidFederationInvite
        ) && self.allow_rejection_diagnostic()
        {
            // The message and field are fixed. They contain no response, family,
            // label, value, target identity, endpoint, or dependency error.
            tracing::warn!(
                safe_to_share = true,
                reason = outcome.label(),
                "guardian metrics projection degraded"
            );
        }
    }

    /// Render the complete fixed-cardinality Prometheus counter family.
    pub(crate) fn render(&self, output: &mut String, now_ms: i64) -> Result<(), std::fmt::Error> {
        output.push_str("# HELP cloud_fman_telemetry_metrics_admission_total Guardian metrics projection events by bounded category.\n");
        output.push_str("# TYPE cloud_fman_telemetry_metrics_admission_total counter\n");
        for outcome in [
            AdmissionOutcome::Admitted,
            AdmissionOutcome::KnownDenyDiscarded,
            AdmissionOutcome::UnknownDiscarded,
            AdmissionOutcome::InvalidAdmittedDiscarded,
            AdmissionOutcome::Rejected,
            AdmissionOutcome::InvalidFederationInvite,
        ] {
            writeln!(
                output,
                "cloud_fman_telemetry_metrics_admission_total{{event=\"{}\"}} {} {now_ms}",
                outcome.label(),
                self.inner.counts[outcome.index()].load(Ordering::Relaxed),
            )?;
        }
        Ok(())
    }

    fn allow_rejection_diagnostic(&self) -> bool {
        let now = Instant::now();
        let mut last = self
            .inner
            .last_rejection_diagnostic
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.is_some_and(|last| now.saturating_duration_since(last) < DIAGNOSTIC_INTERVAL) {
            false
        } else {
            *last = Some(now);
            true
        }
    }
}
