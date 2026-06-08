//! Verifies ozmuxd shuts down gracefully on SIGTERM: clean exit and the socket
//! file is removed (via ServerHandle::Drop), not hard-terminated.

use std::process::Command;
use std::time::{Duration, Instant};

fn sock(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ozmuxd-sig-{name}.sock"));
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
fn sigterm_exits_cleanly_and_removes_socket() {
    let path = sock("term-clean");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ozmuxd"))
        .arg(&path)
        .spawn()
        .expect("spawn ozmuxd");

    // Wait for the daemon to bind the socket.
    assert!(
        wait_until(|| path.exists(), Duration::from_secs(3)),
        "daemon should bind the socket"
    );

    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.id() as i32),
        nix::sys::signal::Signal::SIGTERM,
    )
    .expect("send SIGTERM");

    // Poll for exit, with a hard kill on timeout so a hung daemon never leaks.
    let exited = wait_until(
        || matches!(child.try_wait(), Ok(Some(_))),
        Duration::from_secs(3),
    );
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&path);
        panic!("daemon did not exit on SIGTERM within 3s");
    }
    let status = child.wait().expect("reap ozmuxd");

    assert!(
        status.success(),
        "daemon must exit cleanly (status 0) on SIGTERM, got {status:?}"
    );
    assert!(
        !path.exists(),
        "socket must be removed on graceful shutdown"
    );
}
