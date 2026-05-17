//! ozmux CLI entry point. Exposes the `daemon`, `session`, and `browser`
//! subcommand groups; new subcommands are added under `commands/`.

use clap::{Parser, Subcommand};

use crate::commands::{
    CommandExecute, browser::Browser, daemon::DaemonCommand, session::SessionCommand,
};

mod commands;
mod daemon_client;

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
    /// Open an embedded browser activity in the current pane.
    Browser(Browser),
}

impl CommandExecute for Command {
    async fn run(self) -> anyhow::Result<()> {
        match self {
            Self::Daemon(command) => command.run().await,
            Self::Session(command) => command.run().await,
            Self::Browser(command) => command.run().await,
        }
    }
}
