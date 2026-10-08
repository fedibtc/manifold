use std::{
    fs::{File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, ensure};
use clap::{Args, Parser, Subcommand};
use fedi_decentralized_manifold_environment::ManifoldEnvironment;
use fedi_decentralized_nostr::attester::{
    CREDENTIAL_REVOCATION_EVENT_KIND, CREDENTIAL_REVOCATION_HASHTAG, ISSUER_AUTHORITY_D_TAG,
    ISSUER_AUTHORITY_EVENT_KIND, ISSUER_AUTHORITY_HASHTAG, credential_revocation_d_tag,
};
use fedi_decentralized_nostr_clients::NostrRelayClient;
use fedi_decentralized_peerbadge_signing_server::{
    Signer, SigningServer, issuer::IssuerMaterial, spawn_router,
};
use iroh::{
    Endpoint, EndpointAddr, RelayMode, RelayUrl, SecretKey, TransportAddr, endpoint::presets,
};
use iroh_tickets::{Ticket as _, endpoint::EndpointTicket};
use nostr_sdk::{EventBuilder, Kind, Tag};
use peerbadge_protocol::{IssuerContext, SignedCredential, VerificationContext};

#[derive(Parser)]
#[command(
    name = "peerbadge-signing-server",
    about = "Blind PeerBadge issuance for authorized signers"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve(ServeArgs),
    /// Create fresh issuer keys; never replace an existing file.
    Keygen {
        #[arg(long, env = "SIGNING_SERVER_KEY_FILE", hide_env_values = true)]
        key_file: PathBuf,
    },
    /// Print the authority, optionally publishing it to the environment relays.
    Authority {
        #[command(flatten)]
        issuer: IssuerArgs,
        #[arg(long, env = "SIGNING_SERVER_PUBLISH", hide_env_values = true)]
        publish: bool,
    },
    /// Print a stable endpoint ticket without starting another endpoint.
    Ticket(NetworkArgs),
    /// Revoke a finalized signed credential on the authority's revocation relays.
    Revoke {
        #[command(flatten)]
        issuer: IssuerArgs,
        #[arg(env = "SIGNING_SERVER_SIGNED_CREDENTIAL_FILE", hide_env_values = true)]
        signed_credential: PathBuf,
    },
}

#[derive(Args)]
struct IssuerArgs {
    #[arg(
        long,
        env = "SIGNING_SERVER_MANIFOLD_ENVIRONMENT",
        hide_env_values = true
    )]
    manifold_environment: ManifoldEnvironment,
    #[arg(long, env = "SIGNING_SERVER_KEY_FILE", hide_env_values = true)]
    key_file: Option<PathBuf>,
}

#[derive(Args)]
struct NetworkArgs {
    /// Exactly 32 raw secret bytes; created with mode 0600 if absent.
    #[arg(long, env = "SIGNING_SERVER_IROH_SECRET_FILE", hide_env_values = true)]
    iroh_secret_file: PathBuf,
    /// Repeat to replace n0's default relay set.
    #[arg(
        long,
        env = "SIGNING_SERVER_RELAY_URL",
        value_delimiter = ',',
        hide_env_values = true
    )]
    relay_url: Vec<RelayUrl>,
}

