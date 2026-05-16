//! ozmux CLI entry point. Exposes the `daemon` subcommand group; new
//! subcommands are added under `commands/`.

use clap::{Parser, Subcommand};

use crate::commands::{CommandExecute, daemon::DaemonCommand};

mod commands;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    cli.command.run().await
}

#[derive(Parser)]
#[command(name = "ozmux", version, about = "ozmux terminal multiplexer CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Daemon lifecycle commands.
    #[command(subcommand)]
    Daemon(DaemonCommand),
}

impl CommandExecute for Command {
    async fn run(self) -> anyhow::Result<()> {
        match self {
            Self::Daemon(command) => command.run().await,
        }
    }
}
