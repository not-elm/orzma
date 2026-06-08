//! Verifies `ozmuxd --kill [path]` shuts a running daemon down cleanly, and that
//! `--kill` with no daemon present exits 0.

use std::process::Command;
use std::time::{Duration, Instant};

fn sock(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ozmuxd-kill-{name}.sock"));
    let _ = std::fs::remove_file(&p);
    p
}

fn wait_until(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

#[test]
fn kill_shuts_down_and_removes_socket() {
    let path = sock("shutdown");
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_ozmuxd"))
        .arg(&path)
        .spawn()
        .expect("spawn ozmuxd");
    assert!(
        wait_until(|| path.exists(), Duration::from_secs(3)),
        "daemon binds socket"
    );

    let kill = Command::new(env!("CARGO_BIN_EXE_ozmuxd"))
        .arg("--kill")
        .arg(&path)
        .status()
        .expect("run --kill");
    assert!(kill.success(), "--kill exits 0");

    let exited = wait_until(
        || matches!(daemon.try_wait(), Ok(Some(_))),
        Duration::from_secs(3),
    );
    if !exited {
        let _ = daemon.kill();
        let _ = daemon.wait();
        let _ = std::fs::remove_file(&path);
        panic!("daemon did not exit after --kill within 3s");
    }
    let status = daemon.wait().expect("reap daemon");
    assert!(
        status.success(),
        "daemon exits cleanly after --kill, got {status:?}"
    );
    assert!(!path.exists(), "socket removed after --kill");
}

#[test]
fn kill_with_no_daemon_exits_zero() {
    let path = sock("none");
    let st = Command::new(env!("CARGO_BIN_EXE_ozmuxd"))
        .arg("--kill")
        .arg(&path)
        .status()
        .expect("run --kill");
    assert!(st.success(), "--kill with no daemon exits 0");
}
