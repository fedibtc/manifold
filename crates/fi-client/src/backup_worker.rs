//! Non-blocking multi-relay FI backup reconciliation.

use std::{sync::Arc, time::Duration};

use bitcoin_hashes::sha256;
use fedi_decentralized_manifold_environment::ManifoldEnvironmentProfile;
use fedi_decentralized_nostr_clients::NostrRelayClient;
use fedimint_core::runtime::{Instant, sleep};
use fedimint_core::task::TaskGroup;
use fedimint_derive_secret::DerivableSecret;
use nostr_sdk::{Event, EventBuilder, Filter, Kind, RelayUrl, Tag, Timestamp};
use tokio::sync::{OwnedMutexGuard, watch};

use crate::backup::{EncryptedFiBackup, FI_BACKUP_D_TAG, FI_BACKUP_EVENT_KIND, FiBackupKeys};
use crate::{
    FiError, FiId, FiResult, FiStatus,
    db::{BACKUP_REFRESH_INTERVAL_SECS, BackupRelayConfirmation, FiStore, RecoveryCompletedKey},
};

const SCAN_INTERVAL: Duration = Duration::from_secs(30);
const INITIAL_RETRY_INTERVAL: Duration = Duration::from_secs(2 * 60);
const MAX_RETRY_INTERVAL: Duration = Duration::from_secs(15 * 60);
const RELAY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct Desired {
    event: Event,
    document_hash: sha256::Hash,
    generation: u64,
}

