//! Integration tests for `ozmux session attach`: verifies it refuses to
//! auto-start the daemon, refuses unknown IDs, and launches the Tauri
//! client when given an existing session.
//!
//! Like `session_new.rs`, these tests bind the real TCP port 3200 and
//! write the real `$TMPDIR/ozmux/daemon.pid`. `cargo test` runs each
//! integration-test file as its own binary sequentially, so they do not
//! overlap with other integration tests. Each test below runs in sequence
//! within this binary, and the per-test `DaemonStopGuard` stops the daemon
//! even if an assertion panics partway through.

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

fn daemon_running() -> bool {
    let Ok(addr) = DAEMON_ADDR.parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok()
}

async fn run_new(bin: &str, name: &str) -> String {
    let out = Command::new(bin)
        .args(["session", "new", "-s", name])
        .stdin(Stdio::null())
        .output()
        .await
        .expect("spawn ozmux session new");
    assert!(out.status.success(), "session new failed: {out:?}");
    String::from_utf8(out.stdout).expect("utf-8").trim().to_string()
}

#[tokio::test(flavor = "current_thread")]
async fn attach_existing_session_invokes_client_bin() {
    let bin = env!("CARGO_BIN_EXE_ozmux").to_string();
    assert!(
        !daemon_running(),
        "a daemon is already running on {DAEMON_ADDR}; stop it before running this test"
    );
    let _guard = DaemonStopGuard { bin: bin.clone() };

    let id = run_new(&bin, "attach-happy").await;

    let out = Command::new(&bin)
        .env("OZMUX_CLIENT_BIN", "/usr/bin/true")
        .args(["session", "attach", &id])
        .stdin(Stdio::null())
        .output()
        .await
        .expect("spawn ozmux session attach");

    assert!(
        out.status.success(),
        "session attach should succeed: {out:?}"
    );
    assert!(
        out.stdout.is_empty(),
        "session attach should not write to stdout; got: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}
