//! Periodic Holder-authorized enrollment of one FMan telemetry endpoint.

use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose};
use fedi_decentralized_domain::AdmittedSetupPaymentFederations;
use fedi_decentralized_service_fleet_manager::{
    GuardianTelemetryRegistrationRequest, GuardianTelemetryRegistrationResponse,
    MAX_GUARDIAN_TELEMETRY_REGISTRATION_BYTES,
};
use fman_core::fleet::Fleet;
use nostr_sdk::{EventBuilder, Kind, Tag, Timestamp};
use rand::RngCore as _;
use sha2::{Digest as _, Sha256};
use tokio::{sync::Notify, task::JoinHandle};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// Upper bound on the random spread added to every reconcile wait.
///
/// A fleet whose instances start together would otherwise stay in phase for
/// their whole lifetime, so every instance would re-register in the same second
/// of every interval. A receiver that bounds registrations per source network
/// then refuses part of that burst on every cycle, forever. Spreading each wait
/// decorrelates the fleet within a few intervals and costs nothing else: the
/// registration is idempotent and the lease is far longer than the interval.
const RECONCILE_JITTER_MAX: Duration = Duration::from_secs(120);
const MAX_RESPONSE_BYTES: usize = 8 * 1024;

pub struct TelemetryRegistrationWorkerHandle {
    shutdown: Arc<Notify>,
    stopped: Arc<AtomicBool>,
    join_handle: JoinHandle<()>,
}

impl TelemetryRegistrationWorkerHandle {
    pub async fn shutdown(self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.shutdown.notify_one();
        let _ = self.join_handle.await;
    }
}

/// Start idempotent FMan enrollment. Repeating the complete registration lets
/// a receiver recover lost state without acknowledgement storage or rotation.
pub fn start_registration(
    fleet: Arc<Fleet>,
    nostr: fman_nostr::FleetManagerNostr,
    iroh_endpoint_id: String,
    keys: nostr_sdk::Keys,
    mut policy: tokio::sync::watch::Receiver<Option<AdmittedSetupPaymentFederations>>,
) -> TelemetryRegistrationWorkerHandle {
    let transport = Arc::new(HttpRegistrationTransport::new(keys));
    let shutdown = Arc::new(Notify::new());
    let stopped = Arc::new(AtomicBool::new(false));
    let worker_shutdown = shutdown.clone();
    let worker_stopped = stopped.clone();
    let join_handle = tokio::spawn(async move {
        let mut policy_open = true;
        loop {
            if worker_stopped.load(Ordering::SeqCst) {
                break;
            }
            let admitted = policy.borrow().clone();
            if let Some(admitted) = admitted
                && let Some(holder_authorization) = selected_holder_authorization(&nostr)
            {
                let (generation, capability) = fleet.telemetry_registration_capability();
                let request = GuardianTelemetryRegistrationRequest {
                    version: fedi_decentralized_domain::ProtocolV1,
                    iroh_endpoint_id: iroh_endpoint_id.clone(),
                    generation,
                    capability,
                    holder_authorization,
                };
                match serde_json::to_vec(&request) {
                    Ok(body) if body.len() <= MAX_GUARDIAN_TELEMETRY_REGISTRATION_BYTES => {
                        let url = &admitted.telemetry_registration_url().0;
                        let Some(result) =
                            unless_shutdown(&worker_shutdown, transport.register(url, &body)).await
                        else {
                            break;
                        };
                        if result.is_err() {
                            tracing::warn!("guardian telemetry registration failed");
                        }
                    }
                    _ => tracing::warn!(
                        reason = "registration_material_unavailable",
                        "guardian telemetry registration preparation failed"
                    ),
                }
            }

            tokio::select! {
                () = worker_shutdown.notified() => break,
                changed = policy.changed(), if policy_open => {
                    if changed.is_err() {
                        policy_open = false;
                        tracing::warn!(
                            "setup-payment policy watch closed; telemetry enrollment will use bounded polling"
                        );
                    }
                }
                () = fleet.telemetry_registration_changed() => {}
                () = tokio::time::sleep(reconcile_delay()) => {}
            }
        }
    });

    TelemetryRegistrationWorkerHandle {
        shutdown,
        stopped,
        join_handle,
    }
}

/// One reconcile wait: the fixed interval plus a bounded random spread.
fn reconcile_delay() -> Duration {
    let mut spread = [0_u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut spread);
    let jitter = u64::from_le_bytes(spread) % RECONCILE_JITTER_MAX.as_millis() as u64;
    RECONCILE_INTERVAL.saturating_add(Duration::from_millis(jitter))
}

async fn unless_shutdown<F>(shutdown: &Notify, operation: F) -> Option<F::Output>
where
    F: Future,
{
    tokio::select! {
        biased;
        () = shutdown.notified() => None,
        result = operation => Some(result),
    }
}

