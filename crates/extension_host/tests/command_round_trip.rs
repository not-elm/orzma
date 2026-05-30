//! End-to-end: launch memo as a command extension, spawn a real `sh` PTY with the
//! shim bin dir on PATH + the pane/session env, type `@memo`, and assert the
//! handler's stdout (`memo invoked in pane <id>`) streams back. Skips gracefully
//! when `node`/memo aren't set up.

use ozmux_extension_host::{CommandExtension, CommandExtensionConfig, extension_path_prefix};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

fn memo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extensions/memo")
}

fn available() -> bool {
    let node = std::process::Command::new("sh")
        .arg("-c")
        .arg("command -v node")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    node && memo_dir().join("node_modules/@ozmux/sdk").exists()
}

#[test]
fn typing_memo_runs_the_command() {
    if !available() {
        eprintln!("skipping: node or memo's @ozmux/sdk link not available");
        return;
    }
    let ext = CommandExtension::spawn(CommandExtensionConfig {
        name: "memo".into(),
        dir: memo_dir(),
        main: "bootstrap.ts".into(),
        commands: vec!["@memo".into()],
    })
    .expect("spawn memo");
    // NOTE: generous readiness budget — node cold-start can exceed the default
    // under parallel-test CPU contention (observed flake).
    ext.wait_ready(Duration::from_secs(20)).expect("memo ready");

    let path = extension_path_prefix(
        &[ext.bin_dir().to_path_buf()],
        &std::env::var("PATH").unwrap_or_default(),
    );

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            ..Default::default()
        })
        .unwrap();
    let mut cmd = CommandBuilder::new("sh");
    cmd.env("PATH", path);
    cmd.env("OZMUX_PANE_ID", "pane-123");
    cmd.env("OZMUX_SESSION_ID", "sess-1");
    let mut child = pty.slave.spawn_command(cmd).unwrap();
    drop(pty.slave);

    let mut writer = pty.master.take_writer().unwrap();
    let mut reader = pty.master.try_clone_reader().unwrap();

    // NOTE: blocking PTY reads must run on a dedicated thread — reader.read()
    // parks the thread indefinitely when no data arrives. A channel lets the
    // main thread enforce a wall-clock deadline without a non-blocking pty API.
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if tx.send(chunk).is_err() {
                        break;
                    }
                }
            }
        }
    });

    write!(writer, "@memo\nexit\n").unwrap();
    writer.flush().unwrap();

    let mut out = String::new();
    let deadline = Duration::from_secs(15);
    loop {
        match rx.recv_timeout(deadline) {
            Ok(chunk) => {
                out.push_str(&chunk);
                if out.contains("memo invoked in pane pane-123") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = child.wait();
    assert!(
        out.contains("memo invoked in pane pane-123"),
        "expected greeting in PTY output, got:\n{out}"
    );
}
