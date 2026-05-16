//! `ozmux daemon start` — spawn or attach the ozmux daemon.

use clap::Args;

#[derive(Args)]
pub struct StartArgs {
    /// Run the daemon attached to this terminal instead of detaching.
    #[arg(long)]
    foreground: bool,
}

pub async fn run(args: StartArgs) -> anyhow::Result<()> {
    if args.foreground {
        return daemon_bootstrap::run().await;
    }
    anyhow::bail!("ozmux daemon start (detached mode): not yet implemented")
}
