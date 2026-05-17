//! ozmux CLI entry point. Exposes the `daemon` and `session` subcommand
//! groups; new subcommands are added under `commands/`.

use clap::{Parser, Subcommand};

use crate::commands::{CommandExecute, daemon::DaemonCommand, session::SessionCommand};

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
    /// Session management commands.
    #[command(subcommand)]
    Session(SessionCommand),
}

impl CommandExecute for Command {
    async fn run(self) -> anyhow::Result<()> {
        match self {
            Self::Daemon(command) => command.run().await,
            Self::Session(command) => command.run().await,
        }
    }
}
