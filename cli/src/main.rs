use clap::{Parser, Subcommand};
use interprocess::local_socket::{ConnectOptions, GenericFilePath, ToFsName};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::extension::ExtensionCommand;

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
