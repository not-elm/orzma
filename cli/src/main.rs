//! ozmux CLI entry point. Currently exposes only the `daemon` subcommand
//! group; further verbs from Issue #31 will be added in follow-up PRs.

use clap::{Parser, Subcommand};

mod commands;

#[derive(Parser)]
#[command(name = "ozmux", version, about = "ozmux terminal multiplexer CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Daemon lifecycle commands.
    Daemon(commands::daemon::DaemonArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Daemon(args) => commands::daemon::run(args).await,
    }
}
