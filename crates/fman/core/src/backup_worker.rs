//! The backup worker: reconciles what the relay serves with what each seat's
//! durable state says it should
//! ([SPEC-nostr-backup-restore](../../specs/SPEC-nostr-backup-restore.md)).
//!
//! The relay is semi-trusted: it sees only ciphertext, count, and timing, and
//! it is trusted to keep serving the latest event published at each
//! coordinate. A read-back-confirmed publication is therefore a durable fact
//! (`seat_backup_publications`), and dirtiness is *derived*, never tracked: a
//! seat needs publishing exactly when the document assembled from its durable
//! state no longer hashes to what the relay was last confirmed to serve.
//! There is no in-memory queue to lose, no startup republication of unchanged
//! state, and no runtime flag whose loss could downgrade a relay document.
//!
//! One task scans every seat on a slow cadence and converges the diff;
//! [`BackupWorker::mark`] cuts the wait short so a state transition — a new
//! seat's payment evidence, a fresh guardian archive, a decommission — reaches
//! the relay promptly rather than at the next tick. **Nothing waits for it**:
//! not a caller, not a state transition. A scan reads SQLite and seat data
//! directories only, never a child's API, so no cadence can hammer
//! `fedimintd`; with confirmed archives skipping their file reads, a scan is
//! a few point queries and one small hash per seat.
//!
//! Failures back off exponentially with jitter, capped at the scan interval:
//! the relay is shared fleet-wide, and unjittered retries would hit it in
//! synchronized pulses exactly when it is weakest. Every publish call carries
//! its own deadline, so one wedged relay request costs a timeout, not the
//! worker — and the operator-facing scan record (`last_scan`) goes stale
//! rather than silent if the worker itself dies.

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use tokio::sync::{Notify, watch};

use crate::backup::{BackupSink, seat_publication_plan};
use crate::db::{Db, now_ms};
use crate::seat_process::SeatProcessConfig;

/// How often the worker rescans every seat with nothing marked. Cheap enough
/// to be a liveness floor (a few milliseconds of local reads per scan at the
/// recommended seat counts) while catching anything a mark missed — including
/// a crash between a confirmed publish and its record.
pub const DEFAULT_SCAN_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// First delay after a failed scan; doubles per failure up to the scan
/// interval.
const INITIAL_BACKOFF: Duration = Duration::from_secs(15);

/// Deadline on one seat's whole publication — up to an archive's worth of
/// events plus their read-backs. The sink's own transport timeouts are its
/// business; this bound is the worker's, so a relay that accepts a connection
/// and then wedges cannot own the fleet's only publisher forever.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What the last completed scan found, for the operator's backup-health view.
/// A stale `completed_at_ms` is itself signal: the worker is wedged or dead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackupScanOutcome {
    pub completed_at_ms: i64,
    /// Seats whose publication failed or was refused this scan (an archive
    /// still being written by `fedimintd` counts until it lands).
    pub pending_seats: u32,
}

/// The reconciling publisher of every seat's recovery documents.
pub struct BackupWorker {
    sink: Arc<dyn BackupSink>,
    /// Cuts the current wait short. A wake during a scan is retained by the
    /// `Notify` permit, so a transition landing mid-scan triggers a rescan
    /// rather than being lost to it.
    wake: Arc<Notify>,
    scan_interval: Duration,
    last_scan: watch::Sender<Option<BackupScanOutcome>>,
}

impl BackupWorker {
    pub fn new(sink: Arc<dyn BackupSink>, scan_interval: Duration) -> Arc<Self> {
        let (last_scan, _) = watch::channel(None);
        Arc::new(Self {
            sink,
            wake: Arc::new(Notify::new()),
            scan_interval,
            last_scan,
        })
    }

    /// Start the worker.
    pub fn spawn(self: &Arc<Self>, db: Db, process: SeatProcessConfig) {
        tokio::spawn(Self::run(Arc::downgrade(self), db, process));
    }

    /// Hint that some seat's durable state changed. Call at state transitions
    /// — creation, the first observation of consensus, the recording of a
    /// federation invite, decommission — purely for promptness: correctness
    /// is the scan's, which needs no hint.
    pub fn mark(&self) {
        self.wake.notify_one();
    }

    /// The last completed scan, if any. `None` before the first scan
    /// finishes; a `completed_at_ms` much older than the scan interval means
    /// the worker is wedged or dead.
    pub fn last_scan(&self) -> Option<BackupScanOutcome> {
        *self.last_scan.borrow()
    }

    /// The storage format version the wired sink writes: the version this
    /// worker's confirmed-publication records are scoped to.
    pub fn format_version(&self) -> u32 {
        self.sink.format_version()
    }

