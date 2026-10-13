use std::path::PathBuf;

use clap::Parser;
use mg_cli::AdminVerb;

#[derive(Parser)]
#[command(name = "fman-cli", about = "Manifold Guardian operator admin CLI")]
struct Args {
    /// The daemon's data dir (the admin socket lives inside it).
    #[arg(long)]
    data_dir: PathBuf,
    #[command(subcommand)]
    verb: AdminVerb,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    mg_cli::run(&args.data_dir, args.verb).await
}