fn selected_holder_authorization(
    nostr: &fman_nostr::FleetManagerNostr,
) -> Option<fedi_decentralized_domain::HolderAuthorizationEnvelope> {
    nostr
        .holder_authorizations()
        .into_iter()
        .min_by_key(|envelope| {
            envelope
                .holder_authorization
                .authorization
                .holder_id_pubkey
                .0
                .to_string()
        })
}

struct HttpRegistrationTransport {
    client: tokio::sync::OnceCell<reqwest::Client>,
    keys: nostr_sdk::Keys,
}

impl HttpRegistrationTransport {
    fn new(keys: nostr_sdk::Keys) -> Self {
        Self {
            client: tokio::sync::OnceCell::new(),
            keys,
        }
    }

    async fn client(&self) -> Result<&reqwest::Client, ()> {
        self.client
            .get_or_try_init(|| async {
                reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(3))
                    .timeout(Duration::from_secs(10))
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(|error| {
                        tracing::warn!(%error, "telemetry registration client unavailable");
                    })
            })
            .await
    }

    async fn register(&self, registration_url: &str, body: &[u8]) -> Result<(), ()> {
        let authorization = nip98_authorization(&self.keys, registration_url, body)?;
        let response = self
            .client()
            .await?
            .post(registration_url)
            .header(reqwest::header::AUTHORIZATION, authorization)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_vec())
            .send()
            .await
            .map_err(|_| ())?;
        if !response.status().is_success() {
            return Err(());
        }
        let bytes = bounded_response(response).await?;
        let response: GuardianTelemetryRegistrationResponse =
            serde_json::from_slice(&bytes).map_err(|_| ())?;
        if response.version != fedi_decentralized_domain::ProtocolV1 {
            return Err(());
        }
        Ok(())
    }
}

fn nip98_authorization(
    keys: &nostr_sdk::Keys,
    registration_url: &str,
    body: &[u8],
) -> Result<String, ()> {
    let mut nonce = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let payload = hex::encode(Sha256::digest(body));
    let event = EventBuilder::new(Kind::HttpAuth, "")
        .custom_created_at(Timestamp::now())
        .tag(Tag::parse(["u", registration_url]).map_err(|_| ())?)
        .tag(Tag::parse(["method", "POST"]).map_err(|_| ())?)
        .tag(Tag::parse(["payload", &payload]).map_err(|_| ())?)
        .tag(Tag::parse(["nonce", &hex::encode(nonce)]).map_err(|_| ())?)
        .sign_with_keys(keys)
        .map_err(|_| ())?;
    let encoded = general_purpose::STANDARD.encode(serde_json::to_vec(&event).map_err(|_| ())?);
    Ok(format!("Nostr {encoded}"))
}

async fn bounded_response(mut response: reqwest::Response) -> Result<Vec<u8>, ()> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) < chunk.len() {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use nostr_sdk::Event;

    use super::*;

    #[test]
    fn nip98_auth_is_exact_body_bound_and_nonce_unique() {
        let keys = nostr_sdk::Keys::generate();
        let url = "https://receiver.example/v1/telemetry/registrations";
        let first = nip98_authorization(&keys, url, b"body").unwrap();
        let second = nip98_authorization(&keys, url, b"body").unwrap();
        assert_ne!(first, second);

        let encoded = first.strip_prefix("Nostr ").unwrap();
        let event: Event =
            serde_json::from_slice(&general_purpose::STANDARD.decode(encoded).unwrap()).unwrap();
        event.verify().unwrap();
        assert_eq!(event.kind, Kind::HttpAuth);
        let tags = event
            .tags
            .iter()
            .map(|tag| tag.as_slice())
            .collect::<Vec<_>>();
        assert!(tags.iter().any(|tag| *tag == ["u", url]));
        assert!(tags.iter().any(|tag| *tag == ["method", "POST"]));
        assert!(tags.iter().any(|tag| {
            tag.first().map(String::as_str) == Some("payload")
                && tag.get(1).map(String::as_str)
                    == Some(hex::encode(Sha256::digest(b"body")).as_str())
        }));
    }

    #[test]
    fn reconcile_delay_is_bounded_and_decorrelates_instances() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..256 {
            let delay = reconcile_delay();
            assert!(delay >= RECONCILE_INTERVAL);
            assert!(delay < RECONCILE_INTERVAL + RECONCILE_JITTER_MAX);
            seen.insert(delay);
        }
        // Instances that start together must not keep drawing the same wait.
        assert!(seen.len() > 1);
    }

    #[tokio::test]
    async fn shutdown_permit_prevents_the_registration_operation_from_starting() {
        let shutdown = Notify::new();
        shutdown.notify_one();
        let started = Arc::new(AtomicBool::new(false));
        let operation_started = started.clone();

        let result = unless_shutdown(&shutdown, async move {
            operation_started.store(true, Ordering::SeqCst);
        })
        .await;

        assert!(result.is_none());
        assert!(!started.load(Ordering::SeqCst));
    }
}
