//! Production publisher for the common setup-payment federation policy.
//!
//! Contract: `specs/SPEC-setup-payment-federations.md`.

#[cfg(test)]
mod tests;

use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::{self, IsTerminal as _, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, ensure};
use clap::{Parser, Subcommand};
use fedi_decentralized_domain::{
    SETUP_PAYMENT_FEDERATIONS_MAX_CONTENT_BYTES, SetupPaymentFederationsContent,
};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_nostr::setup_payment_federations::{
    AdmittedSetupPaymentFederationsEvent, AuthenticatedSetupPaymentFederationsAddressEvent,
    SETUP_PAYMENT_FEDERATIONS_MAX_FUTURE_SKEW_SECS, admit_setup_payment_federations_event,
    authenticate_setup_payment_federations_address_event,
    restore_durably_admitted_setup_payment_federations_event,
    setup_payment_federations_event_builder,
};
use fedi_decentralized_nostr_clients::{
    NostrRelayClient, ROLE_FETCHED_EVENT_MAX_BYTES, SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT,
    setup_payment_federations_filter,
};
use fedimint_core::runtime::Instant;
use nostr_sdk::{Event, Filter, Keys, PublicKey, RelayUrl, Timestamp};
use zeroize::Zeroizing;

const RELAY_TIMEOUT: Duration = Duration::from_secs(15);
const SECRET_INPUT_MAX_BYTES: usize = 4_096;

#[derive(Debug, Parser)]
#[command(about = "Sign and publish Manifold's production setup-payment federation policy")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate and sign a policy, save its public receipt, and publish it.
    Publish {
        /// Complete kind-37707 content as JSON.
        #[arg(long)]
        content: PathBuf,

        /// Expected publisher public key, as hex or npub.
        #[arg(long, value_parser = parse_publisher)]
        expected_publisher: PublicKey,

        /// New file in which to save the complete signed event before publication.
        #[arg(long)]
        receipt: PathBuf,

        /// Assert that no kind-37707 publication has ever existed.
        #[arg(
            long,
            conflicts_with = "previous_receipt",
            required_unless_present = "previous_receipt"
        )]
        first_publication: bool,

        /// Latest publisher receipt that this update must replace.
        #[arg(
            long,
            conflicts_with = "first_publication",
            required_unless_present = "first_publication"
        )]
        previous_receipt: Option<PathBuf>,

        /// Explicitly acknowledge publishing a stop-set with no federations.
        #[arg(long)]
        allow_empty_stop_set: bool,

        /// Read the publisher secret from this file instead of piped standard input.
        #[arg(long)]
        secret_key_file: Option<PathBuf>,
    },

    /// Republish and verify a saved signed event without loading the secret key.
    Republish {
        /// Complete signed-event receipt created by `publish`.
        #[arg(long)]
        receipt: PathBuf,

        /// Expected publisher public key, as hex or npub.
        #[arg(long, value_parser = parse_publisher)]
        expected_publisher: PublicKey,
    },
}

enum PublicationBasis {
    First,
    Previous(AdmittedSetupPaymentFederationsEvent),
}

impl PublicationBasis {
    fn load(previous_receipt: Option<&Path>, publisher: PublicKey) -> anyhow::Result<Self> {
        let Some(previous_receipt) = previous_receipt else {
            return Ok(Self::First);
        };
        let event = read_receipt(previous_receipt)?;
        let admitted = restore_durably_admitted_setup_payment_federations_event(&event, publisher)
            .with_context(|| {
                format!(
                    "authenticate previous publisher receipt {}",
                    previous_receipt.display()
                )
            })?;
        Ok(Self::Previous(admitted))
    }

    fn current(&self) -> Option<&AdmittedSetupPaymentFederationsEvent> {
        match self {
            Self::First => None,
            Self::Previous(current) => Some(current),
        }
    }

    fn next_timestamp(&self, now: Timestamp) -> anyhow::Result<Timestamp> {
        let next = self.current().map_or(now.as_secs(), |current| {
            now.as_secs()
                .max(current.event().created_at.as_secs().saturating_add(1))
        });
        ensure!(
            next <= now
                .as_secs()
                .saturating_add(SETUP_PAYMENT_FEDERATIONS_MAX_FUTURE_SKEW_SECS),
            "previous receipt is too far in the future to replace safely"
        );
        Ok(Timestamp::from_secs(next))
    }

