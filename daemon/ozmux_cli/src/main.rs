//! ozmux CLI entrypoint: `ozmux run` runs the daemon in the foreground;
//! `ozmux daemon start` spawns it detached.

use clap::Parser;

mod daemon;
mod run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    CliCommand::parse().command.execute().await
}

trait CommandExecutor {
    async fn execute(self) -> anyhow::Result<()>;
}

#[derive(Debug, clap::Parser)]
struct CliCommand {
    #[command(subcommand)]
    command: CliSubcommand,
}

#[derive(Debug, clap::Subcommand)]
enum CliSubcommand {
    Run(run::Run),
    #[command(subcommand)]
    Daemon(daemon::Daemon),
}

impl CommandExecutor for CliSubcommand {
    async fn execute(self) -> anyhow::Result<()> {
        match self {
            Self::Run(r) => r.execute().await,
            Self::Daemon(d) => d.execute().await,
        }
    }
}
