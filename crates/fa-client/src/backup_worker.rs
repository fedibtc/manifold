//! Non-blocking multi-relay FA backup reconciliation.

use std::{sync::Arc, time::Duration};

use bitcoin_hashes::sha256;
use fedi_decentralized_manifold_environment::ManifoldEnvironmentProfile;
use fedi_decentralized_nostr_clients::NostrRelayClient;
use fedimint_core::runtime::{Instant, sleep};
use fedimint_core::task::TaskGroup;
use fedimint_derive_secret::DerivableSecret;
use nostr_sdk::{Event, EventBuilder, Filter, Kind, RelayUrl, Tag, Timestamp};
use tokio::sync::{Notify, OwnedMutexGuard, watch};

use crate::backup::{EncryptedFaBackup, FA_BACKUP_D_TAG, FA_BACKUP_EVENT_KIND, FaBackupKeys};
use crate::{
    FaError, FaId, FaResult, FaStatus,
    db::{BACKUP_REFRESH_INTERVAL_SECS, BackupRelayConfirmation, FaStore, RecoveryCompletedKey},
};

const SCAN_INTERVAL: Duration = Duration::from_secs(30);
const INITIAL_RETRY_INTERVAL: Duration = Duration::from_secs(2 * 60);
const MAX_RETRY_INTERVAL: Duration = Duration::from_secs(15 * 60);
const RELAY_TIMEOUT: Duration = Duration::from_secs(60);

/// The relays that must hold a backup before a guardian transfer votes its
/// binding. Without relays nothing can be recovered, so nothing is awaited.
#[derive(Default)]
pub(crate) struct BackupPublication {
    relays: Vec<String>,
    quorum: usize,
    wake: Arc<Notify>,
}

impl BackupPublication {
    pub(crate) fn new(relays: &[RelayUrl], quorum: usize) -> Self {
        Self {
            relays: relays.iter().map(ToString::to_string).collect(),
            quorum,
            wake: Arc::default(),
        }
    }

    pub(crate) fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }

    /// Whether a publish quorum of relays confirmed the current backup. If
    /// not, ask the coordinator to publish it now.
    pub(crate) async fn current_backup_confirmed(&self, store: &FaStore) -> FaResult<bool> {
        if self.quorum == 0 {
            return Ok(true);
        }
        let document_hash = store.backup_payload().await?.document_hash;
        let mut confirmed = 0;
        for relay in &self.relays {
            if store
                .backup_confirmation(relay)
                .await
                .is_some_and(|confirmation| confirmation.document_hash == document_hash)
            {
                confirmed += 1;
            }
        }
        if confirmed < self.quorum {
            self.wake.notify_one();
        }
        Ok(self.quorum <= confirmed)
    }
}

#[derive(Clone)]
struct Desired {
    event: Event,
    document_hash: sha256::Hash,
    generation: u64,
}

pub(crate) fn spawn_workers(
    task_group: &TaskGroup,
    store: FaStore,
    root: DerivableSecret,
    relays: Vec<RelayUrl>,
    wake: Arc<Notify>,
) {
    if relays.is_empty() {
        return;
    }
    let (desired_tx, desired_rx) = watch::channel::<Option<Arc<Desired>>>(None);
    let coordinator_store = store.clone();
    task_group.spawn_cancellable("FA backup coordinator", async move {
        let keys = FaBackupKeys::derive(&root);
        let mut last = None;
        loop {
            if let Ok(prepared) = coordinator_store.backup_payload().await {
                let identity = (prepared.document_hash, prepared.created_at);
                if last != Some(identity)
                    && let Ok(sealed) = keys.seal(&prepared.payload)
                {
                    let builder =
                        EventBuilder::new(Kind::Custom(FA_BACKUP_EVENT_KIND), sealed.content())
                            .tags([Tag::identifier(FA_BACKUP_D_TAG)]);
                    let builder =
                        builder.custom_created_at(Timestamp::from_secs(prepared.created_at));
                    if let Ok(event) = builder.sign_with_keys(keys.author()) {
                        desired_tx.send_replace(Some(Arc::new(Desired {
                            event,
                            document_hash: prepared.document_hash,
                            generation: prepared.payload.snapshot_generation,
                        })));
                        last = Some(identity);
                    }
                }
            }
            // A guardian transfer wakes the scan: its binding vote waits for
            // this publication.
            tokio::select! {
                () = sleep(SCAN_INTERVAL) => {}
                () = wake.notified() => {}
            }
        }
    });

    for relay in relays {
        let store = store.clone();
        let desired = desired_rx.clone();
        task_group.spawn_cancellable(
            "FA backup relay delivery",
            relay_worker(store, relay, desired),
        );
    }
}