    fn authenticated_current(
        &self,
        publisher: PublicKey,
    ) -> Option<AuthenticatedSetupPaymentFederationsAddressEvent> {
        self.current().map(|current| {
            authenticate_setup_payment_federations_address_event(current.event(), publisher)
                .expect("a fully admitted receipt is address-authenticated")
        })
    }

    fn validate_relay_high_water(
        &self,
        events: &[Event],
        publisher: PublicKey,
    ) -> anyhow::Result<()> {
        let latest = latest_address_state(events, publisher, self.authenticated_current(publisher));
        match (self, latest) {
            (Self::First, None) => Ok(()),
            (Self::First, Some(existing)) => anyhow::bail!(
                "--first-publication contradicted by existing authenticated event {}",
                existing.event().id
            ),
            (Self::Previous(expected), Some(observed)) => {
                ensure!(
                    observed.event().id == expected.event().id,
                    "relay contains newer authenticated event {} than previous receipt {}",
                    observed.event().id,
                    expected.event().id
                );
                Ok(())
            }
            (Self::Previous(_), None) => unreachable!("previous receipt seeds state"),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    match Args::parse().command {
        Command::Publish {
            content,
            expected_publisher,
            receipt,
            first_publication: _,
            previous_receipt,
            allow_empty_stop_set,
            secret_key_file,
        } => {
            ensure!(
                !receipt.exists(),
                "refusing to overwrite existing receipt {}",
                receipt.display()
            );
            let content = read_content(&content)?;
            validate_content_for_publication(&content, allow_empty_stop_set)?;
            let basis = PublicationBasis::load(previous_receipt.as_deref(), expected_publisher)?;
            let relays = production_relays()?;
            preflight_relays(&relays, expected_publisher, &basis).await?;

            let created_at = basis.next_timestamp(Timestamp::now())?;
            let secret = read_secret_key(secret_key_file.as_deref())?;
            let event = sign_event(&content, expected_publisher, secret, created_at)?;
            admit_setup_payment_federations_event(
                &event,
                expected_publisher,
                Timestamp::now(),
                basis.current(),
            )
            .context("self-admit signed setup-payment federation event")?;

            save_receipt_and_publish(&receipt, &relays, &event).await?;
        }
        Command::Republish {
            receipt,
            expected_publisher,
        } => {
            let event = read_receipt(&receipt)?;
            let current = restore_durably_admitted_setup_payment_federations_event(
                &event,
                expected_publisher,
            )
            .context("authenticate saved setup-payment federation event")?;
            let basis = PublicationBasis::Previous(current);
            let relays = production_relays()?;
            preflight_relays(&relays, expected_publisher, &basis).await?;
            publish_to_relays(&relays, &event).await?;
        }
    }
    Ok(())
}

fn parse_publisher(value: &str) -> Result<PublicKey, String> {
    PublicKey::parse(value)
        .map_err(|error| format!("parse --expected-publisher as hex or npub: {error}"))
}

fn validate_content_for_publication(
    content: &SetupPaymentFederationsContent,
    allow_empty_stop_set: bool,
) -> anyhow::Result<()> {
    ensure!(
        allow_empty_stop_set || !content.federations.is_empty(),
        "empty federation set stops all new paid setup; pass --allow-empty-stop-set to acknowledge"
    );
    setup_payment_federations_event_builder(content)
        .context("validate complete setup-payment federation policy")?;
    Ok(())
}

fn read_content(path: &Path) -> anyhow::Result<SetupPaymentFederationsContent> {
    let bytes = read_file_bounded(
        path,
        SETUP_PAYMENT_FEDERATIONS_MAX_CONTENT_BYTES,
        "policy content",
    )?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse policy content {}", path.display()))
}

fn read_secret_key(path: Option<&Path>) -> anyhow::Result<Zeroizing<String>> {
    let mut secret = Zeroizing::new(String::new());
    let read_limit = u64::try_from(SECRET_INPUT_MAX_BYTES + 1).expect("secret bound fits u64");
    match path {
        Some(path) => {
            File::open(path)
                .with_context(|| format!("open publisher secret key {}", path.display()))?
                .take(read_limit)
                .read_to_string(&mut secret)
                .with_context(|| format!("read publisher secret key {}", path.display()))?;
        }
        None => {
            ensure!(
                !io::stdin().is_terminal(),
                "refusing to read an echoed secret from a terminal; pipe it on stdin or use --secret-key-file"
            );
            io::stdin()
                .take(read_limit)
                .read_to_string(&mut secret)
                .context("read publisher secret key from stdin")?;
        }
    }
    ensure!(
        secret.len() <= SECRET_INPUT_MAX_BYTES,
        "publisher secret key input is too large"
    );
    while secret.ends_with('\r') || secret.ends_with('\n') {
        secret.pop();
    }
    ensure!(!secret.is_empty(), "publisher secret key is empty");
    ensure!(
        !secret.chars().any(char::is_whitespace),
        "publisher secret key contains whitespace"
    );
    Ok(secret)
}

fn sign_event(
    content: &SetupPaymentFederationsContent,
    expected_publisher: PublicKey,
    secret: Zeroizing<String>,
    created_at: Timestamp,
) -> anyhow::Result<Event> {
    let keys = Keys::parse(secret.as_str()).context("parse publisher secret key")?;
    ensure!(
        keys.public_key() == expected_publisher,
        "publisher secret derives {}, not expected {}",
        keys.public_key(),
        expected_publisher
    );
    let event = setup_payment_federations_event_builder(content)?
        .custom_created_at(created_at)
        .sign_with_keys(&keys)
        .context("sign setup-payment federation event")?;
    drop(keys);
    drop(secret);
    Ok(event)
}

fn write_receipt(path: &Path, event: &Event) -> anyhow::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(path)
        .with_context(|| format!("create signed-event receipt {}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, event).context("serialize signed-event receipt")?;
    file.write_all(b"\n")?;
    file.sync_all()
        .with_context(|| format!("sync signed-event receipt {}", path.display()))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent)
        .with_context(|| format!("open receipt directory {}", parent.display()))?
        .sync_all()
        .with_context(|| format!("sync receipt directory {}", parent.display()))
}

fn read_receipt(path: &Path) -> anyhow::Result<Event> {
    let bytes = read_file_bounded(path, ROLE_FETCHED_EVENT_MAX_BYTES, "signed-event receipt")?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse signed-event receipt {}", path.display()))
}

fn read_file_bounded(path: &Path, max_bytes: usize, description: &str) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .with_context(|| format!("open {description} {}", path.display()))?
        .take(u64::try_from(max_bytes + 1).expect("file bound fits u64"))
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {description} {}", path.display()))?;
    ensure!(
        bytes.len() <= max_bytes,
        "{description} exceeds {max_bytes} bytes"
    );
    Ok(bytes)
}

fn production_relays() -> anyhow::Result<Vec<RelayUrl>> {
    Ok(ManifoldEnvironment::Production
        .profile()
        .context("resolve production Manifold relay profile")?
        .nostr_relays()
        .as_urls()
        .to_vec())
}

async fn preflight_relays(
    relays: &[RelayUrl],
    publisher: PublicKey,
    basis: &PublicationBasis,
) -> anyhow::Result<()> {
    preflight_relays_with(relays, publisher, basis, |relay, publisher| async move {
        fetch_address_candidates(&relay, publisher).await
    })
    .await
}

async fn preflight_relays_with<F, Fut>(
    relays: &[RelayUrl],
    publisher: PublicKey,
    basis: &PublicationBasis,
    mut fetch: F,
) -> anyhow::Result<()>
where
    F: FnMut(RelayUrl, PublicKey) -> Fut,
    Fut: Future<Output = anyhow::Result<Vec<Event>>>,
{
    let mut failures = Vec::new();
    for relay in relays {
        let result = fetch(relay.clone(), publisher)
            .await
            .and_then(|events| basis.validate_relay_high_water(&events, publisher));
        if let Err(error) = result {
            eprintln!("preflight failed on {relay}: {error:#}");
            failures.push(relay.to_string());
        }
    }
    ensure!(
        failures.is_empty(),
        "publisher high-water could not be established on: {}",
        failures.join(", ")
    );
    Ok(())
}

fn latest_address_state(
    events: &[Event],
    publisher: PublicKey,
    mut current: Option<AuthenticatedSetupPaymentFederationsAddressEvent>,
) -> Option<AuthenticatedSetupPaymentFederationsAddressEvent> {
    for event in events {
        let Ok(candidate) = authenticate_setup_payment_federations_address_event(event, publisher)
        else {
            continue;
        };
        if current
            .as_ref()
            .is_none_or(|current| candidate.is_newer_than(current))
        {
            current = Some(candidate);
        }
    }
    current
}

async fn fetch_address_candidates(
    relay: &RelayUrl,
    publisher: PublicKey,
) -> anyhow::Result<Vec<Event>> {
    let client = NostrRelayClient::connect_without_signer(relay, RELAY_TIMEOUT)
        .await
        .with_context(|| format!("connect to {relay}"))?;
    client
        .fetch_events_complete_capped(
            setup_payment_federations_filter(publisher),
            Instant::now() + RELAY_TIMEOUT,
            SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT,
        )
        .await
        .with_context(|| format!("query current setup-payment address on {relay}"))
}

async fn save_receipt_and_publish(
    receipt: &Path,
    relays: &[RelayUrl],
    event: &Event,
) -> anyhow::Result<()> {
    save_receipt_and_publish_with(receipt, relays, event, |relay, event| async move {
        publish_and_read_back(&relay, &event).await
    })
    .await
}

async fn save_receipt_and_publish_with<F, Fut>(
    receipt: &Path,
    relays: &[RelayUrl],
    event: &Event,
    publish: F,
) -> anyhow::Result<()>
where
    F: FnMut(RelayUrl, Event) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    write_receipt(receipt, event)?;
    println!("signed event {} and saved {}", event.id, receipt.display());
    publish_to_relays_with(relays, event, publish).await
}

