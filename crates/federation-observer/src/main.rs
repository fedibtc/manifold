//! Archive a Fedimint federation's session outcomes and report on them.
//!
//! `sync` downloads every finished session that is not yet on disk, `daemon`
//! repeats `sync` and prints new LNv2 events, and `lnv2` prints the LNv2
//! contract history from the archive without network access. `balance`
//! sums the mint, wallet and Lightning flows of the whole archive.

mod balance;
mod lnv2;
mod store;

use std::path::PathBuf;
use std::str::FromStr as _;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use fedimint_api_client::api::{DynGlobalApi, FederationApiExt as _};
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
    /// Print the ecash outstanding, the wallet's bitcoin and the funds in
    /// open Lightning contracts, summed over every archived session.
    Balance,
    /// Print what the config and the guardians say about a federation now.
    Inspect {
        #[arg(long)]
        invite: String,
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
        Command::Balance => print_balance(&archive)?,
        Command::Inspect { invite } => inspect(&invite).await?,
    }
    Ok(())
}

async fn inspect(invite: &str) -> anyhow::Result<()> {
    let invite = InviteCode::from_str(invite).context("malformed invite code")?;
    let connectors = ConnectorRegistry::build_from_client_defaults()
        .bind()
        .await?;
    let (config, api) = download_from_invite_code(&connectors, &invite)
        .await
        .context("download the federation config")?;
    let info = FederationInfo::from_config(&config);
    println!("id\t{}", info.federation_id);
    println!("name\t{}", config.global.federation_name().unwrap_or("-"));
    for (id, module) in &config.modules {
        println!("module\t{id}\t{}\t{}", module.kind, module.version);
    }
    for (id, module) in &config.modules {
        if module.kind.as_str() != "mintv2" {
            continue;
        }
        let fedimint_core::encoding::DynRawFallback::Raw { raw, .. } = &module.config else {
            anyhow::bail!("mintv2 config {id} is not raw");
        };
        let mint = <fedimint_mintv2_common::config::MintClientConfig as fedimint_core::encoding::Decodable>::consensus_decode_whole(
            raw,
            &ModuleDecoderRegistry::default(),
        )
        .context("decode mintv2 config")?;
        // Per input and per output; the ppm field is private, so print the
        // serialized schedule.
        println!(
            "mintv2_fee\t{}",
            serde_json::to_string(&mint.fee_consensus)?
        );
        println!(
            "mintv2_amount_unit\t{}",
            serde_json::to_string(&mint.amount_unit)?
        );
        let denoms: Vec<String> = mint
            .tbs_agg_pks
            .keys()
            .map(|d| format!("{}={}msat", d.0, d.amount().msats))
            .collect();
        println!(
            "mintv2_denominations\t{}\t{}",
            denoms.len(),
            denoms.join(",")
        );
        let peers_per_denom: std::collections::BTreeSet<usize> =
            mint.tbs_pks.values().map(|p| p.len()).collect();
        println!("mintv2_pk_shares_per_denom\t{peers_per_denom:?}");
    }
    for (k, v) in &config.global.meta {
        println!("meta\t{k}\t{v}");
    }
    let peers: Vec<_> = config.global.api_endpoints.keys().copied().collect();
    let n = peers.len();
    println!("guardians\t{n}\tthreshold\t{}", n - (n - 1) / 3);
    let answers = futures::future::join_all(peers.iter().map(|peer| {
        let api = api.clone();
        async move {
            let r =
                tokio::time::timeout(Duration::from_secs(20), api.fedimintd_version(*peer)).await;
            (*peer, r)
        }
    }))
    .await;
    for (peer, r) in answers {
        let state = match r {
            Ok(Ok(v)) => format!("up\t{v}"),
            Ok(Err(e)) => format!("error\t{e}"),
            Err(_) => "timeout".to_owned(),
        };
        let endpoint = &config.global.api_endpoints[&peer];
        println!("peer\t{peer}\t{}\t{}\t{state}", endpoint.name, endpoint.url);
    }
    let count = tokio::time::timeout(Duration::from_secs(60), api.session_count()).await;
    let count = match count {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => anyhow::bail!("session_count: {e}"),
        Err(_) => anyhow::bail!("session_count timed out"),
    };
    println!("session_count\t{count}");
    if let (Some(lnv2), Some(last)) = (info.lnv2_instance(), count.checked_sub(2)) {
        let outcome = tokio::time::timeout(
            Duration::from_secs(60),
            api.await_block(last, &info.decoders()),
        )
        .await;
        match outcome {
            Ok(Ok(outcome)) => println!(
                "last_session_lnv2_time\t{last}\t{}",
                lnv2::session_events(last, &outcome, lnv2)
                    .time
                    .map_or("-".to_owned(), lnv2::format_utc)
            ),
            Ok(Err(e)) => println!("last_session\terror\t{e}"),
            Err(_) => println!("last_session\ttimeout"),
        }
    }
    if let Some(meta) = info.instance("meta") {
        let value = tokio::time::timeout(
            Duration::from_secs(60),
            api.with_module(meta)
                .request_current_consensus::<Option<fedimint_meta_common::MetaConsensusValue>>(
                    fedimint_meta_common::endpoint::GET_CONSENSUS_ENDPOINT.to_owned(),
                    fedimint_core::module::ApiRequestErased::new(
                        fedimint_meta_common::endpoint::GetConsensusRequest(
                            fedimint_meta_common::MetaKey(0),
                        ),
                    ),
                ),
        )
        .await;
        match value {
            Ok(Ok(Some(v))) => println!(
                "meta_module\trevision {}\t{}",
                v.revision,
                String::from_utf8_lossy(v.value.as_slice())
            ),
            Ok(Ok(None)) => println!("meta_module\tnone"),
            Ok(Err(e)) => println!("meta_module\terror\t{e}"),
            Err(_) => println!("meta_module\ttimeout"),
        }
    }
    tokio::time::sleep(Duration::from_secs(60)).await;
    match tokio::time::timeout(Duration::from_secs(60), api.session_count()).await {
        Ok(Ok(c)) => println!("session_count_after_60s\t{c}"),
        other => println!("session_count_after_60s\tfailed\t{other:?}"),
    }
    Ok(())
}

