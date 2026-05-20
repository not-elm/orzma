//! Smoke test that `ozmux-daemon` binary can start, serve /health, and exit
//! gracefully on SIGTERM. Phase 1 scaffold: ensures the bin target works
//! end-to-end before Plan 3 changes its internals.

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn ozmux_daemon_path() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bootstrap → daemon
    p.pop(); // daemon → workspace root
    p.push("target");
    p.push(if cfg!(debug_assertions) { "debug" } else { "release" });
    p.push("ozmux-daemon");
    p
}

#[test]
#[ignore = "requires bundled cef_host.app and free port 3200; run via `cargo test --test ozmux_daemon_binary -- --ignored`"]
fn ozmux_daemon_starts_serves_health_and_shuts_down() {
    let bin = ozmux_daemon_path();
    assert!(
        bin.exists(),
        "ozmux-daemon binary not built at {}",
        bin.display()
    );

    let ext_root = std::env::temp_dir().join("ozmux-test-no-ext");
    std::fs::create_dir_all(&ext_root).expect("create empty extension root");

    let mut child = Command::new(&bin)
        .env("OZMUX_EXTENSION_ROOT", &ext_root)
        .env("OZMUX_BROWSER_SKIP_COOKIE_IMPORT", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ozmux-daemon");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut healthy = false;
    while Instant::now() < deadline {
        if let Ok(resp) = ureq::get("http://127.0.0.1:3200/health").call()
            && resp.status() == 200
        {
            healthy = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    if !healthy {
        let mut buf = String::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_string(&mut buf);
        }
        let _ = child.kill();
        panic!("/health never returned 200 within 20s; stderr:\n{buf}");
    }

    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.id() as i32),
        nix::sys::signal::Signal::SIGTERM,
    );

    let status = child.wait().expect("wait child");
    assert!(
        status.success(),
        "ozmux-daemon did not exit cleanly: {status:?}"
    );
}
