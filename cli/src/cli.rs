//! clap definitions for the `ozmux` CLI.

use clap::{Parser, Subcommand};

/// Top-level CLI arguments.
#[derive(Parser)]
#[command(name = "ozmux", version)]
pub struct Args {
    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// All supported `ozmux` subcommands.
#[derive(Subcommand)]
pub enum Command {
    /// Open an embedded browser activity in the current pane.
    Browser(Browser),
}

/// `ozmux browser [URL]` — open a Browser Activity in the active pane.
#[derive(Parser)]
pub struct Browser {
    /// URL to open. Schemes are added automatically: `foo.com` → `https://foo.com`.
    pub url: Option<String>,
    /// Named storage profile to use. Defaults to `default`.
    #[arg(long, conflicts_with = "incognito")]
    pub profile: Option<String>,
    /// Open in an ephemeral in-memory profile (no disk persistence).
    #[arg(long)]
    pub incognito: bool,
}
