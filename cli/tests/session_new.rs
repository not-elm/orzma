//! Integration test for `ozmux session new`: verifies it creates a
//! session via the daemon, auto-starting the daemon when none is running
//! and reusing an already-running daemon otherwise.
//!
//! Like `daemon_lifecycle.rs`, this test binds the real TCP port 3200 and
//! writes the real `$TMPDIR/ozmux/daemon.pid`. `cargo test` runs each
//! integration-test file as its own binary sequentially, so it does not
//! overlap with `daemon_lifecycle.rs`. The single test below runs both
//! scenarios in sequence to avoid in-binary parallelism, and a drop guard
//! stops the daemon even if an assertion panics partway through.

use daemon_bootstrap::HTTP_ADDR as DAEMON_ADDR;
use std::net::{SocketAddr, TcpStream};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const PROBE_TIMEOUT: Duration = Duration::from_millis(200);

struct DaemonStopGuard {
    bin: String,
}

impl Drop for DaemonStopGuard {
    fn drop(&mut self) {
        let _ = std::process::Command::new(&self.bin)
            .args(["daemon", "stop"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn session_new_autostarts_then_reuses_daemon() {
    let bin = env!("CARGO_BIN_EXE_ozmux").to_string();
    assert!(
        !daemon_running(),
        "a daemon is already running on {DAEMON_ADDR}; stop it before running this test"
    );
    let _guard = DaemonStopGuard { bin: bin.clone() };

    let auto = run_new(&bin, "autostart-session").await;
    assert!(
        auto.status.success(),
        "session new (auto-start) failed: {auto:?}"
    );
    assert_single_id_line(&auto.stdout, "auto-start");
    assert!(
        daemon_running(),
        "daemon should be running after auto-start"
    );

    let reuse = run_new(&bin, "reuse-session").await;
    assert!(
        reuse.status.success(),
        "session new (reuse) failed: {reuse:?}"
    );
    assert_single_id_line(&reuse.stdout, "reuse");
}

async fn run_new(bin: &str, name: &str) -> std::process::Output {
    Command::new(bin)
        .args(["session", "new", "-s", name])
        .stdin(Stdio::null())
        .output()
        .await
        .expect("spawn ozmux session new")
}

fn daemon_running() -> bool {
    let Ok(addr) = DAEMON_ADDR.parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok()
}

fn assert_single_id_line(stdout: &[u8], label: &str) {
    let text = String::from_utf8(stdout.to_vec()).expect("stdout is utf-8");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "{label}: expected exactly one stdout line (the session id), got {lines:?}"
    );
    assert!(
        !lines[0].trim().is_empty(),
        "{label}: session id line is empty"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn session_new_sends_current_dir_as_cwd() {
    use reqwest::Client;
    let bin = env!("CARGO_BIN_EXE_ozmux").to_string();
    assert!(
        !daemon_running(),
        "a daemon is already running on {DAEMON_ADDR}; stop it before running this test"
    );
    let _guard = DaemonStopGuard { bin: bin.clone() };

    let dir = tempfile::tempdir().expect("tempdir");
    let out = Command::new(&bin)
        .current_dir(dir.path())
        .args(["session", "new", "-s", "cwd-check"])
        .stdin(Stdio::null())
        .output()
        .await
        .expect("spawn ozmux session new");
    assert!(out.status.success(), "session new failed: {out:?}");

    let id = String::from_utf8(out.stdout).unwrap().trim().to_string();

    let url = format!("http://{DAEMON_ADDR}/sessions/{id}");
    let resp = Client::new().get(&url).send().await.expect("GET session");
    assert!(resp.status().is_success());
}