#[derive(Args)]
struct ServeArgs {
    #[command(flatten)]
    issuer: IssuerArgs,
    #[command(flatten)]
    network: NetworkArgs,
    #[arg(long, env = "SIGNING_SERVER_SIGNERS_FILE", hide_env_values = true)]
    signers_file: PathBuf,
    #[arg(long, env = "SIGNING_SERVER_DATA_DIR", hide_env_values = true)]
    data_dir: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    match Cli::parse().command {
        Command::Serve(args) => serve(args).await,
        Command::Keygen { key_file } => {
            let mut file = create_secret_file(&key_file)?;
            let issuer = IssuerContext::generate()
                .map_err(|_| anyhow::anyhow!("issuer generation failed"))?;
            let secret = issuer
                .export_secret_key()
                .map_err(|_| anyhow::anyhow!("issuer export failed"))?;
            let bytes = serde_json::to_vec_pretty(&secret)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(())
        }
        Command::Authority { issuer, publish } => {
            let profile = issuer.manifold_environment.profile()?;
            let material = IssuerMaterial::load(&profile, issuer.key_file.as_deref())?;
            if publish {
                let event = EventBuilder::new(
                    Kind::Custom(ISSUER_AUTHORITY_EVENT_KIND),
                    material.authority_json.clone(),
                )
                .tags([
                    Tag::identifier(ISSUER_AUTHORITY_D_TAG),
                    Tag::hashtag(ISSUER_AUTHORITY_HASHTAG),
                ]);
                publish_event(
                    &material,
                    profile
                        .nostr_relays()
                        .as_urls()
                        .iter()
                        .map(ToString::to_string),
                    event,
                )
                .await?;
            }
            println!("{}", material.authority_json.trim_end());
            Ok(())
        }
        Command::Ticket(args) => {
            let secret = load_iroh_secret(&args.iroh_secret_file)?;
            println!("{}", ticket(&secret, &args.relay_url));
            Ok(())
        }
        Command::Revoke {
            issuer,
            signed_credential,
        } => {
            let profile = issuer.manifold_environment.profile()?;
            let material = IssuerMaterial::load(&profile, issuer.key_file.as_deref())?;
            let bytes = std::fs::read(signed_credential).context("read credential file")?;
            let credential: SignedCredential = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid signed credential"))?;
            let mut verifier = VerificationContext::new();
            verifier
                .add_issuer_authority(&material.authority)
                .map_err(|_| anyhow::anyhow!("invalid issuer authority"))?;
            verifier
                .verify_credential(&credential)
                .map_err(|_| anyhow::anyhow!("credential does not verify under this authority"))?;
            let revocation = material
                .context
                .revoke_credential(&credential)
                .map_err(|_| anyhow::anyhow!("cannot revoke credential"))?;
            let digest = serde_json::to_value(&revocation.revocation.credential_digest)?;
            let digest = digest
                .as_str()
                .context("invalid credential digest encoding")?;
            let event = EventBuilder::new(
                Kind::Custom(CREDENTIAL_REVOCATION_EVENT_KIND),
                serde_json::to_string(&revocation)?,
            )
            .tags([
                Tag::identifier(credential_revocation_d_tag(digest)),
                Tag::hashtag(CREDENTIAL_REVOCATION_HASHTAG),
            ]);
            let relays = material
                .authority
                .issuer
                .revocation
                .iter()
                .filter(|location| location.protocol == "nostr")
                .map(|location| location.location.clone())
                .collect::<Vec<_>>();
            ensure!(
                !relays.is_empty(),
                "authority has no Nostr revocation relays"
            );
            publish_event(&material, relays, event).await
        }
    }
}

async fn publish_event(
    material: &IssuerMaterial,
    relays: impl IntoIterator<Item = String>,
    event: EventBuilder,
) -> anyhow::Result<()> {
    for relay in relays {
        let client =
            NostrRelayClient::connect(&relay, material.keys.clone(), Duration::from_secs(10))
                .await
                .map_err(|_| anyhow::anyhow!("cannot connect to publication relay"))?;
        client
            .publish_event(event.clone())
            .await
            .map_err(|_| anyhow::anyhow!("relay publication failed"))?;
    }
    Ok(())
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let profile = args.issuer.manifold_environment.profile()?;
    let issuer = IssuerMaterial::load(&profile, args.issuer.key_file.as_deref())?;
    let bytes = std::fs::read(&args.signers_file).context("read signers file")?;
    let signers: Vec<Signer> =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid signers file"))?;
    std::fs::create_dir_all(&args.data_dir).context("create data directory")?;
    let server = SigningServer::new(issuer, signers, &args.data_dir.join("audit.jsonl"))?;
    let secret = load_iroh_secret(&args.network.iroh_secret_file)?;
    let mut builder = Endpoint::builder(presets::N0).secret_key(secret.clone());
    if !args.network.relay_url.is_empty() {
        builder = builder.relay_mode(RelayMode::Custom(
            args.network.relay_url.iter().cloned().collect(),
        ));
    }
    let endpoint = builder.bind().await.context("bind signing endpoint")?;
    let router = spawn_router(endpoint, server.clone());
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    let mut announced = false;
    let mut sweep = tokio::time::interval(Duration::from_secs(1));
    let result = loop {
        tokio::select! {
            // Poll the same signal subscription before announcing readiness;
            // otherwise an immediate interrupt can arrive before registration.
            biased;
            result = &mut shutdown => break result.context("wait for shutdown"),
            _ = std::future::ready(()), if !announced => {
                // Unlike a live socket address, this ticket survives restarts.
                println!("{}", ticket(&secret, &args.network.relay_url));
                std::io::stdout().flush()?;
                announced = true;
            }
            _ = sweep.tick() => {
                if server.sweep().await.is_err() {
                    break Err(anyhow::anyhow!("audit unavailable; signing service stopped"));
                }
            }
        }
    };
    router
        .shutdown()
        .await
        .context("shut down signing endpoint")?;
    result
}

