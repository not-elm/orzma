//! End-to-end: launch memo via the control bridge, spawn a real `sh` PTY wired
//! with `terminal_env`, type `@memo`, and assert (i) the handler greeting
//! streams back AND (ii) a new extension-activity pane appears in the
//! multiplexer ECS world (the split is no longer a no-op). Skips when node /
//! memo are unavailable.

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use ozmux_extension_host::{
    CommandExtension, CommandExtensionConfig, ControlExtension, terminal_env,
};
use ozmux_multiplexer::{ActivityKind, ActivityMarker, MultiplexerCommands, MultiplexerPlugin};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

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

fn drain(ext: Option<Res<ControlExtension>>, mut mux: MultiplexerCommands) {
    let Some(ext) = ext else {
        return;
    };
    while let Ok((req, responder)) = ext.0.control_requests().try_recv() {
        ozmux_extension_host::__bridge_apply(&mut mux, req, responder);
    }
}

fn extension_pane_exists(app: &mut App) -> bool {
    let world = app.world_mut();
    let mut q = world.query_filtered::<&ActivityKind, With<ActivityMarker>>();
    q.iter(world)
        .any(|k| matches!(k, ActivityKind::Extension { .. }))
}

#[test]
fn typing_memo_creates_an_extension_pane() {
    if !available() {
        eprintln!("skipping: node or memo's @ozmux/sdk link not available");
        return;
    }

    // NOTE: spawn_with_timeout (not spawn) — the lifecycle thread polls for
    // shim creation up to this budget. The default 10s starves under
    // parallel-test CPU contention (this binary, command_round_trip, and the
    // lib memo test each cold-start node concurrently); a too-low spawn budget
    // makes the thread emit a timeout event that fails wait_ready regardless of
    // its own (larger) timeout.
    let ext = CommandExtension::spawn_with_timeout(
        CommandExtensionConfig {
            name: "memo".into(),
            dir: memo_dir(),
            main: "bootstrap.ts".into(),
            commands: vec!["@memo".into()],
        },
        Duration::from_secs(20),
    )
    .expect("spawn memo");
    ext.wait_ready(Duration::from_secs(20)).expect("memo ready");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(MultiplexerPlugin);
    let created = app
        .world_mut()
        .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
        .unwrap();
    app.world_mut().flush();

    let env = terminal_env(&ext, created.pane, created.session);

    app.insert_resource(ControlExtension(ext));
    app.add_systems(Update, drain);

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            ..Default::default()
        })
        .unwrap();
    let mut cmd = CommandBuilder::new("sh");
    for (k, v) in &env {
        cmd.env(k, v);
    }
    let mut child = pty.slave.spawn_command(cmd).unwrap();
    drop(pty.slave);
    let mut writer = pty.master.take_writer().unwrap();
    let mut reader = pty.master.try_clone_reader().unwrap();

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx
                        .send(String::from_utf8_lossy(&buf[..n]).into_owned())
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });
    write!(writer, "@memo\nexit\n").unwrap();
    writer.flush().unwrap();

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut out = String::new();
    let mut saw_pane = false;
    while Instant::now() < deadline {
        app.update();
        while let Ok(chunk) = rx.try_recv() {
            out.push_str(&chunk);
        }
        saw_pane = extension_pane_exists(&mut app);
        if out.contains("memo invoked in pane") && saw_pane {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.wait();

    assert!(
        out.contains("memo invoked in pane"),
        "expected greeting, got:\n{out}"
    );
    assert!(
        saw_pane,
        "expected a new ActivityKind::Extension pane in the world"
    );
}
