//! `ozmux daemon start` subcommand: spawns `ozmux run` detached.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use crate::CommandExecutor;

/// Spawns the ozmux daemon in a new process group and returns immediately.
#[derive(Debug, clap::Args)]
pub struct Start {}

impl CommandExecutor for Start {
    async fn execute(self) -> anyhow::Result<()> {
        if ozmux_server::socket_is_live().await {
            println!("ozmux daemon already running");
            return Ok(());
        }
        let exe = std::env::current_exe()?;
        let child = Command::new(exe)
            .arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        println!("started ozmux daemon: pid={}", child.id());
        Ok(())
    }
}
