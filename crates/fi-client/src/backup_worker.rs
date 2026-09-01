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