async fn publish_to_relays(relays: &[RelayUrl], event: &Event) -> anyhow::Result<()> {
    publish_to_relays_with(relays, event, |relay, event| async move {
        publish_and_read_back(&relay, &event).await
    })
    .await
}

async fn publish_to_relays_with<F, Fut>(
    relays: &[RelayUrl],
    event: &Event,
    mut publish: F,
) -> anyhow::Result<()>
where
    F: FnMut(RelayUrl, Event) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let mut failures = Vec::new();
    for relay in relays {
        match publish(relay.clone(), event.clone()).await {
            Ok(()) => println!("verified {} on {relay}", event.id),
            Err(error) => {
                eprintln!("failed {} on {relay}: {error:#}", event.id);
                failures.push(relay.to_string());
            }
        }
    }
    ensure!(
        failures.is_empty(),
        "event {} was not verified on: {}; rerun `republish` with its receipt",
        event.id,
        failures.join(", ")
    );
    Ok(())
}

async fn publish_and_read_back(relay: &RelayUrl, event: &Event) -> anyhow::Result<()> {
    let client = NostrRelayClient::connect_without_signer(relay, RELAY_TIMEOUT)
        .await
        .with_context(|| format!("connect to {relay}"))?;
    let published = client
        .publish_signed_event(event)
        .await
        .with_context(|| format!("publish to {relay}"))?;
    ensure!(published == event.id, "relay returned a different event ID");

    let fetched = client
        .fetch_events_complete_capped(
            Filter::new().id(event.id).limit(1),
            Instant::now() + RELAY_TIMEOUT,
            1,
        )
        .await
        .with_context(|| format!("read event back from {relay}"))?;
    ensure!(
        fetched.iter().any(|candidate| candidate == event),
        "relay did not return the exact signed event"
    );

    let candidates = client
        .fetch_events_complete_capped(
            setup_payment_federations_filter(event.pubkey),
            Instant::now() + RELAY_TIMEOUT,
            SETUP_PAYMENT_FEDERATIONS_CANDIDATE_LIMIT,
        )
        .await
        .with_context(|| format!("read current setup-payment address from {relay}"))?;
    verify_canonical_selection(event, &candidates)
}

fn verify_canonical_selection(event: &Event, candidates: &[Event]) -> anyhow::Result<()> {
    let latest = latest_address_state(candidates, event.pubkey, None)
        .context("relay returned no authenticated current setup-payment event")?;
    ensure!(
        latest.event() == event,
        "relay's current setup-payment event is {}, not {}",
        latest.event().id,
        event.id
    );
    Ok(())
}
