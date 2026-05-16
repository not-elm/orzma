//! `ozmux daemon start` — spawn or attach the ozmux daemon.

use clap::Args;

#[derive(Args)]
pub struct StartArgs {
    /// Run the daemon attached to this terminal instead of detaching.
    #[arg(long)]
    foreground: bool,
}

pub async fn run(_args: StartArgs) -> anyhow::Result<()> {
    anyhow::bail!("ozmux daemon start: not yet implemented")
}
