//! Periodic provider re-check of registration tokens, and removal of
//! registrations whose grace period after a rejection ran out.
//!
//! A registration stays eligible for as long as its provider token is valid;
//! it does not go stale by age, because an app may go months without opening
//! (a phone linked for guardian alerts). Instead, once the provider last
//! accepted a token more than the configured interval ago, this task asks the
//! provider again with a validate-only send, the same check registration makes.
//! A definitive rejection disables the registration, exactly as a rejected
//! delivery does; an ambiguous answer leaves it for a later pass.
//!
//! A provider can reject a token because it replaced it, before the app has
//! re-registered the new one. A disabled registration therefore keeps its
//! hooks for [`REJECTED_REGISTRATION_GRACE`]; the app re-registering the
//! installation in that time resumes delivery. After it, this task deletes
//! the registration, which ends any hook with no expiry targeting it.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use fedi_decentralized_push_gateway_storage::DatabaseWriteLock;
use tokio::{sync::Notify, task::JoinHandle};

use crate::{
    PushProvider, PushProviderErrorKind, PushRegistration, PushRegistrationRepository,
    log_sanitizer::sanitize_log_value,
};

/// How often the task looks for registrations due for a re-check.
const REVALIDATION_POLL: Duration = Duration::from_secs(60 * 60);
/// Registrations re-checked per database query.
const REVALIDATION_BATCH: u64 = 100;
/// Bounds one validate-only provider call.
const PROVIDER_VALIDATION_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a disabled registration waits for its app to re-register.
const REJECTED_REGISTRATION_GRACE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Handle used to stop the revalidation task and wait for it.
#[derive(Debug)]
pub struct RegistrationRevalidationHandle {
    shutdown: Arc<Notify>,
    stopped: Arc<AtomicBool>,
    join_handle: JoinHandle<()>,
}

impl RegistrationRevalidationHandle {
    /// Requests shutdown and waits for the current provider call to finish.
    pub async fn shutdown(self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.shutdown.notify_one();
        let _ = self.join_handle.await;
    }
}

/// Outcome counts of one revalidation pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RevalidationCounts {
    /// Tokens the provider accepted again.
    pub validated: u64,
    /// Registrations disabled because the provider rejected their token.
    pub disabled: u64,
    /// Checks left for a later pass because the provider gave no definitive answer.
    pub deferred: u64,
    /// Disabled registrations deleted after their grace period.
    pub purged: u64,
}

pub(crate) fn start_registration_revalidation(
    registrations: PushRegistrationRepository,
    provider: Arc<dyn PushProvider>,
    revalidation_seconds: u64,
    database_write_lock: DatabaseWriteLock,
) -> RegistrationRevalidationHandle {
    let shutdown = Arc::new(Notify::new());
    let stopped = Arc::new(AtomicBool::new(false));
    let task_shutdown = shutdown.clone();
    let task_stopped = stopped.clone();
    let join_handle = tokio::spawn(async move {
        let mut resume_after = None;
        loop {
            let now = crate::time::unix_timestamp();
            let cutoff =
                now.saturating_sub(i64::try_from(revalidation_seconds).unwrap_or(i64::MAX));
            match run_pass(
                &registrations,
                provider.as_ref(),
                now,
                cutoff,
                &mut resume_after,
                &database_write_lock,
                &task_stopped,
            )
            .await
            {
                Ok(counts) if counts != RevalidationCounts::default() => eprintln!(
                    "event=push_gateway_registration_revalidation validated={} disabled={} deferred={} purged={}",
                    counts.validated, counts.disabled, counts.deferred, counts.purged
                ),
                Ok(_) => {}
                Err(err) => eprintln!(
                    "event=push_gateway_registration_revalidation_error error={}",
                    sanitize_log_value(&err.to_string())
                ),
            }
            if task_stopped.load(Ordering::SeqCst) {
                break;
            }
            tokio::select! {
                () = task_shutdown.notified() => break,
                () = tokio::time::sleep(REVALIDATION_POLL) => {}
            }
        }
    });
    RegistrationRevalidationHandle {
        shutdown,
        stopped,
        join_handle,
    }
}

/// One pass: deletes registrations disabled longer than the grace period,
/// then re-checks those due.
async fn run_pass(
    registrations: &PushRegistrationRepository,
    provider: &dyn PushProvider,
    now: i64,
    cutoff: i64,
    resume_after: &mut Option<PushRegistration>,
    database_write_lock: &DatabaseWriteLock,
    stopped: &AtomicBool,
) -> Result<RevalidationCounts, sqlx::Error> {
    let grace = i64::try_from(REJECTED_REGISTRATION_GRACE.as_secs()).unwrap_or(i64::MAX);
    let purged = {
        let _guard = database_write_lock.acquire_worker().await;
        registrations
            .purge_disabled_before(now.saturating_sub(grace))
            .await?
    };
    let mut counts = revalidate_due(
        registrations,
        provider,
        cutoff,
        resume_after,
        database_write_lock,
        stopped,
    )
    .await?;
    counts.purged = purged;
    Ok(counts)
}

/// Re-checks registrations last validated before `cutoff`, in key order from
/// `resume_after`. A pass normally runs to the end and resets `resume_after`,
/// so the next pass starts over and retries what was deferred. After a batch
/// with no definitive answer (the provider is unavailable) it stops and leaves
/// `resume_after` at that batch, so the next pass moves on instead of asking
/// about the same rows again. Stops early when `stopped` is set.
pub(crate) async fn revalidate_due(
    registrations: &PushRegistrationRepository,
    provider: &dyn PushProvider,
    cutoff: i64,
    resume_after: &mut Option<PushRegistration>,
    database_write_lock: &DatabaseWriteLock,
    stopped: &AtomicBool,
) -> Result<RevalidationCounts, sqlx::Error> {
    let mut counts = RevalidationCounts::default();
    loop {
        let due = registrations
            .due_for_revalidation(cutoff, resume_after.as_ref(), REVALIDATION_BATCH)
            .await?;
        let full_batch = u64::try_from(due.len()).unwrap_or(u64::MAX) == REVALIDATION_BATCH;
        let mut definitive_in_batch = false;
        for registration in due {
            if stopped.load(Ordering::SeqCst) {
                return Ok(counts);
            }
            let verdict = tokio::time::timeout(
                PROVIDER_VALIDATION_TIMEOUT,
                provider.validate_registration(&registration.fcm_token),
            )
            .await;
            let _guard = database_write_lock.acquire_worker().await;
            match verdict {
                Ok(Ok(())) => {
                    registrations
                        .mark_validated_if_token_matches(
                            &registration.recipient_id,
                            &registration.installation_id,
                            &registration.fcm_token,
                        )
                        .await?;
                    counts.validated += 1;
                    definitive_in_batch = true;
                }
                Ok(Err(error)) if error.kind() == PushProviderErrorKind::InvalidToken => {
                    registrations
                        .disable_installation_if_token_matches(
                            &registration.recipient_id,
                            &registration.installation_id,
                            &registration.fcm_token,
                            Some(error.reason),
                        )
                        .await?;
                    counts.disabled += 1;
                    definitive_in_batch = true;
                }
                Ok(Err(_)) | Err(_) => counts.deferred += 1,
            }
            *resume_after = Some(registration);
        }
        if !full_batch {
            *resume_after = None;
            return Ok(counts);
        }
        if !definitive_in_batch {
            return Ok(counts);
        }
    }
}

#[cfg(test)]
mod tests;
