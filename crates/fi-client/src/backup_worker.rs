//! Non-blocking multi-relay FI backup reconciliation.

use std::{sync::Arc, time::Duration};

use bitcoin_hashes::sha256;
use fedi_decentralized_nostr_clients::NostrRelayClient;
use fedimint_core::runtime::{Instant, sleep};
use fedimint_core::task::TaskGroup;
use fedimint_derive_secret::DerivableSecret;
use nostr_sdk::{Event, EventBuilder, Filter, Kind, RelayUrl, Tag, Timestamp};
use tokio::sync::watch;

use crate::backup::{EncryptedFiBackup, FI_BACKUP_D_TAG, FI_BACKUP_EVENT_KIND, FiBackupKeys};
use crate::{
    FiError, FiId, FiResult, FiStatus,
    db::{BACKUP_REFRESH_INTERVAL_SECS, BackupRelayConfirmation, FiStore},
};

const SCAN_INTERVAL: Duration = Duration::from_secs(30);
const INITIAL_RETRY_INTERVAL: Duration = Duration::from_secs(15);
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
        loop {
            if let Ok(prepared) = coordinator_store.backup_payload().await {
                if let Ok(sealed) = keys.seal(&prepared.payload) {
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

pub(crate) async fn restore_from_relays(
    store: &FiStore,
    root: &DerivableSecret,
    fi_id: FiId,
    relays: &[RelayUrl],
) -> FiResult<FiStatus> {
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
    for events in futures::future::join_all(queries)
        .await
        .into_iter()
        .flatten()
    {
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
    let payload = best.ok_or_else(|| {
        FiError::Storage("no authenticated FI backup found on configured relays".to_owned())
    })?;
    store.restore_backup_payload(fi_id, payload).await
}

#[cfg(test)]
mod tests {
    use bitcoin_hashes::Hash as _;

    use super::*;

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
