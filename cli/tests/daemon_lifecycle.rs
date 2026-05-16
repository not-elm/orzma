//! End-to-end lifecycle smoke test for `ozmux daemon start --foreground`.
//! Spawns the binary as a child, waits for `/health` to return 200, sends
//! SIGTERM, and asserts that the process exits cleanly within 5s.
//!
//! This test binds the daemon's real TCP port (3200) and writes to the
//! real `$TMPDIR/ozmux/daemon.pid`. It must run serially with other tests
//! that touch the daemon, and will fail if a daemon is already running.

use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::{Instant, sleep};

const HEALTH_URL: &str = "http://127.0.0.1:3200/health";
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(200);

#[tokio::test(flavor = "current_thread")]
async fn foreground_start_and_sigterm_shutdown() {
    let bin = env!("CARGO_BIN_EXE_ozmux");

    let mut child = Command::new(bin)
        .args(["daemon", "start", "--foreground"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ozmux daemon start --foreground");

    let pid = child.id().expect("child has PID");

    wait_for_health().await;

    // SAFETY: SIGTERM to a child PID owned by this process is well-defined.
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    assert_eq!(
        rc,
        0,
        "kill(SIGTERM) failed: {}",
        std::io::Error::last_os_error()
    );

    let status = tokio::time::timeout(SHUTDOWN_TIMEOUT, child.wait())
        .await
        .expect("daemon did not exit within 5s of SIGTERM")
        .expect("waitpid failed");

    assert!(
        status.success(),
        "daemon exited with non-zero status: {status:?}"
    );
}

async fn wait_for_health() {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if let Ok(r) = client.get(HEALTH_URL).send().await
            && r.status().is_success()
        {
            return;
        }
        if Instant::now() >= deadline {
            panic!("/health did not return 200 within {READY_TIMEOUT:?}");
        }
        sleep(POLL).await;
    }
}