fn print_balance(archive: &Archive) -> anyhow::Result<()> {
    let info = archive.info()?;
    let instances = balance::Instances {
        mint: info
            .instance("mint")
            .context("the federation has no mint module")?,
        wallet: info.instance("wallet"),
        ln: info.instance("ln"),
    };
    let lnv2 = info.lnv2_instance();
    let decoders = info.decoders();
    let indices = archive.indices()?;
    // The sums start from zero, so a missing session would go unnoticed.
    anyhow::ensure!(
        indices.iter().copied().eq(0..indices.len() as u64),
        "the archive misses sessions: run `sync` from 0 first"
    );
    let mut balances = balance::Balances::default();
    let mut lnv2_sessions = vec![];
    for index in indices.iter().copied() {
        let outcome = archive
            .load(index, &decoders)?
            .context("session vanished")?;
        balances.add_session(index, &outcome, instances)?;
        if let Some(lnv2) = lnv2 {
            lnv2_sessions.push(lnv2::session_events(index, &outcome, lnv2));
        }
    }
    let last_time = lnv2_sessions.iter().filter_map(|s| s.time).max();
    let lnv2_open = lnv2::contracts(lnv2_sessions)
        .into_iter()
        .filter(|c| c.settled.is_none())
        .fold(balance::Flow::default(), |mut open, c| {
            open.count += 1;
            open.msat += c.amount_msat;
            open
        });
    let ln_open = balances.ln_open();
    let sat = |msat: i128| msat as f64 / 1000.0;
    println!("federation\t{}", info.federation_id);
    println!("sessions\t{}", indices.len());
    println!(
        "last_lnv2_time_utc\t{}",
        last_time.map_or("-".to_owned(), lnv2::format_utc)
    );
    let flow = |name: &str, f: &balance::Flow| {
        println!("{name}\t{}\t{} sat", f.count, sat(i128::from(f.msat)));
    };
    flow("mint_issued", &balances.mint_issued);
    flow("mint_redeemed", &balances.mint_redeemed);
    println!(
        "ecash_outstanding\t{} sat",
        sat(balances.ecash_outstanding_msat())
    );
    flow("peg_in", &balances.peg_in);
    flow("peg_out", &balances.peg_out);
    flow("peg_out_fees", &balances.peg_out_fees);
    println!("wallet_net\t{} sat", sat(balances.wallet_msat()));
    flow("ln_open", &ln_open);
    flow("lnv2_open", &lnv2_open);
    flow("fees", &balances.fees);
    println!(
        "to_other_modules\t{}\t{} sat",
        balances.to_other_modules.count,
        sat(balances.to_other_modules.msat)
    );
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
        eprintln!(
            "federation {} {:?}",
            info.federation_id,
            config.global.federation_name().unwrap_or("-")
        );
        for (id, module) in &config.modules {
            eprintln!(
                "module {id} {} config version {}",
                module.kind, module.version
            );
        }
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
