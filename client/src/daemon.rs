//! daemon launcher: probe, advisory lock, detached spawn, and readiness wait
//! for the ozmux daemon process backing the Tauri client.

use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;

const DAEMON_ADDR: &str = "127.0.0.1:3200";
const HEALTH_URL: &str = "http://127.0.0.1:3200/health";
const PROBE_TIMEOUT: Duration = Duration::from_millis(200);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const LOCK_TIMEOUT: Duration = Duration::from_secs(20);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Returns `true` if something is already listening on the daemon's TCP port.
///
/// This is a TCP-only check — it does not verify the listener is actually
/// ozmux. Verifying daemon identity (e.g. by querying a version endpoint) is
/// out of scope for the minimal launcher.
fn is_running() -> bool {
    let Ok(addr) = DAEMON_ADDR.parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok()
}

/// Ensures the daemon is running and accepting `/health` requests.
///
/// Fast path: if `is_running()` returns true we assume the daemon is up and
/// return immediately. Otherwise we take an exclusive advisory file lock on
/// `$TMPDIR/ozmux/launcher.lock`, re-check, spawn if still absent, and poll
/// `GET /health` until 200 OK. The lock is held across the readiness wait so
/// concurrent launcher invocations serialize on it instead of all spawning.
pub(crate) async fn ensure_running(app: &AppHandle) -> Result<()> {
    if is_running() {
        return Ok(());
    }

    let lock = acquire_launcher_lock()
        .await
        .context("acquire launcher lock")?;

    if !is_running() {
        spawn_detached(app).context("spawn daemon sidecar")?;
    }
    wait_until_ready()
        .await
        .context("wait for daemon readiness")?;

    // NOTE: drop is explicit so the lock release happens after the wait, not
    // earlier due to NLL shrinking the borrow.
    drop(lock);
    Ok(())
}

/// Spawns `daemon_bootstrap` as a detached child whose stdio is redirected to
/// `$TMPDIR/ozmux/daemon.log` and which `setsid`s into its own session so it
/// survives the parent (Tauri) exiting.
///
/// The caller must already hold the launcher lock.
fn spawn_detached(app: &AppHandle) -> Result<()> {
    let log_path = log_file_path()?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open daemon log at {}", log_path.display()))?;
    let log_err = log
        .try_clone()
        .with_context(|| format!("clone log file handle for {}", log_path.display()))?;

    let sidecar = app.shell().sidecar("daemon_bootstrap")?;
    let mut cmd: std::process::Command = sidecar.into();
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));

    // SAFETY: the closure only calls `setsid`, which is async-signal-safe
    // (POSIX.1-2008 Table 2-5). It runs between fork and exec, where the
    // child has not yet exec'd, so no Rust destructors run here either.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    cmd.spawn().context("fork daemon_bootstrap")?;
    // NOTE: Drop the child handle without waiting — the daemon is intentionally
    // orphaned. init/launchd will adopt it.
    Ok(())
}

async fn wait_until_ready() -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .context("build reqwest client")?;

    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let last_err = match client.get(HEALTH_URL).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => anyhow::anyhow!("HTTP {}", resp.status()),
            Err(e) => anyhow::Error::new(e),
        };
        if Instant::now() >= deadline {
            return Err(last_err).context(format!(
                "/health did not return 200 within {READY_TIMEOUT:?}"
            ));
        }
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    }
}

/// Try to take an exclusive advisory lock on `$TMPDIR/ozmux/launcher.lock`.
///
/// `fs2::FileExt::try_lock_exclusive` returns `WouldBlock` when another
/// process holds the lock. We retry with `LOCK_POLL_INTERVAL` cadence until
/// `LOCK_TIMEOUT`. The lock auto-releases on file close (i.e. when the
/// returned `File` is dropped).
async fn acquire_launcher_lock() -> Result<File> {
    let path = lock_file_path()?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("open lock file at {}", path.display()))?;

    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(anyhow::anyhow!(
                        "another launcher held the lock at {} for more than {:?}",
                        path.display(),
                        LOCK_TIMEOUT
                    ));
                }
                tokio::time::sleep(LOCK_POLL_INTERVAL).await;
            }
            Err(e) => return Err(anyhow::Error::new(e).context("acquire exclusive flock")),
        }
    }
}

fn runtime_dir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("ozmux");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create runtime dir at {}", dir.display()))?;
    Ok(dir)
}

fn log_file_path() -> Result<PathBuf> {
    Ok(runtime_dir()?.join("daemon.log"))
}

fn lock_file_path() -> Result<PathBuf> {
    Ok(runtime_dir()?.join("launcher.lock"))
}
