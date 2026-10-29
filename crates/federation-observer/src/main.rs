//! Archive a Fedimint federation's session outcomes and report on them.
//!
//! `sync` downloads every finished session that is not yet on disk, `daemon`
//! repeats `sync` and prints new LNv2 events, and `lnv2` prints the LNv2
//! contract history from the archive without network access.

mod lnv2;
mod store;

use std::path::PathBuf;
use std::str::FromStr as _;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use fedimint_api_client::api::DynGlobalApi;
use fedimint_api_client::download_from_invite_code;
use fedimint_connectors::ConnectorRegistry;
use fedimint_core::invite_code::InviteCode;
use fedimint_core::module::registry::ModuleDecoderRegistry;
use futures::StreamExt as _;

use crate::lnv2::{Direction, EventKind, SessionEvents};
use crate::store::{Archive, FederationInfo};

#[derive(Parser)]
#[command(about = "Archive Fedimint session outcomes and report LNv2 activity")]
struct Cli {
    /// Archive directory, one per federation.
    #[arg(long)]
    dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct SyncArgs {
    #[arg(long)]
    invite: String,
    /// First session to archive. Earlier sessions are skipped.
    #[arg(long, default_value_t = 0)]
    from: u64,
    /// Sessions fetched in parallel.
    #[arg(long, default_value_t = 8)]
    concurrency: usize,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch every finished session that is missing from the archive.
    Sync(SyncArgs),
    /// Run `sync` in a loop and print LNv2 events of new sessions.
    Daemon {
        #[command(flatten)]
        sync: SyncArgs,
        /// Seconds between two syncs.
        #[arg(long, default_value_t = 60)]
        interval: u64,
    },
    /// Print the LNv2 contracts in the archive as TSV.
    Lnv2 {
        #[arg(long, default_value_t = 0)]
        from: u64,
        /// Only show contracts of this gateway claim key.
        #[arg(long)]
        gateway: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let archive = Archive::new(&cli.dir);
    match cli.command {
        Command::Sync(args) => {
            let fed = Federation::connect(&archive, &args.invite).await?;
            let report = fed.sync(&archive, &args).await?;
            eprintln!("archived {} new sessions", report.new.len());
            anyhow::ensure!(
                report.failed == 0,
                "{} sessions failed, run sync again to retry them",
                report.failed
            );
        }
        Command::Daemon { sync, interval } => {
            let fed = Federation::connect(&archive, &sync.invite).await?;
            let lnv2 = fed.info.lnv2_instance();
            loop {
                match fed.sync(&archive, &sync).await {
                    Ok(report) => {
                        if report.failed > 0 {
                            eprintln!("{} sessions failed, retrying next round", report.failed);
                        }
                        if let Some(lnv2) = lnv2 {
                            for index in report.new {
                                if let Some(outcome) = archive.load(index, &fed.decoders)? {
                                    print_events(&lnv2::session_events(index, &outcome, lnv2));
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("sync failed: {e:#}"),
                }
                tokio::time::sleep(Duration::from_secs(interval)).await;
            }
        }
        Command::Lnv2 { from, gateway } => {
            let info = archive.info()?;
            let lnv2 = info
                .lnv2_instance()
                .context("the federation has no lnv2 module")?;
            let decoders = info.decoders();
            let mut sessions = vec![];
            for index in archive.indices()?.into_iter().filter(|i| *i >= from) {
                let outcome = archive
                    .load(index, &decoders)?
                    .context("session vanished")?;
                sessions.push(lnv2::session_events(index, &outcome, lnv2));
            }
            println!(
                "funded_session\tfunded_utc\tdirection\tamount_sat\tgateway\tresult\tsettled_session\tcontract"
            );
            for c in lnv2::contracts(sessions) {
                if gateway.as_deref().is_some_and(|g| g != c.gateway) {
                    continue;
                }
                let (result, settled_session) = match c.settled {
                    Some((session, s)) => (s.to_string(), session.to_string()),
                    None => ("open".to_owned(), "-".to_owned()),
                };
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    c.funded_session,
                    c.funded_time.map_or("-".to_owned(), lnv2::format_utc),
                    direction_name(c.direction),
                    c.amount_msat / 1000,
                    c.gateway,
                    result,
                    settled_session,
                    c.contract,
                );
            }
        }
    }
    Ok(())
}

struct Federation {
    api: DynGlobalApi,
    info: FederationInfo,
    decoders: ModuleDecoderRegistry,
}

impl Federation {
    async fn connect(archive: &Archive, invite: &str) -> anyhow::Result<Self> {
        let invite = InviteCode::from_str(invite).context("malformed invite code")?;
        let connectors = ConnectorRegistry::build_from_client_defaults()
            .bind()
            .await?;
        let (config, api) = download_from_invite_code(&connectors, &invite)
            .await
            .context("download the federation config")?;
        let info = FederationInfo::from_config(&config);
        archive.init(&info)?;
        let decoders = info.decoders();
        Ok(Self {
            api,
            info,
            decoders,
        })
    }

    /// Archive the missing finished sessions. A session that fails to
    /// download stays missing, so the next sync retries it.
    async fn sync(&self, archive: &Archive, args: &SyncArgs) -> anyhow::Result<SyncReport> {
        // `session_count` includes the session in progress.
        let finished = self.api.session_count().await?.saturating_sub(1);
        let missing: Vec<u64> = (args.from..finished).filter(|i| !archive.has(*i)).collect();
        if !missing.is_empty() {
            eprintln!("fetching {} sessions below {finished}", missing.len());
        }
        let mut fetched = futures::stream::iter(missing)
            .map(|index| async move { (index, self.api.await_block(index, &self.decoders).await) })
            .buffer_unordered(args.concurrency.max(1));
        let mut new = vec![];
        let mut failed = 0;
        while let Some((index, outcome)) = fetched.next().await {
            match outcome {
                Ok(outcome) => {
                    archive.store(index, &outcome)?;
                    new.push(index);
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("session {index}: {e:#}");
                }
            }
        }
        new.sort_unstable();
        Ok(SyncReport { new, failed })
    }
}

struct SyncReport {
    /// Indices of the sessions archived by this sync, ascending.
    new: Vec<u64>,
    failed: usize,
}

fn direction_name(d: Direction) -> &'static str {
    match d {
        Direction::Send => "send",
        Direction::Receive => "receive",
    }
}

fn print_events(events: &SessionEvents) {
    let time = events.time.map_or("-".to_owned(), lnv2::format_utc);
    for e in &events.events {
        let what = match &e.kind {
            EventKind::Funded {
                direction,
                amount_msat,
                gateway,
            } => format!(
                "{} funded {} sat gateway {gateway}",
                direction_name(*direction),
                amount_msat / 1000
            ),
            EventKind::Settled(s) => s.to_string(),
        };
        println!("{}\t{time}\t{}\t{what}", events.session, e.contract);
    }
}
