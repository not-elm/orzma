//! `ozmux` CLI entry point.

use anyhow::Result;
use clap::Parser;

mod cli;
mod commands;
mod daemon_client;

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Args::parse();
    match args.command {
        cli::Command::Browser(b) => commands::browser::run(b).await,
    }
}