async fn relay_worker(
    store: FaStore,
    relay: RelayUrl,
    mut desired_rx: watch::Receiver<Option<Arc<Desired>>>,
) {
    let mut retry_interval = INITIAL_RETRY_INTERVAL;
    loop {
        let Some(desired) = desired_rx.borrow().clone() else {
            if desired_rx.changed().await.is_err() {
                break;
            }
            continue;
        };
        let relay_name = relay.to_string();
        let confirmation = store.backup_confirmation(&relay_name).await;
        let now = fedimint_core::time::duration_since_epoch().as_secs();
        if confirmation_is_fresh(confirmation.as_ref(), desired.document_hash, now) {
            retry_interval = INITIAL_RETRY_INTERVAL;
            if desired_rx.changed().await.is_err() {
                break;
            }
            continue;
        }
        let delivered = deliver(&relay, &desired).await.is_ok();
        if delivered {
            retry_interval = INITIAL_RETRY_INTERVAL;
            let _ = store
                .record_backup_confirmation(
                    &relay_name,
                    BackupRelayConfirmation {
                        document_hash: desired.document_hash,
                        generation: desired.generation,
                        event_id: desired.event.id.to_string(),
                        confirmed_at_secs: fedimint_core::time::duration_since_epoch().as_secs(),
                    },
                )
                .await;
            continue;
        }
        let delay = jittered(retry_interval);
        retry_interval = (retry_interval * 2).min(MAX_RETRY_INTERVAL);
        tokio::select! {
            _ = sleep(delay) => {}
            changed = desired_rx.changed() => {
                if changed.is_err() { break; }
                retry_interval = INITIAL_RETRY_INTERVAL;
            },
        }
    }
}

fn confirmation_is_fresh(
    confirmation: Option<&BackupRelayConfirmation>,
    document_hash: sha256::Hash,
    now: u64,
) -> bool {
    confirmation.is_some_and(|confirmation| {
        confirmation.document_hash == document_hash
            && now.saturating_sub(confirmation.confirmed_at_secs) < BACKUP_REFRESH_INTERVAL_SECS
    })
}

fn jittered(delay: Duration) -> Duration {
    delay.mul_f64(0.8 + rand::random::<f64>() * 0.4)
}