    async fn run(worker: std::sync::Weak<Self>, db: Db, process: SeatProcessConfig) {
        let Some(first) = worker.upgrade() else {
            return;
        };
        // Tests run scan intervals shorter than the production first-retry
        // delay; the interval is the ceiling on every wait, first included.
        let initial_backoff = INITIAL_BACKOFF.min(first.scan_interval);
        let mut backoff = initial_backoff;
        drop(first);
        loop {
            let Some(current) = worker.upgrade() else {
                return;
            };
            let delay = match current.scan(&current.sink, &db, &process).await {
                Ok(0) => {
                    backoff = initial_backoff;
                    current.scan_interval
                }
                Ok(_) => {
                    let delay = backoff;
                    backoff = (backoff * 2).min(current.scan_interval);
                    delay
                }
                Err(error) => {
                    tracing::warn!(
                        error = format_args!("{error:#}"),
                        "backup scan failed to enumerate seats"
                    );
                    tracing::warn!(
                        safe_to_share = true,
                        stage = "backup_scan",
                        failure_kind = "enumeration_failed",
                        "backup scan failed to enumerate seats"
                    );
                    let delay = backoff;
                    backoff = (backoff * 2).min(current.scan_interval);
                    delay
                }
            };
            let wake = current.wake.clone();
            drop(current);
            tokio::select! {
                () = tokio::time::sleep(jittered(delay)) => {}
                () = wake.notified() => {}
            }
        }
    }

    /// Reconcile every seat once. Per-seat failures are counted and logged,
    /// never fatal to the scan: one seat mid-DKG must not delay another
    /// seat's payment evidence.
    async fn scan(
        &self,
        sink: &Arc<dyn BackupSink>,
        db: &Db,
        process: &SeatProcessConfig,
    ) -> anyhow::Result<u32> {
        let seats = db.list_seats().await?;
        let mut pending = 0_u32;
        for seat in &seats {
            if let Err(error) = self.reconcile_seat(sink, db, process, seat).await {
                pending += 1;
                tracing::warn!(
                    error = format_args!("{error:#}"),
                    seat_id = %seat.facts.seat_id,
                    "failed to publish the seat's recovery document"
                );
                tracing::warn!(
                    safe_to_share = true,
                    seat_id = %seat.facts.seat_id,
                    stage = "backup_publish",
                    failure_kind = "publish_failed",
                    "failed to publish the seat's recovery document"
                );
            }
        }
        self.last_scan.send_replace(Some(BackupScanOutcome {
            completed_at_ms: now_ms(),
            pending_seats: pending,
        }));
        Ok(pending)
    }

    async fn reconcile_seat(
        &self,
        sink: &Arc<dyn BackupSink>,
        db: &Db,
        process: &SeatProcessConfig,
        seat: &crate::db::SeatRecord,
    ) -> anyhow::Result<()> {
        // Version-scoped: a record written by another storage-format version
        // is no confirmation here — the relay would be serving events the
        // sink's own reader refuses, so the whole publication (archive
        // included) republishes under the current version.
        let recorded = db
            .backup_publication(&seat.facts.seat_id, sink.format_version())
            .await?;
        let confirmed_digest = recorded
            .as_ref()
            .and_then(|record| record.archive_digest.as_deref());
        let plan = seat_publication_plan(db, process, &seat.facts, confirmed_digest).await?;
        let doc_sha256 = plan.doc_sha256();
        // An unconfirmed archive republishes even under an unchanged document:
        // a crash after publishing it but before the record leaves exactly
        // that state, and rewriting immutable bytes at the same coordinates
        // is the cheap side of the ambiguity.
        let unchanged = plan.archive.is_none()
            && recorded.is_some_and(|record| record.doc_sha256 == doc_sha256);
        if unchanged {
            return Ok(());
        }
        tokio::time::timeout(PUBLISH_TIMEOUT, sink.publish(&plan))
            .await
            .map_err(|_| {
                anyhow!(
                    "publish did not resolve within {}s",
                    PUBLISH_TIMEOUT.as_secs()
                )
            })??;
        // The plan's consensus check and this publish are not one atomic step:
        // the first observation can commit between them, making the document
        // just confirmed a guardian-less description of a seat that now holds
        // key shares. Recording it would suppress the republish that fixes it,
        // so re-derive the requirement and leave the seat pending instead —
        // the next scan publishes the document with its archive.
        if plan.archive_digest().is_none()
            && db
                .formed_federation_invite(&seat.facts.seat_id)
                .await?
                .is_some()
        {
            anyhow::bail!(
                "consensus was observed while a pre-consensus document was in flight; \
                 republishing with the guardian archive"
            );
        }
        db.record_backup_publication(
            &seat.facts.seat_id,
            &doc_sha256,
            plan.archive_digest(),
            sink.format_version(),
        )
        .await?;
        // Fires per confirmed publication, not per scan: an unchanged seat
        // returns before the publish call, so a quiet fleet logs nothing.
        tracing::info!(
            safe_to_share = true,
            seat_id = %seat.facts.seat_id,
            stage = "backup_publish",
            archive_confirmed = plan.archive_digest().is_some(),
            "confirmed the seat's recovery document on the relay"
        );
        Ok(())
    }
}

/// 80–120% of `delay`: enough spread that a fleet restarted by one deploy
/// does not retry a shared relay in synchronized pulses.
fn jittered(delay: Duration) -> Duration {
    delay.mul_f64(0.8 + rand::random::<f64>() * 0.4)
}

#[cfg(test)]
#[path = "../tests/backup_worker.rs"]
mod tests;
