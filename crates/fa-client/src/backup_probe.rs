//! Read-only diagnostic probe of the FA backup on each configured relay.
//!
//! Runs the relay lookup that [`crate::backup_worker::restore_from_relays`]
//! performs (same client, timeout, filter, and candidate checks) and logs
//! what each relay answered. It never writes to the FA store or changes the
//! client status, and it logs no payload content or secret.

use std::time::Duration;

use fedi_decentralized_manifold_environment::ManifoldEnvironmentProfile;
use fedi_decentralized_nostr_clients::NostrRelayClient;
use fedimint_core::runtime::Instant;
use fedimint_derive_secret::DerivableSecret;
use nostr_sdk::{Event, RelayUrl};

use crate::backup::{EncryptedFaBackup, FaBackupKeys};
use crate::backup_worker::{RELAY_TIMEOUT, RESTORE_CANDIDATE_LIMIT, backup_filter};

/// Log, for every relay in `profile`, what the FA backup lookup sees.
///
/// `root` is the consumer's FA scoped root, the same value
/// [`crate::FaIdentity::scoped_root`] returns. Each relay is queried
/// concurrently and gets one `FA backup probe: relay` line; each returned
/// event gets one `FA backup probe: event` line.
pub async fn probe_backup_relays(root: &DerivableSecret, profile: &ManifoldEnvironmentProfile) {
    probe_relays(root, profile.nostr_relays().as_urls(), RELAY_TIMEOUT).await;
}

pub(crate) async fn probe_relays(root: &DerivableSecret, relays: &[RelayUrl], timeout: Duration) {
    let keys = FaBackupKeys::derive(root);
    let author = keys.public_key().to_hex();
    tracing::info!(
        author = %author,
        relays = relays.len(),
        "FA backup probe: start"
    );
    futures::future::join_all(
        relays
            .iter()
            .map(|relay| probe_relay(&keys, &author, relay, timeout)),
    )
    .await;
    tracing::info!(author = %author, "FA backup probe: done");
}

async fn probe_relay(keys: &FaBackupKeys, author: &str, relay: &RelayUrl, timeout: Duration) {
    let started = Instant::now();
    let client = match NostrRelayClient::connect_without_signer(relay, timeout).await {
        Ok(client) => client,
        Err(error) => {
            tracing::info!(
                relay = %relay,
                author = %author,
                connect = %format_args!("failed: {error}"),
                read = %"not attempted",
                elapsed_ms = started.elapsed().as_millis(),
                "FA backup probe: relay"
            );
            return;
        }
    };
    let events = client
        .fetch_events_complete_capped(
            backup_filter(keys.public_key()),
            Instant::now() + timeout,
            RESTORE_CANDIDATE_LIMIT,
        )
        .await;
    let elapsed_ms = started.elapsed().as_millis();
    let events = match events {
        Ok(events) => {
            tracing::info!(
                relay = %relay,
                author = %author,
                connect = %"ok",
                read = %format_args!("eose events={}", events.len()),
                elapsed_ms,
                "FA backup probe: relay"
            );
            events
        }
        Err(error) => {
            tracing::info!(
                relay = %relay,
                author = %author,
                connect = %"ok",
                read = %format_args!("failed: {error}"),
                elapsed_ms,
                "FA backup probe: relay"
            );
            return;
        }
    };
    for event in events {
        probe_event(keys, relay, event);
    }
}

