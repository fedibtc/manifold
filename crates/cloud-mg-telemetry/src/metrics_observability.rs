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

/// The collection stage at which one target poll lost data before admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CollectionFailure {
    /// No authenticated connection to the MG.
    Connect,
    /// The seat listing failed or was malformed.
    List,
    /// One seat's scrape failed or returned an unusable response.
    Scrape,
    /// The target budget ran out; recorded at most once per target poll.
    Deadline,
}

impl CollectionFailure {
    const ALL: [Self; 4] = [Self::Connect, Self::List, Self::Scrape, Self::Deadline];

    fn index(self) -> usize {
        match self {
            Self::Connect => 0,
            Self::List => 1,
            Self::Scrape => 2,
            Self::Deadline => 3,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::List => "list",
            Self::Scrape => "scrape",
            Self::Deadline => "deadline",
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
    collection_failures: [AtomicU64; 4],
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

    /// Record one collection failure. It carries no target or error detail.
    pub(crate) fn record_collection_failure(&self, failure: CollectionFailure) {
        self.inner.collection_failures[failure.index()].fetch_add(1, Ordering::Relaxed);
    }

    /// Render the complete fixed-cardinality Prometheus counter families.
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
        output.push_str("# HELP cloud_fman_telemetry_metrics_collection_failures_total Guardian metrics target-poll failures by collection stage.\n");
        output.push_str("# TYPE cloud_fman_telemetry_metrics_collection_failures_total counter\n");
        for failure in CollectionFailure::ALL {
            writeln!(
                output,
                "cloud_fman_telemetry_metrics_collection_failures_total{{stage=\"{}\"}} {} {now_ms}",
                failure.label(),
                self.inner.collection_failures[failure.index()].load(Ordering::Relaxed),
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