async fn deliver(relay: &RelayUrl, desired: &Desired) -> Result<(), FaError> {
    let client = NostrRelayClient::connect_without_signer(relay, RELAY_TIMEOUT)
        .await
        .map_err(|_| FaError::Registry("FA backup relay connection failed".to_owned()))?;
    let published = client
        .publish_signed_event(&desired.event)
        .await
        .map_err(|_| FaError::Registry("FA backup relay publication failed".to_owned()))?;
    if published != desired.event.id {
        return Err(FaError::Registry(
            "FA backup relay acknowledged another event".to_owned(),
        ));
    }
    let events = client
        .fetch_events_complete_capped(
            Filter::new()
                .author(desired.event.pubkey)
                .kind(Kind::Custom(FA_BACKUP_EVENT_KIND))
                .identifier(FA_BACKUP_D_TAG),
            Instant::now() + RELAY_TIMEOUT,
            4,
        )
        .await
        .map_err(|_| FaError::Registry("FA backup read-back failed".to_owned()))?;
    if !events.iter().any(|event| event == &desired.event) {
        return Err(FaError::Registry(
            "FA backup relay did not serve the exact event".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BackupRestoreOutcome {
    Restored,
    NoBackup,
}

pub(crate) struct RecoveryWorker {
    pub(crate) store: FaStore,
    pub(crate) root: DerivableSecret,
    pub(crate) fa_id: FaId,
    pub(crate) profile: ManifoldEnvironmentProfile,
    pub(crate) progress: watch::Sender<FaStatus>,
}

pub(crate) fn spawn_recovery(
    task_group: &TaskGroup,
    worker: RecoveryWorker,
    guard: OwnedMutexGuard<()>,
) {
    let RecoveryWorker {
        store,
        root,
        fa_id,
        profile,
        progress,
    } = worker;
    let key = RecoveryCompletedKey::new(profile.environment());
    // Capture independent state rather than FaClient: the task group is owned
    // by FaClient, so capturing it here would keep the client alive forever.
    task_group.spawn_cancellable("FA backup recovery", async move {
        let guard = guard;
        let mut delay = Duration::ZERO;
        loop {
            let result = restore_from_relays(
                &store,
                &root,
                fa_id,
                profile.nostr_relays().as_urls(),
                profile.fa_backup_read_quorum(),
                &key,
            )
            .await;
            let result = match result {
                Ok(BackupRestoreOutcome::Restored) => store.load_status(fa_id).await.map(Some),
                Ok(BackupRestoreOutcome::NoBackup) => Ok(None),
                Err(error) => Err(error),
            };
            match result {
                Ok(status) => {
                    // The first ready status must be published only after the
                    // mutation guard is released.
                    drop(guard);
                    progress.send_replace(status.unwrap_or(FaStatus::Idle));
                    break;
                }
                Err(error) => {
                    tracing::warn!(error_code = ?error.code(), "FA backup lookup will retry");
                    progress.send_replace(FaStatus::Recovery {
                        last_error: Some(error.code()),
                    });
                    delay = if delay.is_zero() {
                        Duration::from_secs(1)
                    } else {
                        delay.saturating_mul(2).min(Duration::from_secs(5 * 60))
                    };
                    sleep(delay).await;
                    progress.send_replace(FaStatus::Recovery { last_error: None });
                }
            }
        }
    });
}

// Either outcome is admitted only after a majority completed. A backup that
// a publication quorum confirmed is then among the reads, so a pending
// guardian move is never restored from an older backup. The remaining relays
// are still queried until their deadlines.
fn require_read_quorum(successful_reads: usize, required_reads: usize) -> FaResult<()> {
    if required_reads > 0 && successful_reads >= required_reads {
        Ok(())
    } else {
        Err(FaError::Registry(
            "FA backup relay lookup incomplete".to_owned(),
        ))
    }
}

pub(crate) async fn restore_from_relays(
    store: &FaStore,
    root: &DerivableSecret,
    fa_id: FaId,
    relays: &[RelayUrl],
    required_reads: usize,
    recovery: &RecoveryCompletedKey,
) -> FaResult<BackupRestoreOutcome> {
    if store.recovery_completed(recovery).await {
        return Ok(
            if matches!(store.load_status(fa_id).await?, FaStatus::Restored(_)) {
                BackupRestoreOutcome::Restored
            } else {
                BackupRestoreOutcome::NoBackup
            },
        );
    }
    let keys = FaBackupKeys::derive(root);
    let queries = relays.iter().cloned().map(|relay| {
        let author = keys.public_key();
        async move {
            let client = NostrRelayClient::connect_without_signer(&relay, RELAY_TIMEOUT)
                .await
                .ok()?;
            client
                .fetch_events_complete_capped(
                    Filter::new()
                        .author(author)
                        .kind(Kind::Custom(FA_BACKUP_EVENT_KIND))
                        .identifier(FA_BACKUP_D_TAG),
                    Instant::now() + RELAY_TIMEOUT,
                    16,
                )
                .await
                .ok()
        }
    });
    let mut best = None;
    let mut successful_reads = 0;
    for events in futures::future::join_all(queries)
        .await
        .into_iter()
        .flatten()
    {
        successful_reads += 1;
        for event in events {
            if event.pubkey != keys.public_key() || event.verify().is_err() {
                continue;
            }
            let Ok(candidate) = EncryptedFaBackup::from_bytes(event.content.into_bytes()) else {
                continue;
            };
            let Ok(payload) = keys.open(&candidate) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|current: &crate::backup::FaBackupPayload| {
                    payload.snapshot_generation > current.snapshot_generation
                })
            {
                best = Some(payload);
            }
        }
    }
    require_read_quorum(successful_reads, required_reads)?;
    let Some(payload) = best else {
        store.complete_empty_recovery(recovery).await?;
        return Ok(BackupRestoreOutcome::NoBackup);
    };
    store
        .restore_backup_payload_with_completion(fa_id, payload, Some(recovery))
        .await?;
    Ok(BackupRestoreOutcome::Restored)
}

#[cfg(test)]
mod tests {
    use bitcoin_hashes::Hash as _;

    use super::*;

    #[test]
    fn restore_requires_a_complete_read_quorum() {
        require_read_quorum(2, 2).unwrap();
        require_read_quorum(3, 2).unwrap();
        for (completed, required) in [(0, 1), (1, 2), (0, 2)] {
            assert!(matches!(
                require_read_quorum(completed, required),
                Err(FaError::Registry(_))
            ));
        }
    }

    #[test]
    fn backup_confirmation_expires_after_refresh_interval() {
        let hash = sha256::Hash::from_byte_array([7; 32]);
        let confirmation = BackupRelayConfirmation {
            document_hash: hash,
            generation: 3,
            event_id: "event".to_owned(),
            confirmed_at_secs: 100,
        };
        assert!(confirmation_is_fresh(
            Some(&confirmation),
            hash,
            100 + BACKUP_REFRESH_INTERVAL_SECS - 1,
        ));
        assert!(!confirmation_is_fresh(
            Some(&confirmation),
            hash,
            100 + BACKUP_REFRESH_INTERVAL_SECS,
        ));
        assert!(!confirmation_is_fresh(
            Some(&confirmation),
            sha256::Hash::from_byte_array([8; 32]),
            100,
        ));
    }
}
