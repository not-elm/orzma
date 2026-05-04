use crate::extension::ExtensionCommand;
use clap::{Parser, Subcommand};

mod extension;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Extension(c) => c.execute().await?,
    }
    Ok(())
}

#[derive(Debug, Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Extension(ExtensionCommand),
}

trait CommandExecutor {
    async fn execute(self) -> anyhow::Result<()>;
}
