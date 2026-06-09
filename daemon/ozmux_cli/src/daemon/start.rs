use crate::CommandExecutor;

#[derive(Debug, clap::Args)]
pub struct Start {}

impl CommandExecutor for Start {
    async fn execute(self) -> anyhow::Result<()> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        if is_daemon_running()? {
            println!("ozmux daemon is already running");
            return Ok(());
        }

        let exe = std::env::current_exe()?;
        let child = Command::new(exe)
            .arg("daemon")
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
