//! `ozmux daemon stop` — signal the running daemon to shut down.

use clap::Args;
use std::io;
use std::time::{Duration, Instant};

const SIGTERM_WAIT: Duration = Duration::from_secs(10);
const SIGKILL_WAIT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Args)]
pub struct StopArgs {
    /// If the daemon does not exit within 10s of SIGTERM, escalate to
    /// SIGKILL and remove the PID file.
    #[arg(long)]
    force: bool,
}

pub async fn run(args: StopArgs) -> anyhow::Result<()> {
    let Some(pid) = daemon_bootstrap::pidfile::read()? else {
        eprintln!("ozmux daemon not running");
        return Ok(());
    };

    match send_signal(pid, libc::SIGTERM) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {
            eprintln!("ozmux daemon not running (stale PID {pid})");
            daemon_bootstrap::pidfile::remove()?;
            return Ok(());
        }
        Err(e) if e.raw_os_error() == Some(libc::EPERM) => {
            anyhow::bail!("permission denied sending SIGTERM to pid {pid}");
        }
        Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
            eprintln!("ozmux daemon not running (corrupted PID file: {pid})");
            daemon_bootstrap::pidfile::remove()?;
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    }

    if wait_for_exit(pid, SIGTERM_WAIT).await {
        return Ok(());
    }

    if !args.force {
        anyhow::bail!(
            "ozmux daemon (pid {pid}) did not exit within {:?}; rerun with --force to SIGKILL",
            SIGTERM_WAIT
        );
    }

    match send_signal(pid, libc::SIGKILL) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {
            // NOTE: it died between SIGTERM timeout and SIGKILL; treat as success.
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
    // preserves the value. Signal 0 (used by wait_for_exit) has no
    // side effects beyond the errno return.
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
        match send_signal(pid, 0) {
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => return true,
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => return true,
            _ => {}
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