fn probe_event(keys: &FaBackupKeys, relay: &RelayUrl, event: Event) {
    let author_matches = event.pubkey == keys.public_key();
    let verify = match event.verify() {
        Ok(()) => "ok".to_owned(),
        Err(error) => format!("failed: {error}"),
    };
    let (from_bytes, open) = match EncryptedFaBackup::from_bytes(event.content.into_bytes()) {
        Ok(candidate) => (
            "ok".to_owned(),
            match keys.open(&candidate) {
                Ok(_payload) => "ok".to_owned(),
                Err(error) => format!("failed: {error}"),
            },
        ),
        Err(error) => (format!("failed: {error}"), "skipped".to_owned()),
    };
    tracing::info!(
        relay = %relay,
        event_id = %event.id,
        created_at = event.created_at.as_secs(),
        author_matches,
        verify = %verify,
        from_bytes = %from_bytes,
        open = %open,
        "FA backup probe: event"
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use fedi_decentralized_service_manifold_guardian::{InviteCode, Locator, QuoteId, SeatId};
    use fedi_iroh_rpc::iroh::{EndpointAddr, SecretKey as IrohSecretKey};
    use futures::{SinkExt as _, StreamExt as _};
    use nostr_sdk::{EventBuilder, JsonUtil as _, Kind, Tag};
    use tokio_tungstenite::tungstenite::Message;

    use super::*;
    use crate::backup::{
        BACKUP_SCHEMA_VERSION, FA_BACKUP_D_TAG, FA_BACKUP_EVENT_KIND, FaBackupPayload, FaBackupSeat,
    };

    /// Backup author for this root, as written by the September writer
    /// (Manifold 48c8c94190), independent of the derivation under test.
    const EXPECTED_AUTHOR: &str =
        "2e43002ff4cd4b42e736fec231a6bc47c764ba5e558976e891590062ed34b358";

    fn root() -> DerivableSecret {
        DerivableSecret::new_root(&[7; 64], b"fi-client-test-root")
    }

    fn sealed_backup(keys: &FaBackupKeys) -> Event {
        let payload = FaBackupPayload {
            schema_version: BACKUP_SCHEMA_VERSION,
            snapshot_generation: 3,
            federation_invite: InviteCode("private-invite-must-not-be-logged".into()),
            seats: vec![FaBackupSeat {
                mg_identity: nostr_sdk::Keys::generate().public_key(),
                seat_id: SeatId::from(QuoteId([1; 32])),
                locator: Locator::new(
                    EndpointAddr::new(IrohSecretKey::from_bytes(&[2; 32]).public()),
                    secp256k1::Keypair::from_seckey_slice(secp256k1::SECP256K1, &[3; 32])
                        .unwrap()
                        .x_only_public_key()
                        .0,
                ),
            }],
            liquidity: None,
            past_hosts: Vec::new(),
        };
        let sealed = keys.seal(&payload).unwrap();
        backup_event(keys, sealed.content(), 1_790_096_504)
    }

    fn backup_event(keys: &FaBackupKeys, content: &str, created_at: u64) -> Event {
        EventBuilder::new(Kind::Custom(FA_BACKUP_EVENT_KIND), content)
            .tags([Tag::identifier(FA_BACKUP_D_TAG)])
            .custom_created_at(nostr_sdk::Timestamp::from_secs(created_at))
            .sign_with_keys(keys.author())
            .unwrap()
    }

    /// A one-connection relay answering every REQ with `reply(subscription_id)`.
    async fn test_relay(
        reply: impl Fn(&serde_json::Value) -> Vec<serde_json::Value> + Send + 'static,
    ) -> RelayUrl {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(message)) = socket.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let request: serde_json::Value = serde_json::from_str(&text).unwrap();
                if request[0] != "REQ" {
                    continue;
                }
                for message in reply(&request[1]) {
                    socket
                        .send(Message::text(message.to_string()))
                        .await
                        .unwrap();
                }
            }
        });
        RelayUrl::parse(&url).unwrap()
    }

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn probe_logs_each_relay_outcome_and_each_candidate_check() {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let keys = FaBackupKeys::derive(&root());
        let good = sealed_backup(&keys);
        let garbage = backup_event(&keys, "not a sealed document", 1_790_000_000);
        // Sealed under another root's content key but signed by this author.
        let other_key = FaBackupKeys::derive(&DerivableSecret::new_root(&[8; 64], b"other-root"));
        let foreign = backup_event(
            &keys,
            sealed_backup(&other_key).content.as_str(),
            1_790_000_001,
        );
        let (good_id, garbage_id, foreign_id) = (good.id, garbage.id, foreign.id);
        let serving = test_relay(move |sub| {
            vec![
                serde_json::json!([
                    "EVENT",
                    sub,
                    serde_json::from_str::<serde_json::Value>(&good.as_json()).unwrap()
                ]),
                serde_json::json!([
                    "EVENT",
                    sub,
                    serde_json::from_str::<serde_json::Value>(&garbage.as_json()).unwrap()
                ]),
                serde_json::json!([
                    "EVENT",
                    sub,
                    serde_json::from_str::<serde_json::Value>(&foreign.as_json()).unwrap()
                ]),
                serde_json::json!(["EOSE", sub]),
            ]
        })
        .await;
        let empty = test_relay(|sub| vec![serde_json::json!(["EOSE", sub])]).await;
        let auth = test_relay(|sub| {
            vec![serde_json::json!([
                "CLOSED",
                sub,
                "auth-required: NIP-42 authentication required"
            ])]
        })
        .await;
        let refused = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            drop(listener);
            RelayUrl::parse(&url).unwrap()
        };

        probe_relays(
            &root(),
            &[
                serving.clone(),
                empty.clone(),
                auth.clone(),
                refused.clone(),
            ],
            Duration::from_secs(5),
        )
        .await;

        let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        eprintln!("{logs}");
        let line = |needles: &[&str]| {
            logs.lines()
                .find(|line| needles.iter().all(|needle| line.contains(needle)))
                .unwrap_or_else(|| panic!("no log line containing {needles:?}"))
        };
        line(&[
            "fa_client::backup_probe",
            "FA backup probe: start",
            EXPECTED_AUTHOR,
            "relays=4",
        ]);
        line(&[
            "FA backup probe: relay",
            &format!("relay={serving}"),
            EXPECTED_AUTHOR,
            "connect=ok",
            "read=eose events=3",
        ]);
        line(&[
            "FA backup probe: event",
            &format!("event_id={good_id}"),
            "created_at=1790096504",
            "author_matches=true",
            "verify=ok",
            "from_bytes=ok",
            "open=ok",
        ]);
        line(&[
            "FA backup probe: event",
            &format!("event_id={garbage_id}"),
            "verify=ok",
            "from_bytes=failed: ",
            "open=skipped",
        ]);
        line(&[
            "FA backup probe: event",
            &format!("event_id={foreign_id}"),
            "verify=ok",
            "from_bytes=ok",
            "open=failed: ",
            "document authentication failed",
        ]);
        line(&[
            "FA backup probe: relay",
            &format!("relay={empty}"),
            "connect=ok",
            "read=eose events=0",
        ]);
        line(&[
            "FA backup probe: relay",
            &format!("relay={auth}"),
            "connect=ok",
            "read=failed: ",
            "relay closed the subscription before EOSE",
        ]);
        line(&[
            "Nostr relay closed a subscription",
            "auth-required: NIP-42 authentication required",
        ]);
        line(&[
            "FA backup probe: relay",
            &format!("relay={refused}"),
            EXPECTED_AUTHOR,
            "connect=failed: ",
            "read=not attempted",
        ]);
        line(&["FA backup probe: done", EXPECTED_AUTHOR]);
        assert!(
            !logs.contains("private-invite-must-not-be-logged"),
            "the probe must not log payload contents"
        );
    }
}
