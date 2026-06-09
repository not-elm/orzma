//! `ozmux run`: runs the control-plane daemon in the foreground on the shared
//! filesystem socket. Idempotent — exits early if a daemon is already live.

use std::os::unix::fs::PermissionsExt;
use crate::CommandExecutor;

/// The `ozmux run` subcommand: foreground daemon on the shared socket.
#[derive(Debug, clap::Args)]
pub struct Run {}

impl CommandExecutor for Run {
    async fn execute(self) -> anyhow::Result<()> {
        let path = ozmux_server::socket_path();
        if ozmux_server::socket_is_live().await {
            println!("ozmux daemon already running at {}", path.display());
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            // NOTE: 0700 so only this user can reach the control socket.
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        // NOTE: this unlink MUST follow the liveness check above — removing a
        // live daemon's socket would corrupt it. Reaching here means no daemon
        // answered, so the path is stale (a losing concurrent run errors at bind).
        let _ = std::fs::remove_file(&path);
        let server = ozmux_server::OzmuxServer::new(&path)?;
        println!("ozmux daemon listening at {}", path.display());
        server.start().await?;
        Ok(())
    }
}