pub(crate) fn spawn_workers(
    task_group: &TaskGroup,
    store: FiStore,
    root: DerivableSecret,
    relays: Vec<RelayUrl>,
) {
    if relays.is_empty() {
        return;
    }
    let (desired_tx, desired_rx) = watch::channel::<Option<Arc<Desired>>>(None);
    let coordinator_store = store.clone();
    task_group.spawn_cancellable("FI backup coordinator", async move {
        let keys = FiBackupKeys::derive(&root);
        let mut last = None;
        loop {
            if let Ok(prepared) = coordinator_store.backup_payload().await {
                let identity = (prepared.document_hash, prepared.created_at);
                if last != Some(identity)
                    && let Ok(sealed) = keys.seal(&prepared.payload)
                {
                    let builder =
                        EventBuilder::new(Kind::Custom(FI_BACKUP_EVENT_KIND), sealed.content())
                            .tags([Tag::identifier(FI_BACKUP_D_TAG)]);
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
            sleep(SCAN_INTERVAL).await;
        }
    });

    for relay in relays {
        let store = store.clone();
        let desired = desired_rx.clone();
        task_group.spawn_cancellable(
            "FI backup relay delivery",
            relay_worker(store, relay, desired),
        );
    }
}

async fn relay_worker(
    store: FiStore,
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

async fn deliver(relay: &RelayUrl, desired: &Desired) -> Result<(), FiError> {
    let client = NostrRelayClient::connect_without_signer(relay, RELAY_TIMEOUT)
        .await
        .map_err(|_| FiError::Registry("FI backup relay connection failed".to_owned()))?;
    let published = client
        .publish_signed_event(&desired.event)
        .await
        .map_err(|_| FiError::Registry("FI backup relay publication failed".to_owned()))?;
    if published != desired.event.id {
        return Err(FiError::Registry(
            "FI backup relay acknowledged another event".to_owned(),
        ));
    }
    let events = client
        .fetch_events_complete_capped(
            Filter::new()
                .author(desired.event.pubkey)
                .kind(Kind::Custom(FI_BACKUP_EVENT_KIND))
                .identifier(FI_BACKUP_D_TAG),
            Instant::now() + RELAY_TIMEOUT,
            4,
        )
        .await
        .map_err(|_| FiError::Registry("FI backup read-back failed".to_owned()))?;
    if !events.iter().any(|event| event == &desired.event) {
        return Err(FiError::Registry(
            "FI backup relay did not serve the exact event".to_owned(),
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
    pub(crate) store: FiStore,
    pub(crate) root: DerivableSecret,
    pub(crate) fi_id: FiId,
    pub(crate) profile: ManifoldEnvironmentProfile,
    pub(crate) progress: watch::Sender<FiStatus>,
}

pub(crate) fn spawn_recovery(
    task_group: &TaskGroup,
    worker: RecoveryWorker,
    guard: OwnedMutexGuard<()>,
) {
    let RecoveryWorker {
        store,
        root,
        fi_id,
        profile,
        progress,
    } = worker;
    let key = RecoveryCompletedKey::new(profile.environment());
    // Capture independent state rather than FiClient: the task group is owned
    // by FiClient, so capturing it here would keep the client alive forever.
    task_group.spawn_cancellable("FI backup recovery", async move {
        let guard = guard;
        let mut delay = Duration::ZERO;
        loop {
            let result = restore_from_relays(
                &store,
                &root,
                fi_id,
                profile.nostr_relays().as_urls(),
                profile.fi_backup_empty_read_quorum(),
                &key,
            )
            .await;
            let result = match result {
                Ok(BackupRestoreOutcome::Restored) => store.load_status(fi_id).await.map(Some),
                Ok(BackupRestoreOutcome::NoBackup) => Ok(None),
                Err(error) => Err(error),
            };
            match result {
                Ok(status) => {
                    // The first ready status must be published only after the
                    // mutation guard is released.
                    drop(guard);
                    progress.send_replace(status.unwrap_or(FiStatus::Idle));
                    break;
                }
                Err(error) => {
                    tracing::warn!(error_code = ?error.code(), "FI backup lookup will retry");
                    progress.send_replace(FiStatus::Recovery {
                        last_error: Some(error.code()),
                    });
                    delay = if delay.is_zero() {
                        Duration::from_secs(1)
                    } else {
                        delay.saturating_mul(2).min(Duration::from_secs(5 * 60))
                    };
                    sleep(delay).await;
                    progress.send_replace(FiStatus::Recovery { last_error: None });
                }
            }
        }
    });
}

// Empty is admitted only after a majority completed. The remaining relays
// are still queried until their deadlines to find a possible backup.
fn no_backup_or_unavailable(
    successful_reads: usize,
    required_reads: usize,
) -> FiResult<BackupRestoreOutcome> {
    if required_reads > 0 && successful_reads >= required_reads {
        Ok(BackupRestoreOutcome::NoBackup)
    } else {
        Err(FiError::Registry(
            "FI backup relay lookup incomplete".to_owned(),
        ))
    }
}

pub(crate) async fn restore_from_relays(
    store: &FiStore,
    root: &DerivableSecret,
    fi_id: FiId,
    relays: &[RelayUrl],
    required_reads: usize,
    recovery: &RecoveryCompletedKey,
) -> FiResult<BackupRestoreOutcome> {
    if store.recovery_completed(recovery).await {
        return Ok(
            if matches!(store.load_status(fi_id).await?, FiStatus::Restored(_)) {
                BackupRestoreOutcome::Restored
            } else {
                BackupRestoreOutcome::NoBackup
            },
        );
    }
    let keys = FiBackupKeys::derive(root);
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
                        .kind(Kind::Custom(FI_BACKUP_EVENT_KIND))
                        .identifier(FI_BACKUP_D_TAG),
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
            let Ok(candidate) = EncryptedFiBackup::from_bytes(event.content.into_bytes()) else {
                continue;
            };
            let Ok(payload) = keys.open(&candidate) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|current: &crate::backup::FiBackupPayload| {
                    payload.snapshot_generation > current.snapshot_generation
                })
            {
                best = Some(payload);
            }
        }
    }
    let Some(payload) = best else {
        let outcome = no_backup_or_unavailable(successful_reads, required_reads)?;
        store.complete_empty_recovery(recovery).await?;
        return Ok(outcome);
    };
    store
        .restore_backup_payload_with_completion(fi_id, payload, Some(recovery))
        .await?;
    Ok(BackupRestoreOutcome::Restored)
}

#[cfg(test)]
mod tests {
    use bitcoin_hashes::Hash as _;

    use super::*;

    #[test]
    fn empty_restore_requires_a_complete_read_quorum() {
        assert_eq!(
            no_backup_or_unavailable(2, 2).unwrap(),
            BackupRestoreOutcome::NoBackup
        );
        assert_eq!(
            no_backup_or_unavailable(3, 2).unwrap(),
            BackupRestoreOutcome::NoBackup
        );
        for (completed, required) in [(0, 1), (1, 2), (0, 2)] {
            assert!(matches!(
                no_backup_or_unavailable(completed, required),
                Err(FiError::Registry(_))
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
