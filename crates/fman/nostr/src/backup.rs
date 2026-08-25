//! Relay side of the encrypted FMan backup
//! (`crates/fman/specs/SPEC-nostr-backup-restore.md`).
//!
//! Separate from the advertiser in every way that matters: its own connection,
//! its own signing identity, and no shared state. The backup identity must not
//! be linkable to the service identity, so nothing here may publish under the
//! service key or tag a backup event with anything the advertiser also emits.

use std::time::Duration;

use fedi_decentralized_nostr_clients::NostrRelayClient;
use fman_core::backup::{BackupArchive, BackupSink, RecoverError, RecoveredFleet, SeatPublication};
use fman_core::identity::RootMnemonic;
use nostr_sdk::{Event, EventBuilder, Filter, Keys, Kind};

use crate::format::{BACKUP_DOCUMENT_VERSION, BackupIdentity, recover_from_events};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Restore enumerates a whole install's history in one query, so it is allowed
/// longer than a publication round trip.
const RESTORE_TIMEOUT: Duration = Duration::from_secs(60);

/// Ceiling on the events one restore will read.
///
/// A resource bound that is also a correctness one, because reaching it is
/// indistinguishable from a backup with more documents than this: the
/// enumeration fails closed on the cap rather than returning a prefix, so a
/// restore never quietly rebuilds part of a fleet. Nobody but the phrase holder
/// can author events under this identity, so no third party can push a restore
/// into that failure.
///
/// Derived from a per-seat budget rather than asserted: each seat is one
/// document plus its guardian archive's ciphertext slices, so the cap
/// supports [`MAX_RESTORE_SEATS`] seats each carrying up to
/// `MAX_RESTORE_EVENTS_PER_SEAT - 1` slices — 480 KiB of guardian config per
/// seat at the 32 KiB slice size, several times the size a federation's
/// config reaches today. An operator running more seats than the budget
/// supports would hit the failed-closed refusal above, not a truncated fleet.
const MAX_RESTORE_SEATS: u16 = 256;
const MAX_RESTORE_EVENTS_PER_SEAT: u16 = 16;
const MAX_RESTORE_DOCUMENTS: u16 = MAX_RESTORE_SEATS * MAX_RESTORE_EVENTS_PER_SEAT;

/// A relay's worth of one FMan's backup documents.
///
/// Deliberately not folded into `FleetManagerNostr`: that runtime is built
/// around the service key and a publish cadence, while this one is used by the
/// seat lifecycle synchronously and by restore before any fleet exists.
pub struct BackupRelay {
    client: NostrRelayClient,
    keys: Keys,
}

impl BackupRelay {
    pub async fn connect(keys: Keys, relay_url: &str) -> anyhow::Result<Self> {
        // As in the advertiser: the URL is operator configuration that may
        // embed credentials, so it never reaches an error or a log line.
        let client = NostrRelayClient::connect(relay_url, keys.clone(), REQUEST_TIMEOUT)
            .await
            .map_err(|err| anyhow::anyhow!("connect to the configured nostr relay: {err}"))?;
        Ok(Self { client, keys })
    }

    /// Every backup event this identity has published, for restore.
    ///
    /// Enumeration is by author and kind only — the coordinates are blinded, so
    /// there is nothing else to ask for, and no index document to keep
    /// consistent with the events it would describe.
    ///
    /// **Complete or nothing.** A restore happens once, so a query that ended
    /// early — timeout, dropped connection, or the candidate cap — must be an
    /// error rather than a shorter fleet: a prefix here is a guardian silently
    /// not restored, with no later attempt to correct it. Only the holder of
    /// the phrase can author events under this identity, so no third party can
    /// push a restore into that failure by publishing extra events.
    pub async fn fetch_own_documents(&self, limit: u16) -> anyhow::Result<Vec<Event>> {
        self.client
            .fetch_events_complete_capped(
                Filter::new()
                    .author(self.keys.public_key())
                    .kind(Kind::Custom(
                        fedi_decentralized_nostr::fman::FMAN_BACKUP_EVENT_KIND,
                    )),
                tokio::time::Instant::now() + RESTORE_TIMEOUT,
                limit,
            )
            .await
            .map_err(|err| anyhow::anyhow!("enumerate backup documents: {err}"))
    }

    /// Publish a document and confirm the relay will serve it back.
    ///
    /// The read-back is the whole point: nothing waits for this, but its `Ok`
    /// is what stops the FMan retrying, so an unconfirmed write reported as
    /// success would retire a seat from the publisher's list on a copy the
    /// relay may never have kept.
    pub async fn publish_confirmed(&self, builder: EventBuilder) -> anyhow::Result<()> {
        let published = self
            .client
            .publish_event(builder)
            .await
            .map_err(|err| anyhow::anyhow!("publish backup document: {err}"))?;

        let found = self
            .client
            .fetch_events_capped(Filter::new().id(published).limit(1), REQUEST_TIMEOUT, 1)
            .await
            .map_err(|err| anyhow::anyhow!("read back backup document: {err}"))?;

        anyhow::ensure!(
            found.iter().any(|event| event.id == published),
            "relay accepted the backup document but did not serve it back"
        );
        Ok(())
    }
}

/// The [`BackupSink`] the daemon runs: encodes a publication as sealed
/// addressable events and confirms each on the configured relay.
pub struct RelayBackupSink {
    identity: BackupIdentity,
    relay_url: String,
    /// Connected on the first publication rather than at startup. Nothing in
    /// the daemon waits on a relay (SPEC-nostr-backup-restore), and that has to
    /// include the daemon's own start: an unreachable relay would otherwise
    /// stop an FMan running its existing guardians. The publisher retries, so
    /// a failed connection costs a retry interval and nothing else.
    relay: tokio::sync::Mutex<Option<BackupRelay>>,
}

impl RelayBackupSink {
    pub fn new(identity: BackupIdentity, relay_url: String) -> Self {
        Self {
            identity,
            relay_url,
            relay: tokio::sync::Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl BackupSink for RelayBackupSink {
    async fn publish(&self, publication: &SeatPublication) -> anyhow::Result<()> {
        // The publisher is one task, so serializing publications behind the
        // connection costs nothing and keeps a failed connect from being
        // retried by several callers at once.
        let mut relay = self.relay.lock().await;
        let relay = match &*relay {
            Some(relay) => relay,
            None => relay
                .insert(BackupRelay::connect(self.identity.keys().clone(), &self.relay_url).await?),
        };
        for event in self.identity.publication_events(publication)? {
            relay.publish_confirmed(event).await?;
        }
        Ok(())
    }

    fn format_version(&self) -> u32 {
        BACKUP_DOCUMENT_VERSION
    }
}

/// Reads a mnemonic's published documents back for a restore. Connects per
/// call: the identity to read under arrives with the operator's phrase, and
/// nothing else in the daemon holds this connection.
pub struct RelayBackupArchive {
    relay_url: String,
}

impl RelayBackupArchive {
    pub fn new(relay_url: String) -> Self {
        Self { relay_url }
    }
}

#[async_trait::async_trait]
impl BackupArchive for RelayBackupArchive {
    async fn recover(&self, identity: &RootMnemonic) -> Result<RecoveredFleet, RecoverError> {
        let backup = BackupIdentity::derive(identity);
        let events = BackupRelay::connect(backup.keys().clone(), &self.relay_url)
            .await?
            .fetch_own_documents(MAX_RESTORE_DOCUMENTS)
            .await?;
        recover_from_events(&backup, events)
    }
}
