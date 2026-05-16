//! `ozmux daemon stop` — signal the running daemon to shut down.

use clap::Args;
use std::io;
use std::time::{Duration, Instant};

use crate::commands::CommandExecute;

const SIGTERM_WAIT: Duration = Duration::from_secs(10);
const SIGKILL_WAIT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Args)]
pub(crate) struct StopArgs {
    /// If the daemon does not exit within 10s of SIGTERM, escalate to
    /// SIGKILL and remove the PID file.
    #[arg(long)]
    force: bool,
}

impl CommandExecute for StopArgs {
    async fn run(self) -> anyhow::Result<()> {
        let Some(pid) = daemon_bootstrap::pidfile::read()? else {
            eprintln!("ozmux daemon not running");
            return Ok(());
        };

        if try_graceful_stop(pid).await? {
            return Ok(());
        }

        if !self.force {
            anyhow::bail!(
                "ozmux daemon (pid {pid}) did not exit within {SIGTERM_WAIT:?}; rerun with --force to SIGKILL"
            );
        }

        force_kill(pid).await
    }
}

/// Sends SIGTERM and waits up to `SIGTERM_WAIT` for the process to exit.
/// Returns `Ok(true)` if the process is gone (either it never existed or
/// it exited in time), `Ok(false)` if it is still alive after the wait.
async fn try_graceful_stop(pid: u32) -> anyhow::Result<bool> {
    match send_signal(pid, libc::SIGTERM) {
        Ok(()) => Ok(wait_for_exit(pid, SIGTERM_WAIT).await),
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {
            eprintln!("ozmux daemon not running (stale PID {pid})");
            daemon_bootstrap::pidfile::remove()?;
            Ok(true)
        }
        Err(e) if e.raw_os_error() == Some(libc::EPERM) => {
            anyhow::bail!("permission denied sending SIGTERM to pid {pid}");
        }
        Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
            eprintln!("ozmux daemon not running (corrupted PID file: {pid})");
            daemon_bootstrap::pidfile::remove()?;
            Ok(true)
        }
        Err(e) => Err(e.into()),
    }
}

/// Sends SIGKILL and waits for the process to actually exit, then clears
/// the stale PID file. Errors if the process refuses to die.
async fn force_kill(pid: u32) -> anyhow::Result<()> {
    match send_signal(pid, libc::SIGKILL) {
        Ok(()) => {}
        // NOTE: it died between SIGTERM timeout and SIGKILL; treat as success.
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {
            daemon_bootstrap::pidfile::remove()?;
            return Ok(());
        }
        Err(e) if e.raw_os_error() == Some(libc::EPERM) => {
            anyhow::bail!("permission denied sending SIGKILL to pid {pid}");
        }
        Err(e) => return Err(e.into()),
    }

    if wait_for_exit(pid, SIGKILL_WAIT).await {
        daemon_bootstrap::pidfile::remove()?;
        return Ok(());
    }
    anyhow::bail!("process did not die even after SIGKILL (pid {pid})")
}

fn send_signal(pid: u32, sig: libc::c_int) -> io::Result<()> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "PID out of valid range",
        ));
    }
    // SAFETY: libc::kill is documented as MT-safe; the guard above
    // ensures pid is in (0, i32::MAX], so the cast to pid_t (i32)
    // preserves the value.
    let rc = unsafe { libc::kill(pid as libc::pid_t, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

async fn wait_for_exit(pid: u32, total: Duration) -> bool {
    let deadline = Instant::now() + total;
    loop {
        if !daemon_bootstrap::pidfile::is_process_alive(pid).unwrap_or(false) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