fn ticket(secret: &SecretKey, relays: &[RelayUrl]) -> String {
    let relays = if relays.is_empty() {
        RelayMode::Default.relay_map().urls::<Vec<_>>()
    } else {
        relays.to_vec()
    };
    EndpointTicket::new(EndpointAddr::from_parts(
        secret.public(),
        relays.into_iter().map(TransportAddr::Relay),
    ))
    .encode_string()
}

fn create_secret_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

fn load_iroh_secret(path: &Path) -> anyhow::Result<SecretKey> {
    match File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::with_capacity(33);
            file.take(33).read_to_end(&mut bytes)?;
            let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
                anyhow::anyhow!("iroh secret file must contain exactly 32 raw bytes")
            })?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut file = create_secret_file(path).context("create iroh secret file")?;
            let secret = SecretKey::generate();
            file.write_all(&secret.to_bytes())?;
            file.sync_all()?;
            Ok(secret)
        }
        Err(error) => Err(error).context("read iroh secret file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_is_stable_raw_private_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iroh.key");
        let first = load_iroh_secret(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), first.to_bytes());
        assert_eq!(first.public(), load_iroh_secret(&path).unwrap().public());
        assert!(create_secret_file(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::write(&path, [1; 33]).unwrap();
        assert!(load_iroh_secret(&path).is_err());
        std::fs::write(&path, [1; 31]).unwrap();
        assert!(load_iroh_secret(&path).is_err());
    }

    #[test]
    fn ticket_is_stable_and_uses_configured_relays() {
        let secret = SecretKey::generate();
        let relays = vec!["https://relay.example".parse().unwrap()];
        let encoded = ticket(&secret, &relays);
        let decoded = EndpointTicket::decode_string(&encoded).unwrap();
        assert_eq!(decoded.endpoint_addr().id, secret.public());
        assert_eq!(
            decoded
                .endpoint_addr()
                .relay_urls()
                .cloned()
                .collect::<Vec<_>>(),
            relays
        );
        assert_eq!(encoded, ticket(&secret, &relays));
        assert!(ticket(&secret, &[]).starts_with("endpoint"));
    }

    #[test]
    fn clap_contract_supports_all_commands_and_hides_env_values() {
        use clap::CommandFactory as _;
        let command = Cli::command();
        for subcommand in command.get_subcommands() {
            for arg in subcommand
                .get_arguments()
                .filter(|arg| arg.get_env().is_some())
            {
                assert!(arg.is_hide_env_values_set());
            }
        }
        for args in [
            vec!["server", "keygen", "--key-file", "issuer.json"],
            vec![
                "server",
                "authority",
                "--manifold-environment",
                "development",
                "--publish",
            ],
            vec![
                "server",
                "ticket",
                "--iroh-secret-file",
                "iroh.key",
                "--relay-url",
                "https://one.example",
                "--relay-url",
                "https://two.example",
            ],
            vec![
                "server",
                "revoke",
                "credential.json",
                "--manifold-environment",
                "staging",
            ],
            vec![
                "server",
                "serve",
                "--manifold-environment",
                "development",
                "--signers-file",
                "signers.json",
                "--iroh-secret-file",
                "iroh.key",
                "--data-dir",
                "data",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
    }
}
