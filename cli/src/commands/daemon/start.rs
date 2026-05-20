//! `ozmux daemon start` — spawn or attach the ozmux daemon.

use anyhow::Context;
use clap::Args;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::commands::CommandExecute;

const DAEMON_BIN_NAME: &str = "ozmux-daemon";
#[cfg(target_os = "macos")]
const DAEMON_APP_BUNDLE: &str = "ozmux-daemon.app";

const PROBE_TIMEOUT: Duration = Duration::from_millis(200);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const LOCK_TIMEOUT: Duration = Duration::from_secs(20);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);
const LOG_TAIL_BYTES: u64 = 8192;
const LOG_TAIL_LINES: usize = 20;

/// Arguments for the `daemon start` subcommand.
#[derive(Args)]
pub(crate) struct StartArgs {
    /// Run the daemon attached to this terminal instead of detaching.
    #[arg(long)]
    foreground: bool,
}

impl CommandExecute for StartArgs {
    async fn run(self) -> anyhow::Result<()> {
        if self.foreground {
            return exec_daemon_binary();
        }
        match super::ensure_running().await? {
            super::DaemonStartOutcome::AlreadyRunning => {
                eprintln!(
                    "ozmux daemon already running on {}",
                    daemon_bootstrap::HTTP_ADDR
                );
            }
            super::DaemonStartOutcome::Started => {
                if let Some(pid) = daemon_bootstrap::pidfile::read()? {
                    println!("{pid}");
                }
            }
        }
        Ok(())
    }
}

pub(super) fn is_running() -> bool {
    let Ok(addr) = daemon_bootstrap::HTTP_ADDR.parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok()
}

fn runtime_dir() -> anyhow::Result<PathBuf> {
    daemon_bootstrap::runtime_dir()
        .with_context(|| "create runtime dir at $TMPDIR/ozmux".to_string())
}

pub(super) async fn acquire_lock() -> anyhow::Result<File> {
    let path = runtime_dir()?.join("daemon.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("open lock file at {}", path.display()))?;

    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    anyhow::bail!(
                        "another launcher held the lock at {} for more than {:?}",
                        path.display(),
                        LOCK_TIMEOUT
                    );
                }
                tokio::time::sleep(LOCK_POLL_INTERVAL).await;
            }
            Err(TryLockError::Error(e)) => {
                return Err(anyhow::Error::new(e).context("acquire exclusive flock"));
            }
        }
    }
}

pub(super) fn spawn_detached() -> anyhow::Result<()> {
    let daemon_bin = resolve_daemon_binary()?;
    let (log, log_err) = open_daemon_log_pair()?;

    let mut cmd = std::process::Command::new(&daemon_bin);
    // NOTE: ozmux-daemon takes no CLI arguments; pass none.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    crate::process::detach::configure_detached(&mut cmd);

    cmd.spawn()
        .with_context(|| format!("fork detached daemon at {}", daemon_bin.display()))?;
    // NOTE: drop the child handle without waiting; the daemon is intentionally
    // orphaned and init/launchd will adopt it.
    Ok(())
}

/// Replaces the current CLI process with the resolved `ozmux-daemon` binary.
/// On success this never returns; the daemon takes over the CLI's pid and
/// stdio. Used by `ozmux daemon start --foreground`.
fn exec_daemon_binary() -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt as _;

    let daemon_bin = resolve_daemon_binary()?;
    let err = std::process::Command::new(&daemon_bin).exec();
    Err(anyhow::Error::new(err).context(format!("exec ozmux-daemon at {}", daemon_bin.display())))
}

/// Resolves the path to the `ozmux-daemon` binary using a layered fallback:
/// 1. `OZMUX_DAEMON_BIN` env override
/// 2. macOS-only: an `ozmux-daemon.app/Contents/MacOS/ozmux-daemon` bundle
///    sitting next to the running `ozmux` executable
/// 3. A plain sibling `ozmux-daemon` next to the running `ozmux` executable
/// 4. `which::which("ozmux-daemon")` against `PATH`
fn resolve_daemon_binary() -> anyhow::Result<PathBuf> {
    if let Some(v) = std::env::var_os("OZMUX_DAEMON_BIN") {
        return Ok(PathBuf::from(v));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        #[cfg(target_os = "macos")]
        {
            let bundled = parent
                .join(DAEMON_APP_BUNDLE)
                .join("Contents")
                .join("MacOS")
                .join(DAEMON_BIN_NAME);
            if bundled.is_file() {
                return Ok(bundled);
            }
        }
        let sibling = parent.join(DAEMON_BIN_NAME);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    which::which(DAEMON_BIN_NAME).with_context(|| {
        format!(
            "resolve `{DAEMON_BIN_NAME}` binary (set OZMUX_DAEMON_BIN, install via `make dev`, or add to PATH)"
        )
    })
}

fn open_daemon_log_pair() -> anyhow::Result<(File, File)> {
    let log_path = runtime_dir()?.join("daemon.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open daemon log at {}", log_path.display()))?;
    let log_err = log
        .try_clone()
        .with_context(|| format!("clone log file handle for {}", log_path.display()))?;
    Ok((log, log_err))
}

pub(super) async fn wait_until_ready() -> anyhow::Result<()> {
    let client = super::http_client(Duration::from_secs(1))?;

    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let last_err = match client.get(daemon_bootstrap::HEALTH_URL).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => anyhow::anyhow!("HTTP {}", resp.status()),
            Err(e) => anyhow::Error::new(e),
        };
        if Instant::now() >= deadline {
            print_log_tail();
            return Err(last_err).context(format!(
                "/health did not return 200 within {READY_TIMEOUT:?}"
            ));
        }
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    }
}

fn print_log_tail() {
    let Ok(parent) = runtime_dir() else { return };
    let path = parent.join("daemon.log");
    let Ok(tail) = read_log_tail_bytes(&path) else {
        return;
    };
    let text = String::from_utf8_lossy(&tail);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(LOG_TAIL_LINES);
    eprintln!("--- last {LOG_TAIL_LINES} lines of {} ---", path.display());
    for line in &lines[start..] {
        eprintln!("{line}");
    }
}

fn read_log_tail_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL_BYTES)))?;
    let mut buf = Vec::with_capacity(LOG_TAIL_BYTES as usize);
    f.read_to_end(&mut buf)?;
    Ok(buf)
}
